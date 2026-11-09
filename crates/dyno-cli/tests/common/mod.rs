//! Helpers that run the `dyno` binary. Most tests run it in `tests/data`, so paths in results are
//! short and the same on every machine.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// What one run of `dyno` gave.
pub struct Outcome {
    /// The arguments it ran with, for assert messages.
    pub args: Vec<String>,
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// The build folder of a static scan fixture, relative to `tests/data`. It is relative, so a
/// result holds the same path on every machine.
pub fn fixture_build(name: &str) -> String {
    format!("../../../../fixtures/{name}/build")
}

/// Runs `dyno` with `args` in `tests/data`.
pub fn dyno(args: &[&str]) -> Outcome {
    dyno_in(&data_dir(), &[], args)
}

/// A Chromium that does not exist, so a check with a launch URL fails the same way on every
/// machine and never starts a browser.
pub const NO_CHROMIUM: &str = "no-chromium-in-tests";

/// Runs `dyno` with `args` in `dir`, with `extra_env` added to its environment.
pub fn dyno_in(dir: &Path, extra_env: &[(&str, &str)], args: &[&str]) -> Outcome {
    let output = Command::new(env!("CARGO_BIN_EXE_dyno"))
        .args(args)
        .current_dir(dir)
        .env("DYNO_CHROMIUM", NO_CHROMIUM)
        .envs(extra_env.iter().copied())
        .output()
        .expect("the dyno binary runs");
    Outcome {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        code: output.status.code().expect("dyno exits with a code"),
        stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
    }
}

/// Copies the folder `from` to `to`, with everything in it.
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Snapshot settings that hide what differs between machines and runs.
pub fn without_run_details() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    for prefix in [".run", ".games[].result.run"] {
        settings.add_redaction(&format!("{prefix}.run_id"), "[run_id]");
        settings.add_redaction(&format!("{prefix}.started_at"), "[started_at]");
        settings.add_redaction(&format!("{prefix}.duration_ms"), "[duration_ms]");
        settings.add_redaction(&format!("{prefix}.machine"), "[machine]");
        settings.add_redaction(&format!("{prefix}.tool_build"), "[tool_build]");
    }
    settings
}

/// Parses stdout as JSON.
pub fn json(outcome: &Outcome) -> serde_json::Value {
    serde_json::from_str(&outcome.stdout).expect("stdout is one JSON value")
}
