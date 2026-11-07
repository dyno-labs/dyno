//! Overall statuses and verdicts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The overall status of a result. The exit code comes from it alone.
///
/// When several statuses apply, the worst wins. From worst to best: fail, invalid, incomplete,
/// warn, pass. The variants below are not declared in that order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// None of the other statuses applies.
    Pass,
    /// A budget or a static rule warns. Nothing fails.
    Warn,
    /// A budget or a static rule fails. Nothing offsets a fail.
    Fail,
    /// A required measure is missing or absent from the result, a budget could not be checked, or
    /// a static rule could not be checked because a value from the build is missing. Never a pass.
    Incomplete,
    /// A required measure failed its check, such as a run whose frame DPR was not 3, or a measure
    /// in the result does not fit the check.
    Invalid,
}

/// The outcome of one budget, rule or doctor check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
    /// It could not be checked, because the inputs it needs are missing, invalid or partial.
    Incomplete,
}

/// Whether the browser and device runs that a check asked for gave every required measure.
///
/// It sits in the deterministic part, so the status and exit code never read the run part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunCoverage {
    /// The check asked for no browser or device run.
    NotRequested,
    /// Every required run measure is present, and every run measure fits the check.
    Complete,
    /// A required run measure is missing or absent from the result.
    Missing,
    /// A required run measure is invalid, or a run measure does not fit the check. Invalid wins
    /// over missing.
    Invalid,
}
