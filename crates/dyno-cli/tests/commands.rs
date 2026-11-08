//! Every command runs and prints a valid result. A stub never passes.

mod common;

use std::fs;
use std::path::Path;

use common::{Outcome, data_dir, dyno, dyno_in, fixture_build, json, without_run_details};
use dyno_schema::canonical::{sha256_of_canonical_json, to_canonical_json};
use dyno_schema::catalogue::CatalogueResult;
use dyno_schema::compare::CompareResult;
use dyno_schema::doctor::DoctorReport;
use dyno_schema::result::raw_hash_matches;
use dyno_schema::status::Verdict;

const PASS: i32 = 0;
const FAIL: i32 = 1;
const INCOMPLETE: i32 = 2;
const INVALID: i32 = 3;
const USAGE: i32 = 64;
const INTERNAL: i32 = 70;

const LAUNCH_URL: &str = "https://games.example/tiny/index.html?session=secret#play";

/// A budget file with a misspelt key.
const BAD_BUDGET: &str = "schema_version = 0\n[audio]\ndecoded_mbb = { fail = 1 }\n";

/// Three games with launch URLs only, so a check needs no build.
const THREE_GAMES: &str = r#"
schema_version = 0

[[game]]
id = "c"
launch_url = "https://games.example/c/"

[[game]]
id = "a"
launch_url = "https://games.example/a/"

[[game]]
id = "b"
launch_url = "https://games.example/b/"
"#;

/// The environment of another machine: its home folder, user, time zone and language.
///
/// Only the hash test can show that a time zone or language changes the deterministic part. The
/// values are also rare, so a test can look for each one in it.
const ANOTHER_MACHINE: [(&str, &str); 5] = [
    ("HOME", "/home/someone-else"),
    ("USER", "someone-else"),
    ("TZ", "Pacific/Chatham"),
    ("LANG", "de_DE.UTF-8"),
    ("LC_ALL", "fr_CH.UTF-8"),
];

fn assert_exit(outcome: &Outcome, code: i32) {
    assert_eq!(
        outcome.code, code,
        "args: {:?}\nstdout: {}\nstderr: {}",
        outcome.args, outcome.stdout, outcome.stderr
    );
}

/// Writes `content` to `name` in `dir` and returns the path.
fn write_file(dir: &Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    path.display().to_string()
}

/// Copies the folder `from` to `to`, with everything in it.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The ids of the games in a catalogue result.
fn game_ids(outcome: &Outcome) -> Vec<String> {
    let catalogue: CatalogueResult = serde_json::from_str(&outcome.stdout).unwrap();
    catalogue.games.into_iter().map(|game| game.id).collect()
}

/// Runs a check into a result file and returns its path.
fn check_into(dir: &Path, name: &str, args: &[&str]) -> String {
    let out = dir.join(name).display().to_string();
    let mut all_args = vec!["check", "--out", out.as_str()];
    all_args.extend(args);
    assert_exit(&dyno(&all_args), INCOMPLETE);
    out
}

#[test]
fn a_stub_static_check_is_incomplete() {
    let outcome = dyno(&["check", "tiny-build"]);
    assert_exit(&outcome, INCOMPLETE);
    assert!(raw_hash_matches(&json(&outcome)));
    without_run_details().bind(|| insta::assert_json_snapshot!("check_static", json(&outcome)));
}

#[test]
fn a_stub_check_with_a_budget_and_a_launch_url_is_incomplete() {
    let outcome = dyno(&[
        "check",
        "tiny-build",
        LAUNCH_URL,
        "--budget",
        "budget.toml",
        "--release",
        "v1",
    ]);
    assert_exit(&outcome, INCOMPLETE);
    assert!(
        !outcome.stdout.contains("secret"),
        "the query string leaked into the result"
    );
    without_run_details()
        .bind(|| insta::assert_json_snapshot!("check_budget_and_url", json(&outcome)));
}

