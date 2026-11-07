//! The golden result hashes in `parity/golden-hashes.json`: the status and `result_hash` that
//! `dyno check` gives for each fixture build, with no flags.
//!
//! A build-only check gives the same hash on every machine, so anyone who runs another copy of the
//! same release, such as a hosted runner, can check that it gives the same results. When a test
//! fails, review the change, then run `DYNO_UPDATE_GENERATED=1 cargo test --workspace`.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dyno_schema::TOOL_VERSION;
use dyno_schema::generate::UPDATE_GENERATED_VARIABLE;
use serde::{Deserialize, Serialize};

/// The golden file, as written to `parity/golden-hashes.json`.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GoldenHashes {
    /// The dyno version that gave these hashes. The hash covers the version, so each release has
    /// its own file.
    dyno_version: String,
    /// Each fixture's input, relative to the repository root, and what checking it gives.
    checks: BTreeMap<String, GoldenCheck>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GoldenCheck {
    input: String,
    status: String,
    exit_code: i32,
    result_hash: String,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Each fixture's scan input, relative to the repository root: its `build` folder, or its
/// `build.zip`.
fn fixture_inputs() -> BTreeMap<String, String> {
    let mut inputs = BTreeMap::new();
    for entry in fs::read_dir(repo_root().join("fixtures")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for input in ["build", "build.zip"] {
            if path.join(input).exists() {
                inputs.insert(name.clone(), format!("fixtures/{name}/{input}"));
            }
        }
    }
    inputs
}

fn check_fixture(input: &str) -> GoldenCheck {
    let outcome = common::dyno_in(&repo_root(), &[], &["check", input]);
    let result = common::json(&outcome);
    GoldenCheck {
        input: input.to_owned(),
        status: result["deterministic"]["status"]
            .as_str()
            .unwrap()
            .to_owned(),
        exit_code: outcome.code,
        result_hash: result["result_hash"].as_str().unwrap().to_owned(),
    }
}

#[test]
fn the_golden_hashes_match_this_build() {
    let generated = GoldenHashes {
        dyno_version: TOOL_VERSION.to_owned(),
        checks: fixture_inputs()
            .into_iter()
            .map(|(name, input)| (name, check_fixture(&input)))
            .collect(),
    };
    assert!(!generated.checks.is_empty(), "no fixture builds were found");
    let path = repo_root().join("parity/golden-hashes.json");
    let mut written = serde_json::to_string_pretty(&generated).unwrap();
    written.push('\n');
    if std::env::var(UPDATE_GENERATED_VARIABLE).is_ok_and(|value| value == "1") {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, written).unwrap();
        return;
    }
    let committed = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == written,
        "{} is stale. Run {UPDATE_GENERATED_VARIABLE}=1 cargo test --workspace, then read the diff",
        path.display()
    );
}
