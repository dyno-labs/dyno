//! A served check through the `dyno` binary, with this test as the browser on the network.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use common::{NO_CHROMIUM, Outcome, copy_dir, data_dir};

/// One HTTP exchange with a server, named by its host and port and, for the harness page, the
/// rest of its URL as dyno printed it: the status line and the body. The run key of a harness
/// page's URL goes in the key header, as the page's own requests send it.
fn exchange(server: &str, method: &str, path: &str, body: &str) -> (String, String) {
    let (address, rest) = server.split_once('/').unwrap_or((server, ""));
    let key = rest
        .split_once("key=")
        .map_or(String::new(), |(_, key)| format!("X-Dyno-Key: {key}\r\n"));
    let mut socket = TcpStream::connect(address).unwrap();
    write!(
        socket,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\n{key}Content-Length: {}\r\n\r\n{body}",
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
    /// The harness page's URL as dyno printed it, without its `http://`: its host and port, and
    /// the run's key.
    harness: String,
    /// What dyno printed before the URL, and the reader of the rest.
    stderr_before: String,
    stderr_rest: JoinHandle<String>,
    /// What dyno has printed after the URL so far.
    stderr_so_far: Arc<Mutex<String>>,
}

/// Starts `dyno check` with `args` in the test data folder, served on the loopback address, and
/// reads the URL it prints.
fn start(extra_env: &[(&str, &str)], args: &[&str]) -> Served {
    let mut at_loopback = args.to_vec();
    at_loopback.extend_from_slice(&["--serve-host", "127.0.0.1"]);
    start_where_dyno_chooses(extra_env, &at_loopback)
}

/// Starts `dyno check` with `args` in the test data folder, and reads the URL it prints. With no
/// `--serve-host` in `args`, dyno chooses the address it serves at.
fn start_where_dyno_chooses(extra_env: &[(&str, &str)], args: &[&str]) -> Served {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dyno"))
        .args(args)
        .args(["--serve-port", "0"])
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
            break rest.split(' ').next().unwrap().to_owned();
        }
    };
    let stderr_so_far = Arc::new(Mutex::new(String::new()));
    let so_far = Arc::clone(&stderr_so_far);
    let stderr_rest = thread::spawn(move || {
        let mut line = String::new();
        while stderr.read_line(&mut line).is_ok_and(|read| read > 0) {
            so_far.lock().unwrap().push_str(&line);
            line.clear();
        }
        so_far.lock().unwrap().clone()
    });
    Served {
        child,
        harness,
        stderr_before,
        stderr_rest,
        stderr_so_far,
    }
}

