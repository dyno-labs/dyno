//! A served run in a real browser: headless Chromium, which dyno starts on the harness page with
//! `open_with`, as `--open-with` does. It shows what no test that plays the browser can: that the
//! page's own requests carry the run's key, and that the game frame learns nothing of the key
//! from the browser. Ignored by default, because it needs Chromium: run it with
//! `cargo test -p dyno-serve -- --ignored`. `DYNO_CHROMIUM` names the binary when it is not on
//! the PATH, and `DYNO_CHROMIUM_SANDBOX=off` runs it with no sandbox, as in a container.

#![cfg(target_os = "linux")]

use std::fs;
use std::time::Duration;

use dyno_run::Driver;
use dyno_schema::probe::ProbeEvent;
use dyno_schema::profile::{HostProfile, Scenario};
use dyno_schema::runner::{DriverMessage, RUNNER_PROTOCOL_VERSION, RunRequest};
use dyno_serve::{ServeDriver, ServeOptions};

/// A game that tells its host what it knows of the page around it, and then tries to drive the
/// run itself: it posts a frame document with a pixel ratio of 99 and the end of a flush to the
/// harness server, on the port before its own, with no key, as the game of another check could.
const GAME: &str = r#"
parent.postMessage("referrer:" + document.referrer, "*");
parent.postMessage("ancestors:" + Array.from(location.ancestorOrigins || []).join(" "), "*");
const harness = location.protocol + "//" + location.hostname + ":" + (Number(location.port) - 1);
const forged = JSON.stringify([{ kind: "document", time_origin: 1, device_pixel_ratio: 99 }, { kind: "flushed" }]);
for (const path of ["/__dyno/events", "/__dyno/hello", "/__dyno/next"]) {
  fetch(harness + path, { method: "POST", mode: "no-cors", body: forged }).then(
    () => parent.postMessage("tried:" + path, "*"),
    () => parent.postMessage("tried:" + path, "*"),
  );
}
"#;

#[test]
#[ignore = "needs Chromium"]
fn in_chromium_the_page_drives_the_run_with_its_key_and_the_frame_learns_none_of_it() {
    let chromium = dyno_cdp::browser::find_chromium().expect("Chromium is installed");
    let build = tempfile::tempdir().unwrap();
    fs::write(
        build.path().join("index.html"),
        r#"<!doctype html><html><head><title>game</title></head><body><script src="game.js"></script></body></html>"#,
    )
    .unwrap();
    fs::write(build.path().join("game.js"), GAME).unwrap();
    let browser = tempfile::tempdir().unwrap();
    let program = browser.path().join("chromium.sh");
    let no_sandbox =
        if std::env::var_os("DYNO_CHROMIUM_SANDBOX").is_some_and(|value| value == "off") {
            "--no-sandbox"
        } else {
            ""
        };
    fs::write(
        &program,
        format!(
            "#!/bin/sh\nexec '{}' --headless=new --no-first-run --no-default-browser-check \
             --user-data-dir='{}' {no_sandbox} \"$1\"\n",
            chromium.display(),
            browser.path().join("profile").display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&program).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&program, permissions).unwrap();
    let options = ServeOptions {
        host: Some("127.0.0.1".to_owned()),
        tunnel_host: None,
        port: 0,
        wait_for_browser: Duration::from_secs(60),
        open_with: Some(program),
    };
    let mut driver = ServeDriver::bind(build.path(), &options).unwrap();
    let harness = driver.harness_url().to_owned();
    let harness_origin = harness.split("/?").next().unwrap().to_owned();
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url: driver.launch_url(None),
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 1_000,
        timeout_ms: 30_000,
    };
    let messages = driver.run(&request).unwrap();

    // The page said hello and ran to the end: its requests carried the key.
    assert!(
        matches!(messages.last(), Some(DriverMessage::Done { .. })),
        "{messages:?}"
    );
    let DriverMessage::Hello { driver: hello, .. } = &messages[0] else {
        panic!("the first message is not hello: {:?}", messages[0]);
    };
    assert!(
        hello
            .browser
            .as_deref()
            .is_some_and(|name| name.starts_with("Chrome")),
        "{hello:?}"
    );
    let told: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            DriverMessage::Probe {
                event: ProbeEvent::HostMessage { name, .. },
            } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    // The frame's document has no referrer, and its ancestors are origins with no URL.
    assert!(told.contains(&"referrer:"), "{told:?}");
    assert!(
        told.contains(&format!("ancestors:{harness_origin}").as_str()),
        "{told:?}"
    );
    assert!(told.iter().all(|name| !name.contains("key")), "{told:?}");
    // The game's tries reached the harness server and were refused: the self-check has the
    // frame's own pixel ratio, from its one document.
    for path in ["/__dyno/events", "/__dyno/hello", "/__dyno/next"] {
        assert!(told.contains(&format!("tried:{path}").as_str()), "{told:?}");
    }
    let self_check = messages.iter().find_map(|message| match message {
        DriverMessage::SelfCheck {
            device_pixel_ratio, ..
        } => Some(*device_pixel_ratio),
        _ => None,
    });
    assert!(
        self_check.is_some_and(|ratio| ratio != 99.0),
        "{self_check:?}"
    );
}
