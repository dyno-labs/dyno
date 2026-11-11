//! `--open-with`: a browser on this machine that dyno starts on the harness page, reads the
//! memory of through the run, and stops when the run ends.
//!
//! The program gets the harness page's URL as its only argument, so a script that sets up the
//! browser, such as one that starts a virtual display and then runs WebKit's MiniBrowser, can
//! stand in for it. It runs in a process group of its own, so stopping the group stops
//! everything it started. The sampler reads every process under it, as the CDP driver reads
//! Chromium.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dyno_run::memory::{self, Reading, Sampler};
use dyno_schema::runner::{AssignedRole, DriverMessage};
use dyno_schema::timeline::ProcessRole;

use crate::ServeError;

/// How long the browser has to exit after it is asked to, before it is killed.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// How often the exit is checked while dyno waits for it.
const EXIT_POLL: Duration = Duration::from_millis(50);

/// A browser that dyno started, and the sampler that reads its processes.
pub struct LocalBrowser {
    child: Child,
    /// None once `close` took it.
    sampler: Option<Sampler>,
    /// Whether `close` stopped the group, so dropping it does not signal a group id that may
    /// have been reused.
    stopped: bool,
}

impl LocalBrowser {
    /// Starts the program on this URL, in a process group of its own, and starts reading the
    /// memory of its processes.
    pub fn open(program: &Path, url: &str) -> Result<LocalBrowser, ServeError> {
        let mut command = Command::new(program);
        command
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let child = command
            .spawn()
            .map_err(|error| ServeError::OpenWith(format!("{}: {error}", program.display())))?;
        let sampler = Some(Sampler::start(child.id()));
        Ok(LocalBrowser {
            child,
            sampler,
            stopped: false,
        })
    }

    /// Stops the sampler, then the browser and everything it started, and gives the readings
    /// from `navigated` to `ended` as `memory` messages, with a `roles` message at `t_ms`. The
    /// readings after `ended` are left out: the harness drops the game when the run ends.
    ///
    /// The program is the `browser`, and a GPU process is `gpu`. The game's process is the web
    /// process with the largest footprint at `idle`, or in the last reading when the run never
    /// went idle: in WebKit on Linux, as in Safari, the page and its game frame share it.
    pub fn close(
        mut self,
        navigated: Instant,
        idle: Option<Instant>,
        ended: Instant,
        t_ms: f64,
    ) -> Vec<DriverMessage> {
        let mut readings = self.sampler.take().map(Sampler::stop).unwrap_or_default();
        readings.retain(|reading| reading.at <= ended);
        stop_group(&mut self.child);
        self.stopped = true;
        let root = self.child.id();
        let roles = roles(&readings, root, idle);
        let mut messages = memory::memory_messages(readings, navigated);
        messages.push(DriverMessage::Roles { roles, t_ms });
        messages
    }
}

impl Drop for LocalBrowser {
    fn drop(&mut self) {
        if !self.stopped {
            stop_group(&mut self.child);
        }
    }
}

/// Asks the child's process group to stop, kills it after [`EXIT_WAIT`], and reaps the child.
fn stop_group(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        // The group can outlive its leader, such as a virtual display it started.
        signal_group(child, Signal::Kill);
        return;
    }
    signal_group(child, Signal::Stop);
    let until = Instant::now() + EXIT_WAIT;
    while Instant::now() < until {
        if matches!(child.try_wait(), Ok(Some(_))) {
            // The group can outlive its leader, such as a virtual display it started.
            signal_group(child, Signal::Kill);
            return;
        }
        std::thread::sleep(EXIT_POLL);
    }
    signal_group(child, Signal::Kill);
    let _ = child.wait();
}

enum Signal {
    Stop,
    Kill,
}

/// Sends the signal to the process group that the child leads, which `process_group(0)` made
/// at spawn. A group that is gone is fine.
#[cfg(unix)]
fn signal_group(child: &mut Child, signal: Signal) {
    use rustix::process::{Pid, Signal as Sig, kill_process_group};

    let signal = match signal {
        Signal::Stop => Sig::TERM,
        Signal::Kill => Sig::KILL,
    };
    if let Some(group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(group, signal);
    }
}

#[cfg(not(unix))]
fn signal_group(child: &mut Child, _signal: Signal) {
    let _ = child.kill();
}

/// The roles of the processes the readings saw: the root is the `browser`, a GPU process is
/// `gpu`, and the web process with the largest footprint at idle is the `game`.
fn roles(readings: &[Reading], root: u32, idle: Option<Instant>) -> Vec<AssignedRole> {
    let mut roles = vec![AssignedRole {
        pid: root,
        role: ProcessRole::Browser,
    }];
    let mut seen = Vec::new();
    for process in readings.iter().flat_map(|reading| &reading.processes) {
        if process.pid != root && is_gpu(&process.kind) && !seen.contains(&process.pid) {
            seen.push(process.pid);
            roles.push(AssignedRole {
                pid: process.pid,
                role: ProcessRole::Gpu,
            });
        }
    }
    let at_idle = readings
        .iter()
        .rev()
        .find(|reading| idle.is_none_or(|idle| reading.at <= idle))
        .or(readings.last());
    let game = at_idle.and_then(|reading| {
        reading
            .processes
            .iter()
            .filter(|process| is_web_process(&process.kind))
            .max_by_key(|process| process.private_bytes)
    });
    if let Some(game) = game {
        roles.push(AssignedRole {
            pid: game.pid,
            role: ProcessRole::Game,
        });
    }
    roles
}

