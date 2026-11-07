//! The exec driver: an outside program, such as a Playwright script, that speaks the runner
//! protocol over stdin and stdout. `dyno check --driver exec:<program>` selects it.
//!
//! dyno starts the program directly, with no shell and no arguments. It writes the
//! [`RunRequest`] as one line on stdin and closes stdin. The program writes [`DriverMessage`]
//! lines on stdout: `hello` first, and `done` or `error` last. dyno stops reading at that last
//! message. The program's stderr is free text: dyno reads it on its own thread, so a full pipe
//! never blocks the program, and keeps its tail for an error.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use dyno_schema::runner::{DriverMessage, RUNNER_PROTOCOL_VERSION, RunRequest};

use crate::protocol::{decode_line, encode_line};
use crate::{Driver, DriverError, DriverErrorCode};

/// The driver's name in errors.
pub const DRIVER_NAME: &str = "exec";

/// The most bytes of the program's stderr that an error quotes.
pub const STDERR_TAIL_BYTES: usize = 500;

/// How long an error waits for the rest of stderr after stdout closed.
const STDERR_WAIT: Duration = Duration::from_secs(1);

/// How long a program that sent its last message gets to exit before it is killed. Chromium gets
/// the same after `Browser.close` in the CDP driver.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// How often dyno looks to see if the program exited.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How many bytes of stderr one read takes.
const READ_CHUNK_BYTES: usize = 4096;

/// A driver that runs `program`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecDriver {
    program: PathBuf,
}

impl ExecDriver {
    pub fn new(program: impl Into<PathBuf>) -> ExecDriver {
        ExecDriver {
            program: program.into(),
        }
    }

    fn start(&self) -> Result<Child, DriverError> {
        Command::new(&self.program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                let detail = format!("cannot start {}: {error}", self.program.display());
                DriverError::new(DriverErrorCode::Run, &detail)
            })
    }
}

impl Driver for ExecDriver {
    fn name(&self) -> &str {
        DRIVER_NAME
    }

    /// Runs the program until its last message, or until `timeout_ms` has passed since the start.
    /// The program is killed on any error, and after `EXIT_GRACE` if it does not exit by itself.
    fn run(&mut self, request: &RunRequest) -> Result<Vec<DriverMessage>, DriverError> {
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms.into());
        let mut child = self.start()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let lines = read_lines_on_a_thread(child.stdout.take().expect("stdout is piped"));
        let stderr_tail = read_tail_on_a_thread(child.stderr.take().expect("stderr is piped"));
        let outcome = send_request(stdin, request)
            .and_then(|()| read_messages(&lines, deadline, request.timeout_ms, &stderr_tail));
        if outcome.is_ok() {
            wait_then_stop(&mut child, deadline.min(Instant::now() + EXIT_GRACE));
        } else {
            stop(&mut child);
        }
        outcome
    }
}

/// Writes the request line, then closes stdin by dropping it.
fn send_request(mut stdin: ChildStdin, request: &RunRequest) -> Result<(), DriverError> {
    let line = encode_line(request)
        .map_err(|error| DriverError::new(DriverErrorCode::Run, &error.to_string()))?;
    match stdin.write_all(line.as_bytes()) {
        Ok(()) => Ok(()),
        // A program that exits before it reads the request breaks the pipe. Its stdout and
        // stderr then tell why, so the read gives the error.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => {
            let detail = format!("cannot write the request: {error}");
            Err(DriverError::new(DriverErrorCode::Run, &detail))
        }
    }
}

/// Reads messages until `done` or `error`, which is the last one.
fn read_messages(
    lines: &Receiver<io::Result<String>>,
    deadline: Instant,
    timeout_ms: u32,
    stderr_tail: &Receiver<String>,
) -> Result<Vec<DriverMessage>, DriverError> {
    let mut messages = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let received = if remaining.is_zero() {
            Err(RecvTimeoutError::Timeout)
        } else {
            lines.recv_timeout(remaining)
        };
        let line = match received {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => return Err(protocol_error(&format!("cannot read stdout: {error}"))),
            Err(RecvTimeoutError::Timeout) => {
                let detail = format!("the run did not end within {timeout_ms} ms");
                return Err(DriverError::new(DriverErrorCode::Run, &detail));
            }
            Err(RecvTimeoutError::Disconnected) => return Err(closed_early(stderr_tail, deadline)),
        };
        let line_number = messages.len() + 1;
        let message = decode_line(&line).map_err(|error| {
            protocol_error(&format!(
                "line {line_number} is not a runner message: {error}"
            ))
        })?;
        if messages.is_empty() {
            check_hello(&message)?;
        }
        let is_last = matches!(
            message,
            DriverMessage::Done { .. } | DriverMessage::Error { .. }
        );
        messages.push(message);
        if is_last {
            return Ok(messages);
        }
    }
}

