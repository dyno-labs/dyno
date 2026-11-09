//! What ties a result to a run, a build and a game: the run id, the build link, the environment,
//! the game id from `--game-id` or `dyno.toml`, and the empty build of `--baseline`.

mod common;

use std::fs;
use std::path::Path;

use common::{Outcome, copy_dir, data_dir, dyno, dyno_in, json};
use serde_json::Value;

const USAGE: i32 = 64;

const MOON_LANDER: &str = "6f1c2a8e-0000-4000-8000-000000000000";
const TILE_POP: &str = "1b4e28ba-2fa1-41d2-883f-0016d3cca427";

/// The empty build's id, which never changes.
const EMPTY_BUILD_ID: &str = "7114691fda7748ccc7c091ffeae74100f190d0557000f8739823a040446c5c4e";

fn assert_exit(outcome: &Outcome, code: i32) {
    assert_eq!(
        outcome.code, code,
        "args: {:?}\nstdout: {}\nstderr: {}",
        outcome.args, outcome.stdout, outcome.stderr
    );
}

/// Whether the text is a UUID v4 in lower case with dashes.
fn is_uuid_v4(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    let lengths: Vec<usize> = parts.iter().map(|part| part.len()).collect();
    lengths == [8, 4, 4, 4, 12]
        && text
            .chars()
            .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
        && parts[2].starts_with('4')
        && parts[3].starts_with(['8', '9', 'a', 'b'])
}

/// A repository with tiny-build at `games/tiny/dist`, and a `dyno.toml` at its root.
fn repository(link_file: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    copy_dir(
        &data_dir().join("tiny-build"),
        &root.path().join("games/tiny/dist"),
    );
    fs::write(root.path().join("dyno.toml"), link_file).unwrap();
    root
}

fn tiny_link_file() -> String {
    format!(
        "schema_version = 0\n\n[[game]]\nid = \"{MOON_LANDER}\"\nname = \"Tiny\"\nbuild = \"games/tiny/dist\"\n"
    )
}

/// Runs `dyno check` in `dir`.
fn check_in(dir: &Path, args: &[&str]) -> Value {
    let mut all_args = vec!["check"];
    all_args.extend_from_slice(args);
    json(&dyno_in(dir, &[], &all_args))
}

#[test]
fn every_check_gets_its_own_run_id() {
    let first = json(&dyno(&["check", "tiny-build"]));
    let second = json(&dyno(&["check", "tiny-build"]));
    let first_id = first["run"]["run_id"].as_str().unwrap();
    let second_id = second["run"]["run_id"].as_str().unwrap();
    assert!(is_uuid_v4(first_id), "{first_id}");
    assert!(is_uuid_v4(second_id), "{second_id}");
    assert_ne!(first_id, second_id);
    assert_eq!(first["result_hash"], second["result_hash"]);
}

