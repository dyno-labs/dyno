//! The summary on stderr gives the status and why, the key numbers, the advice and where the full
//! result is. It has no stable format, so these snapshots change with it.

mod common;

use common::{Outcome, data_dir, dyno, dyno_in, fixture_build, json};

const LAUNCH_URL: &str = "https://games.example/tiny/index.html";

/// The summary, with its result hash checked against the JSON and then hidden. The hash changes
/// with each scoring change, and the JSON snapshots already show it.
fn summary(outcome: &Outcome) -> String {
    let hash = json(outcome)["result_hash"].as_str().unwrap().to_owned();
    let hash_line = format!("result_hash: {hash}\n");
    assert!(outcome.stderr.contains(&hash_line), "{}", outcome.stderr);
    outcome.stderr.replace(&hash_line, "result_hash: [hash]\n")
}

#[test]
fn a_check_that_passes_says_nothing_warns_or_fails() {
    let outcome = dyno(&["check", &fixture_build("pixi8-webgpu")]);
    insta::assert_snapshot!("pass", summary(&outcome));
}

#[test]
fn a_check_that_warns_gives_the_rule_and_its_fixes() {
    let outcome = dyno(&["check", &fixture_build("pixi7-aa")]);
    insta::assert_snapshot!("warn", summary(&outcome));
}

#[test]
fn a_check_that_fails_gives_the_fails_before_the_warns() {
    let outcome = dyno(&["check", &fixture_build("over-budget")]);
    insta::assert_snapshot!("fail", summary(&outcome));
}

#[test]
fn a_copy_gives_its_download_and_decoded_savings() {
    let outcome = dyno(&["check", &fixture_build("duplicates")]);
    insta::assert_snapshot!("copies", summary(&outcome));
}

#[test]
fn an_incomplete_check_gives_its_first_cause() {
    let outcome = dyno(&["check", "tiny-build"]);
    insta::assert_snapshot!("incomplete", summary(&outcome));
}

#[test]
fn a_check_with_a_budget_and_a_launch_url_counts_the_budgets_and_run_measures() {
    let outcome = dyno(&["check", "tiny-build", LAUNCH_URL, "--budget", "budget.toml"]);
    insta::assert_snapshot!("budget_and_url", summary(&outcome));
}

#[test]
fn a_catalogue_gives_each_game_its_status_and_why() {
    let outcome = dyno(&["check", "--games", "games.toml"]);
    insta::assert_snapshot!("games", outcome.stderr);
}

#[test]
fn the_summary_names_the_out_file() {
    let dir = tempfile::tempdir().unwrap();
    let build = data_dir().join("tiny-build").display().to_string();
    let outcome = dyno_in(dir.path(), &[], &["check", &build, "--out", "result.json"]);
    assert!(
        outcome.stderr.contains("\nfull result: result.json\n"),
        "{}",
        outcome.stderr
    );
}