#[test]
fn a_rule_that_warns_makes_a_check_with_no_budget_file_warn() {
    // pixi7-aa turns antialias on and leaves the resolution uncapped.
    let outcome = dyno(&["check", &fixture_build("pixi7-aa")]);
    assert_exit(&outcome, PASS);
    assert_eq!(json(&outcome)["deterministic"]["status"], "warn");
}

#[test]
fn a_rule_that_fails_makes_a_check_with_no_budget_file_fail() {
    // over-budget decodes to more audio and textures than the default fail limits.
    let outcome = dyno(&["check", &fixture_build("over-budget")]);
    assert_exit(&outcome, FAIL);
    assert_eq!(json(&outcome)["deterministic"]["status"], "fail");
}

#[test]
fn a_stub_catalogue_is_incomplete() {
    let outcome = dyno(&["check", "--games", "games.toml"]);
    assert_exit(&outcome, INCOMPLETE);
    let catalogue: CatalogueResult = serde_json::from_str(&outcome.stdout).unwrap();
    assert_eq!(catalogue.games.len(), 2);
    without_run_details().bind(|| insta::assert_json_snapshot!("check_games", json(&outcome)));
}

#[test]
fn a_shard_checks_its_share_of_the_games_in_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let games = write_file(dir.path(), "games.toml", THREE_GAMES);
    let first = dyno(&["check", "--games", &games, "--shard", "1/2"]);
    assert_exit(&first, INCOMPLETE);
    assert_eq!(game_ids(&first), ["a", "c"]);
    let second = dyno(&["check", "--games", &games, "--shard", "2/2"]);
    assert_eq!(game_ids(&second), ["b"]);
}

#[test]
fn an_empty_shard_is_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let games = write_file(dir.path(), "games.toml", THREE_GAMES);
    let outcome = dyno(&["check", "--games", &games, "--shard", "4/4"]);
    assert_exit(&outcome, INCOMPLETE);
    assert!(game_ids(&outcome).is_empty());
}

#[test]
fn a_compare_of_two_incomplete_checks_is_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let base = check_into(dir.path(), "base.json", &["tiny-build"]);
    let head = check_into(dir.path(), "head.json", &["tiny-build", "--release", "v2"]);
    let outcome = dyno(&["compare", &base, &head]);
    assert_exit(&outcome, INCOMPLETE);
    let compare: CompareResult = serde_json::from_str(&outcome.stdout).unwrap();
    assert!(compare.deterministic.base.hash_matches && compare.deterministic.head.hash_matches);
    without_run_details().bind(|| insta::assert_json_snapshot!("compare", json(&outcome)));
}

#[test]
fn a_stub_doctor_reports_its_phase() {
    let outcome = dyno(&["doctor"]);
    assert_exit(&outcome, INCOMPLETE);
    let report: DoctorReport = serde_json::from_str(&outcome.stdout).unwrap();
    assert_eq!(report.phase, 2);
    insta::assert_json_snapshot!("doctor", json(&outcome));
}

/// A check of a fixture build with the test budget, written to `name` in `dir`, whatever its
/// status.
fn fixture_check_into(dir: &Path, name: &str, fixture: &str) -> String {
    let out = dir.join(name).display().to_string();
    let build = fixture_build(fixture);
    dyno(&["check", &build, "--budget", "budget.toml", "--out", &out]);
    out
}

