//! `dyno login`, `dyno logout`, `dyno upload` and `dyno check --upload`, against a fake dyno site
//! on this machine.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use common::{NO_CHROMIUM, Outcome, data_dir, fixture_build};

const PASS: i32 = 0;
const INCOMPLETE: i32 = 2;
const USAGE: i32 = 64;
const INTERNAL: i32 = 70;

/// A token in the site's format: `dyno_` and 43 base64url characters.
const TOKEN: &str = "dyno_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const OTHER_TOKEN: &str = "dyno_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const EMAIL: &str = "tester@example.com";
const ID: &str = "0b9e2c4e-5f1a-4d7e-9c3b-2a6f8e1d4c70";

/// What the fake site got.
#[derive(Clone, Debug)]
struct Request {
    method: String,
    path: String,
    /// Header names in lower case.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// How the fake site answers a request: an HTTP status and a body.
type Answer = fn(&Request) -> (u16, String);

/// The site as the contract has it: every token but OTHER_TOKEN is good, and every upload is
/// taken.
fn good_site(request: &Request) -> (u16, String) {
    if request.header("authorization") == Some(&format!("Bearer {OTHER_TOKEN}")) {
        return (401, r#"{"error":"that token was revoked"}"#.to_owned());
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/me") => (200, format!(r#"{{"email":"{EMAIL}"}}"#)),
        ("POST", "/api/results") => (201, format!(r#"{{"id":"{ID}"}}"#)),
        _ => (404, r#"{"error":"no such page"}"#.to_owned()),
    }
}

/// A fake dyno site on a free port, and every request it got.
struct FakeSite {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeSite {
    fn start(answer: Answer) -> FakeSite {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let request = read_request(&stream);
                let (status, body) = answer(&request);
                seen.lock().unwrap().push(request);
                write!(
                    &stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        FakeSite { url, requests }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// The one request the site got.
    fn only_request(&self) -> Request {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        requests[0].clone()
    }

    fn page(&self) -> String {
        format!("{}/#/checks/{ID}", self.url)
    }
}

fn read_request(stream: &TcpStream) -> Request {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap().to_owned();
    let path = parts.next().unwrap().to_owned();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
    }
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map_or(0, |(_, value)| value.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Request {
        method,
        path,
        headers,
        body,
    }
}

/// A config folder of its own, and the site that the runs talk to.
struct Setup {
    config: tempfile::TempDir,
    site_url: String,
}

impl Setup {
    fn new(site: &FakeSite) -> Setup {
        Setup {
            config: tempfile::tempdir().unwrap(),
            site_url: site.url.clone(),
        }
    }

    fn credentials(&self) -> PathBuf {
        self.config.path().join("dyno").join("credentials.json")
    }

    /// Runs `dyno` in `tests/data` with this config folder and site.
    fn dyno(&self, args: &[&str]) -> Outcome {
        self.dyno_with(&[], None, args)
    }

    /// Runs `dyno` with `env` added and `stdin` as its input.
    fn dyno_with(&self, env: &[(&str, &str)], stdin: Option<&str>, args: &[&str]) -> Outcome {
        let config = self.config.path().join("dyno");
        let mut all_env = vec![
            ("DYNO_CONFIG_DIR", config.to_str().unwrap()),
            ("DYNO_API_URL", self.site_url.as_str()),
        ];
        all_env.extend(env);
        run(&all_env, stdin, args)
    }

    fn sign_in(&self) {
        assert_exit(&self.dyno(&["login", "--token", TOKEN]), PASS);
    }
}

/// Runs `dyno` in `tests/data` with only `env` of dyno's own variables set.
fn run(env: &[(&str, &str)], stdin: Option<&str>, args: &[&str]) -> Outcome {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dyno"));
    command
        .args(args)
        .current_dir(data_dir())
        .env("DYNO_CHROMIUM", NO_CHROMIUM)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in [
        "DYNO_TOKEN",
        "DYNO_CONFIG_DIR",
        "DYNO_API_URL",
        "XDG_CONFIG_HOME",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    command.envs(env.iter().copied());
    let mut child = command.spawn().expect("the dyno binary runs");
    let mut input = child.stdin.take().unwrap();
    if let Some(stdin) = stdin {
        input.write_all(stdin.as_bytes()).unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    Outcome {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        code: output.status.code().expect("dyno exits with a code"),
        stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
    }
}

fn assert_exit(outcome: &Outcome, code: i32) {
    assert_eq!(
        outcome.code, code,
        "args: {:?}\nstdout: {}\nstderr: {}",
        outcome.args, outcome.stdout, outcome.stderr
    );
}

fn assert_stderr_has(outcome: &Outcome, text: &str) {
    assert!(
        outcome.stderr.contains(text),
        "stderr has no {text:?}: {}",
        outcome.stderr
    );
}

fn saved_token(path: &Path) -> String {
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    json["token"].as_str().unwrap().to_owned()
}

/// A port with nothing listening on it.
fn closed_site() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    url
}

#[test]
fn login_asks_for_a_token_checks_it_and_saves_it() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    let outcome = setup.dyno_with(&[], Some(&format!("  {TOKEN}\n")), &["login"]);
    assert_exit(&outcome, PASS);
    assert_stderr_has(
        &outcome,
        &format!(
            "Paste a token from {}/#/account, then press Enter.",
            site.url
        ),
    );
    assert_stderr_has(&outcome, &format!("dyno login: signed in as {EMAIL}."));
    assert!(outcome.stdout.is_empty(), "{}", outcome.stdout);
    assert_eq!(saved_token(&setup.credentials()), TOKEN);

    let request = site.only_request();
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("GET", "/api/me")
    );
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    let user_agent = request.header("user-agent").unwrap();
    assert!(user_agent.starts_with("dyno/0."), "{user_agent}");
}

#[cfg(unix)]
#[test]
fn only_the_owner_can_read_the_saved_token() {
    use std::os::unix::fs::PermissionsExt;

    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&setup.credentials()), 0o600);
    assert_eq!(mode(setup.credentials().parent().unwrap()), 0o700);
}

#[test]
fn a_token_the_site_does_not_take_is_a_usage_error_and_is_not_saved() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    let outcome = setup.dyno(&["login", "--token", OTHER_TOKEN]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "dyno: that token was revoked");
    assert!(!setup.credentials().exists());
}

#[test]
fn login_with_no_token_or_a_token_that_is_not_one_line_is_a_usage_error() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    let empty = setup.dyno_with(&[], Some("\n"), &["login"]);
    assert_exit(&empty, USAGE);
    assert_stderr_has(&empty, "no token given");
    let spaced = setup.dyno(&["login", "--token", "dyno_a b"]);
    assert_exit(&spaced, USAGE);
    assert_stderr_has(&spaced, "that is not a token from");
    assert!(site.requests().is_empty());
}

#[test]
fn login_to_a_site_that_cannot_be_reached_is_an_io_error() {
    let config = tempfile::tempdir().unwrap();
    let site = closed_site();
    let outcome = run(
        &[
            ("DYNO_CONFIG_DIR", config.path().to_str().unwrap()),
            ("DYNO_API_URL", &site),
        ],
        None,
        &["login", "--token", TOKEN],
    );
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, &format!("cannot reach {site}"));
}

#[test]
fn a_site_error_without_an_error_text_names_its_status() {
    let site = FakeSite::start(|_| (502, "<html>bad gateway</html>".to_owned()));
    let setup = Setup::new(&site);
    let outcome = setup.dyno(&["login", "--token", TOKEN]);
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, "the site answered HTTP 502");
}

#[test]
fn an_answer_that_is_not_the_contract_is_an_io_error() {
    let site = FakeSite::start(|_| (200, r#"{"name":"tester"}"#.to_owned()));
    let setup = Setup::new(&site);
    let outcome = setup.dyno(&["login", "--token", TOKEN]);
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, "gave an answer dyno cannot read");
    assert!(!setup.credentials().exists());
}

#[test]
fn a_site_url_that_is_not_http_is_a_usage_error() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    let outcome = setup.dyno_with(
        &[("DYNO_API_URL", "dynoapp.io")],
        None,
        &["login", "--token", TOKEN],
    );
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "must be an http or https URL");
}

