//! The output of `dyno doctor` (Phase 2): whether this Mac can run the iPhone measure.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::Phase;
use crate::status::{Status, Verdict};

/// The output of `dyno doctor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DoctorReport {
    pub schema_version: u32,
    /// The plan phase that builds `doctor`.
    pub phase: Phase,
    /// Pass when every check passes. Incomplete while a check is not built.
    pub status: Status,
    /// Sorted by check id.
    pub checks: Vec<DoctorCheck>,
}

/// One doctor check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DoctorCheck {
    pub id: DoctorCheckId,
    pub verdict: Verdict,
    pub phase: Phase,
    /// What the check found, such as the Xcode version. Null when it found nothing.
    pub detail: Option<String>,
    /// Why the verdict is incomplete. Null otherwise.
    pub reason: Option<String>,
}

/// The things `doctor` checks. Declared in alphabetical order, so checks sort by name.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum DoctorCheckId {
    /// An iPhone is connected.
    DeviceConnection,
    /// The iPhone trusts this Mac, and Developer Mode and Web Inspector are on.
    DeviceTrust,
    /// Xcode and its command line tools, with xctrace and devicectl.
    Xcode,
}

impl DoctorCheckId {
    /// Every check, in declaration order.
    pub const ALL: &[DoctorCheckId] = &[
        DoctorCheckId::DeviceConnection,
        DoctorCheckId::DeviceTrust,
        DoctorCheckId::Xcode,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_ids_are_declared_in_json_name_order() {
        let names: Vec<String> = DoctorCheckId::ALL
            .iter()
            .map(|id| serde_json::to_string(id).unwrap())
            .collect();
        let mut sorted_names = names.clone();
        sorted_names.sort();
        assert_eq!(names, sorted_names);
        assert!(DoctorCheckId::ALL.is_sorted());
    }
}
