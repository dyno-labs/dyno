//! A whole served run, with this test as the browser: it loads the harness page and the game the
//! way a browser would, and posts what the harness page would relay.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use dyno_run::Driver;
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::MeasureStatus;
use dyno_schema::probe::{MessageDirection, ProbeEvent};
use dyno_schema::profile::{HostProfile, Scenario};
use dyno_schema::runner::{DriverMessage, RUNNER_PROTOCOL_VERSION, RunMark, RunRequest};
use dyno_serve::{ServeDriver, ServeOptions};

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
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        port: 0,
        wait_for_browser: Duration::from_secs(30),
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

    // The game's two requests, and the 404, count; the probe's own does not. No URL keeps the
    // query string, which can hold a session.
    let requests: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            DriverMessage::Request { url, .. } => Some(url.trim_start_matches(&game)),
            _ => None,
        })
        .collect();
    assert_eq!(requests, ["", "game.js", "../secret"]);

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
        port: 0,
        wait_for_browser: Duration::from_millis(200),
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
        port: 0,
        wait_for_browser: Duration::from_secs(30),
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
        port: 0,
        wait_for_browser: Duration::from_secs(30),
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
