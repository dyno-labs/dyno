//! A whole served run, with this test as the browser: it loads the harness page and the game the
//! way a browser would, and posts what the harness page would relay.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dyno_run::Driver;
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::MeasureStatus;
use dyno_schema::probe::{MessageDirection, ProbeEvent};
use dyno_schema::profile::{HostProfile, Scenario};
use dyno_schema::result::ServePath;
use dyno_schema::runner::{DriverMessage, RUNNER_PROTOCOL_VERSION, RunMark, RunRequest};
use dyno_serve::{Opened, ServeDriver, ServeOptions};

const IPHONE_SAFARI: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.2 Mobile/15E148 Safari/604.1";

/// One HTTP exchange: the status line and the body.
fn exchange(url: &str, method: &str, path: &str, body: &str) -> (String, String) {
    let address = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    let mut socket = TcpStream::connect(&address).unwrap();
    write!(
        socket,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.lines().next().unwrap().to_owned();
    (status, body.to_owned())
}

fn get(url: &str, path: &str) -> (String, String) {
    exchange(url, "GET", path, "")
}

fn post(url: &str, path: &str, body: &str) -> (String, String) {
    exchange(url, "POST", path, body)
}

/// Polls for the next command until it is not `wait`.
fn next_command(harness: &str) -> String {
    loop {
        let (_, body) = get(harness, "/__dyno/next");
        if !body.contains(r#""wait""#) {
            return body;
        }
    }
}

fn probe(event: &str) -> String {
    format!(
        r#"{{"kind":"probe","payload":{}}}"#,
        serde_json::to_string(event).unwrap()
    )
}

#[test]
fn a_browser_that_opens_the_harness_page_gives_a_full_run() {
    let build = tempfile::tempdir().unwrap();
    fs::write(
        build.path().join("index.html"),
        r#"<!doctype html><html><head><title>game</title></head><body><script src="game.js"></script></body></html>"#,
    )
    .unwrap();
    fs::write(build.path().join("game.js"), "console.log('game')").unwrap();
    fs::create_dir(build.path().join("__dyno")).unwrap();
    fs::write(build.path().join("__dyno/data.json"), "{\"the build's\":1}").unwrap();
    fs::create_dir(build.path().join(".git")).unwrap();
    fs::write(build.path().join(".git/config"), "not the build's").unwrap();
    fs::write(build.path().join(".DS_Store"), "litter").unwrap();
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        tunnel_host: None,
        wait_for_tunnel: Duration::from_secs(15),
        port: 0,
        wait_for_browser: Duration::from_secs(30),
        open_with: None,
    };
    let mut driver = ServeDriver::bind(build.path(), &options).unwrap();
    let harness = driver.harness_url().to_owned();
    let game = driver.game_url().to_owned();
    let launch_url = driver.launch_url(Some("sessionID=secret"));
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url: launch_url.clone(),
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 300,
        timeout_ms: 20_000,
    };
    let run_request = request.clone();
    let run = thread::spawn(move || {
        let messages = driver.run(&run_request);
        (messages, driver.last_run())
    });

    // The browser opens the harness page, which says hello and then loads the game.
    let (status, page) = get(&harness, "/");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(page.contains(&format!(r#"const GAME_URL = "{launch_url}";"#)));
    let hello = format!(r#"{{"user_agent":"{IPHONE_SAFARI}"}}"#);
    assert_eq!(post(&harness, "/__dyno/hello", &hello).0, "HTTP/1.1 200 OK");
    // A second page in the same run is turned away.
    assert_eq!(
        post(&harness, "/__dyno/hello", &hello).0,
        "HTTP/1.1 409 Conflict"
    );
    post(
        &harness,
        "/__dyno/events",
        r#"[{"kind":"navigated_at","epoch_ms":1000}]"#,
    );
    let (status, index) = get(&game, "/?sessionID=secret");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(index.contains(r#"<head><script src="/__dyno/probe.js" data-dyno-relay></script>"#));
    let (status, probe_script) = get(&game, "/__dyno/probe.js");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(probe_script.contains("data-dyno-relay"));
    assert_eq!(get(&game, "/game.js").1, "console.log('game')");
    assert_eq!(get(&game, "/../secret").0, "HTTP/1.1 404 Not Found");
    // A file that the scan skips is not part of the build, so the game cannot load it.
    assert_eq!(get(&game, "/.git/config").0, "HTTP/1.1 404 Not Found");
    assert_eq!(get(&game, "/.DS_Store").0, "HTTP/1.1 404 Not Found");
    // Only the probe's own path is dyno's: a build file beside it is served as any other.
    assert_eq!(get(&game, "/__dyno/data.json").1, "{\"the build's\":1}");
    assert_eq!(get(&game, "/__dyno/other.js").0, "HTTP/1.1 404 Not Found");
    post(
        &harness,
        "/__dyno/events",
        &format!(
            "[{},{},{},{}]",
            r#"{"kind":"document","time_origin":1050,"device_pixel_ratio":3}"#,
            probe(r#"{"type":"first_frame","t_ms":40}"#),
            r#"{"kind":"host","name":"reels:ready","epoch_ms":1200}"#,
            r#"{"kind":"unknown_to_this_version"}"#,
        ),
    );

    // Once the game is idle, dyno asks for the self-check, then makes the game active.
    assert_eq!(next_command(&harness), r#"{"command":"self_check"}"#);
    post(
        &harness,
        "/__dyno/events",
        r#"[{"kind":"self_check","webgl_renderer":"Apple GPU"}]"#,
    );
    assert_eq!(
        next_command(&harness),
        r#"{"command":"send","name":"reels:active"}"#
    );
    // The page held the frame's answer until dyno asked for everything it holds.
    assert_eq!(next_command(&harness), r#"{"command":"flush"}"#);
    post(
        &harness,
        "/__dyno/events",
        &format!(
            r#"[{},{{"kind":"flushed"}}]"#,
            probe(
                r#"{"type":"host_message","direction":"from_host","name":"reels:active","t_ms":900}"#
            )
        ),
    );
    assert_eq!(next_command(&harness), r#"{"command":"done"}"#);

    let (messages, times) = run.join().unwrap();
    let messages = messages.unwrap();
    let DriverMessage::Hello { driver, .. } = &messages[0] else {
        panic!("the first message is not hello: {:?}", messages[0]);
    };
    assert_eq!(driver.name, "serve");
    assert_eq!(driver.browser.as_deref(), Some("Safari 26.2 on iPhone"));

    // The run's steps on this machine's clock, for a device run's trace.
    let times = times.unwrap();
    let idle = times.idle_epoch_ms.unwrap();
    assert!(idle - times.navigated_epoch_ms >= 300.0, "{times:?}");
    assert!(times.ended_epoch_ms > idle, "{times:?}");
    assert!(matches!(messages.last(), Some(DriverMessage::Done { .. })));

    let marks: Vec<RunMark> = messages
        .iter()
        .filter_map(|message| match message {
            DriverMessage::Mark { mark, .. } => Some(*mark),
            _ => None,
        })
        .collect();
    assert_eq!(
        marks,
        [
            RunMark::Navigated,
            RunMark::FrameDocument,
            RunMark::Idle,
            RunMark::Activated
        ]
    );

    // Every request for a path of the build counts, a 404 too; the probe's own does not. No URL
    // keeps the query string, which can hold a session.
    let requests: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            DriverMessage::Request { url, .. } => Some(url.trim_start_matches(&game)),
            _ => None,
        })
        .collect();
    assert_eq!(
        requests,
        [
            "",
            "game.js",
            "../secret",
            ".git/config",
            ".DS_Store",
            "__dyno/data.json",
            "__dyno/other.js"
        ]
    );

    let self_check = messages.iter().find_map(|message| match message {
        DriverMessage::SelfCheck {
            device_pixel_ratio,
            webgl_renderer,
            frame_clock_at_ms,
            ..
        } => Some((
            *device_pixel_ratio,
            webgl_renderer.clone(),
            *frame_clock_at_ms,
        )),
        _ => None,
    });
    assert_eq!(
        self_check,
        Some((3.0, Some("Apple GPU".to_owned()), Some(50.0)))
    );

    // The game's message to its host is placed on the frame's clock: 1200 - 1050.
    assert!(messages.iter().any(|message| matches!(
        message,
        DriverMessage::Probe {
            event: ProbeEvent::HostMessage {
                direction: MessageDirection::ToHost,
                name,
                t_ms,
            }
        } if name == "reels:ready" && (*t_ms - 150.0).abs() < f64::EPSILON
    )));

    assert!(messages.iter().any(|message| matches!(
        message,
        DriverMessage::Probe {
            event: ProbeEvent::HostMessage {
                direction: MessageDirection::FromHost,
                ..
            }
        }
    )));

    // The lab reads the run as it reads a CDP run.
    let (measures, _) = dyno_run::lab::read(&messages, &request).unwrap();
    let status = |id: MeasureId| {
        measures
            .iter()
            .find(|measure| measure.id == id)
            .map(|measure| measure.status)
    };
    assert_eq!(
        status(MeasureId::LabSelfCheckFrameDpr),
        Some(MeasureStatus::Measured)
    );
    assert_eq!(
        status(MeasureId::LabSelfCheckFrameRequests),
        Some(MeasureStatus::Measured)
    );
    assert_eq!(
        status(MeasureId::LabTimeFirstFrame),
        Some(MeasureStatus::Measured)
    );
    assert_eq!(
        status(MeasureId::LabTimeReady),
        Some(MeasureStatus::Measured)
    );
    assert_eq!(status(MeasureId::LabJsHeap), Some(MeasureStatus::Missing));
}

#[test]
fn a_run_that_no_browser_opens_fails_with_a_reason() {
    let build = tempfile::tempdir().unwrap();
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        tunnel_host: None,
        wait_for_tunnel: Duration::from_secs(15),
        port: 0,
        wait_for_browser: Duration::from_millis(200),
        open_with: None,
    };
    let mut driver = ServeDriver::bind(build.path(), &options).unwrap();
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url: driver.game_url().to_owned(),
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 300,
        timeout_ms: 20_000,
    };
    let error = driver.run(&request).unwrap_err();
    assert!(
        error
            .detail()
            .contains("no browser opened the harness page")
    );
}

#[test]
fn a_run_that_never_goes_idle_ends_with_an_error_and_bad_requests_are_refused() {
    let build = tempfile::tempdir().unwrap();
    fs::write(build.path().join("game.js"), "console.log('game')").unwrap();
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        tunnel_host: None,
        wait_for_tunnel: Duration::from_secs(15),
        port: 0,
        wait_for_browser: Duration::from_secs(30),
        open_with: None,
    };
    let mut driver = ServeDriver::bind(build.path(), &options).unwrap();
    let harness = driver.harness_url().to_owned();
    let game = driver.game_url().to_owned();
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url: driver.launch_url(None),
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 300,
        timeout_ms: 800,
    };
    let run = thread::spawn(move || driver.run(&request));

    assert_eq!(
        post(&harness, "/__dyno/hello", "not json").0,
        "HTTP/1.1 400 Bad Request"
    );
    assert_eq!(
        post(&harness, "/__dyno/hello", r#"{"user_agent":"curl/8.0"}"#).0,
        "HTTP/1.1 200 OK"
    );
    assert_eq!(
        post(&harness, "/__dyno/events", "{}").0,
        "HTTP/1.1 400 Bad Request"
    );
    assert_eq!(get(&harness, "/elsewhere").0, "HTTP/1.1 404 Not Found");
    let (status, body) = exchange(&game, "HEAD", "/game.js", "");
    assert_eq!((status.as_str(), body.as_str()), ("HTTP/1.1 200 OK", ""));
    assert_eq!(
        post(&game, "/game.js", "").0,
        "HTTP/1.1 405 Method Not Allowed"
    );
    assert_eq!(get(&game, "/__dyno/other.js").0, "HTTP/1.1 404 Not Found");
    // No document with the probe ever loads, so the frame never goes idle.
    assert_eq!(next_command(&harness), r#"{"command":"done"}"#);

    let messages = run.join().unwrap().unwrap();
    let DriverMessage::Hello { driver, .. } = &messages[0] else {
        panic!("the first message is not hello: {:?}", messages[0]);
    };
    assert_eq!(driver.browser, None);
    let Some(DriverMessage::Error { message, .. }) = messages.last() else {
        panic!("the run did not end with an error: {messages:?}");
    };
    assert_eq!(message, "the run timed out before the game frame was idle");
}

#[test]
fn a_page_that_does_not_send_what_it_held_ends_the_run_with_an_error() {
    let build = tempfile::tempdir().unwrap();
    fs::write(build.path().join("index.html"), "<head>").unwrap();
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        tunnel_host: None,
        wait_for_tunnel: Duration::from_secs(15),
        port: 0,
        wait_for_browser: Duration::from_secs(30),
        open_with: None,
    };
    let mut driver = ServeDriver::bind(build.path(), &options).unwrap();
    let harness = driver.harness_url().to_owned();
    let game = driver.game_url().to_owned();
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url: driver.launch_url(None),
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 300,
        timeout_ms: 20_000,
    };
    let run = thread::spawn(move || driver.run(&request));

    post(&harness, "/__dyno/hello", r#"{"user_agent":"curl/8.0"}"#);
    get(&game, "/");
    post(
        &harness,
        "/__dyno/events",
        r#"[{"kind":"document","time_origin":1050,"device_pixel_ratio":3}]"#,
    );
    assert_eq!(next_command(&harness), r#"{"command":"self_check"}"#);
    post(
        &harness,
        "/__dyno/events",
        r#"[{"kind":"self_check","webgl_renderer":null}]"#,
    );
    assert_eq!(
        next_command(&harness),
        r#"{"command":"send","name":"reels:active"}"#
    );
    // The page takes the flush command but never sends what it held.
    assert_eq!(next_command(&harness), r#"{"command":"flush"}"#);
    assert_eq!(next_command(&harness), r#"{"command":"done"}"#);

    let messages = run.join().unwrap().unwrap();
    let Some(DriverMessage::Error { message, .. }) = messages.last() else {
        panic!("the run did not end with an error: {messages:?}");
    };
    assert_eq!(message, "the harness page did not send the events it held");
}

/// A build of one page, and a served run of it with these options, ready to run.
fn page_run(options: &ServeOptions) -> (tempfile::TempDir, ServeDriver, RunRequest) {
    let build = tempfile::tempdir().unwrap();
    fs::write(build.path().join("index.html"), "<head>").unwrap();
    let driver = ServeDriver::bind(build.path(), options).unwrap();
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url: driver.launch_url(None),
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 300,
        timeout_ms: 20_000,
    };
    (build, driver, request)
}

/// Plays the browser that opens the harness page at this URL, through to the end of the run. It
/// gives the URL of the game that the page names.
fn play_browser(harness: &str) -> String {
    let (status, page) = get(harness, "/");
    assert_eq!(status, "HTTP/1.1 200 OK");
    let game = page
        .split("const GAME_URL = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_owned();
    post(harness, "/__dyno/hello", r#"{"user_agent":"curl/8.0"}"#);
    get(&game, "/");
    post(
        harness,
        "/__dyno/events",
        r#"[{"kind":"document","time_origin":1050,"device_pixel_ratio":3}]"#,
    );
    assert_eq!(next_command(harness), r#"{"command":"self_check"}"#);
    post(
        harness,
        "/__dyno/events",
        r#"[{"kind":"self_check","webgl_renderer":null}]"#,
    );
    assert_eq!(
        next_command(harness),
        r#"{"command":"send","name":"reels:active"}"#
    );
    assert_eq!(next_command(harness), r#"{"command":"flush"}"#);
    post(harness, "/__dyno/events", r#"[{"kind":"flushed"}]"#);
    assert_eq!(next_command(harness), r#"{"command":"done"}"#);
    game
}

/// The options of a run on a phone whose device link has the loopback address as this machine's
/// tunnel address: this machine has it, and it is not the local network address.
fn options_with_a_tunnel(wait_for_tunnel: Duration, wait_for_browser: Duration) -> ServeOptions {
    ServeOptions {
        host: None,
        tunnel_host: Some("127.0.0.1".parse().unwrap()),
        wait_for_tunnel,
        port: 0,
        wait_for_browser,
        open_with: None,
    }
}

/// The URLs that a run asked to be opened, in order.
type Asked = Arc<Mutex<Vec<String>>>;

/// Waits until the run has asked for this many URLs, and gives the last.
fn wait_for_ask(asked: &Asked, count: usize) -> String {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let urls = asked.lock().unwrap().clone();
        if urls.len() >= count {
            return urls[count - 1].clone();
        }
        assert!(Instant::now() < until, "the run asked for {urls:?} only");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_servers_answer_while_the_browser_is_being_asked_to_open_the_page() {
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        tunnel_host: None,
        wait_for_tunnel: Duration::from_secs(15),
        port: 0,
        wait_for_browser: Duration::from_millis(500),
        open_with: None,
    };
    let (_build, mut driver, request) = page_run(&options);
    let harness = driver.harness_url().to_owned();
    let mut answered = None;
    // The opener asks for the page itself before it returns, as a browser that is quick would:
    // it gets the page, so the servers were serving before it was asked.
    let mut open = |url: &str| {
        answered = Some((url.to_owned(), get(url, "/").0));
        Opened::No
    };
    let error = driver.run_opening(&request, &mut open).unwrap_err();
    assert!(
        error
            .detail()
            .contains("no browser opened the harness page")
    );
    assert_eq!(answered, Some((harness, "HTTP/1.1 200 OK".to_owned())));
    // The request came to the one address there is.
    assert_eq!(driver.serve_path(), Some(ServePath::Given));
}

#[test]
fn a_browser_that_comes_to_the_tunnel_address_in_time_is_asked_for_no_other() {
    let options = options_with_a_tunnel(Duration::from_millis(300), Duration::from_secs(30));
    let (_build, mut driver, request) = page_run(&options);
    let harness = driver.harness_url().to_owned();
    assert!(harness.starts_with("http://127.0.0.1:"), "{harness}");
    let asked = Asked::default();
    let asks = Arc::clone(&asked);
    let run = thread::spawn(move || {
        let mut open = |url: &str| {
            asks.lock().unwrap().push(url.to_owned());
            Opened::Yes
        };
        let messages = driver.run_opening(&request, &mut open);
        (messages, driver.serve_path())
    });
    assert_eq!(wait_for_ask(&asked, 1), harness);
    let game = play_browser(&harness);
    assert!(game.starts_with("http://127.0.0.1:"), "{game}");

    let (messages, path) = run.join().unwrap();
    assert!(matches!(
        messages.unwrap().last(),
        Some(DriverMessage::Done { .. })
    ));
    // The run lasted far longer than the wait, and still asked once.
    assert_eq!(*asked.lock().unwrap(), [harness]);
    assert_eq!(path, Some(ServePath::Tunnel));
}

#[test]
fn with_no_request_at_the_tunnel_address_the_browser_is_asked_for_the_local_network_address() {
    let options = options_with_a_tunnel(Duration::from_millis(300), Duration::from_secs(30));
    let (_build, mut driver, request) = page_run(&options);
    let Some(local) = driver.fallback_harness_url().map(str::to_owned) else {
        // A machine with no network has no second address for a phone.
        return;
    };
    let tunnel = driver.harness_url().to_owned();
    let asked = Asked::default();
    let asks = Arc::clone(&asked);
    let started = Instant::now();
    let run = thread::spawn(move || {
        let mut open = |url: &str| {
            asks.lock().unwrap().push(url.to_owned());
            Opened::Yes
        };
        let messages = driver.run_opening(&request, &mut open);
        (messages, driver.serve_path())
    });
    // The browser never comes to the tunnel address, as when the tunnel has gone.
    assert_eq!(wait_for_ask(&asked, 2), local);
    assert!(started.elapsed() >= Duration::from_millis(300));
    // The page at the local network address loads the game from that address too.
    let game = play_browser(&local);
    let local_host = local
        .trim_start_matches("http://")
        .rsplit_once(':')
        .unwrap()
        .0;
    assert!(game.starts_with(&format!("http://{local_host}:")), "{game}");

    let (messages, path) = run.join().unwrap();
    let messages = messages.unwrap();
    assert!(matches!(messages.last(), Some(DriverMessage::Done { .. })));
    assert_eq!(*asked.lock().unwrap(), [tunnel, local]);
    // The path is where the browser came, not where it was first sent.
    assert_eq!(path, Some(ServePath::LocalNetwork));
    // The game's request is named by the server it came to.
    let request_url = messages.iter().find_map(|message| match message {
        DriverMessage::Request { url, .. } => Some(url.clone()),
        _ => None,
    });
    assert_eq!(request_url.as_deref(), Some(game.as_str()));
}

#[test]
fn a_browser_that_was_not_told_is_not_asked_again_and_no_request_is_no_path() {
    let options = options_with_a_tunnel(Duration::from_millis(100), Duration::from_millis(700));
    let (_build, mut driver, request) = page_run(&options);
    let mut asked = Vec::new();
    // A person opens the page by hand, so a second ask would not help them.
    let mut open = |url: &str| {
        asked.push(url.to_owned());
        Opened::No
    };
    let error = driver.run_opening(&request, &mut open).unwrap_err();
    assert!(
        error
            .detail()
            .contains("no browser opened the harness page")
    );
    assert_eq!(asked, [driver.harness_url().to_owned()]);
    // The tunnel address was tried, and no request came anywhere: no path is recorded.
    assert_eq!(driver.asked_path(), ServePath::Tunnel);
    assert_eq!(driver.serve_path(), None);
}

#[test]
fn an_opener_that_says_stop_ends_the_run_at_once_with_its_reason() {
    let options = options_with_a_tunnel(Duration::from_secs(15), Duration::from_secs(60));
    let (_build, mut driver, request) = page_run(&options);
    let started = Instant::now();
    let mut open = |_: &str| Opened::Stop("the phone is locked".to_owned());
    let error = driver.run_opening(&request, &mut open).unwrap_err();
    assert_eq!(error.detail(), "the phone is locked");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_run_with_no_opener_asks_no_one_and_records_where_the_browser_came() {
    let options = options_with_a_tunnel(Duration::from_millis(100), Duration::from_secs(30));
    let (_build, mut driver, request) = page_run(&options);
    // A person opens whichever address reaches this machine: here the second.
    let harness = driver
        .fallback_harness_url()
        .unwrap_or(driver.harness_url())
        .to_owned();
    let path = match driver.fallback_harness_url() {
        Some(_) => ServePath::LocalNetwork,
        None => ServePath::Tunnel,
    };
    let run = thread::spawn(move || {
        let messages = driver.run(&request);
        (messages, driver.serve_path())
    });
    play_browser(&harness);
    let (messages, came_by) = run.join().unwrap();
    assert!(matches!(
        messages.unwrap().last(),
        Some(DriverMessage::Done { .. })
    ));
    assert_eq!(came_by, Some(path));
}

/// What a server answers a browser that starts a TLS connection on its port and waits for the
/// server's hello, and how long the answer took. The browser sends the start of a client hello,
/// which is all a plain HTTP server needs to know it.
fn try_tls(url: &str) -> (Vec<u8>, Duration) {
    let address = url.trim_start_matches("http://").trim_end_matches('/');
    let mut socket = TcpStream::connect(address).unwrap();
    // Safari waits far longer than this for the server's hello.
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let started = Instant::now();
    socket
        .write_all(&[
            0x16, 0x03, 0x01, 0x02, 0x00, 0x01, 0x00, 0x01, 0xfc, 0x03, 0x03,
        ])
        .unwrap();
    let mut answer = Vec::new();
    socket.read_to_end(&mut answer).unwrap();
    (answer, started.elapsed())
}

#[test]
fn a_browser_that_tries_https_first_is_refused_at_once_and_its_http_run_completes() {
    let build = tempfile::tempdir().unwrap();
    fs::write(build.path().join("index.html"), "<head>").unwrap();
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        tunnel_host: None,
        wait_for_tunnel: Duration::from_secs(15),
        port: 0,
        wait_for_browser: Duration::from_secs(30),
        open_with: None,
    };
    let mut driver = ServeDriver::bind(build.path(), &options).unwrap();
    let harness = driver.harness_url().to_owned();
    let game = driver.game_url().to_owned();
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url: driver.launch_url(None),
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 300,
        timeout_ms: 20_000,
    };
    let run = thread::spawn(move || driver.run(&request));

    // Safari tries HTTPS on each port before plain HTTP. Each server says at once, with a fatal
    // TLS alert, that it has no TLS.
    for server in [&harness, &game] {
        let (answer, took) = try_tls(server);
        assert_eq!(
            answer,
            [0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28],
            "{server}"
        );
        assert!(took < Duration::from_secs(1), "{server}: {took:?}");
    }

    // The fall back to HTTP then runs as if the browser had never tried.
    assert_eq!(get(&harness, "/").0, "HTTP/1.1 200 OK");
    post(&harness, "/__dyno/hello", r#"{"user_agent":"curl/8.0"}"#);
    get(&game, "/");
    post(
        &harness,
        "/__dyno/events",
        r#"[{"kind":"document","time_origin":1050,"device_pixel_ratio":3}]"#,
    );
    assert_eq!(next_command(&harness), r#"{"command":"self_check"}"#);
    post(
        &harness,
        "/__dyno/events",
        r#"[{"kind":"self_check","webgl_renderer":null}]"#,
    );
    assert_eq!(
        next_command(&harness),
        r#"{"command":"send","name":"reels:active"}"#
    );
    assert_eq!(next_command(&harness), r#"{"command":"flush"}"#);
    post(&harness, "/__dyno/events", r#"[{"kind":"flushed"}]"#);
    assert_eq!(next_command(&harness), r#"{"command":"done"}"#);

    let messages = run.join().unwrap().unwrap();
    assert!(
        matches!(messages.last(), Some(DriverMessage::Done { .. })),
        "{messages:?}"
    );
    // The refused connection to the game server is not a game request: only the page is.
    let requests = messages
        .iter()
        .filter(|message| matches!(message, DriverMessage::Request { .. }))
        .count();
    assert_eq!(requests, 1, "{messages:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_browser_that_dyno_starts_gives_its_memory_before_the_last_message_and_is_stopped() {
    use dyno_schema::timeline::ProcessRole;

    let build = tempfile::tempdir().unwrap();
    fs::write(build.path().join("index.html"), "<head>").unwrap();
    // The program stands in for a browser: this test opens the page itself. Only this test in
    // the file starts a program, so no other fork can hold the script open while it runs.
    let program = build.path().join("browser.sh");
    fs::write(&program, "#!/bin/sh\nexec sleep 30\n").unwrap();
    let mut permissions = fs::metadata(&program).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&program, permissions).unwrap();
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        tunnel_host: None,
        wait_for_tunnel: Duration::from_secs(15),
        port: 0,
        wait_for_browser: Duration::from_secs(30),
        open_with: Some(program),
    };
    let mut driver = ServeDriver::bind(build.path(), &options).unwrap();
    let harness = driver.harness_url().to_owned();
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url: driver.launch_url(None),
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 300,
        timeout_ms: 800,
    };
    let run = thread::spawn(move || driver.run(&request));

    post(&harness, "/__dyno/hello", r#"{"user_agent":"curl/8.0"}"#);
    // No document with the probe loads, so the run times out and ends with an error.
    assert_eq!(next_command(&harness), r#"{"command":"done"}"#);

    let messages = run.join().unwrap().unwrap();
    assert!(
        matches!(messages.last(), Some(DriverMessage::Error { .. })),
        "{messages:?}"
    );
    let last = messages.len() - 1;
    let memory = messages
        .iter()
        .position(|message| matches!(message, DriverMessage::Memory { .. }));
    assert!(memory.is_some_and(|at| at < last), "{messages:?}");
    let Some(DriverMessage::Roles { roles, .. }) = messages.get(last - 1) else {
        panic!("no roles before the last message: {messages:?}");
    };
    let browser = roles
        .iter()
        .find(|role| role.role == ProcessRole::Browser)
        .expect("the program is the browser")
        .pid;
    // The program's process group is gone once the run is over.
    let group = rustix::process::Pid::from_raw(i32::try_from(browser).unwrap()).unwrap();
    assert!(rustix::process::test_kill_process_group(group).is_err());
}
