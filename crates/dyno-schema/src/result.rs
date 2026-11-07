//! The result of `dyno check` for one target.
//!
//! A result has two parts. `deterministic` holds everything the same build, tool version and
//! budget file give on any machine, and `result_hash` covers it. The one exception is the run
//! coverage: with a launch URL or `--device` it says whether this machine's run gave every
//! required measure, and so does the status it feeds. `run` holds the rest: what differs between
//! machines and runs, and the labels that must not change the hash. That is timestamps,
//! durations, the machine, the tool build, target paths and URLs, release labels, and the
//! browser, device and model values. The hash excludes it.

use std::collections::HashSet;
use std::hash::Hash;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::SCHEMA_VERSION;
use crate::budget::{BudgetRef, BudgetVerdict};
use crate::canonical::{self, tie_breaker};
use crate::ids::{BudgetId, MeasureId, Phase};
use crate::manifest::BuildManifest;
use crate::measure::{Measure, MeasureStatus};
use crate::path::RelPath;
use crate::profile::{ProfileRef, Scenario};
use crate::rule::Finding;
use crate::status::{RunCoverage, Status};

/// The output of `dyno check` for one target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CheckResult {
    /// Read this first. It bumps only on a breaking change.
    pub schema_version: u32,
    /// Lowercase hex SHA-256 of the RFC 8785 canonical JSON of `deterministic`.
    pub result_hash: String,
    pub deterministic: Deterministic,
    pub run: RunInfo,
}

/// Everything the same build, tool version and budget file give on any machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Deterministic {
    pub versions: Versions,
    pub inputs: Inputs,
    /// The build's files and id. Null when the check had no build folder, or the inventory has
    /// not listed it yet.
    pub build: Option<BuildManifest>,
    /// The overall status. The exit code comes from it alone.
    pub status: Status,
    pub run_coverage: RunCoverage,
    /// Static measures. Sorted by id.
    pub measures: Vec<Measure>,
    /// One verdict per budget the budget file sets. Sorted by budget id.
    pub verdicts: Vec<BudgetVerdict>,
    /// Static rule findings. Sorted by rule id, then by canonical JSON.
    pub findings: Vec<Finding>,
}

/// A check result read back from JSON, such as a result file that `compare` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedResult {
    /// The typed result.
    pub result: CheckResult,
    /// Whether `result_hash` matches the `deterministic` JSON as read, before typed parsing.
    pub hash_matches: bool,
}

/// Why a result could not be built. dyno stops with an internal error rather than write a wrong
/// result. A repeated file path can also come from the build, such as a zip that holds two entries
/// with the same name.
#[derive(Debug, thiserror::Error)]
pub enum ResultError {
    #[error("measure {0} appears more than once")]
    DuplicateMeasure(MeasureId),
    #[error("budget {0} has more than one verdict")]
    DuplicateVerdict(BudgetId),
    #[error("build file {0} appears more than once")]
    DuplicateFile(RelPath),
    #[error("the result does not serialize: {0}")]
    Json(#[from] serde_json::Error),
}

/// The versions of everything that shapes a result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Versions {
    /// The schema version. `CheckResult::new` sets it.
    pub schema: u32,
    /// The dyno version, such as "0.1.0".
    pub tool: String,
    /// The version of the budget and rule logic in `dyno-score`.
    pub scoring: u32,
    /// The fitted memory model's version. Null until the phone calibration sets one.
    pub model: Option<String>,
}

/// What the check was asked to do. Paths and URLs are in `run.target`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Inputs {
    /// Whether a build folder or a .zip of one was given.
    pub build: bool,
    /// Whether a launch URL was given.
    pub launch_url: bool,
    pub host_profile: ProfileRef,
    /// Null when no budget file was given.
    pub budget: Option<BudgetRef>,
    pub scenario: Scenario,
    /// Spins asked for with `--spins` (Phase 2). Null when none.
    pub spins: Option<u32>,
    /// Whether `--device` asked for an iPhone run (Phase 2).
    pub device: bool,
}

/// What differs between machines and runs. The hash excludes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RunInfo {
    /// RFC 3339 UTC time the check started.
    pub started_at: String,
    pub duration_ms: u64,
    pub machine: Machine,
    pub tool_build: ToolBuild,
    pub target: Target,
    pub release: Release,
    /// The browser driver. Null when there was no browser run.
    pub driver: Option<DriverInfo>,
    /// Browser, device and model measures. Sorted by id.
    pub measures: Vec<Measure>,
    /// Lab rule findings, such as the host contract checks. Sorted by rule id, then by the
    /// finding's canonical JSON.
    pub findings: Vec<Finding>,
    pub tier: TierReport,
}

/// The machine dyno ran on. No host name or user name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Machine {
    /// "macos", "linux" or "windows".
    pub os: String,
    /// "aarch64" or "x86_64".
    pub arch: String,
    pub cpus: u32,
}

/// The dyno binary that made the result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ToolBuild {
    pub version: String,
    /// The commit the binary was built from. Null for a local build.
    pub git_sha: Option<String>,
    /// "debug" or "release".
    pub profile: String,
}

/// The target as given on the command line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Target {
    /// The build folder or .zip path as given. Null when none.
    pub build: Option<String>,
    /// The launch URL without its user info, query string or fragment, which can carry passwords
    /// and session ids. Null when none.
    pub launch_url: Option<String>,
}

/// Labels the caller gives. Outside the hash, so two callers who name a release differently
/// still get the same hash.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Release {
    pub game: Option<String>,
    pub version: Option<String>,
}

/// The browser driver behind a run: the Rust CDP driver, or a fallback subprocess.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DriverInfo {
    pub name: String,
    pub version: String,
    /// The browser and its version, such as "Chromium 140.0.7339.0".
    pub browser: Option<String>,
}

