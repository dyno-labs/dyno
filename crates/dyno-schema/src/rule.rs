//! Rule findings and the fixes they suggest.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::canonical::tie_breaker;
use crate::ids::{MeasureId, Phase, RuleId};
use crate::path::RelPath;
use crate::status::Verdict;

/// The outcome of one rule, with fixes that clear it.
///
/// A static finding counts toward the status. A lab finding is in the run part, so it does not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Finding {
    pub rule: RuleId,
    pub verdict: Verdict,
    /// The plan phase that builds this rule: 1 or 2.
    pub phase: Phase,
    /// The measures the rule read. Sorted by id.
    pub measures: Vec<MeasureId>,
    /// Fixes that clear the finding. Sorted by path, then action, then bytes saved, then the
    /// rest of the fix.
    pub fixes: Vec<Fix>,
    /// Why the verdict is incomplete. Null otherwise.
    pub reason: Option<String>,
}

/// One change that clears a finding, and what it saves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Fix {
    pub action: FixAction,
    /// The file to change. Null when the fix is in the bundle's code or config.
    pub path: Option<RelPath>,
    /// Estimated bytes the fix saves, by the same arithmetic as the measure.
    pub saves_bytes: u64,
    /// Decoded bytes the fix also saves, when `saves_bytes` counts download bytes. Removing a
    /// copy saves its file's bytes, and the decoded bytes of an image or sound the game loads
    /// once per copy. Null for other fixes, whose `saves_bytes` are decoded bytes already.
    pub saves_decoded_bytes: Option<u64>,
}

/// What to change. Declared in the alphabetical order of the JSON names, so fixes for the same
/// path sort by name.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum FixAction {
    /// Cap the renderer's resolution below the device pixel ratio.
    CapResolution,
    /// Mix a stereo file down to mono.
    DownmixToMono,
    /// Keep the one copy the game loads of a set of identical files, and delete the others. The
    /// path names the set by its first path, when the build does not show which copy loads.
    KeepOneCopy,
    /// Delete a file that repeats another one.
    RemoveDuplicate,
    /// Play audio through a 24 kHz context, or resample the file to 24 kHz.
    ResampleTo24k,
    /// Use smaller or GPU-compressed textures.
    ShrinkTextures,
    /// Cut the silence out of a sprite map.
    TrimSpriteSilence,
    /// Turn antialias off.
    TurnOffAntialias,
}

impl Finding {
    /// A finding that was not checked, and the reason why. The phase and measures come from the
    /// rule.
    pub fn incomplete(rule: RuleId, reason: &str) -> Finding {
        let spec = rule.spec();
        Finding {
            rule,
            verdict: Verdict::Incomplete,
            phase: spec.phase,
            measures: spec.measures.to_vec(),
            fixes: Vec::new(),
            reason: Some(reason.to_owned()),
        }
    }

    /// Sorts every list in the finding by a complete key. The canonical JSON of a fix breaks a
    /// tie, so a field added to `Fix` is part of the key too.
    pub fn sort_lists(&mut self) {
        self.measures.sort_by_key(|id| id.as_str());
        self.fixes.sort_by_cached_key(|fix| {
            (
                fix.path.clone(),
                fix.action,
                fix.saves_bytes,
                tie_breaker(fix),
            )
        });
    }
}

/// One incomplete finding per rule, each with the same reason.
pub fn incomplete_findings(rules: &[RuleId], reason: &str) -> Vec<Finding> {
    rules
        .iter()
        .map(|&rule| Finding::incomplete(rule, reason))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_actions_are_declared_in_json_name_order() {
        let actions = [
            FixAction::CapResolution,
            FixAction::DownmixToMono,
            FixAction::KeepOneCopy,
            FixAction::RemoveDuplicate,
            FixAction::ResampleTo24k,
            FixAction::ShrinkTextures,
            FixAction::TrimSpriteSilence,
            FixAction::TurnOffAntialias,
        ];
        let names: Vec<String> = actions
            .iter()
            .map(|action| serde_json::to_string(action).unwrap())
            .collect();
        let mut sorted_names = names.clone();
        sorted_names.sort();
        assert_eq!(names, sorted_names);
        assert!(actions.is_sorted());
    }

    #[test]
    fn fixes_sort_by_path_then_action_then_bytes_saved() {
        let fix = |path: Option<&str>, action: FixAction, saves_bytes: u64| Fix {
            action,
            path: path.map(|path| RelPath::new(path).unwrap()),
            saves_bytes,
            saves_decoded_bytes: None,
        };
        let mut finding = Finding::incomplete(RuleId::FilesDuplicate, "test");
        finding.fixes = vec![
            fix(Some("b.png"), FixAction::RemoveDuplicate, 1),
            fix(Some("a.png"), FixAction::RemoveDuplicate, 2),
            fix(Some("a.png"), FixAction::RemoveDuplicate, 1),
            fix(Some("a.png"), FixAction::CapResolution, 9),
            fix(None, FixAction::TurnOffAntialias, 5),
        ];
        finding.sort_lists();
        assert_eq!(
            finding.fixes,
            [
                fix(None, FixAction::TurnOffAntialias, 5),
                fix(Some("a.png"), FixAction::CapResolution, 9),
                fix(Some("a.png"), FixAction::RemoveDuplicate, 1),
                fix(Some("a.png"), FixAction::RemoveDuplicate, 2),
                fix(Some("b.png"), FixAction::RemoveDuplicate, 1),
            ]
        );
    }
}