/// An error when the first message is not a `hello` that speaks this dyno's protocol version.
fn check_hello(message: &DriverMessage) -> Result<(), DriverError> {
    match message {
        DriverMessage::Hello {
            protocol_version, ..
        } if *protocol_version == RUNNER_PROTOCOL_VERSION => Ok(()),
        DriverMessage::Hello {
            protocol_version, ..
        } => Err(protocol_error(&format!(
            "the driver speaks protocol version {protocol_version}, but this dyno speaks {RUNNER_PROTOCOL_VERSION}"
        ))),
        _ => Err(protocol_error("the first message is not hello")),
    }
}

/// The error when stdout closed before `done` or `error`, with the tail of stderr.
fn closed_early(stderr_tail: &Receiver<String>, deadline: Instant) -> DriverError {
    let wait = deadline
        .saturating_duration_since(Instant::now())
        .min(STDERR_WAIT);
    let tail = stderr_tail.recv_timeout(wait).unwrap_or_default();
    let stderr = if tail.is_empty() {
        "nothing on stderr".to_owned()
    } else {
        format!("stderr ends: {tail}")
    };
    protocol_error(&format!("stdout closed before done or error; {stderr}"))
}

fn protocol_error(detail: &str) -> DriverError {
    DriverError::new(DriverErrorCode::Protocol, detail)
}

/// Sends each stdout line, or the read error that ends them. The channel closes at the end of
/// stdout.
fn read_lines_on_a_thread(stdout: ChildStdout) -> Receiver<io::Result<String>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let failed = line.is_err();
            // A send fails when dyno has stopped reading.
            if sender.send(line).is_err() || failed {
                break;
            }
        }
    });
    receiver
}

/// Sends the tail of stderr once stderr ends.
fn read_tail_on_a_thread(stderr: ChildStderr) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(read_tail(stderr));
    });
    receiver
}

