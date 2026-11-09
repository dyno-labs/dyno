//! `dyno check <build> --browser`: dyno serves the build folder on 127.0.0.1 and loads it in its
//! own Chromium, so the run is tied to the files the scan read.

mod common;

use std::fs;

use common::{Outcome, copy_dir, data_dir, dyno, dyno_in, json};
use serde_json::Value;

const INCOMPLETE: i32 = 2;
const USAGE: i32 = 64;

const LAUNCH_URL: &str = "https://games.example/tiny/";

fn assert_exit(outcome: &Outcome, code: i32) {
    assert_eq!(
        outcome.code, code,
        "args: {:?}\nstdout: {}\nstderr: {}",
        outcome.args, outcome.stdout, outcome.stderr
    );
}

/// Asserts that the check is a usage error that names `problem`, and writes no result.
fn assert_usage_error(outcome: &Outcome, problem: &str) {
    assert_exit(outcome, USAGE);
    assert!(outcome.stderr.contains(problem), "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "", "{:?}", outcome.args);
}

/// The run measure with this id.
fn run_measure<'a>(result: &'a Value, id: &str) -> &'a Value {
    result["run"]["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|measure| measure["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in the result"))
}

#[test]
fn a_browser_check_serves_the_build_on_the_loopback_address_and_links_the_run_to_it() {
    let outcome = dyno(&["check", "tiny-build", "--browser"]);
    // The tests have no Chromium, so the run's measures are missing.
    assert_exit(&outcome, INCOMPLETE);
    let result = json(&outcome);
    assert_eq!(result["run"]["build_link"], "served");
    assert_eq!(result["run"]["target"]["build"], "tiny-build");
    // The build's root, with no query string.
    let launch_url = result["run"]["target"]["launch_url"].as_str().unwrap();
    let port = launch_url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix('/'))
        .unwrap_or_else(|| panic!("{launch_url}"));
    assert!(port.parse::<u16>().is_ok(), "{launch_url}");
    assert_eq!(result["deterministic"]["inputs"]["launch_url"], true);
}

#[test]
fn a_browser_check_gives_what_a_check_with_a_launch_url_gives() {
    let served = json(&dyno(&["check", "tiny-build", "--browser"]));
    let asserted = json(&dyno(&["check", "tiny-build", LAUNCH_URL]));
    assert_eq!(served["deterministic"], asserted["deterministic"]);
    assert_eq!(served["result_hash"], asserted["result_hash"]);
    assert_eq!(served["run"]["measures"], asserted["run"]["measures"]);
    assert_eq!(served["run"]["findings"], asserted["run"]["findings"]);
    assert_eq!(asserted["run"]["build_link"], "asserted");
}

#[test]
fn a_games_file_with_browser_serves_each_game_build() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&data_dir().join("tiny-build"), &dir.path().join("dist"));
    fs::write(
        dir.path().join("games.toml"),
        "schema_version = 0\n\n[[game]]\nid = \"tiny\"\nbuild = \"dist\"\n",
    )
    .unwrap();
    let outcome = dyno_in(
        dir.path(),
        &[],
        &["check", "--games", "games.toml", "--browser"],
    );
    assert_exit(&outcome, INCOMPLETE);
    let result = &json(&outcome)["games"][0]["result"];
    assert_eq!(result["run"]["build_link"], "served");
    assert_eq!(result["deterministic"]["inputs"]["launch_url"], true);
}

#[test]
fn browser_needs_a_build_folder_and_no_launch_url() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("build.zip");
    fs::write(&zip, "PK").unwrap();
    let zip = zip.display().to_string();
    let cases: [(&[&str], &str); 4] = [
        (
            &["check", LAUNCH_URL, "--browser"],
            "give a build folder and no launch URL",
        ),
        (
            &["check", "tiny-build", LAUNCH_URL, "--browser"],
            "give a build folder and no launch URL",
        ),
        (&["check", &zip, "--browser"], "unzip a .zip first"),
        (
            &["check", "no-such-build", "--browser"],
            "no-such-build is not one",
        ),
    ];
    for (args, problem) in cases {
        assert_usage_error(&dyno(args), problem);
    }
}

#[test]
fn browser_takes_no_driver_but_cdp() {
    for args in [
        &["check", "tiny-build", "--browser", "--driver", "serve"][..],
        &[
            "check",
            "tiny-build",
            "--browser",
            "--driver",
            "exec:./fake-driver.sh",
        ],
    ] {
        assert_usage_error(&dyno(args), "takes no --driver but cdp");
    }
    let named = dyno(&["check", "tiny-build", "--browser", "--driver", "cdp"]);
    assert_exit(&named, INCOMPLETE);
}

#[test]
fn browser_takes_no_device_run() {
    assert_usage_error(
        &dyno(&["check", "tiny-build", "--browser", "--device"]),
        "--device needs --driver serve",
    );
}

#[test]
fn browser_with_a_games_file_needs_a_build_folder_and_no_launch_url_for_every_game() {
    // The second game of games.toml has a launch URL and no build.
    assert_usage_error(
        &dyno(&["check", "--games", "games.toml", "--browser"]),
        "game \"tiny-online\": --browser serves the build",
    );
}

/// A page that uploads one 64 x 32 RGBA texture, 8,192 bytes, and tells its host it is ready.
const PAGE: &str = r#"<!doctype html>
<canvas id="c"></canvas>
<script src="game.js"></script>
"#;

const GAME: &str = r#"
const gl = document.getElementById("c").getContext("webgl");
gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 64, 32, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(64 * 32 * 4));
parent.postMessage({ type: "reels:ready" }, "*");
"#;

#[test]
#[ignore = "needs Chromium"]
fn a_browser_check_loads_the_served_build_in_chromium_and_the_drivers_probe_sees_the_game() {
    let build = tempfile::tempdir().unwrap();
    fs::write(build.path().join("index.html"), PAGE).unwrap();
    fs::write(build.path().join("game.js"), GAME).unwrap();
    let chromium = dyno_cdp::browser::find_chromium().unwrap();
    let outcome = dyno_in(
        &data_dir(),
        &[("DYNO_CHROMIUM", chromium.to_str().unwrap())],
        &["check", build.path().to_str().unwrap(), "--browser"],
    );
    let result = json(&outcome);
    assert_eq!(result["run"]["build_link"], "served");
    assert_eq!(result["run"]["driver"]["name"], "cdp");
    assert_eq!(
        run_measure(&result, "lab.self_check.frame_requests")["value"],
        2,
        "the page and its script"
    );
    assert_eq!(
        run_measure(&result, "lab.textures.uploaded")["value"],
        8_192,
        "the driver's probe saw the upload"
    );
    assert_eq!(run_measure(&result, "lab.time.ready")["status"], "measured");
}