#[test]
fn logout_removes_the_saved_token() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let first = setup.dyno(&["logout"]);
    assert_exit(&first, PASS);
    assert_stderr_has(&first, "dyno logout: removed the saved token.");
    assert!(!setup.credentials().exists());
    let second = setup.dyno(&["logout"]);
    assert_exit(&second, PASS);
    assert_stderr_has(&second, "dyno logout: there was no saved token.");
}

#[test]
fn login_and_logout_say_when_dyno_token_wins() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    let env = [("DYNO_TOKEN", OTHER_TOKEN)];
    let login = setup.dyno_with(&env, None, &["login", "--token", TOKEN]);
    assert_exit(&login, PASS);
    assert_stderr_has(&login, "DYNO_TOKEN is set, so dyno uses it");
    let logout = setup.dyno_with(&env, None, &["logout"]);
    assert_exit(&logout, PASS);
    assert_stderr_has(&logout, "DYNO_TOKEN is still set");
}

#[test]
fn the_token_is_in_xdg_config_home_or_else_in_home() {
    let site = FakeSite::start(good_site);
    let folder = tempfile::tempdir().unwrap();
    let xdg = folder.path().join("xdg");
    let home = folder.path().join("home");
    let login = |env: &[(&str, &str)]| {
        let mut all_env = vec![("DYNO_API_URL", site.url.as_str())];
        all_env.extend(env);
        run(&all_env, None, &["login", "--token", TOKEN])
    };
    assert_exit(
        &login(&[
            ("XDG_CONFIG_HOME", xdg.to_str().unwrap()),
            ("HOME", home.to_str().unwrap()),
        ]),
        PASS,
    );
    assert_eq!(saved_token(&xdg.join("dyno/credentials.json")), TOKEN);
    assert_exit(&login(&[("HOME", home.to_str().unwrap())]), PASS);
    assert_eq!(
        saved_token(&home.join(".config/dyno/credentials.json")),
        TOKEN
    );
    let homeless = login(&[("HOME", "")]);
    assert_exit(&homeless, USAGE);
    assert_stderr_has(&homeless, "set DYNO_CONFIG_DIR or HOME");
}