#[test]
fn a_compare_fails_when_a_budget_verdict_gets_worse_and_passes_when_it_gets_better() {
    let dir = tempfile::tempdir().unwrap();
    let small = fixture_check_into(dir.path(), "small.json", "pixi8-webgpu");
    let large = fixture_check_into(dir.path(), "large.json", "over-budget");
    let worse = dyno(&["compare", &small, &large, "--budget", "budget.toml"]);
    assert_exit(&worse, FAIL);
    let compare: CompareResult = serde_json::from_str(&worse.stdout).unwrap();
    let audio = compare
        .deterministic
        .verdicts
        .iter()
        .find(|change| change.budget.as_str() == "audio.decoded_mb")
        .expect("the audio verdict changed");
    assert_eq!(
        (audio.base, audio.head),
        (Some(Verdict::Pass), Some(Verdict::Fail))
    );
    let decoded = compare
        .deterministic
        .measures
        .iter()
        .find(|change| change.id.as_str() == "static.audio.decoded_48k")
        .expect("decoded audio moved");
    assert!(decoded.delta.unwrap() > 0);
    assert!(!compare.deterministic.suspect_assets.is_empty());
    assert!(
        worse
            .stderr
            .contains("budget audio.decoded_mb: pass to fail"),
        "{}",
        worse.stderr
    );
    let better = dyno(&["compare", &large, &small, "--budget", "budget.toml"]);
    assert_exit(&better, PASS);
}

#[test]
fn a_compare_of_a_result_with_itself_has_no_changes() {
    let dir = tempfile::tempdir().unwrap();
    let small = fixture_check_into(dir.path(), "small.json", "pixi8-webgpu");
    let outcome = dyno(&["compare", &small, &small]);
    let compare: CompareResult = serde_json::from_str(&outcome.stdout).unwrap();
    assert!(compare.deterministic.measures.is_empty());
    assert!(compare.deterministic.verdicts.is_empty());
    assert!(compare.deterministic.suspect_assets.is_empty());
}

#[test]
fn compare_of_an_edited_result_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let base = check_into(dir.path(), "base.json", &["tiny-build"]);
    let mut edited =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&base).unwrap()).unwrap();
    edited["deterministic"]["status"] = "pass".into();
    let head = dir.path().join("head.json");
    fs::write(&head, edited.to_string()).unwrap();
    let outcome = dyno(&["compare", &base, &head.display().to_string()]);
    assert_exit(&outcome, INVALID);
    let compare: CompareResult = serde_json::from_str(&outcome.stdout).unwrap();
    assert!(!compare.deterministic.head.hash_matches);
    assert!(compare.deterministic.reason.unwrap().contains("head"));
}

#[test]
fn compare_accepts_a_result_with_a_field_this_dyno_does_not_know() {
    let dir = tempfile::tempdir().unwrap();
    let base = check_into(dir.path(), "base.json", &["tiny-build"]);
    let mut newer: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&base).unwrap()).unwrap();
    newer["deterministic"]["added_later"] = "by a newer dyno".into();
    newer["result_hash"] = sha256_of_canonical_json(&newer["deterministic"])
        .unwrap()
        .into();
    let head = write_file(dir.path(), "head.json", &newer.to_string());
    let outcome = dyno(&["compare", &base, &head]);
    assert_exit(&outcome, INCOMPLETE);
    let compare: CompareResult = serde_json::from_str(&outcome.stdout).unwrap();
    assert!(compare.deterministic.head.hash_matches);
}

#[test]
fn compare_rejects_a_result_from_another_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let base = check_into(dir.path(), "base.json", &["tiny-build"]);
    let future = dir.path().join("future.json");
    fs::write(&future, r#"{"schema_version": 99}"#).unwrap();
    let outcome = dyno(&["compare", &base, &future.display().to_string()]);
    assert_exit(&outcome, USAGE);
    assert!(
        outcome.stderr.contains("schema_version 99"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn compare_rejects_a_file_that_is_not_a_result() {
    let outcome = dyno(&["compare", "budget.toml", "budget.toml"]);
    assert_exit(&outcome, USAGE);
}

#[test]
fn stdout_is_canonical_json_with_a_final_newline() {
    let dir = tempfile::tempdir().unwrap();
    let base = check_into(dir.path(), "base.json", &["tiny-build"]);
    for args in [
        &["check", "tiny-build"][..],
        &["check", "--games", "games.toml"],
        &["compare", &base, &base],
        &["doctor"],
    ] {
        let outcome = dyno(args);
        let canonical = to_canonical_json(&json(&outcome)).unwrap() + "\n";
        assert_eq!(outcome.stdout, canonical, "{args:?}");
    }
}

#[test]
fn out_writes_the_file_and_leaves_stdout_empty() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("result.json");
    let outcome = dyno(&["check", "tiny-build", "--out", &out.display().to_string()]);
    assert_exit(&outcome, INCOMPLETE);
    assert_eq!(outcome.stdout, "");
    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out).unwrap()).unwrap();
    assert!(raw_hash_matches(&written));
}

