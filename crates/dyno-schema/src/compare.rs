//! The result of `dyno compare`: the diff between two check results.
//!
//! `compare` reads two result files from `dyno check`, never build folders. Like a check result,
//! it has a hashed deterministic part and a run part outside the hash.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::SCHEMA_VERSION;
use crate::budget::BudgetRef;
use crate::canonical::{self, tie_breaker};
use crate::ids::{BudgetId, MeasureId};
use crate::measure::{MeasureStatus, MeasureValue, Unit};
use crate::path::RelPath;
use crate::result::{Machine, Release, ToolBuild};
use crate::status::{Status, Verdict};

/// The output of `dyno compare`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CompareResult {
    pub schema_version: u32,
    /// Lowercase hex SHA-256 of the RFC 8785 canonical JSON of `deterministic`.
    pub result_hash: String,
    pub deterministic: CompareDeterministic,
    pub run: CompareRun,
}

/// Everything the same two results, tool version and budget file give on any machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CompareDeterministic {
    /// The dyno version, such as "0.1.0".
    pub tool: String,
    pub scoring: u32,
    /// The budget file applied to both sides. Null to keep each result's own verdicts.
    pub budget: Option<BudgetRef>,
    pub base: ResultRef,
    pub head: ResultRef,
    /// Fail when a budget verdict got worse. Incomplete when either side is incomplete. Invalid
    /// when either side's hash does not match its content.
    pub status: Status,
    /// Why the status is incomplete or invalid. Null otherwise.
    pub reason: Option<String>,
    /// Every deterministic measure that moved. Sorted by id, then by canonical JSON.
    pub measures: Vec<MeasureChange>,
    /// Every budget verdict that changed. Sorted by budget id, then by canonical JSON.
    pub verdicts: Vec<VerdictChange>,
    /// Files that differ between the two build manifests. Sorted by path, then by canonical
    /// JSON.
    pub suspect_assets: Vec<AssetChange>,
}

/// One side of a comparison.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ResultRef {
    pub result_hash: String,
    /// Whether `result_hash` matches the result's deterministic part.
    pub hash_matches: bool,
    pub build_id: Option<String>,
    pub status: Status,
    /// The dyno version that made the result.
    pub tool: String,
}

/// One deterministic measure that moved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MeasureChange {
    pub id: MeasureId,
    pub unit: Unit,
    pub base_status: MeasureStatus,
    pub head_status: MeasureStatus,
    pub base: Option<MeasureValue>,
    pub head: Option<MeasureValue>,
    /// Head minus base, for integer values on both sides. Null otherwise.
    pub delta: Option<i64>,
}

/// One budget verdict that changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct VerdictChange {
    pub budget: BudgetId,
    /// Null when the side has no verdict for this budget.
    pub base: Option<Verdict>,
    pub head: Option<Verdict>,
}

/// One file that differs between the two build manifests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssetChange {
    pub path: RelPath,
    pub change: AssetChangeKind,
    pub base_bytes: Option<u64>,
    pub head_bytes: Option<u64>,
}

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AssetChangeKind {
    Added,
    Removed,
    /// Same path, different SHA-256.
    Changed,
}

/// What differs between machines and runs. The hash excludes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CompareRun {
    /// RFC 3339 UTC time the compare started.
    pub started_at: String,
    pub duration_ms: u64,
    pub machine: Machine,
    pub tool_build: ToolBuild,
    /// The release labels of each side, from its own run part.
    pub base_release: Release,
    pub head_release: Release,
}

impl CompareResult {
    /// Sorts every list and computes the hash.
    pub fn new(
        mut deterministic: CompareDeterministic,
        run: CompareRun,
    ) -> Result<CompareResult, serde_json::Error> {
        deterministic.sort_lists();
        let result_hash = canonical::sha256_of_canonical_json(&deterministic)?;
        Ok(CompareResult {
            schema_version: SCHEMA_VERSION,
            result_hash,
            deterministic,
            run,
        })
    }
}

impl CompareDeterministic {
    /// Sorts measures by id, verdicts by budget id and suspect assets by path. The canonical JSON
    /// of an item breaks a tie, so the order never depends on the order the lists were built in.
    pub fn sort_lists(&mut self) {
        self.measures
            .sort_by_cached_key(|change| (change.id.as_str(), tie_breaker(change)));
        self.verdicts
            .sort_by_cached_key(|change| (change.budget.as_str(), tie_breaker(change)));
        self.suspect_assets
            .sort_by_cached_key(|change| (change.path.clone(), tie_breaker(change)));
    }
}