#[test]
fn upload_posts_the_file_as_it_is_and_prints_its_page() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("result.json");
    // Not canonical JSON, so a parse and a write again would change it.
    let bytes = b"{ \"schema_version\": 0,\n  \"z\": [1, 2] }\n";
    fs::write(&file, bytes).unwrap();
    let file = file.display().to_string();

    let outcome = setup.dyno(&["upload", &file]);
    assert_exit(&outcome, PASS);
    assert_stderr_has(&outcome, &format!("dyno upload: {file}: {}", site.page()));
    let upload = site.requests().pop().unwrap();
    assert_eq!(
        (upload.method.as_str(), upload.path.as_str()),
        ("POST", "/api/results")
    );
    assert_eq!(upload.body, bytes);
    assert_eq!(upload.header("content-type"), Some("application/json"));
    assert_eq!(
        upload.header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
}

#[test]
fn dyno_token_wins_over_the_saved_token() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let other = "dyno_CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("result.json");
    fs::write(&file, "{}").unwrap();
    let outcome = setup.dyno_with(
        &[("DYNO_TOKEN", other)],
        None,
        &["upload", file.to_str().unwrap()],
    );
    assert_exit(&outcome, PASS);
    let upload = site.requests().pop().unwrap();
    assert_eq!(
        upload.header("authorization"),
        Some(format!("Bearer {other}").as_str())
    );
}

#[test]
fn upload_without_a_token_is_a_usage_error() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    let outcome = setup.dyno(&["upload", "games.toml"]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(
        &outcome,
        "dyno: sign in first: run dyno login, or set DYNO_TOKEN",
    );
    assert!(site.requests().is_empty());
}

#[test]
fn a_saved_token_file_that_cannot_be_read_is_a_usage_error() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    fs::create_dir_all(setup.credentials().parent().unwrap()).unwrap();
    fs::write(setup.credentials(), "token = 1").unwrap();
    let outcome = setup.dyno(&["upload", "games.toml"]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "is not a dyno credentials file");
}

#[test]
fn a_file_that_is_not_json_or_not_there_uploads_nothing() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let folder = tempfile::tempdir().unwrap();
    let good = folder.path().join("good.json");
    fs::write(&good, "{}").unwrap();
    let good = good.to_str().unwrap();
    let not_json = setup.dyno(&["upload", good, "games.toml"]);
    assert_exit(&not_json, USAGE);
    assert_stderr_has(&not_json, "games.toml is not JSON");
    let not_there = setup.dyno(&["upload", good, "no-such-result.json"]);
    assert_exit(&not_there, USAGE);
    assert_stderr_has(&not_there, "cannot read result file no-such-result.json");
    // Only the sign in reached the site.
    assert_eq!(site.requests().len(), 1);
}