impl Served {
    /// Waits until dyno has printed a line with `words` after the URL, and gives that line.
    fn wait_for_stderr(&self, words: &str, within: Duration) -> String {
        let until = Instant::now() + within;
        loop {
            let so_far = self.stderr_so_far.lock().unwrap().clone();
            if let Some(line) = so_far.lines().find(|line| line.contains(words)) {
                return line.to_owned();
            }
            assert!(
                Instant::now() < until,
                "dyno did not say {words:?}: {so_far}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

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

/// The user agent of Safari 26.2 on an iPhone, which freezes the iOS version at 18.6.
const IPHONE_SAFARI: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.2 Mobile/15E148 Safari/604.1";

/// Plays the browser that opens the harness page, through to the end of the run. `query` is
/// the query string the game loads with, which the page must name.
fn open_as_browser(harness: &str, query: &str) {
    let game = say_hello(harness, query);
    finish_as_browser(harness, &game, query);
}

/// Plays the browser that opens the harness page, as far as its hello, after which the game
/// server serves it. Gives the game server's host and port.
fn say_hello(harness: &str, query: &str) -> String {
    let (_, page) = exchange(harness, "GET", "/", "");
    let game_url = page
        .split("const GAME_URL = \"http://")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    let (game, path) = game_url.split_once('/').unwrap();
    assert_eq!(path, query);
    let (status, _) = exchange(
        harness,
        "POST",
        "/__dyno/hello",
        &format!(r#"{{"user_agent":"{IPHONE_SAFARI}"}}"#),
    );
    assert_eq!(status, "HTTP/1.1 200 OK");
    game.to_owned()
}

/// Plays the browser whose page has said hello, through to the end of the run.
fn finish_as_browser(harness: &str, game: &str, query: &str) {
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

/// Plays a browser that opens the harness page, says hello and loads the game, whose document
/// posts, and then loses its link to dyno: it takes no command.
fn open_and_lose_the_link(harness: &str) {
    let (_, page) = exchange(harness, "GET", "/", "");
    let game_url = page
        .split("const GAME_URL = \"http://")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    let (game, _) = game_url.split_once('/').unwrap();
    exchange(
        harness,
        "POST",
        "/__dyno/hello",
        &format!(r#"{{"user_agent":"{IPHONE_SAFARI}"}}"#),
    );
    exchange(game, "GET", "/", "");
    exchange(
        harness,
        "POST",
        "/__dyno/events",
        r#"[{"kind":"navigated_at","epoch_ms":1000},{"kind":"document","time_origin":1050,"device_pixel_ratio":3}]"#,
    );
}

/// The record of each run in a result, in order.
fn run_records(result: &serde_json::Value) -> &Vec<serde_json::Value> {
    result["run"]["runs"].as_array().unwrap()
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
            .contains(&format!("open http://{harness} in the browser")),
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
    assert_eq!(result["run"]["build_link"], "served");
    // The address was given, so dyno does not know which path it is. No lab was named.
    assert_eq!(result["run"]["serve_path"], "given");
    assert_eq!(result["run"]["lab"], serde_json::Value::Null);
}

#[test]
fn the_run_s_key_is_printed_for_the_person_who_opens_the_page_and_stored_nowhere() {
    let folder = tempfile::tempdir().unwrap();
    let out = folder.path().join("result.json");
    let served = start(
        &[("DYNO_FAKE_PHONE", "")],
        &[
            "check",
            "tiny-build",
            "--driver",
            "serve",
            "--device",
            "--no-open",
            "--out",
            &out.display().to_string(),
        ],
    );
    let harness = served.harness.clone();
    let (address, key) = harness.split_once("/?key=").unwrap();
    assert_eq!(key.len(), 32, "{harness}");
    // A page without the key, at the same address, starts nothing.
    let hello = format!(r#"{{"user_agent":"{IPHONE_SAFARI}"}}"#);
    for path in ["/", "/__dyno/hello", "/__dyno/next"] {
        assert_eq!(
            exchange(address, "POST", path, &hello).0,
            "HTTP/1.1 403 Forbidden",
            "{path}"
        );
    }
    open_as_browser(&harness, "");
    let outcome = served.finish();
    // The person was told the URL, key and all.
    assert!(
        outcome
            .stderr
            .contains(&format!("open http://{harness} in the browser")),
        "{}",
        outcome.stderr
    );
    // Nothing that dyno stores holds it: the result and the phone's recording.
    let recording = folder.path().join("result.device.json");
    for file in [&out, &recording] {
        let text = std::fs::read_to_string(file).unwrap();
        assert!(!text.contains(key), "{}", file.display());
    }
    assert!(!outcome.stdout.contains(key));
}

#[test]
fn a_served_check_takes_its_environment_from_the_user_agent() {
    let served = start(&[], &["check", "tiny-build", "--driver", "serve"]);
    open_as_browser(&served.harness.clone(), "");
    let result: serde_json::Value = serde_json::from_str(&served.finish().stdout).unwrap();
    // The user agent names no model, and its iOS version is frozen, so neither is read.
    assert_eq!(
        result["run"]["environment"],
        serde_json::json!({
            "browser": { "name": "Safari", "version": "26.2", "engine": "webkit", "headless": false },
            "os": { "name": "ios", "version": null },
            "device": {
                "key": null,
                "name": "iPhone",
                "vendor": "Apple",
                "kind": "phone",
                "memory_bytes": null
            }
        })
    );
}

#[test]
fn a_served_check_gives_the_browser_the_bytes_of_its_scan_and_nothing_else() {
    let folder = tempfile::tempdir().unwrap();
    let build = folder.path().join("build");
    copy_dir(&data_dir().join("tiny-build"), &build);
    let scanned_script = std::fs::read_to_string(build.join("js/game.js")).unwrap();
    // What the scan skips is not in the build id.
    std::fs::create_dir(build.join(".git")).unwrap();
    std::fs::write(build.join(".git/config"), "not the build's").unwrap();
    std::fs::write(build.join(".DS_Store"), "litter").unwrap();
    let served = start(
        &[],
        &["check", &build.display().to_string(), "--driver", "serve"],
    );
    let harness = served.harness.clone();
    // dyno has scanned the build by the time it prints the URL. After that, a file is added,
    // and one is edited: neither the new file nor the new bytes are what the build id covers.
    std::fs::write(build.join("late.js"), "late").unwrap();
    std::fs::write(build.join("js/game.js"), "edited after the scan").unwrap();
    // The game server serves the page's browser once its page has said hello.
    let game = say_hello(&harness, "");
    for path in ["/.git/config", "/.DS_Store", "/late.js"] {
        let (status, _) = exchange(&game, "GET", path, "");
        assert_eq!(status, "HTTP/1.1 404 Not Found", "{path}");
    }
    let (status, script) = exchange(&game, "GET", "/js/game.js", "");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(script, scanned_script);
    finish_as_browser(&harness, &game, "");
    let result: serde_json::Value = serde_json::from_str(&served.finish().stdout).unwrap();
    // Every file of the build id is one that the server gives.
    let files = result["deterministic"]["build"]["files"]
        .as_array()
        .unwrap();
    assert!(!files.is_empty());
    for file in files {
        let path = file["path"].as_str().unwrap();
        assert!(!path.contains(".git") && path != "late.js", "{path}");
    }
    assert_eq!(result["run"]["build_link"], "served");
}

#[test]
fn a_build_with_a_file_at_the_probe_s_path_stops_a_served_check_before_the_phone() {
    let folder = tempfile::tempdir().unwrap();
    let build = folder.path().join("build");
    copy_dir(&data_dir().join("tiny-build"), &build);
    std::fs::create_dir(build.join("__dyno")).unwrap();
    std::fs::write(build.join("__dyno/probe.js"), "the build's own").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dyno"))
        .args(["check", &build.display().to_string()])
        .args(["--driver", "serve", "--serve-port", "0", "--device"])
        .current_dir(data_dir())
        .env("DYNO_FAKE_PHONE", "")
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(64), "{stderr}");
    assert!(
        stderr.contains(
            "the build has a file at __dyno/probe.js, which is where a served run gives the page \
             dyno's own probe: give the file another name"
        ),
        "{stderr}"
    );
    // The phone was not touched.
    assert!(!stderr.contains("recording the"), "{stderr}");
}

#[test]
fn a_build_file_beside_the_probe_s_path_is_served_as_the_build_s() {
    let folder = tempfile::tempdir().unwrap();
    let build = folder.path().join("build");
    copy_dir(&data_dir().join("tiny-build"), &build);
    std::fs::create_dir(build.join("__dyno")).unwrap();
    std::fs::write(build.join("__dyno/data.json"), "{\"the build's\":1}").unwrap();
    let served = start(
        &[],
        &["check", &build.display().to_string(), "--driver", "serve"],
    );
    let harness = served.harness.clone();
    let game = say_hello(&harness, "");
    let (status, body) = exchange(&game, "GET", "/__dyno/data.json", "");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, "{\"the build's\":1}");
    // The probe is still at its own path.
    let (status, probe) = exchange(&game, "GET", "/__dyno/probe.js", "");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(probe.contains("data-dyno-relay"), "{probe}");
    finish_as_browser(&harness, &game, "");
    served.finish();
}

/// The fake phone's UDID (crates/dyno-ios/src/fake.rs).
const FAKE_UDID: &str = "00008120-000000000000001E";

/// The fake xcrun, for devicectl.
fn fake_xcrun() -> String {
    data_dir().join("fake-xcrun.sh").display().to_string()
}

#[test]
fn a_device_check_records_the_phone_and_reads_its_footprint_at_idle() {
    let folder = tempfile::tempdir().unwrap();
    let out = folder.path().join("result.json");
    // A recording that an earlier check saved is replaced.
    let saved = folder.path().join("result.device.json");
    std::fs::write(&saved, "old").unwrap();
    let served = start(
        &[("DYNO_FAKE_PHONE", "")],
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
            .contains("recording the iPhone 14 Pro Max, iOS 26.6.2 (23G90), on USB."),
        "{}",
        outcome.stderr
    );
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(result["deterministic"]["inputs"]["device"], true);
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
    assert_eq!(footprint["value"], 209_715_200);
    assert_eq!(footprint["evidence"][0]["method"], "sysmontap");
    assert_eq!(
        footprint["evidence"][0]["detail"],
        "sysmontap samples of the phone: iPhone 14 Pro Max, iOS 26.6.2 (23G90), process \
         com.apple.WebKit.WebContent (412)"
    );
    let peak = run_measure(&result, "device.phys_footprint_peak");
    assert_eq!(peak["status"], "measured", "{peak}");
    assert_eq!(peak["required"], true, "{peak}");
    assert_eq!(peak["value"], 209_715_200);
    // One run gives no footprint over the empty build, and does not need one.
    let over = run_measure(&result, "device.footprint_over_baseline");
    assert_eq!(over["status"], "unsupported", "{over}");
    assert_eq!(over["required"], false, "{over}");
    assert!(
        result["deterministic"]["inputs"]
            .get("device_runs")
            .is_none()
    );
    // Readers from before the peak measure find it as an item.
    assert_eq!(footprint["items"][0]["key"], "peak during the run");
    let kills = run_measure(&result, "device.kills");
    assert_eq!(kills["value"], 0, "{kills}");
    assert_eq!(kills["evidence"][0]["method"], "syslog");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&saved).unwrap()).unwrap();
    assert_eq!(saved["phone"]["identifier"], "iPhone15,3");
    assert!(!saved["samples"].as_array().unwrap().is_empty());
    // The timeline holds Safari's processes from the phone's samples, with the run's steps.
    let timeline = &result["run"]["timeline"];
    assert_eq!(timeline["method"], "sysmontap", "{timeline}");
    let roles: Vec<(u64, &str)> = timeline["processes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|process| {
            (
                process["pid"].as_u64().unwrap(),
                process["role"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(roles, [(88, "browser"), (90, "gpu"), (412, "game")]);
    let marks: Vec<&str> = timeline["marks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|mark| mark["mark"].as_str().unwrap())
        .collect();
    for step in ["navigated", "idle", "activated"] {
        assert!(marks.contains(&step), "{marks:?}");
    }
}

#[test]
fn a_recording_that_cannot_be_saved_does_not_stop_the_check() {
    let folder = tempfile::tempdir().unwrap();
    let out = folder.path().join("result.json");
    std::fs::create_dir(folder.path().join("result.device.json")).unwrap();
    let served = start(
        &[("DYNO_FAKE_PHONE", "")],
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
            .contains("dyno check: the recording was not saved: cannot write"),
        "{}",
        outcome.stderr
    );
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

#[test]
fn a_device_check_takes_the_model_ios_version_and_memory_from_the_phone() {
    let (outcome, result, _) = device_check(&[], &[], &[]);
    let environment = &result["run"]["environment"];
    // The browser still comes from the user agent.
    assert_eq!(environment["browser"]["name"], "Safari");
    assert_eq!(
        environment["os"],
        serde_json::json!({ "name": "ios", "version": "26.6.2 (23G90)" })
    );
    assert_eq!(
        environment["device"],
        serde_json::json!({
            "key": "iphone-14-pro-max",
            "name": "iPhone 14 Pro Max",
            "vendor": "Apple",
            "kind": "phone",
            "memory_bytes": 6_442_450_944_u64
        })
    );
    // The phone's UDID never reaches the result.
    assert!(!outcome.stdout.contains(FAKE_UDID), "{}", outcome.stdout);
}

#[test]
fn a_baseline_device_check_serves_the_empty_build() {
    let served = start(
        &[("DYNO_FAKE_PHONE", "")],
        &["check", "--baseline", "--driver", "serve", "--device"],
    );
    open_as_browser(&served.harness.clone(), "");
    let result: serde_json::Value = serde_json::from_str(&served.finish().stdout).unwrap();
    assert_eq!(
        result["deterministic"]["build"]["build_id"],
        "7114691fda7748ccc7c091ffeae74100f190d0557000f8739823a040446c5c4e"
    );
    assert_eq!(result["run"]["build_link"], "served");
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

/// A device check of the fake phone with these settings (`DYNO_FAKE_PHONE`), this environment
/// and these extra flags, run to the end with this test as the phone's browser. It gives the
/// outcome, the result and the harness page's host and port.
fn device_check(
    phone: &[&str],
    env: &[(&str, &str)],
    args: &[&str],
) -> (Outcome, serde_json::Value, String) {
    let settings = phone.join(";");
    let mut all_env = vec![("DYNO_FAKE_PHONE", settings.as_str())];
    all_env.extend_from_slice(env);
    let mut all_args = vec!["check", "tiny-build", "--driver", "serve", "--device"];
    all_args.extend_from_slice(args);
    let served = start(&all_env, &all_args);
    let harness = served.harness.clone();
    open_as_browser(&harness, "");
    let outcome = served.finish();
    let result = serde_json::from_str(&outcome.stdout).unwrap();
    (outcome, result, harness)
}

#[test]
fn a_device_check_opens_the_harness_page_in_the_phones_safari() {
    let folder = tempfile::tempdir().unwrap();
    let launched = folder.path().join("launched");
    let setting = format!("launched={}", launched.display());
    let (outcome, result, harness) = device_check(&[&setting], &[], &[]);
    assert_eq!(
        std::fs::read_to_string(&launched).unwrap(),
        format!("http://{harness}")
    );
    assert!(
        outcome
            .stderr
            .contains("opened the page in Safari on the iPhone 14 Pro Max."),
        "{}",
        outcome.stderr
    );
    // The person is still told where the page is, to open it by hand.
    assert!(
        outcome
            .stderr
            .contains(&format!("open http://{harness} in the browser")),
        "{}",
        outcome.stderr
    );
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

#[test]
fn no_open_leaves_safari_to_the_person() {
    let folder = tempfile::tempdir().unwrap();
    let launched = folder.path().join("launched");
    let setting = format!("launched={}", launched.display());
    let (outcome, result, _) = device_check(&[&setting], &[], &["--no-open"]);
    assert!(!launched.exists());
    assert!(
        !outcome.stderr.contains("the page in Safari"),
        "{}",
        outcome.stderr
    );
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

#[test]
fn a_launch_that_fails_says_to_open_the_page_by_hand_and_the_check_runs() {
    let (outcome, result, _) = device_check(&["fail=launch"], &[], &[]);
    assert!(
        outcome.stderr.contains(
            "dyno check: Safari did not open the page: the fake phone did not open it. Open the \
             URL above on the iPhone 14 Pro Max by hand."
        ),
        "{}",
        outcome.stderr
    );
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

#[test]
fn devicectl_opens_the_page_when_the_phone_did_not() {
    let folder = tempfile::tempdir().unwrap();
    let launched = folder.path().join("launched");
    let xcrun = fake_xcrun();
    let env = [
        ("DYNO_XCRUN", xcrun.as_str()),
        ("FAKE_XCRUN_LAUNCHED", launched.to_str().unwrap()),
    ];
    let (outcome, _, harness) = device_check(&["fail=launch"], &env, &[]);
    assert_eq!(
        std::fs::read_to_string(&launched).unwrap().trim(),
        format!("http://{harness}")
    );
    assert!(
        outcome
            .stderr
            .contains("opened the page in Safari on the iPhone 14 Pro Max, with devicectl."),
        "{}",
        outcome.stderr
    );
    let env = [
        ("DYNO_XCRUN", xcrun.as_str()),
        ("FAKE_XCRUN_FAIL", "launch"),
    ];
    let (outcome, _, _) = device_check(&["fail=launch"], &env, &[]);
    assert!(
        outcome.stderr.contains(
            "Safari did not open the page: the fake phone did not open it; devicectl: ERROR: \
             devicectl launch failed for the test."
        ),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_kill_in_the_phone_s_log_is_a_kill() {
    // The run goes idle 10 s after the page opens, so the kill comes before idle.
    let (_, result, _) = device_check(&["kill-after-ms=3000"], &[], &[]);
    let kills = run_measure(&result, "device.kills");
    assert_eq!(kills["value"], 1, "{kills}");
    let seen = kills["items"][0]["key"].as_str().unwrap();
    assert!(
        seen.starts_with(
            "memorystatus: killing process 412 [com.apple.WebKit.WebContent] in high band \
             FOREGROUND (100), "
        ),
        "{seen}"
    );
    let peak = run_measure(&result, "device.phys_footprint_peak");
    assert_eq!(peak["value"], 209_715_200, "{peak}");
    // A process killed before idle has no footprint at idle.
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "missing", "{footprint}");
    // The timeline marks the kill.
    let marks = result["run"]["timeline"]["marks"].as_array().unwrap();
    assert!(
        marks.iter().any(|mark| mark["mark"] == "killed"),
        "{marks:?}"
    );
    // The run's record has the kernel's line, and the page pid that the samples had.
    let recording = &run_records(&result)[0]["recording"];
    assert_eq!(recording["kill"]["pid"], 412, "{recording}");
    assert!(recording["kill"]["received_ms"].is_i64(), "{recording}");
    assert_eq!(recording["pages"][0]["pid"], 412, "{recording}");
}

#[test]
fn a_page_killed_before_its_first_sample_is_in_the_record_and_not_known_to_be_the_games() {
    // The page is never in the samples, and the kernel kills it 3 s after the recording starts.
    let (_, result, _) = device_check(&["kill-after-ms=3000", "sampled-after-ms=600000"], &[], &[]);
    // The measures are as they were: no page process in the samples leaves every one missing.
    let kills = run_measure(&result, "device.kills");
    assert_eq!(kills["status"], "missing", "{kills}");
    assert_eq!(
        kills["reason"],
        "the phone's samples have no Safari WebContent process during the run"
    );
    for id in ["device.phys_footprint", "device.phys_footprint_peak"] {
        let measure = run_measure(&result, id);
        assert_eq!(measure["status"], "missing", "{measure}");
        assert_eq!(
            measure["reason"],
            "the phone's samples have no Safari WebContent process during the run",
            "{measure}"
        );
    }
    let records = run_records(&result);
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record["of"], "game");
    assert_eq!(record["index"], 1);
    assert_eq!(record["page"]["end"], "ended", "{record}");
    // A served run hears the probe, and does not see the frame's document.
    assert_eq!(record["page"]["document"], "not_observed", "{record}");
    assert_eq!(record["page"]["probe"], "heard", "{record}");
    assert!(record["page"].get("events_capped").is_none(), "{record}");
    let recording = &record["recording"];
    assert_eq!(recording["samples"]["read"], 0, "{recording}");
    assert!(recording["samples"]["last_ms"].is_i64(), "{recording}");
    // No page process was sampled; the kill is in the record, with nothing said of whose it was.
    assert_eq!(recording["pages"], serde_json::json!([]), "{recording}");
    assert_eq!(recording["kill"]["pid"], 412, "{recording}");
    assert!(recording["kill"].get("attributed").is_none(), "{recording}");
    assert_eq!(
        recording["log"]["read"].as_u64().map(|read| read > 0),
        Some(true)
    );
    assert_eq!(
        recording["kill"]["line"],
        "memorystatus: killing process 412 [com.apple.WebKit.WebContent] in high band FOREGROUND \
         (100)"
    );
}

#[test]
fn samples_that_stop_leave_the_device_measures_missing_and_say_why() {
    let started = Instant::now();
    let (_, result, _) = device_check(&["fail=samples"], &[], &[]);
    assert!(started.elapsed() < Duration::from_secs(60));
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "missing", "{footprint}");
    let reason = footprint["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("the phone's samples ended")
            && reason.ends_with(": the fake phone stopped sending samples"),
        "{reason}"
    );
    // The phone still names itself.
    assert_eq!(
        result["run"]["environment"]["device"]["key"],
        "iphone-14-pro-max"
    );
    let samples = &run_records(&result)[0]["recording"]["samples"];
    assert_eq!(samples["read"], 0, "{samples}");
    assert_eq!(
        samples["stopped"], "the fake phone stopped sending samples",
        "{samples}"
    );
}

#[test]
fn a_log_that_stops_cannot_tell_a_kill() {
    let (_, result, _) = device_check(&["fail=log"], &[], &[]);
    let kills = run_measure(&result, "device.kills");
    assert_eq!(kills["status"], "missing", "{kills}");
    let reason = kills["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("the phone's log ended")
            && reason.ends_with(": the fake phone's log closed"),
        "{reason}"
    );
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

#[test]
fn a_phone_on_the_network_needs_its_address_and_its_pairing_file() {
    let folder = tempfile::tempdir().unwrap();
    let pairing = folder.path().join("phone.plist");
    let (outcome, result, _) = device_check(
        &[],
        &[],
        &[
            "--device-ip",
            "192.0.2.4",
            "--pairing-file",
            pairing.to_str().unwrap(),
        ],
    );
    assert!(
        outcome
            .stderr
            .contains("recording the iPhone 14 Pro Max, iOS 26.6.2 (23G90), on the network."),
        "{}",
        outcome.stderr
    );
    assert_eq!(result["deterministic"]["inputs"]["device"], true);
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

/// The path a served check's page takes when dyno serves at this machine's own address: the
/// local network's, or the loopback address on a machine with no network.
fn own_address_path() -> (String, &'static str) {
    let address = dyno_serve::local_network_address();
    let path = if address.is_loopback() {
        "loopback"
    } else {
        "local_network"
    };
    (address.to_string(), path)
}

/// A device check of the fake phone at the address that dyno chooses, with this environment and
/// these extra flags, run to the end with this test as the phone's browser. It gives the
/// outcome, the result and the harness page's host and port.
fn device_check_where_dyno_chooses(
    env: &[(&str, &str)],
    args: &[&str],
) -> (Outcome, serde_json::Value, String) {
    device_check_of_phone_where_dyno_chooses("", env, args)
}

/// [`device_check_where_dyno_chooses`], for a fake phone with these settings.
fn device_check_of_phone_where_dyno_chooses(
    phone: &str,
    env: &[(&str, &str)],
    args: &[&str],
) -> (Outcome, serde_json::Value, String) {
    let mut all_env = vec![("DYNO_FAKE_PHONE", phone)];
    all_env.extend_from_slice(env);
    let mut all_args = vec!["check", "tiny-build", "--driver", "serve", "--device"];
    all_args.extend_from_slice(args);
    let served = start_where_dyno_chooses(&all_env, &all_args);
    let harness = served.harness.clone();
    open_as_browser(&harness, "");
    let outcome = served.finish();
    let result = serde_json::from_str(&outcome.stdout).unwrap();
    (outcome, result, harness)
}

const NOT_OVER_THE_DEVICE_LINK: &str = "dyno check: the page is not served over the device link: ";

/// The fake phone's setting for this machine's address in its tunnel. The loopback address
/// stands in for a tunnel's: this machine has it, and it is not the local network address.
const TUNNEL_AT_LOOPBACK: &str = "tunnel=127.0.0.1";

#[test]
fn a_device_check_serves_over_the_device_link_when_this_machine_has_a_tunnel_to_the_phone() {
    let folder = tempfile::tempdir().unwrap();
    let launched = folder.path().join("launched");
    let phone = format!("{TUNNEL_AT_LOOPBACK};launched={}", launched.display());
    let (outcome, result, harness) = device_check_of_phone_where_dyno_chooses(&phone, &[], &[]);
    // The servers are at the tunnel address, and that is the page the phone's Safari was told
    // to open.
    assert!(harness.starts_with("127.0.0.1:"), "{harness}");
    assert_eq!(
        std::fs::read_to_string(&launched).unwrap(),
        format!("http://{harness}")
    );
    assert!(
        outcome.stderr.contains(
            "dyno check: the page is served over the device link, so the phone needs no network \
             path to this machine. Waiting up to 10 minutes."
        ),
        "{}",
        outcome.stderr
    );
    // A request came over the device link, so dyno says nothing of one that did not.
    for not_said in [NOT_OVER_THE_DEVICE_LINK, NO_REQUEST] {
        assert!(!outcome.stderr.contains(not_said), "{}", outcome.stderr);
    }
    // The page that ran said hello at the tunnel address.
    assert_eq!(result["run"]["serve_path"], "tunnel");
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
    // The page is at the local network address too, unless this machine has no network.
    let (address, path) = own_address_path();
    let also_at = format!("dyno check: the page is also at http://{address}:");
    assert_eq!(
        outcome.stderr.contains(&also_at),
        path == "local_network",
        "{}",
        outcome.stderr
    );
}

/// What dyno says when no request has come over the device link.
const NO_REQUEST: &str = "dyno check: no request has come over the device link in 30 s.";

#[test]
fn with_no_request_over_the_device_link_dyno_says_so_and_a_person_opens_the_other_address() {
    let (address, path) = own_address_path();
    if path != "local_network" {
        // A machine with no network has no second address for a phone.
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let launched = folder.path().join("launched");
    let phone = format!("{TUNNEL_AT_LOOPBACK};launched={}", launched.display());
    let served = start_where_dyno_chooses(
        &[("DYNO_FAKE_PHONE", phone.as_str())],
        &["check", "tiny-build", "--driver", "serve", "--device"],
    );
    let at_tunnel = format!("http://{}", served.harness);
    // The phone's Safari never comes to the tunnel address, as when the tunnel has gone. After
    // 30 s dyno says so, with the local network address to open and the flag to run again with.
    let said = served.wait_for_stderr(NO_REQUEST, Duration::from_secs(90));
    let at_local = said
        .split("can open ")
        .nth(1)
        .and_then(|rest| rest.split(" by hand").next())
        .unwrap()
        .to_owned();
    assert!(
        at_local.starts_with(&format!("http://{address}:")),
        "{at_local}"
    );
    assert!(
        said.contains(&format!(
            "or run again with --serve-host {address} to use the local network from the start."
        )),
        "{said}"
    );
    // A person opens that address by hand.
    let harness = at_local
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    open_as_browser(&harness, "");
    let outcome = served.finish();
    // dyno said it once, and asked Safari for one page: the tunnel address. Nothing was
    // launched again.
    assert_eq!(outcome.stderr.matches(NO_REQUEST).count(), 1);
    assert_eq!(std::fs::read_to_string(&launched).unwrap(), at_tunnel);
    assert_eq!(outcome.stderr.matches("in Safari on").count(), 1);
    // The path is the one of the page that ran, not the one that was asked for.
    let result: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    assert_eq!(result["run"]["serve_path"], "local_network");
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

#[test]
fn with_no_open_dyno_names_both_addresses_and_asks_safari_for_neither() {
    let (outcome, result, harness) =
        device_check_of_phone_where_dyno_chooses(TUNNEL_AT_LOOPBACK, &[], &["--no-open"]);
    assert!(harness.starts_with("127.0.0.1:"), "{harness}");
    let (address, path) = own_address_path();
    assert_eq!(
        outcome.stderr.contains(&format!(
            "dyno check: the page is also at http://{address}:"
        )),
        path == "local_network",
        "{}",
        outcome.stderr
    );
    // A person opens the page, so dyno asks Safari for nothing.
    assert!(
        !outcome.stderr.contains("in Safari on"),
        "{}",
        outcome.stderr
    );
    // The person opened the first address, so that is the path.
    assert_eq!(result["run"]["serve_path"], "tunnel");
}

#[test]
fn a_phone_address_from_devicectl_that_is_this_machine_s_own_is_no_tunnel() {
    let xcrun = fake_xcrun();
    // The loopback address is in its own /64 network, but no phone is at it.
    let env = [("DYNO_XCRUN", xcrun.as_str()), ("FAKE_XCRUN_TUNNEL", "::1")];
    let (outcome, result, harness) = device_check_where_dyno_chooses(&env, &[]);
    let (address, path) = own_address_path();
    assert!(harness.starts_with(&format!("{address}:")), "{harness}");
    assert_eq!(result["run"]["serve_path"], path);
    assert!(
        outcome.stderr.contains(&format!(
            "{NOT_OVER_THE_DEVICE_LINK}devicectl gave ::1 as the phone's tunnel address, which \
             is not an address of another machine in a tunnel."
        )),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_device_check_on_a_machine_with_no_tunnel_of_its_own_serves_on_the_local_network() {
    // No devicectl, as on Linux: dyno's own tunnel to the phone takes no connections from it.
    let (outcome, result, harness) = device_check_where_dyno_chooses(&[], &[]);
    let (address, path) = own_address_path();
    assert!(harness.starts_with(&format!("{address}:")), "{harness}");
    assert_eq!(result["run"]["serve_path"], path);
    assert!(
        outcome.stderr.contains(&format!(
            "{NOT_OVER_THE_DEVICE_LINK}this machine has no tunnel of its own to the phone, which \
             only a Mac has, and dyno's own tunnel takes no connections from the phone."
        )),
        "{}",
        outcome.stderr
    );
    assert!(
        outcome
            .stderr
            .contains("dyno check: the phone must be on this machine's network."),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_mac_with_no_tunnel_to_the_phone_now_serves_on_the_local_network_and_says_why() {
    let xcrun = fake_xcrun();
    let (outcome, result, harness) =
        device_check_where_dyno_chooses(&[("DYNO_XCRUN", xcrun.as_str())], &[]);
    let (address, path) = own_address_path();
    assert!(harness.starts_with(&format!("{address}:")), "{harness}");
    assert_eq!(result["run"]["serve_path"], path);
    assert!(
        outcome.stderr.contains(&format!(
            "{NOT_OVER_THE_DEVICE_LINK}devicectl gave no tunnel address for the phone: this Mac \
             has no tunnel to the phone now (tunnel state: disconnected)."
        )),
        "{}",
        outcome.stderr
    );
    // The run itself is as good as any on the local network.
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["status"], "measured", "{footprint}");
}

#[test]
fn a_tunnel_address_that_this_machine_has_no_tunnel_to_is_not_served_at() {
    let xcrun = fake_xcrun();
    // A documentation address (RFC 3849): this machine has no route to it, or a route that
    // starts outside its network.
    let env = [
        ("DYNO_XCRUN", xcrun.as_str()),
        ("FAKE_XCRUN_TUNNEL", "2001:db8::1"),
    ];
    let (outcome, result, harness) = device_check_where_dyno_chooses(&env, &[]);
    let (address, path) = own_address_path();
    assert!(harness.starts_with(&format!("{address}:")), "{harness}");
    assert_eq!(result["run"]["serve_path"], path);
    let no_route = format!(
        "{NOT_OVER_THE_DEVICE_LINK}this machine has no route to the phone's tunnel address \
         2001:db8::1: "
    );
    let other_network = format!(
        "{NOT_OVER_THE_DEVICE_LINK}the route to the phone's tunnel address 2001:db8::1 starts at "
    );
    assert!(
        outcome.stderr.contains(&no_route) || outcome.stderr.contains(&other_network),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_devicectl_that_fails_leaves_the_check_on_the_local_network() {
    let xcrun = fake_xcrun();
    let env = [("DYNO_XCRUN", xcrun.as_str()), ("FAKE_XCRUN_FAIL", "list")];
    let (outcome, result, _) = device_check_where_dyno_chooses(&env, &[]);
    assert_eq!(result["run"]["serve_path"], own_address_path().1);
    assert!(
        outcome.stderr.contains(&format!(
            "{NOT_OVER_THE_DEVICE_LINK}devicectl gave no tunnel address for the phone: devicectl: \
             ERROR: devicectl list failed for the test."
        )),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_given_serve_host_wins_over_the_tunnel_address() {
    // The fake phone has a tunnel address that is not the given one.
    let (outcome, result, harness) = device_check(&["tunnel=192.0.2.9"], &[], &[]);
    assert!(harness.starts_with("127.0.0.1:"), "{harness}");
    assert_eq!(result["run"]["serve_path"], "given");
    // The tunnel was not looked for, so nothing is said about it.
    assert!(
        !outcome.stderr.contains("device link"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_served_check_with_no_phone_is_at_this_machine_s_own_address() {
    let served = start_where_dyno_chooses(&[], &["check", "tiny-build", "--driver", "serve"]);
    let harness = served.harness.clone();
    open_as_browser(&harness, "");
    let outcome = served.finish();
    let (address, path) = own_address_path();
    assert!(harness.starts_with(&format!("{address}:")), "{harness}");
    let result: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    assert_eq!(result["run"]["serve_path"], path);
    // With no phone there is no device link to say anything about.
    assert!(
        !outcome.stderr.contains("device link"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_lab_name_labels_a_device_check() {
    let (_, result, _) = device_check(&[], &[], &["--lab", "rig"]);
    assert_eq!(result["run"]["lab"], "rig");
    assert_eq!(result["deterministic"]["inputs"]["device"], true);
}

/// A device check with `--runs` of the fake phone with these settings, run to the end with this
/// test as the phone's browser in every run. dyno takes free ports for each run, so each run
/// prints its own URL. It gives the outcome.
fn device_runs_check(phone: &[&str], args: &[&str]) -> Outcome {
    device_runs_check_with(phone, args, |_, harness| open_as_browser(harness, ""))
}

/// [`device_runs_check`], with `browser` as the phone's browser: it gets each run's number, from
/// 1 in the order the runs go, and the harness page's host and port.
fn device_runs_check_with(phone: &[&str], args: &[&str], browser: impl Fn(usize, &str)) -> Outcome {
    let mut all_args = vec!["check", "tiny-build", "--driver", "serve", "--device"];
    all_args.extend_from_slice(args);
    device_check_in(&data_dir(), phone, &all_args, browser)
}

/// `dyno` with these arguments in `dir`, served on the loopback address, with this test as the
/// phone's browser, which `browser` plays: it gets each run's number from 1, and the harness
/// page's host and port. It gives the outcome.
fn device_check_in(
    dir: &std::path::Path,
    phone: &[&str],
    args: &[&str],
    browser: impl Fn(usize, &str),
) -> Outcome {
    let mut runs = 0;
    let mut child = Command::new(env!("CARGO_BIN_EXE_dyno"))
        .args(args)
        .args(["--serve-host", "127.0.0.1", "--serve-port", "0"])
        .current_dir(dir)
        .env("DYNO_CHROMIUM", NO_CHROMIUM)
        .env("DYNO_FAKE_PHONE", phone.join(";"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the dyno binary runs");
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut said = String::new();
    loop {
        let mut line = String::new();
        if stderr.read_line(&mut line).unwrap() == 0 {
            break;
        }
        said.push_str(&line);
        if let Some(rest) = line.split("open http://").nth(1) {
            runs += 1;
            browser(runs, rest.split(' ').next().unwrap());
        }
    }
    let output = child.wait_with_output().unwrap();
    Outcome {
        args: Vec::new(),
        code: output.status.code().unwrap(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: said,
    }
}

#[test]
fn runs_give_the_medians_the_kills_and_the_game_over_the_empty_build() {
    let folder = tempfile::tempdir().unwrap();
    let out = folder.path().join("result.json");
    // The runs go empty build, game, empty build, game, so the fake phone gives the empty build
    // 60 MiB and the game 200 MiB.
    let outcome = device_runs_check(
        &["footprints=62914560,209715200"],
        &["--runs", "2", "--out", &out.display().to_string()],
    );
    for run in ["1 of 2 of the empty build", "2 of 2 of the game"] {
        assert!(
            outcome.stderr.contains(&format!("dyno check: run {run}.")),
            "{}",
            outcome.stderr
        );
    }
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(result["deterministic"]["inputs"]["device_runs"], 2);
    // The path is the one of the game run that the lab measures come from.
    assert_eq!(result["run"]["serve_path"], "given");
    let footprint = run_measure(&result, "device.phys_footprint");
    assert_eq!(footprint["value"], 209_715_200, "{footprint}");
    assert_eq!(footprint["uncertainty"]["kind"], "range", "{footprint}");
    let peak = run_measure(&result, "device.phys_footprint_peak");
    assert_eq!(peak["value"], 209_715_200, "{peak}");
    assert_eq!(peak["items"].as_array().unwrap().len(), 2, "{peak}");
    let kills = run_measure(&result, "device.kills");
    assert_eq!(kills["value"], 0, "{kills}");
    assert_eq!(kills["items"].as_array().unwrap().len(), 2, "{kills}");
    let over = run_measure(&result, "device.footprint_over_baseline");
    assert_eq!(over["status"], "measured", "{over}");
    assert_eq!(over["required"], true, "{over}");
    assert_eq!(over["value"], 209_715_200 - 62_914_560, "{over}");
    assert_eq!(result["deterministic"]["run_coverage"], "complete");
    // The lab measures and the phone come from a game run.
    let frame_dpr = run_measure(&result, "lab.self_check.frame_dpr");
    assert_eq!(frame_dpr["status"], "measured", "{frame_dpr}");
    assert_eq!(
        result["run"]["environment"]["device"]["key"],
        "iphone-14-pro-max"
    );
    for recording in ["empty-1", "game-1", "empty-2", "game-2"] {
        assert!(
            folder
                .path()
                .join(format!("result.{recording}.device.json"))
                .is_file(),
            "{recording}"
        );
    }
    // A record of each run, in the order they ran, each a whole page and a whole recording.
    let records: Vec<(String, u64)> = run_records(&result)
        .iter()
        .map(|record| {
            assert_eq!(record["page"]["end"], "ended", "{record}");
            assert_eq!(record["page"]["hello"], true, "{record}");
            assert!(record["recording"]["samples"]["read"].as_u64().unwrap() > 0);
            assert_eq!(
                record["recording"]["log"]["stopped"],
                serde_json::Value::Null
            );
            assert_eq!(
                record["recording"]["kill"],
                serde_json::Value::Null,
                "{record}"
            );
            let of = record["of"].as_str().unwrap().to_owned();
            (of, record["index"].as_u64().unwrap())
        })
        .collect();
    let in_order = [
        ("empty_build", 1),
        ("game", 1),
        ("empty_build", 2),
        ("game", 2),
    ];
    assert_eq!(
        records,
        in_order.map(|(of, index)| (of.to_owned(), index)),
        "{records:?}"
    );
}

#[test]
fn a_game_run_whose_page_lost_its_link_says_so_though_its_recording_gave_every_measure() {
    // The page of the first game run says hello and loads, then takes no command: the second
    // run of the game completes.
    let outcome = device_runs_check_with(
        &["footprints=62914560,209715200"],
        &["--runs", "2"],
        |run, harness| match run {
            2 => open_and_lose_the_link(harness),
            _ => open_as_browser(harness, ""),
        },
    );
    let result: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    // Every device measure rests on both game runs, and the lab measures come from the second.
    for id in [
        "device.phys_footprint",
        "device.phys_footprint_peak",
        "device.kills",
        "device.footprint_over_baseline",
        "lab.self_check.frame_dpr",
    ] {
        let measure = run_measure(&result, id);
        assert_eq!(measure["status"], "measured", "{measure}");
    }
    let peak = run_measure(&result, "device.phys_footprint_peak");
    assert_eq!(peak["items"].as_array().unwrap().len(), 2, "{peak}");
    // Only the run's record says that the first game run's page did not complete.
    let records = run_records(&result);
    let game_1 = &records[1];
    assert_eq!(
        (game_1["of"].as_str(), game_1["index"].as_u64()),
        (Some("game"), Some(1))
    );
    assert_eq!(game_1["page"]["hello"], true, "{game_1}");
    assert_eq!(game_1["page"]["document"], "not_observed", "{game_1}");
    assert_eq!(game_1["page"]["probe"], "heard", "{game_1}");
    assert_eq!(game_1["page"]["end"], "command_not_taken", "{game_1}");
    // The recording went on: its samples came, and neither feed reported a stop.
    assert!(game_1["recording"]["samples"]["read"].as_u64().unwrap() > 0);
    for feed in ["samples", "log"] {
        assert_eq!(
            game_1["recording"][feed]["stopped"],
            serde_json::Value::Null
        );
    }
    for other in [&records[0], &records[2], &records[3]] {
        assert_eq!(other["page"]["end"], "ended", "{other}");
    }
}

#[test]
fn a_check_of_runs_that_stops_after_some_runs_keeps_their_records_beside_the_result() {
    let folder = tempfile::tempdir().unwrap();
    let out = folder.path().join("result.json");
    // The phone is gone when the second run of the empty build begins: recording 3.
    let outcome = device_runs_check(
        &["connect-fails-in=3"],
        &["--runs", "2", "--out", &out.display().to_string()],
    );
    // The check stops with the same error as before, and writes no result.
    assert_eq!(outcome.code, 64, "{}", outcome.stderr);
    assert!(!out.exists());
    let kept = folder.path().join("result.runs.json");
    assert!(
        outcome.stderr.contains(&format!(
            "the record of each run made before the error is in {}",
            kept.display()
        )),
        "{}",
        outcome.stderr
    );
    let kept: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&kept).unwrap()).unwrap();
    assert_eq!(kept.as_array().unwrap().len(), 1, "{kept}");
    assert_eq!(kept[0]["game"], serde_json::Value::Null);
    let runs: Vec<(&str, u64, &str)> = kept[0]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                record["of"].as_str().unwrap(),
                record["index"].as_u64().unwrap(),
                record["page"]["end"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(runs, [("empty_build", 1, "ended"), ("game", 1, "ended")]);
}

#[test]
fn a_games_file_of_runs_that_stops_keeps_each_games_records_by_its_id() {
    let folder = tempfile::tempdir().unwrap();
    for game in ["first", "second"] {
        copy_dir(&data_dir().join("tiny-build"), &folder.path().join(game));
    }
    std::fs::write(
        folder.path().join("games.toml"),
        "schema_version = 0\n\n[[game]]\nid = \"first\"\nbuild = \"first\"\n\n\
         [[game]]\nid = \"second\"\nbuild = \"second\"\n",
    )
    .unwrap();
    let out = folder.path().join("catalogue.json");
    // Recordings 1 and 2 are the first game's runs; the phone is gone for the second game's
    // run of the game, recording 4.
    let outcome = device_check_in(
        folder.path(),
        &["connect-fails-in=4"],
        &[
            "check",
            "--games",
            "games.toml",
            "--driver",
            "serve",
            "--device",
            "--runs",
            "1",
            "--out",
            &out.display().to_string(),
        ],
        |_, harness| open_as_browser(harness, ""),
    );
    assert_eq!(outcome.code, 64, "{}", outcome.stderr);
    assert!(!out.exists());
    let kept: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(folder.path().join("catalogue.runs.json")).unwrap(),
    )
    .unwrap();
    let games: Vec<(&str, Vec<&str>)> = kept
        .as_array()
        .unwrap()
        .iter()
        .map(|game| {
            let runs = game["runs"].as_array().unwrap();
            let of = runs.iter().map(|run| run["of"].as_str().unwrap()).collect();
            (game["game"].as_str().unwrap(), of)
        })
        .collect();
    assert_eq!(
        games,
        [
            ("first", vec!["empty_build", "game"]),
            ("second", vec!["empty_build"])
        ]
    );
}

#[test]
fn an_empty_build_run_whose_log_stopped_says_so_in_its_record() {
    // Recording 1 is the empty build's run: its log closes at once. The game's is whole.
    let outcome = device_runs_check(&["log-fails-in=1"], &["--runs", "1"]);
    let result: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    let records = run_records(&result);
    assert_eq!(records[0]["of"], "empty_build");
    let log = &records[0]["recording"]["log"];
    assert_eq!(log["read"], 0, "{log}");
    assert_eq!(log["stopped"], "the fake phone's log closed", "{log}");
    assert_eq!(records[1]["of"], "game");
    assert_eq!(
        records[1]["recording"]["log"]["stopped"],
        serde_json::Value::Null
    );
    // The kills count the game runs only, and the footprint over the empty build reads the empty
    // build's samples, which ran: both measure. The record is what says the empty build's run
    // could not tell whether its page was killed.
    assert_eq!(run_measure(&result, "device.kills")["status"], "measured");
    let over = run_measure(&result, "device.footprint_over_baseline");
    assert_eq!(over["status"], "measured", "{over}");
}

#[test]
fn a_run_of_runs_that_did_not_complete_says_which_run_and_why() {
    let outcome = device_runs_check(&["fail=samples"], &["--runs", "1"]);
    let result: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    for id in [
        "device.phys_footprint",
        "device.phys_footprint_peak",
        "device.kills",
        "device.footprint_over_baseline",
    ] {
        let measure = run_measure(&result, id);
        assert_eq!(measure["status"], "missing", "{measure}");
        assert!(
            measure["reason"]
                .as_str()
                .unwrap()
                .starts_with("game run 1 of 1 did not complete: the phone's samples ended"),
            "{measure}"
        );
    }
    assert_eq!(result["deterministic"]["run_coverage"], "missing");
}

/// A device check of the fake phone with these settings and extra flags that stops before its
/// run. It gives the exit code and what dyno printed.
fn device_check_before_the_run(phone: &[&str], args: &[&str]) -> (Option<i32>, String) {
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
        .args(args)
        .current_dir(data_dir())
        .env("DYNO_FAKE_PHONE", phone.join(";"))
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn a_phone_that_does_not_connect_or_does_not_start_stops_the_check() {
    let (code, stderr) = device_check_before_the_run(&["fail=connect"], &[]);
    assert_eq!(code, Some(64), "{stderr}");
    assert!(
        stderr.contains("cannot connect to the phone: the fake phone did not connect"),
        "{stderr}"
    );
    assert!(stderr.contains("iOS 17 or later"), "{stderr}");
    let (code, stderr) = device_check_before_the_run(&["fail=record"], &[]);
    assert_eq!(code, Some(70), "{stderr}");
    assert!(
        stderr.contains("cannot start recording the phone: the fake phone did not start its tap"),
        "{stderr}"
    );
}

#[test]
fn a_locked_phone_is_a_usage_error() {
    let (code, stderr) = device_check_before_the_run(&["fail=locked"], &[]);
    assert_eq!(code, Some(64), "{stderr}");
    assert!(stderr.contains("the phone is locked"), "{stderr}");
    assert!(stderr.contains("Auto-Lock to Never"), "{stderr}");
}

#[test]
fn a_device_check_with_no_phone_or_the_wrong_one_is_a_usage_error() {
    let (code, stderr) = device_check_before_the_run(&["phones=0"], &[]);
    assert_eq!(code, Some(64), "{stderr}");
    assert!(stderr.contains("no iPhone is connected"), "{stderr}");
    let (code, stderr) = device_check_before_the_run(&[], &["--device-id", "nope"]);
    assert_eq!(code, Some(64), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "no connected iPhone has the UDID nope. Connected: {FAKE_UDID} on USB"
        )),
        "{stderr}"
    );
}

#[test]
fn a_pairing_file_needs_an_address() {
    let (code, stderr) = device_check_before_the_run(&[], &["--pairing-file", "phone.plist"]);
    assert_ne!(code, Some(0), "{stderr}");
    assert!(stderr.contains("--device-ip"), "{stderr}");
}
