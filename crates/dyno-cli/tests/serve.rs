//! A served check through the `dyno` binary, with this test as the browser on the network.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use common::{data_dir, dyno_in, json};

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

/// A port with the next one free too, for the harness page and the game.
fn two_free_ports() -> u16 {
    loop {
        let first = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = first.local_addr().unwrap().port();
        if port < u16::MAX && TcpListener::bind(("127.0.0.1", port + 1)).is_ok() {
            return port;
        }
    }
}

/// Waits until the server at `address` takes connections.
fn wait_for(address: &str) {
    let until = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect(address).is_err() {
        assert!(Instant::now() < until, "dyno did not serve on {address}");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_served_check_runs_in_the_browser_that_opens_it_and_records_no_query() {
    let port = two_free_ports();
    let harness = format!("127.0.0.1:{port}");
    let game = format!("127.0.0.1:{}", port + 1);
    let port_text = port.to_string();
    let check = thread::spawn(move || {
        dyno_in(
            &data_dir(),
            &[],
            &[
                "check",
                "tiny-build",
                "--driver",
                "serve",
                "--serve-host",
                "127.0.0.1",
                "--serve-port",
                &port_text,
                "--serve-query",
                "sessionID=secret",
            ],
        )
    });

    wait_for(&harness);
    let (_, page) = exchange(&harness, "GET", "/", "");
    assert!(page.contains(&format!("http://{game}/?sessionID=secret")));
    exchange(
        &harness,
        "POST",
        "/__dyno/hello",
        r#"{"user_agent":"curl/8.0"}"#,
    );
    exchange(&game, "GET", "/?sessionID=secret", "");
    exchange(
        &harness,
        "POST",
        "/__dyno/events",
        r#"[{"kind":"navigated_at","epoch_ms":1000},{"kind":"document","time_origin":1050,"device_pixel_ratio":3}]"#,
    );
    assert_eq!(next_command(&harness), r#"{"command":"self_check"}"#);
    exchange(
        &harness,
        "POST",
        "/__dyno/events",
        r#"[{"kind":"self_check","webgl_renderer":"Apple GPU"}]"#,
    );
    assert_eq!(
        next_command(&harness),
        r#"{"command":"send","name":"reels:active"}"#
    );
    assert_eq!(next_command(&harness), r#"{"command":"flush"}"#);
    exchange(
        &harness,
        "POST",
        "/__dyno/events",
        r#"[{"kind":"flushed"}]"#,
    );
    assert_eq!(next_command(&harness), r#"{"command":"done"}"#);

    let outcome = check.join().unwrap();
    assert!(
        outcome
            .stderr
            .contains(&format!("open http://{harness}/ in the browser")),
        "{}",
        outcome.stderr
    );
    assert!(!outcome.stdout.contains("secret"));
    let result = json(&outcome);
    assert_eq!(result["run"]["driver"]["name"], "serve");
    assert_eq!(
        result["run"]["target"]["launch_url"],
        serde_json::Value::Null
    );
    assert_eq!(result["deterministic"]["inputs"]["launch_url"], true);
    let frame_dpr = result["run"]["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|measure| measure["id"] == "lab.self_check.frame_dpr")
        .unwrap();
    assert_eq!(frame_dpr["status"], "measured", "{frame_dpr}");
}