#[test]
fn a_file_the_site_does_not_take_is_an_error_and_the_rest_upload() {
    let site = FakeSite::start(|request| {
        if request.body.len() > 10 {
            (413, r#"{"error":"the result is over 4 MiB"}"#.to_owned())
        } else {
            good_site(request)
        }
    });
    let setup = Setup::new(&site);
    setup.sign_in();
    let folder = tempfile::tempdir().unwrap();
    let big = folder.path().join("big.json");
    fs::write(&big, r#"{"padding":"0123456789"}"#).unwrap();
    let small = folder.path().join("small.json");
    fs::write(&small, "{}").unwrap();
    let (big, small) = (big.to_str().unwrap(), small.to_str().unwrap());

    let outcome = setup.dyno(&["upload", big, small]);
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(
        &outcome,
        &format!("dyno upload: {big}: the result is over 4 MiB"),
    );
    assert_stderr_has(&outcome, &format!("dyno upload: {small}: {}", site.page()));
    assert_stderr_has(&outcome, "dyno: 1 of 2 results did not upload");
}

#[test]
fn an_upload_with_a_revoked_token_says_to_sign_in_again() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("result.json");
    fs::write(&file, "{}").unwrap();
    let outcome = setup.dyno_with(
        &[("DYNO_TOKEN", OTHER_TOKEN)],
        None,
        &["upload", file.to_str().unwrap()],
    );
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, "that token was revoked (run dyno login again)");
}

#[test]
fn check_upload_uploads_the_result_it_prints() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let outcome = setup.dyno(&["check", "tiny-build", "--upload"]);
    // The stub static check is incomplete, and the upload does not change that.
    assert_exit(&outcome, INCOMPLETE);
    assert_stderr_has(
        &outcome,
        &format!("dyno check: uploaded to {}", site.page()),
    );
    let upload = site.requests().pop().unwrap();
    assert_eq!(upload.path, "/api/results");
    assert_eq!(upload.body, outcome.stdout.as_bytes());
}

#[test]
fn check_upload_uploads_the_out_file() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let folder = tempfile::tempdir().unwrap();
    let out = folder.path().join("result.json");
    let outcome = setup.dyno(&[
        "check",
        "tiny-build",
        "--out",
        out.to_str().unwrap(),
        "--upload",
    ]);
    assert_exit(&outcome, INCOMPLETE);
    let upload = site.requests().pop().unwrap();
    assert_eq!(upload.body, fs::read(&out).unwrap());
}

#[test]
fn check_games_upload_uploads_each_game() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let outcome = setup.dyno(&["check", "--games", "games.toml", "--upload"]);
    assert_exit(&outcome, INCOMPLETE);
    assert_stderr_has(
        &outcome,
        &format!("dyno check: game \"tiny\": uploaded to {}", site.page()),
    );
    assert_stderr_has(&outcome, "dyno check: game \"tiny-online\": uploaded to");
    let catalogue: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    let uploads: Vec<serde_json::Value> = site
        .requests()
        .iter()
        .filter(|request| request.path == "/api/results")
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert_eq!(uploads.len(), 2);
    for (upload, game) in uploads.iter().zip(catalogue["games"].as_array().unwrap()) {
        assert_eq!(upload, &game["result"]);
    }
}

#[test]
fn check_upload_without_a_token_stops_before_the_check() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    let outcome = setup.dyno(&["check", "tiny-build", "--upload"]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "sign in first");
    assert!(outcome.stdout.is_empty(), "{}", outcome.stdout);
}

#[test]
fn a_failed_upload_keeps_the_exit_code_of_a_check_that_did_not_pass() {
    let site = FakeSite::start(good_site);
    let setup = Setup::new(&site);
    setup.sign_in();
    let outcome = setup.dyno_with(
        &[("DYNO_TOKEN", OTHER_TOKEN)],
        None,
        &["check", "tiny-build", "--upload"],
    );
    assert_exit(&outcome, INCOMPLETE);
    assert_stderr_has(
        &outcome,
        "dyno check: upload failed: that token was revoked",
    );
}

#[test]
fn a_failed_upload_of_a_check_that_passed_is_an_io_error() {
    let site = closed_site();
    let config = tempfile::tempdir().unwrap();
    let pixi7_aa = fixture_build("pixi7-aa");
    let outcome = run(
        &[
            ("DYNO_CONFIG_DIR", config.path().to_str().unwrap()),
            ("DYNO_API_URL", &site),
            ("DYNO_TOKEN", TOKEN),
        ],
        None,
        // pixi7-aa warns, which exits 0.
        &["check", &pixi7_aa, "--upload"],
    );
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(
        &outcome,
        &format!("dyno check: upload failed: cannot reach {site}"),
    );
    assert_stderr_has(&outcome, "dyno: 1 of 1 results did not upload");
    assert!(!outcome.stdout.is_empty());
}