/// A GPU process: WebKit's or Chromium's.
fn is_gpu(kind: &str) -> bool {
    kind.contains("GPUProcess") || kind == memory::GPU_KIND
}

/// A process that runs pages: WebKit's web process or Chromium's renderer.
fn is_web_process(kind: &str) -> bool {
    kind.contains("WebProcess") || kind == "renderer"
}

#[cfg(test)]
mod tests {
    use dyno_schema::runner::ProcessMemory;

    use super::*;

    fn reading(at: Instant, processes: &[(u32, &str, u64)]) -> Reading {
        Reading {
            at,
            processes: processes
                .iter()
                .map(|&(pid, kind, private_bytes)| ProcessMemory {
                    pid,
                    kind: kind.to_owned(),
                    private_bytes,
                    resident_bytes: None,
                    shared_bytes: None,
                })
                .collect(),
        }
    }

    fn role_of(roles: &[AssignedRole], pid: u32) -> Option<ProcessRole> {
        roles
            .iter()
            .rev()
            .find(|role| role.pid == pid)
            .map(|role| role.role)
    }

    #[test]
    fn the_game_is_the_largest_web_process_at_idle() {
        let start = Instant::now();
        let idle = start + Duration::from_secs(1);
        let readings = [
            reading(
                start,
                &[
                    (1, "MiniBrowser", 50),
                    (2, "WebKitWebProcess", 300),
                    (3, "WebKitWebProcess", 100),
                    (4, "WebKitGPUProcess", 80),
                    (5, "WebKitNetworkProcess", 400),
                ],
            ),
            // After idle, another web process grows; it is not the game's.
            reading(
                start + Duration::from_secs(2),
                &[(2, "WebKitWebProcess", 300), (3, "WebKitWebProcess", 900)],
            ),
        ];
        let roles = roles(&readings, 1, Some(idle));
        assert_eq!(role_of(&roles, 1), Some(ProcessRole::Browser));
        assert_eq!(role_of(&roles, 2), Some(ProcessRole::Game));
        assert_eq!(role_of(&roles, 3), None);
        assert_eq!(role_of(&roles, 4), Some(ProcessRole::Gpu));
        assert_eq!(role_of(&roles, 5), None);
    }

    #[test]
    fn with_no_idle_the_last_reading_names_the_game() {
        let start = Instant::now();
        let readings = [
            reading(start, &[(2, "renderer", 300), (3, "renderer", 100)]),
            reading(start, &[(2, "renderer", 300), (3, "renderer", 900)]),
        ];
        assert_eq!(
            role_of(&roles(&readings, 1, None), 3),
            Some(ProcessRole::Game)
        );
    }

    /// Starts `/bin/sh` on a script with this body, as `open` starts a launcher with its URL.
    /// The shell reads the script rather than running the file: running a file that was just
    /// written fails with ETXTBSY when another test forks while it is still open for writing.
    /// The script is deleted when the returned path is dropped.
    #[cfg(unix)]
    fn open_script(body: &str) -> (LocalBrowser, tempfile::TempPath) {
        let script = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(script.path(), body).unwrap();
        let path = script.into_temp_path();
        let browser = LocalBrowser::open(Path::new("/bin/sh"), path.to_str().unwrap()).unwrap();
        (browser, path)
    }

    /// Whether the group still has a process 2 s on. A killed process whose parent has gone
    /// stays a zombie, which signal 0 still finds, until the system reaps it.
    #[cfg(unix)]
    fn group_alive(group: u32) -> bool {
        let group = rustix::process::Pid::from_raw(i32::try_from(group).unwrap()).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while rustix::process::test_kill_process_group(group).is_ok() {
            if Instant::now() > until {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[cfg(unix)]
    #[test]
    fn closing_stops_what_a_program_that_exited_left_running() {
        let started = Instant::now();
        let (browser, _script) = open_script("sleep 30 &\nexit 0\n");
        let group = browser.child.id();
        std::thread::sleep(Duration::from_millis(300));
        browser.close(started, None, Instant::now(), 0.0);
        assert!(!group_alive(group), "the group outlived close");
    }

    #[cfg(unix)]
    #[test]
    fn dropping_a_browser_that_was_not_closed_stops_its_group() {
        let (browser, _script) = open_script("sleep 30 &\nexec sleep 30\n");
        let group = browser.child.id();
        std::thread::sleep(Duration::from_millis(100));
        drop(browser);
        assert!(!group_alive(group), "the group outlived the drop");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn closing_stops_the_program_and_what_it_started() {
        let started = Instant::now();
        let (browser, _script) = open_script("sleep 30 &\nexec sleep 30\n");
        let group = browser.child.id();
        std::thread::sleep(Duration::from_millis(700));
        // The run ended at 300 ms: the readings after it are left out.
        let ended = started + Duration::from_millis(300);
        let messages = browser.close(started, None, ended, 0.0);
        assert!(messages.iter().all(|message| match message {
            DriverMessage::Memory { t_ms, .. } => *t_ms <= 300.0,
            _ => true,
        }));
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, DriverMessage::Memory { .. })),
            "{messages:?}"
        );
        assert!(matches!(messages.last(), Some(DriverMessage::Roles { .. })));
        assert!(!group_alive(group), "the group outlived close");
        assert!(started.elapsed() < EXIT_WAIT, "{:?}", started.elapsed());
    }
}