#[test]
fn each_game_of_a_games_file_gets_its_own_run_id() {
    let catalogue = json(&dyno(&["check", "--games", "games.toml"]));
    let ids: Vec<&str> = catalogue["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|game| game["result"]["run"]["run_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn the_build_link_and_environment_follow_the_target() {
    let build_only = json(&dyno(&["check", "tiny-build"]));
    assert_eq!(build_only["run"]["build_link"], "scanned");
    assert_eq!(build_only["run"]["environment"], Value::Null);
    let url_only = json(&dyno(&["check", "https://games.example/tiny/"]));
    assert_eq!(url_only["run"]["build_link"], "none");
    // Chromium did not start, so nothing ran.
    assert_eq!(url_only["run"]["environment"], Value::Null);
    let both = json(&dyno(&[
        "check",
        "tiny-build",
        "https://games.example/tiny/",
    ]));
    assert_eq!(both["run"]["build_link"], "asserted");
}

#[cfg(unix)]
#[test]
fn an_exec_run_has_only_the_browser_its_hello_names() {
    let result = json(&dyno(&[
        "check",
        "https://games.example/tiny/",
        "--driver",
        "exec:./fake-driver.sh",
    ]));
    // The fake driver names no browser, so nothing is known.
    assert_eq!(
        result["run"]["environment"],
        serde_json::json!({ "browser": null, "os": null, "device": null })
    );
}

#[test]
fn a_game_id_labels_the_result_and_stays_out_of_the_hash() {
    let plain = json(&dyno(&["check", "tiny-build"]));
    let outcome = dyno(&[
        "check",
        "tiny-build",
        "--game-id",
        &MOON_LANDER.to_ascii_uppercase(),
    ]);
    let linked = json(&outcome);
    assert_eq!(linked["run"]["release"]["game_id"], MOON_LANDER);
    assert_eq!(linked["result_hash"], plain["result_hash"]);
    assert_eq!(plain["run"]["release"]["game_id"], Value::Null);
}

#[test]
fn a_game_id_that_is_not_a_uuid_is_a_usage_error() {
    let outcome = dyno(&["check", "tiny-build", "--game-id", "moon_lander"]);
    assert_exit(&outcome, USAGE);
    assert!(
        outcome.stderr.contains("is not a UUID"),
        "{}",
        outcome.stderr
    );
    let outcome = dyno(&["check", "--games", "games.toml", "--game-id", MOON_LANDER]);
    assert_exit(&outcome, USAGE);
}

#[test]
fn the_dyno_toml_above_the_build_names_the_game_and_its_label() {
    let root = repository(&tiny_link_file());
    let result = check_in(root.path(), &["games/tiny/dist"]);
    assert_eq!(result["run"]["release"]["game_id"], MOON_LANDER);
    assert_eq!(result["run"]["release"]["game"], "Tiny");

    // A label on the command line wins over the file's name, and the id still comes from it.
    let labelled = check_in(root.path(), &["games/tiny/dist", "--game", "tiny v2"]);
    assert_eq!(labelled["run"]["release"]["game_id"], MOON_LANDER);
    assert_eq!(labelled["run"]["release"]["game"], "tiny v2");
}

#[test]
fn a_game_id_on_the_command_line_wins_over_dyno_toml() {
    let root = repository(&tiny_link_file());
    let result = check_in(root.path(), &["games/tiny/dist", "--game-id", TILE_POP]);
    assert_eq!(result["run"]["release"]["game_id"], TILE_POP);
    assert_eq!(result["run"]["release"]["game"], Value::Null);
}

#[test]
fn a_dyno_toml_with_no_entry_for_the_build_gives_no_game() {
    let root = repository(&format!(
        "schema_version = 0\n[[game]]\nid = \"{TILE_POP}\"\nbuild = \"games/other/dist\"\n"
    ));
    let result = check_in(root.path(), &["games/tiny/dist"]);
    assert_eq!(result["run"]["release"]["game_id"], Value::Null);
}

#[test]
fn a_malformed_dyno_toml_is_a_usage_error() {
    let root = repository(
        "schema_version = 0\n[[game]]\nid = \"moon_lander\"\nbuild = \"games/tiny/dist\"\n",
    );
    let outcome = dyno_in(root.path(), &[], &["check", "games/tiny/dist"]);
    assert_exit(&outcome, USAGE);
    assert_eq!(outcome.stdout, "");
    assert!(outcome.stderr.contains("dyno.toml"), "{}", outcome.stderr);
}

#[test]
fn a_games_file_links_each_build_by_dyno_toml() {
    let root = repository(&tiny_link_file());
    fs::write(
        root.path().join("games.toml"),
        "schema_version = 0\n[[game]]\nid = \"tiny\"\nbuild = \"games/tiny/dist\"\n",
    )
    .unwrap();
    let catalogue = check_in(root.path(), &["--games", "games.toml"]);
    let release = &catalogue["games"][0]["result"]["run"]["release"];
    assert_eq!(release["game_id"], MOON_LANDER);
    // The games file's id is the label, as before.
    assert_eq!(release["game"], "tiny");
}

#[test]
fn a_malformed_dyno_toml_stops_a_games_file_before_any_check() {
    let root = repository("schema_version = 7\n");
    fs::write(
        root.path().join("games.toml"),
        "schema_version = 0\n[[game]]\nid = \"tiny\"\nbuild = \"games/tiny/dist\"\n",
    )
    .unwrap();
    let outcome = dyno_in(root.path(), &[], &["check", "--games", "games.toml"]);
    assert_exit(&outcome, USAGE);
    assert!(
        outcome.stderr.contains("game \"tiny\""),
        "{}",
        outcome.stderr
    );
}

#[test]
fn the_baseline_is_the_empty_build_with_a_constant_id() {
    let result = json(&dyno(&["check", "--baseline"]));
    assert_eq!(result["deterministic"]["build"]["build_id"], EMPTY_BUILD_ID);
    assert_eq!(result["deterministic"]["inputs"]["build"], true);
    assert_eq!(result["run"]["build_link"], "scanned");
    // dyno wrote the build to a folder of its own, which nobody gave.
    assert_eq!(result["run"]["target"]["build"], Value::Null);
}

#[test]
fn the_baseline_takes_no_target_and_no_game() {
    let cases: [&[&str]; 5] = [
        &["check", "--baseline", "tiny-build"],
        &["check", "--baseline", "--games", "games.toml"],
        &["check", "--baseline", "--game", "tiny"],
        &["check", "--baseline", "--release", "v1"],
        &["check", "--baseline", "--game-id", MOON_LANDER],
    ];
    for args in cases {
        let outcome = dyno(args);
        assert_exit(&outcome, USAGE);
    }
}