#[test]
fn the_summary_goes_to_stderr() {
    let outcome = dyno(&["check", "tiny-build"]);
    assert!(
        outcome.stderr.starts_with("dyno check: incomplete"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn the_result_hash_does_not_depend_on_the_run() {
    let first = json(&dyno(&["check", "tiny-build"]));
    let second = json(&dyno(&["check", "tiny-build", "--release", "v9"]));
    assert_eq!(first["result_hash"], second["result_hash"]);
    assert_ne!(first["run"]["release"], second["run"]["release"]);
}

#[test]
fn a_build_only_check_gives_the_same_hash_and_exit_code_on_another_machine() {
    let here = dyno(&["check", "tiny-build", "--budget", "budget.toml"]);
    let machine = tempfile::tempdir().unwrap();
    let folder = machine.path().join("another/folder");
    copy_dir(&data_dir().join("tiny-build"), &folder.join("tiny-build"));
    fs::copy(data_dir().join("budget.toml"), folder.join("budget.toml")).unwrap();
    let build = folder.join("tiny-build").display().to_string();
    let budget = folder.join("budget.toml").display().to_string();
    let args = ["check", build.as_str(), "--budget", budget.as_str()];
    let elsewhere = dyno_in(machine.path(), &ANOTHER_MACHINE, &args);
    assert_eq!(elsewhere.code, here.code);
    assert_eq!(json(&elsewhere)["result_hash"], json(&here)["result_hash"]);
}

#[test]
fn a_build_only_check_keeps_the_machine_out_of_its_deterministic_part() {
    let temp = tempfile::tempdir().unwrap();
    let build_dir = temp.path().join("tiny-build");
    copy_dir(&data_dir().join("tiny-build"), &build_dir);
    let build = build_dir.display().to_string();
    let result = json(&dyno_in(temp.path(), &ANOTHER_MACHINE, &["check", &build]));
    let deterministic = result["deterministic"].to_string();
    let machine = &result["run"]["machine"];
    let os = machine["os"].as_str().unwrap().to_owned();
    let arch = machine["arch"].as_str().unwrap().to_owned();
    let environment = ANOTHER_MACHINE.iter().map(|&(_, value)| value.to_owned());
    let temp_folder = temp.path().display().to_string();
    for text in environment.chain([os, arch, temp_folder]) {
        assert!(
            !deterministic.contains(&text),
            "{text:?} in {deterministic}"
        );
    }
}

#[test]
fn a_check_with_spins_and_a_device_is_incomplete_not_invalid() {
    let outcome = dyno(&["check", LAUNCH_URL, "--spins", "10", "--device"]);
    assert_exit(&outcome, INCOMPLETE);
    assert_eq!(json(&outcome)["deterministic"]["run_coverage"], "missing");
}

#[test]
fn a_driver_other_than_cdp_without_a_launch_url_is_a_usage_error() {
    let outcome = dyno(&["check", "tiny-build", "--driver", "exec:./fake-driver.sh"]);
    assert_exit(&outcome, USAGE);
    assert!(
        outcome.stderr.contains("a --driver other than cdp"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_served_run_needs_a_build_folder_and_no_launch_url() {
    for args in [
        &["check", LAUNCH_URL, "--driver", "serve"][..],
        &["check", "tiny-build", LAUNCH_URL, "--driver", "serve"],
    ] {
        let outcome = dyno(args);
        assert_exit(&outcome, USAGE);
        assert!(
            outcome
                .stderr
                .contains("give a build folder and no launch URL"),
            "{}",
            outcome.stderr
        );
    }
}

#[test]
fn a_served_run_of_a_zip_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let zip = write_file(dir.path(), "build.zip", "PK");
    let outcome = dyno(&["check", &zip, "--driver", "serve", "--serve-port", "0"]);
    assert_exit(&outcome, USAGE);
    assert!(outcome.stderr.contains("unzip"), "{}", outcome.stderr);
}

#[test]
fn serve_flags_without_the_serve_driver_are_a_usage_error() {
    for args in [
        &["check", "tiny-build", "--serve-host", "10.0.0.2"][..],
        &["check", "tiny-build", "--serve-port", "8000"],
        &["check", "tiny-build", "--serve-query", "sessionID=s"],
    ] {
        let outcome = dyno(args);
        assert_exit(&outcome, USAGE);
        assert!(
            outcome.stderr.contains("need --driver serve"),
            "{}",
            outcome.stderr
        );
    }
}

#[cfg(unix)]
#[test]
fn an_exec_driver_runs_the_browser_part_of_a_check() {
    let outcome = dyno(&["check", LAUNCH_URL, "--driver", "exec:./fake-driver.sh"]);
    let result = json(&outcome);
    assert_eq!(result["run"]["driver"]["name"], "fake");
    let first_frame = result["run"]["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|measure| measure["id"] == "lab.time.first_frame")
        .unwrap();
    assert_eq!(first_frame["status"], "measured", "{first_frame}");
    assert_eq!(first_frame["value"], 120);
}

#[test]
fn help_and_version_exit_0() {
    assert_exit(&dyno(&["--help"]), PASS);
    assert_exit(&dyno(&["--version"]), PASS);
}

#[test]
fn a_piped_version_is_one_line_with_no_banner() {
    let outcome = dyno(&["--version"]);
    assert_eq!(
        outcome.stdout,
        format!("dyno {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn usage_and_config_errors_exit_64() {
    let dir = tempfile::tempdir().unwrap();
    let bad_budget = write_file(dir.path(), "bad.toml", BAD_BUDGET);
    let cases: [&[&str]; 14] = [
        &[],
        &["check"],
        &["check", "no-such-build"],
        &["check", "tiny-build", "--budget", &bad_budget],
        &["check", "tiny-build", "--spins", "10"],
        &["check", "tiny-build", "--device"],
        &["check", "tiny-build", "--scenario", "aged"],
        &["check", LAUNCH_URL, "--driver", "playwright"],
        &["check", LAUNCH_URL, "--spins", "0"],
        &["check", "tiny-build", "other-build"],
        &["check", "tiny-build", "--host-profile", "arcade"],
        &["check", "--games", "games.toml", "--shard", "3/2"],
        &["check", "--games", "games.toml", "--spins", "10"],
        &["check", "--shard", "1/2", "tiny-build"],
    ];
    for args in cases {
        let outcome = dyno(args);
        assert_exit(&outcome, USAGE);
        assert_eq!(outcome.stdout, "", "{args:?}");
    }
}

#[test]
fn an_output_file_that_cannot_be_written_exits_70() {
    let outcome = dyno(&["check", "tiny-build", "--out", "/nonexistent/dir/x.json"]);
    assert_exit(&outcome, INTERNAL);
}

#[test]
fn an_unknown_budget_key_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let bad_budget = write_file(dir.path(), "bad.toml", BAD_BUDGET);
    let outcome = dyno(&["check", "tiny-build", "--budget", &bad_budget]);
    assert!(
        outcome.stderr.contains("unknown field `decoded_mbb`"),
        "{}",
        outcome.stderr
    );
}
