//! A served check through the `dyno` binary, with this test as the browser on the network.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};

use common::{NO_CHROMIUM, Outcome, data_dir};

/// One HTTP exchange: the status line and the body.
fn exchange(address: &str, method: &str, path: &str, body: &str) -> (String, String) {
    let mut socket = TcpStream::connect(address).unwrap();
    write!(
        socket,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

/// Polls for the next command until it is not `wait`.
fn next_command(harness: &str) -> String {
    loop {
        let (_, body) = exchange(harness, "GET", "/__dyno/next", "");
        if !body.contains(r#""wait""#) {
            return body;
        }
    }
}

/// A served check in progress: dyno takes free ports, and prints the harness page's URL.
struct Served {
    child: Child,
    /// The harness page's host and port.
    harness: String,
    /// What dyno printed before the URL, and the reader of the rest.
    stderr_before: String,
    stderr_rest: JoinHandle<String>,
}

/// Starts `dyno check` with `args` in the test data folder, and reads the URL it prints.
fn start(extra_env: &[(&str, &str)], args: &[&str]) -> Served {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dyno"))
        .args(args)
        .args(["--serve-host", "127.0.0.1", "--serve-port", "0"])
        .current_dir(data_dir())
        .env("DYNO_CHROMIUM", NO_CHROMIUM)
        .envs(extra_env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the dyno binary runs");
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut stderr_before = String::new();
    let harness = loop {
        let mut line = String::new();
        assert!(
            stderr.read_line(&mut line).unwrap() > 0,
            "dyno printed no URL: {stderr_before}"
        );
        stderr_before.push_str(&line);
        if let Some(rest) = line.split("open http://").nth(1) {
            break rest.split('/').next().unwrap().to_owned();
        }
    };
    let stderr_rest = thread::spawn(move || {
        let mut rest = String::new();
        let _ = stderr.read_to_string(&mut rest);
        rest
    });
    Served {
        child,
        harness,
        stderr_before,
        stderr_rest,
    }
}

impl Served {
    /// Waits for dyno to exit.
    fn finish(self) -> Outcome {
        let output = self.child.wait_with_output().unwrap();
        Outcome {
            args: Vec::new(),
            code: output.status.code().unwrap(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: self.stderr_before + &self.stderr_rest.join().unwrap(),
        }
    }
}

/// Plays the browser that opens the harness page, through to the end of the run. `query` is
/// the query string the game loads with, which the page must name.
fn open_as_browser(harness: &str, query: &str) {
    let (_, page) = exchange(harness, "GET", "/", "");
    let game_url = page
        .split("const GAME_URL = \"http://")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    let (game, path) = game_url.split_once('/').unwrap();
    assert_eq!(path, query);
    exchange(
        harness,
        "POST",
        "/__dyno/hello",
        r#"{"user_agent":"curl/8.0"}"#,
    );
    exchange(game, "GET", &format!("/{query}"), "");
    exchange(
        harness,
        "POST",
        "/__dyno/events",
        r#"[{"kind":"navigated_at","epoch_ms":1000},{"kind":"document","time_origin":1050,"device_pixel_ratio":3}]"#,
    );
    assert_eq!(next_command(harness), r#"{"command":"self_check"}"#);
    exchange(
        harness,
        "POST",
        "/__dyno/events",
        r#"[{"kind":"self_check","webgl_renderer":"Apple GPU"}]"#,
    );
    assert_eq!(
        next_command(harness),
        r#"{"command":"send","name":"reels:active"}"#
    );
    assert_eq!(next_command(harness), r#"{"command":"flush"}"#);
    exchange(harness, "POST", "/__dyno/events", r#"[{"kind":"flushed"}]"#);
    assert_eq!(next_command(harness), r#"{"command":"done"}"#);
}

/// The measure with this id in a result's run part.
fn run_measure<'r>(result: &'r serde_json::Value, id: &str) -> &'r serde_json::Value {
    result["run"]["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|measure| measure["id"] == id)
        .unwrap()
}

#[test]
fn a_served_check_runs_in_the_browser_that_opens_it_and_records_no_query() {
    let served = start(
        &[],
        &[
            "check",
            "tiny-build",
            "--driver",
            "serve",
            "--serve-query",
            "sessionID=secret",
        ],
    );
    let harness = served.harness.clone();
    open_as_browser(&harness, "?sessionID=secret");

    let outcome = served.finish();
    assert!(
        outcome
            .stderr
            .contains(&format!("open http://{harness}/ in the browser")),
        "{}",
        outcome.stderr
    );
    assert!(!outcome.stdout.contains("secret"));
    let result: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    assert_eq!(result["run"]["driver"]["name"], "serve");
    assert_eq!(
        result["run"]["target"]["launch_url"],
        serde_json::Value::Null
    );
    assert_eq!(result["deterministic"]["inputs"]["launch_url"], true);
    let frame_dpr = run_measure(&result, "lab.self_check.frame_dpr");
    assert_eq!(frame_dpr["status"], "measured", "{frame_dpr}");
}

/// The fake xcrun, for the device runs.
fn fake_xcrun() -> String {
    data_dir().join("fake-xcrun.sh").display().to_string()
}

#[test]
fn a_device_check_records_the_phone_and_reads_its_footprint_at_idle() {
    let folder = tempfile::tempdir().unwrap();
    let out = folder.path().join("result.json");
    // A trace that an earlier check kept is replaced.
    std::fs::create_dir(folder.path().join("result.trace")).unwrap();
    let xcrun = fake_xcrun();
    let served = start(
        &[("DYNO_XCRUN", &xcrun)],
        &[
            "check",
            "tiny-build",
            "--driver",
            "serve",
            "--device",
            "--out",
            &out.display().to_string(),
        ],
    );
    open_as_browser(&served.harness.clone(), "");

    let outcome = served.finish();
    assert!(
        outcome
            .stderr
            .contains("recording Test iPhone with xctrace"),
        "{}",
        outcome.stderr
    );
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(result["deterministic"]["inputs"]["device"], true);
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
    assert_eq!(footprint["value"], 209_715_200);
    assert!(
        footprint["evidence"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("Test iPhone, a iPhone 14 Pro Max on 26.6.2")
    );
    let kills = run_measure(&result, "device.kills");
    assert_eq!(kills["value"], 0, "{kills}");
    assert!(folder.path().join("result.trace").is_dir());
}

#[test]
fn a_trace_that_cannot_be_read_leaves_the_device_measures_missing() {
    let xcrun = fake_xcrun();
    let served = start(
        &[("DYNO_XCRUN", &xcrun), ("FAKE_XCRUN_FAIL", "export")],
        &["check", "tiny-build", "--driver", "serve", "--device"],
    );
    open_as_browser(&served.harness.clone(), "");

    let outcome = served.finish();
    let result: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "missing", "{footprint}");
    assert!(
        footprint["reason"]
            .as_str()
            .unwrap()
            .starts_with("cannot read the trace: xctrace export failed")
    );
}

/// A device check that stops before its run, with the fake xcrun's environment.
fn device_check_before_the_run(env: &[(&str, &str)]) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_dyno"))
        .args([
            "check",
            "tiny-build",
            "--driver",
            "serve",
            "--serve-port",
            "0",
            "--device",
        ])
        .current_dir(data_dir())
        .env("DYNO_XCRUN", fake_xcrun())
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn a_mac_without_xctrace_or_a_recording_that_does_not_start_stops_the_check() {
    let (code, stderr) = device_check_before_the_run(&[("FAKE_XCRUN_FAIL", "list")]);
    assert_eq!(code, Some(64), "{stderr}");
    assert!(stderr.contains("cannot run xcrun"), "{stderr}");
    let (code, stderr) = device_check_before_the_run(&[("FAKE_XCRUN_FAIL", "record")]);
    assert_eq!(code, Some(70), "{stderr}");
    assert!(
        stderr.contains("xctrace did not start recording: xctrace record failed"),
        "{stderr}"
    );
}

#[test]
fn a_device_check_with_no_phone_connected_is_a_usage_error() {
    let (code, stderr) = device_check_before_the_run(&[("FAKE_XCRUN_PHONES", "0")]);
    assert_eq!(code, Some(64), "{stderr}");
    assert!(stderr.contains("no iPhone is connected"), "{stderr}");
}
