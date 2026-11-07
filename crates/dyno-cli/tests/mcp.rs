//! `dyno mcp` serves `check`, `compare` and `read_result` to an MCP client. Each test starts the
//! server and speaks newline-delimited JSON-RPC to it, as an MCP client does.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use common::{NO_CHROMIUM, data_dir, dyno, json};
use dyno_schema::compare::CompareResult;
use dyno_schema::result::raw_hash_matches;
use serde_json::{Value, json};

/// The MCP protocol version the client asks for.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How long the client waits for each line, so a server that never answers fails the test.
const LINE_TIMEOUT: Duration = Duration::from_secs(60);

/// One `dyno mcp` server and the client end of its stdin and stdout.
struct Session {
    server: Child,
    stdin: ChildStdin,
    /// Each line of stdout. The channel closes when stdout does.
    lines: Receiver<String>,
    next_id: u64,
}

impl Session {
    /// Starts `dyno mcp` in `tests/data` and initializes the session.
    fn start() -> Session {
        let mut server = Command::new(env!("CARGO_BIN_EXE_dyno"))
            .arg("mcp")
            .current_dir(data_dir())
            .env("DYNO_CHROMIUM", NO_CHROMIUM)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("dyno mcp starts");
        let stdin = server.stdin.take().unwrap();
        let stdout = BufReader::new(server.stdout.take().unwrap());
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in stdout.lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut session = Session {
            server,
            stdin,
            lines,
            next_id: 1,
        };
        let initialized = session.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "dyno-test", "version": "0" },
            }),
        );
        assert_eq!(initialized["serverInfo"]["name"], "dyno");
        session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        session
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Sends a request and returns the `result` of its response. Every line on stdout must be a
    /// JSON-RPC message.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self
                .lines
                .recv_timeout(LINE_TIMEOUT)
                .unwrap_or_else(|error| panic!("no answer to {method}: {error}"));
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("not JSON on stdout ({error}): {line}"));
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{line}");
                return message["result"].clone();
            }
        }
    }

    /// Calls a tool and returns its result: `content` and `isError`.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

/// The text of a tool result's content block.
fn text(result: &Value, block: usize) -> &str {
    result["content"][block]["text"].as_str().unwrap()
}

/// The JSON in a working tool result's first block.
fn answer_json(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    serde_json::from_str(text(result, 0)).unwrap()
}

/// Checks `tiny-build` with the check tool and keeps the result in `out`.
fn check_into(session: &mut Session, out: &Path, release: &str) -> Value {
    let result = session.call(
        "check",
        json!({ "build": "tiny-build", "release": release, "out": out }),
    );
    answer_json(&result)
}

#[test]
fn the_server_lists_check_compare_and_read_result() {
    let mut session = Session::start();
    let listed = session.request("tools/list", json!({}));
    let mut names: Vec<&str> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["check", "compare", "read_result"]);
}

#[test]
fn the_check_tool_gives_the_result_that_dyno_check_gives() {
    let mut session = Session::start();
    let result = session.call("check", json!({ "build": "tiny-build" }));
    let from_tool = answer_json(&result);
    let from_cli = json(&dyno(&["check", "tiny-build"]));
    assert!(raw_hash_matches(&from_tool));
    assert_eq!(from_tool["result_hash"], from_cli["result_hash"]);
}

#[test]
fn the_check_tool_gives_the_summary_after_the_json() {
    let mut session = Session::start();
    let result = session.call("check", json!({ "build": "tiny-build" }));
    let summary = text(&result, 1);
    assert!(summary.starts_with("dyno check: incomplete"), "{summary}");
    assert!(summary.contains("full result: the JSON before this summary."));
}

#[test]
fn a_check_that_fails_is_not_a_tool_error() {
    // over-budget decodes to more audio and textures than the default fail limits.
    let build = "../../../../fixtures/over-budget/build";
    let mut session = Session::start();
    let result = session.call("check", json!({ "build": build }));
    assert_eq!(answer_json(&result)["deterministic"]["status"], "fail");
}

#[test]
fn a_check_with_a_launch_url_runs_the_browser_driver_outside_the_server_runtime() {
    // The CDP driver starts its own async runtime, which panics inside the server's. The tests
    // have no Chromium, so the run fails and its measures are missing.
    let mut session = Session::start();
    let result = session.call(
        "check",
        json!({ "launch_url": "https://games.example/tiny/index.html" }),
    );
    let checked = answer_json(&result);
    assert_eq!(checked["deterministic"]["run_coverage"], "missing");
    assert!(
        checked["run"]["measures"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("the cdp driver failed"),
        "{checked}"
    );
}

#[test]
fn a_check_of_a_build_that_does_not_exist_is_a_tool_error() {
    let mut session = Session::start();
    let result = session.call("check", json!({ "build": "no-such-build" }));
    assert_eq!(result["isError"], true, "{result}");
    assert!(text(&result, 0).contains("no-such-build"), "{result}");
}

#[test]
fn a_check_with_no_build_and_no_launch_url_is_a_tool_error() {
    let mut session = Session::start();
    let result = session.call("check", json!({}));
    assert_eq!(result["isError"], true, "{result}");
}

#[test]
fn the_compare_tool_compares_two_results_of_the_check_tool() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base.json");
    let head = dir.path().join("head.json");
    let mut session = Session::start();
    check_into(&mut session, &base, "v1");
    check_into(&mut session, &head, "v2");
    let result = session.call("compare", json!({ "base": base, "head": head }));
    let compare: CompareResult = serde_json::from_value(answer_json(&result)).unwrap();
    assert!(compare.deterministic.base.hash_matches && compare.deterministic.head.hash_matches);
    assert!(compare.deterministic.measures.is_empty());
    assert!(text(&result, 1).starts_with("dyno compare: incomplete"));
}

#[test]
fn read_result_gives_the_status_and_whether_the_hash_matches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    let mut session = Session::start();
    let checked = check_into(&mut session, &path, "v1");
    let result = session.call("read_result", json!({ "path": path }));
    let status = answer_json(&result);
    assert_eq!(status["status"], "incomplete");
    assert_eq!(status["result_hash"], checked["result_hash"]);
    assert_eq!(status["hash_matches"], true);
}

#[test]
fn read_result_says_when_a_result_was_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    let mut session = Session::start();
    let mut edited = check_into(&mut session, &path, "v1");
    edited["deterministic"]["status"] = "pass".into();
    std::fs::write(&path, edited.to_string()).unwrap();
    let result = session.call("read_result", json!({ "path": path }));
    assert_eq!(answer_json(&result)["hash_matches"], false);
    assert!(text(&result, 1).starts_with("this result was edited"));
}
