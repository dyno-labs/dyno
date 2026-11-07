//! Scoring: budget verdicts, the overall status, run coverage, the memory model, the provisional
//! tier and the compare diff.
//!
//! Required measures, budget verdicts, static rule findings and run coverage set the status.

pub mod budget;
pub mod calibration;
pub mod compare;
pub mod model;
pub mod status;

/// The version of the scoring logic in this crate. It bumps when the same measures and budget
/// file can give a different verdict or status.
pub const SCORING_VERSION: u32 = 1;