/// Reads `stderr` to its end and returns its last `STDERR_TAIL_BYTES` as one line of text.
pub fn read_tail(mut stderr: impl Read) -> String {
    let mut tail = Vec::new();
    let mut cut = false;
    let mut chunk = [0; READ_CHUNK_BYTES];
    loop {
        let count = match stderr.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        tail.extend_from_slice(&chunk[..count]);
        if tail.len() > STDERR_TAIL_BYTES {
            tail.drain(..tail.len() - STDERR_TAIL_BYTES);
            cut = true;
        }
    }
    let text = String::from_utf8_lossy(&tail);
    // A cut tail can start inside a URL, where the scrub cannot find its scheme. A URL ends at
    // whitespace, so the tail starts after the first one.
    let whole = if cut {
        text.split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    whole.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Gives the program until `until` to exit, then stops it.
fn wait_then_stop(child: &mut Child, until: Instant) {
    while Instant::now() < until && matches!(child.try_wait(), Ok(None)) {
        thread::sleep(EXIT_POLL_INTERVAL);
    }
    stop(child);
}

/// Kills the program if it still runs, and reaps it.
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A URL with a user name, a password, a token and a fragment.
    const URL_WITH_SECRETS: &str = "https://user:hunter2@games.example/g?token=SECRET#frag";

    #[test]
    fn a_short_stderr_is_kept_whole_on_one_line() {
        let tail = read_tail("no browser\n  at launch\n".as_bytes());
        assert_eq!(tail, "no browser at launch");
    }

    #[test]
    fn a_long_stderr_keeps_its_end() {
        let end = "y".repeat(STDERR_TAIL_BYTES / 2);
        let stderr = format!("{} {end}", "x".repeat(STDERR_TAIL_BYTES));
        assert_eq!(read_tail(stderr.as_bytes()), end);
    }

    #[test]
    fn a_tail_cut_inside_a_url_starts_after_it() {
        // The cut falls 19 bytes before the end of the URL, after its scheme.
        let end = "y".repeat(STDERR_TAIL_BYTES - 20);
        let stderr = format!("{URL_WITH_SECRETS} {end}");
        assert_eq!(read_tail(stderr.as_bytes()), end);
    }

    /// Tests that run a `#!/bin/sh` program. They take turns: a process that another test starts
    /// holds a copy of every open file until it runs its program, and a program that is open for
    /// writing cannot start ("Text file busy").
    #[cfg(unix)]
    mod with_a_program {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::Path;
        use std::sync::{Mutex, MutexGuard, PoisonError};

        use dyno_schema::ids::MeasureId;
        use dyno_schema::measure::MeasureValue;
        use dyno_schema::probe::ProbeEvent;
        use dyno_schema::profile::{HostProfile, Scenario};
        use dyno_schema::result::DriverInfo;
        use dyno_schema::runner::RunMark;

        use super::*;
        use crate::run_lab;

        /// Long enough for any test program that does not hang.
        const TIMEOUT_MS: u32 = 10_000;
        /// Short, for the program that hangs.
        const SHORT_TIMEOUT_MS: u32 = 300;
        /// Far longer than the short timeout.
        const HANG_SECONDS: u32 = 30;
        /// Bytes of stderr that fill a pipe many times over.
        const FLOOD_BYTES: usize = 1_000_000;

        static TURN: Mutex<()> = Mutex::new(());

        /// This test's turn to write and start programs, until the guard drops.
        fn take_turn() -> MutexGuard<'static, ()> {
            TURN.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn request(timeout_ms: u32) -> RunRequest {
            RunRequest {
                protocol_version: RUNNER_PROTOCOL_VERSION,
                launch_url: "https://example.com/game?session=1".to_owned(),
                host_profile: HostProfile::reels(),
                scenario: Scenario::Alone,
                spins: None,
                idle_after_ms: 1_000,
                timeout_ms,
            }
        }

        fn hello(protocol_version: u32) -> DriverMessage {
            DriverMessage::Hello {
                protocol_version,
                driver: DriverInfo {
                    name: "fake".to_owned(),
                    version: "1".to_owned(),
                    browser: None,
                },
            }
        }

        fn done() -> DriverMessage {
            DriverMessage::Done { t_ms: 1_000.0 }
        }

        /// Shell lines that write `messages` on stdout, one line each.
        fn send(messages: &[DriverMessage]) -> String {
            let lines: String = messages
                .iter()
                .map(|message| encode_line(message).unwrap())
                .collect();
            format!("cat <<'END'\n{lines}END\n")
        }

        /// A driver for a `#!/bin/sh` program in `dir` that reads the request, then runs `body`.
        fn program(dir: &Path, body: &str) -> ExecDriver {
            let path = dir.join("driver.sh");
            fs::write(&path, format!("#!/bin/sh\nread -r request\n{body}")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            ExecDriver::new(path)
        }

        fn run_program(body: &str, timeout_ms: u32) -> Result<Vec<DriverMessage>, DriverError> {
            let dir = tempfile::tempdir().unwrap();
            program(dir.path(), body).run(&request(timeout_ms))
        }

        fn assert_protocol_error(outcome: &Result<Vec<DriverMessage>, DriverError>) -> &str {
            match outcome {
                Err(error) if error.code() == DriverErrorCode::Protocol => error.detail(),
                other => panic!("not a protocol error: {other:?}"),
            }
        }

        #[test]
        fn the_program_gets_the_request_as_one_line_on_stdin() {
            let _turn = take_turn();
            let dir = tempfile::tempdir().unwrap();
            let saved = dir.path().join("request.json");
            let body = format!(
                "printf '%s\\n' \"$request\" > '{}'\n{}",
                saved.display(),
                send(&[hello(RUNNER_PROTOCOL_VERSION), done()])
            );
            program(dir.path(), &body)
                .run(&request(TIMEOUT_MS))
                .unwrap();
            let line = fs::read_to_string(saved).unwrap();
            let received: RunRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(received, request(TIMEOUT_MS));
        }

        #[test]
        fn a_clean_run_gives_lab_measures() {
            let _turn = take_turn();
            let messages = [
                hello(RUNNER_PROTOCOL_VERSION),
                DriverMessage::Mark {
                    mark: RunMark::Navigated,
                    t_ms: 0.0,
                },
                DriverMessage::Probe {
                    event: ProbeEvent::FirstFrame { t_ms: 120.0 },
                },
                DriverMessage::Mark {
                    mark: RunMark::Idle,
                    t_ms: 900.0,
                },
                done(),
            ];
            let dir = tempfile::tempdir().unwrap();
            let mut driver = program(dir.path(), &send(&messages));
            let lab = run_lab(&mut driver, &request(TIMEOUT_MS));
            assert_eq!(
                lab.driver.map(|driver| driver.name),
                Some("fake".to_owned())
            );
            let first_frame = lab
                .measures
                .iter()
                .find(|measure| measure.id == MeasureId::LabTimeFirstFrame)
                .unwrap();
            assert_eq!(first_frame.value, Some(MeasureValue::Integer(120)));
        }

        #[test]
        fn the_messages_end_at_done_or_error() {
            let _turn = take_turn();
            let error = DriverMessage::Error {
                message: "the game frame crashed".to_owned(),
                t_ms: 5.0,
            };
            let body = send(&[hello(RUNNER_PROTOCOL_VERSION), error.clone(), done()]);
            let messages = run_program(&body, TIMEOUT_MS).unwrap();
            assert_eq!(messages, [hello(RUNNER_PROTOCOL_VERSION), error]);
        }

        #[test]
        fn a_line_that_is_not_a_message_is_a_protocol_error() {
            let _turn = take_turn();
            let body = format!(
                "{}echo 'not json'\n",
                send(&[hello(RUNNER_PROTOCOL_VERSION)])
            );
            let outcome = run_program(&body, TIMEOUT_MS);
            assert!(assert_protocol_error(&outcome).starts_with("line 2 is not a runner message"));
        }

        #[test]
        fn another_protocol_version_is_a_protocol_error() {
            let _turn = take_turn();
            let outcome = run_program(&send(&[hello(99), done()]), TIMEOUT_MS);
            assert!(assert_protocol_error(&outcome).contains("protocol version 99"));
        }

        #[test]
        fn a_first_message_that_is_not_hello_is_a_protocol_error() {
            let _turn = take_turn();
            let outcome = run_program(&send(&[done()]), TIMEOUT_MS);
            assert_eq!(
                assert_protocol_error(&outcome),
                "the first message is not hello"
            );
        }

        #[test]
        fn an_early_exit_is_a_protocol_error_with_the_end_of_stderr() {
            let _turn = take_turn();
            let body = format!(
                "{}echo 'no browser found' >&2\nexit 3\n",
                send(&[hello(RUNNER_PROTOCOL_VERSION)])
            );
            let outcome = run_program(&body, TIMEOUT_MS);
            assert_eq!(
                assert_protocol_error(&outcome),
                "stdout closed before done or error; stderr ends: no browser found"
            );
        }

        #[test]
        fn a_secret_in_a_url_on_stderr_is_scrubbed() {
            let _turn = take_turn();
            let body = format!("echo 'could not load {URL_WITH_SECRETS}' >&2\nexit 1\n");
            let outcome = run_program(&body, TIMEOUT_MS);
            let detail = assert_protocol_error(&outcome);
            assert!(
                detail.ends_with("could not load https://games.example/g"),
                "{detail}"
            );
            for secret in ["user", "hunter2", "SECRET", "frag"] {
                assert!(!detail.contains(secret), "{secret} in {detail}");
            }
        }

        #[test]
        fn a_program_that_fills_its_stderr_pipe_still_runs() {
            let _turn = take_turn();
            let body = format!(
                "head -c {FLOOD_BYTES} /dev/zero | tr '\\0' x >&2\n{}",
                send(&[hello(RUNNER_PROTOCOL_VERSION), done()])
            );
            let outcome = run_program(&body, TIMEOUT_MS);
            assert!(outcome.is_ok(), "{outcome:?}");
        }

        #[test]
        fn a_timeout_kills_the_program() {
            let _turn = take_turn();
            let dir = tempfile::tempdir().unwrap();
            let pid_file = dir.path().join("pid");
            let body = format!(
                "echo $$ > '{}'\nexec sleep {HANG_SECONDS}\n",
                pid_file.display()
            );
            let started = Instant::now();
            let outcome = program(dir.path(), &body).run(&request(SHORT_TIMEOUT_MS));
            let error = outcome.unwrap_err();
            assert_eq!(error.code(), DriverErrorCode::Run);
            assert_eq!(
                error.detail(),
                format!("the run did not end within {SHORT_TIMEOUT_MS} ms")
            );
            assert!(started.elapsed() < Duration::from_secs(HANG_SECONDS.into()) / 2);
            let pid = fs::read_to_string(pid_file).unwrap();
            let still_runs = Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(!still_runs, "process {pid} still runs");
        }

        #[test]
        fn a_program_that_does_not_start_is_a_run_error() {
            let _turn = take_turn();
            let dir = tempfile::tempdir().unwrap();
            let mut driver = ExecDriver::new(dir.path().join("no-such-driver"));
            let error = driver.run(&request(TIMEOUT_MS)).unwrap_err();
            assert_eq!(error.code(), DriverErrorCode::Run);
            assert!(
                error.detail().starts_with("cannot start"),
                "{}",
                error.detail()
            );
        }
    }
}
