//! The compare diff between two check results.
//!
//! It compares the deterministic parts only: the static measures, the budget verdicts and the
//! build manifests. Run measures differ from run to run, so they never enter a compare.

use std::collections::BTreeMap;

use dyno_schema::TOOL_VERSION;
use dyno_schema::budget::{Budget, BudgetVerdict};
use dyno_schema::compare::{
    AssetChange, AssetChangeKind, CompareDeterministic, MeasureChange, ResultRef, VerdictChange,
};
use dyno_schema::ids::{BudgetId, MeasureId};
use dyno_schema::manifest::ManifestFile;
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue};
use dyno_schema::path::RelPath;
use dyno_schema::result::LoadedResult;
use dyno_schema::status::{Status, Verdict};

use crate::SCORING_VERSION;
use crate::budget::budget_verdicts;
use crate::status::worst;

/// The deterministic part of a compare.
///
/// With a budget file, both sides get its verdicts afresh; without one, each side keeps its own.
/// The status is the worst of: fail when a budget verdict got worse, incomplete when either side
/// is incomplete, and invalid when either side's hash does not match its content.
pub fn diff(
    base: &LoadedResult,
    head: &LoadedResult,
    budget: Option<&Budget>,
) -> Result<CompareDeterministic, serde_json::Error> {
    let base_ref = result_ref(base);
    let head_ref = result_ref(head);
    let base_verdicts = verdicts(base, budget);
    let head_verdicts = verdicts(head, budget);
    let verdict_changes = verdict_changes(&base_verdicts, &head_verdicts);
    let got_worse = verdict_changes
        .iter()
        .any(|change| rank(change.head) > rank(change.base));
    let edited: Vec<&str> = [("base", &base_ref), ("head", &head_ref)]
        .into_iter()
        .filter(|(_, side)| !side.hash_matches)
        .map(|(name, _)| name)
        .collect();
    let incomplete: Vec<&str> = [("base", &base_ref), ("head", &head_ref)]
        .into_iter()
        .filter(|(_, side)| side.status == Status::Incomplete)
        .map(|(name, _)| name)
        .collect();
    let status = worst([
        if got_worse {
            Status::Fail
        } else {
            Status::Pass
        },
        if incomplete.is_empty() {
            Status::Pass
        } else {
            Status::Incomplete
        },
        if edited.is_empty() {
            Status::Pass
        } else {
            Status::Invalid
        },
    ]);
    let reason = match status {
        Status::Invalid => Some(format!(
            "result_hash does not match its content in: {}",
            edited.join(", ")
        )),
        Status::Incomplete => Some(format!("incomplete: {}", incomplete.join(", "))),
        _ => None,
    };
    Ok(CompareDeterministic {
        tool: TOOL_VERSION.to_owned(),
        scoring: SCORING_VERSION,
        budget: budget.map(Budget::reference).transpose()?,
        base: base_ref,
        head: head_ref,
        status,
        reason,
        measures: measure_changes(
            &base.result.deterministic.measures,
            &head.result.deterministic.measures,
        ),
        verdicts: verdict_changes,
        suspect_assets: asset_changes(base, head),
    })
}

fn result_ref(loaded: &LoadedResult) -> ResultRef {
    let result = &loaded.result;
    ResultRef {
        result_hash: result.result_hash.clone(),
        hash_matches: loaded.hash_matches,
        build_id: result
            .deterministic
            .build
            .as_ref()
            .and_then(|build| build.build_id.clone()),
        status: result.deterministic.status,
        tool: result.deterministic.versions.tool.clone(),
    }
}

/// A side's verdict per budget, keyed by the budget's id: the budget file's afresh, else the
/// side's own.
fn verdicts(
    side: &LoadedResult,
    budget: Option<&Budget>,
) -> BTreeMap<&'static str, (BudgetId, Verdict)> {
    let deterministic = &side.result.deterministic;
    let verdicts: Vec<BudgetVerdict> = match budget {
        Some(budget) => budget_verdicts(budget, &deterministic.measures),
        None => deterministic.verdicts.clone(),
    };
    verdicts
        .into_iter()
        .map(|verdict| (verdict.budget.as_str(), (verdict.budget, verdict.verdict)))
        .collect()
}