/// The provisional Reels tier. Report-only: it decides nothing in Phase 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TierReport {
    /// Estimated once the model is calibrated. Unsupported with no launch URL, because the
    /// total needs a JS heap reading.
    pub status: MeasureStatus,
    /// Null unless the status is estimated.
    pub tier: Option<Tier>,
    pub device_class: DeviceClass,
    /// The plan phase that builds the tier.
    pub phase: Phase,
    /// Why there is no tier. Null otherwise.
    pub reason: Option<String>,
}

/// A provisional tier. The worst gate sets it.
///
/// A line is a modelled footprint limit on the reference iPhone. The proposed lines are 500 MB
/// while active and 250 MB while inactive. The phone calibration sets them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Within the line while active, minus the headroom for the worst held-out underprediction.
    Playable,
    /// Over the line, or killed in 2 of 3 fresh runs of the game alone.
    Blocked,
    /// Within the inactive line too. Reserved for Phase 2. Phase 1 never gives it.
    Preloadable,
}

/// The phone class a tier is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    /// The Phase 1 reference: an iPhone with 6 GB of memory.
    ReferenceIphone,
}

impl CheckResult {
    /// Sets the schema version, makes each measure with a bad number invalid, sorts every list,
    /// rejects a repeated measure id, budget id or file path, and computes the hash.
    ///
    /// `Measure::bad_number_reason` defines a bad number. A measure id may appear once in the
    /// whole result, a budget id once in the verdicts and a path once in the manifest.
    pub fn new(
        mut deterministic: Deterministic,
        mut run: RunInfo,
    ) -> Result<CheckResult, ResultError> {
        deterministic.versions.schema = SCHEMA_VERSION;
        for measure in deterministic.measures.iter_mut().chain(&mut run.measures) {
            measure.invalidate_bad_numbers();
        }
        deterministic.sort_lists();
        run.sort_lists();
        check_unique_keys(&deterministic, &run)?;
        let result_hash = canonical::sha256_of_canonical_json(&deterministic)?;
        Ok(CheckResult {
            schema_version: SCHEMA_VERSION,
            result_hash,
            deterministic,
            run,
        })
    }
}

impl LoadedResult {
    /// Checks the hash over the JSON as it is, then reads the JSON as a typed result.
    ///
    /// The typed result drops fields this dyno does not know, so hashing it again would reject a
    /// result from a dyno that added a field. A result with reordered lists was edited, so it
    /// does not match.
    pub fn from_json(json: Value) -> Result<LoadedResult, serde_json::Error> {
        let hash_matches = raw_hash_matches(&json);
        let result = serde_json::from_value(json)?;
        Ok(LoadedResult {
            result,
            hash_matches,
        })
    }
}

/// Whether `result_hash` matches the canonical JSON of `deterministic` as the JSON holds it.
///
/// False when either field is absent, when `result_hash` is not a string, or when `deterministic`
/// holds an integer outside the safe integer range, because dyno never writes one.
pub fn raw_hash_matches(result: &Value) -> bool {
    let (Some(deterministic), Some(Value::String(result_hash))) =
        (result.get("deterministic"), result.get("result_hash"))
    else {
        return false;
    };
    canonical::sha256_of_canonical_json(deterministic).is_ok_and(|hash| hash == *result_hash)
}

impl Deterministic {
    /// Sorts every list by a complete key: measures by id, files by path, verdicts by budget id
    /// and findings by rule id, then by the finding's canonical JSON. With no repeated id or path,
    /// equal keys mean equal items, so the order never depends on the order the lists were built
    /// in.
    pub fn sort_lists(&mut self) {
        if let Some(build) = &mut self.build {
            build.sort_lists();
        }
        sort_measures(&mut self.measures);
        self.verdicts.sort_by_key(|verdict| verdict.budget.as_str());
        sort_findings(&mut self.findings);
    }
}

impl RunInfo {
    /// Sorts every list the same way as the deterministic part.
    pub fn sort_lists(&mut self) {
        sort_measures(&mut self.measures);
        sort_findings(&mut self.findings);
    }
}

fn sort_measures(measures: &mut [Measure]) {
    for measure in measures.iter_mut() {
        measure.sort_lists();
    }
    measures.sort_by_key(|measure| measure.id.as_str());
}

fn sort_findings(findings: &mut [Finding]) {
    for finding in findings.iter_mut() {
        finding.sort_lists();
    }
    findings.sort_by_cached_key(|finding| (finding.rule.as_str(), tie_breaker(finding)));
}

/// Rejects a measure id that appears twice in either part, a budget id with two verdicts, and a
/// file path listed twice.
fn check_unique_keys(deterministic: &Deterministic, run: &RunInfo) -> Result<(), ResultError> {
    let measure_ids = deterministic
        .measures
        .iter()
        .chain(&run.measures)
        .map(|measure| measure.id);
    if let Some(id) = first_repeat(measure_ids) {
        return Err(ResultError::DuplicateMeasure(id));
    }
    let budget_ids = deterministic.verdicts.iter().map(|verdict| verdict.budget);
    if let Some(budget) = first_repeat(budget_ids) {
        return Err(ResultError::DuplicateVerdict(budget));
    }
    let paths = deterministic
        .build
        .iter()
        .flat_map(|build| &build.files)
        .map(|file| &file.path);
    if let Some(path) = first_repeat(paths) {
        return Err(ResultError::DuplicateFile(path.clone()));
    }
    Ok(())
}

/// The first key that appears a second time.
fn first_repeat<K: Copy + Eq + Hash>(keys: impl IntoIterator<Item = K>) -> Option<K> {
    let mut seen = HashSet::new();
    keys.into_iter().find(|&key| !seen.insert(key))
}