/// Every budget whose verdict differs, including a budget only one side has.
fn verdict_changes(
    base: &BTreeMap<&'static str, (BudgetId, Verdict)>,
    head: &BTreeMap<&'static str, (BudgetId, Verdict)>,
) -> Vec<VerdictChange> {
    let mut budgets: Vec<BudgetId> = base
        .values()
        .chain(head.values())
        .map(|(budget, _)| *budget)
        .collect();
    budgets.sort_by_key(|budget| budget.as_str());
    budgets.dedup();
    budgets
        .into_iter()
        .filter_map(|budget| {
            let verdict = |side: &BTreeMap<&str, (BudgetId, Verdict)>| {
                side.get(budget.as_str()).map(|(_, verdict)| *verdict)
            };
            let (base, head) = (verdict(base), verdict(head));
            (base != head).then_some(VerdictChange { budget, base, head })
        })
        .collect()
}

/// How bad a verdict is, for "got worse". No verdict and an incomplete one rank as a pass: they
/// say nothing worse about the build.
fn rank(verdict: Option<Verdict>) -> u8 {
    match verdict {
        Some(Verdict::Warn) => 1,
        Some(Verdict::Fail) => 2,
        Some(Verdict::Pass | Verdict::Incomplete) | None => 0,
    }
}

/// Every measure whose status or value differs, including a measure only one side has, whose
/// other side counts as missing.
fn measure_changes(base: &[Measure], head: &[Measure]) -> Vec<MeasureChange> {
    let mut ids: Vec<MeasureId> = base.iter().chain(head).map(|measure| measure.id).collect();
    ids.sort_by_key(|id| id.as_str());
    ids.dedup();
    let find = |side: &[Measure], id| side.iter().find(|measure| measure.id == id).cloned();
    ids.into_iter()
        .filter_map(|id| {
            let base = find(base, id);
            let head = find(head, id);
            let status = |side: &Option<Measure>| side.as_ref().map(|measure| measure.status);
            let value =
                |side: &Option<Measure>| side.as_ref().and_then(|measure| measure.value.clone());
            if status(&base) == status(&head) && value(&base) == value(&head) {
                return None;
            }
            let unit = base.as_ref().or(head.as_ref())?.unit;
            let delta = match (value(&base), value(&head)) {
                (Some(MeasureValue::Integer(before)), Some(MeasureValue::Integer(after))) => {
                    after.checked_sub(before)
                }
                _ => None,
            };
            Some(MeasureChange {
                id,
                unit,
                base_status: status(&base).unwrap_or(MeasureStatus::Missing),
                head_status: status(&head).unwrap_or(MeasureStatus::Missing),
                base: value(&base),
                head: value(&head),
                delta,
            })
        })
        .collect()
}

/// Files added, removed, or changed in their SHA-256, between the two build manifests.
fn asset_changes(base: &LoadedResult, head: &LoadedResult) -> Vec<AssetChange> {
    let files = |side: &LoadedResult| -> BTreeMap<RelPath, ManifestFile> {
        side.result
            .deterministic
            .build
            .iter()
            .flat_map(|build| build.files.iter())
            .map(|file| (file.path.clone(), file.clone()))
            .collect()
    };
    let (base_files, head_files) = (files(base), files(head));
    let mut paths: Vec<&RelPath> = base_files.keys().chain(head_files.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let (before, after) = (base_files.get(path), head_files.get(path));
            let change = match (before, after) {
                (None, Some(_)) => AssetChangeKind::Added,
                (Some(_), None) => AssetChangeKind::Removed,
                (Some(before), Some(after)) if before.sha256 != after.sha256 => {
                    AssetChangeKind::Changed
                }
                _ => return None,
            };
            Some(AssetChange {
                path: path.clone(),
                change,
                base_bytes: before.map(|file| file.bytes),
                head_bytes: after.map(|file| file.bytes),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warn_ranks_above_a_pass_and_below_a_fail() {
        assert!(rank(Some(Verdict::Warn)) > rank(Some(Verdict::Pass)));
        assert!(rank(Some(Verdict::Fail)) > rank(Some(Verdict::Warn)));
    }

    #[test]
    fn no_verdict_and_an_incomplete_one_rank_as_a_pass() {
        assert_eq!(rank(None), rank(Some(Verdict::Pass)));
        assert_eq!(rank(Some(Verdict::Incomplete)), rank(Some(Verdict::Pass)));
    }
}
