//! The overall status and run coverage.
//!
//! When several statuses apply, the worst wins. From worst to best: fail, invalid, incomplete,
//! warn, pass.
//!
//! The check's inputs decide which measures a result must have. A row cannot make itself required
//! or optional. A required measure with no row, or whose row is missing or invalid, never passes.
//!
//! A static finding counts like a budget verdict, because the rules use the default limits when
//! the budget file does not set them. An incomplete finding counts only when a value that dyno
//! should read from the build is missing. A value that needs a browser or device run does not
//! make a static check incomplete.

use std::collections::HashSet;

use dyno_schema::budget::BudgetVerdict;
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{Covers, Measure, MeasureStatus, NOT_BUILT_YET, Source};
use dyno_schema::result::Inputs;
use dyno_schema::rule::Finding;
use dyno_schema::status::{RunCoverage, Status, Verdict};

/// What a check's inputs make of a measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expectation {
    /// The result must not have it.
    Unexpected,
    /// The result may have it, and its status does not count.
    Optional,
    /// The result must have it, and its status counts.
    Required,
}

/// The part of a result that holds a measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// Static measures.
    Deterministic,
    /// Every measure that is not static: lab, play, device and model measures.
    Run,
}

/// The overall status of a check, from the deterministic part alone.
///
/// A required static measure with no row, or a missing one, gives incomplete. An invalid one
/// gives invalid, and so does any row that does not fit the inputs. An unsupported measure gives
/// nothing: dyno cannot measure it for this input, and says why. Each budget verdict and static
/// finding gives the status that `status_of_finding` names.
pub fn overall_status(
    inputs: &Inputs,
    measures: &[Measure],
    verdicts: &[BudgetVerdict],
    findings: &[Finding],
    run_coverage: RunCoverage,
) -> Status {
    let from_measures = status_of_part(Part::Deterministic, inputs, measures);
    let from_coverage = status_of_coverage(run_coverage);
    let from_verdicts = verdicts
        .iter()
        .map(|verdict| status_of_verdict(verdict.verdict));
    let from_findings = findings
        .iter()
        .map(|finding| status_of_finding(finding, measures));
    worst(
        [from_measures, from_coverage]
            .into_iter()
            .chain(from_verdicts)
            .chain(from_findings),
    )
}

/// The status a static finding gives, with `measures` the check's static measures.
///
/// Warn and fail give the same status. Incomplete gives incomplete, except for a rule that is not
/// built yet, and for a rule that reads an unsupported measure: that value needs a browser or
/// device run, so it cannot make a static check incomplete.
pub fn status_of_finding(finding: &Finding, measures: &[Measure]) -> Status {
    let is_unbuilt = finding.reason.as_deref() == Some(NOT_BUILT_YET);
    let waits_for_a_run = reads_an_unsupported_measure(finding, measures);
    match finding.verdict {
        Verdict::Incomplete if is_unbuilt || waits_for_a_run => Status::Pass,
        verdict => status_of_verdict(verdict),
    }
}

/// Whether the browser and device runs gave every required measure. Invalid wins over missing.
///
/// Not requested when the check has no launch URL and no `--device`, unless a row is invalid.
pub fn run_coverage(inputs: &Inputs, run_measures: &[Measure]) -> RunCoverage {
    // The CLI asks for a launch URL with `--device`, but this does not rely on it.
    let requested = inputs.launch_url || inputs.device;
    match status_of_part(Part::Run, inputs, run_measures) {
        Status::Invalid => RunCoverage::Invalid,
        Status::Incomplete => RunCoverage::Missing,
        _ if requested => RunCoverage::Complete,
        _ => RunCoverage::NotRequested,
    }
}

/// The worst of several statuses. Pass when there are none.
pub fn worst(statuses: impl IntoIterator<Item = Status>) -> Status {
    statuses
        .into_iter()
        .max_by_key(|&status| severity(status))
        .unwrap_or(Status::Pass)
}

/// Pass, incomplete or invalid, from one part's rows.
///
/// Invalid when an id repeats or a row does not fit the inputs. Otherwise the worst of the
/// required measures, where one with no row is missing.
fn status_of_part(part: Part, inputs: &Inputs, rows: &[Measure]) -> Status {
    if has_a_repeated_id(rows) || rows.iter().any(|row| !fits_the_check(row, part, inputs)) {
        return Status::Invalid;
    }
    let required_ids = MeasureId::ALL
        .iter()
        .filter(|&&id| part_of(id) == part && expectation(id, inputs) == Expectation::Required);
    worst(required_ids.map(|&id| status_of_required(id, rows)))
}

/// Whether two rows share an id. `CheckResult::new` also rejects a repeated id, but the status
/// does not rely on it.
fn has_a_repeated_id(rows: &[Measure]) -> bool {
    let mut seen = HashSet::new();
    rows.iter().any(|row| !seen.insert(row.id))
}

/// Whether a row belongs in this part, is expected, and agrees with its id's spec, the inputs and
/// its own value.
fn fits_the_check(row: &Measure, part: Part, inputs: &Inputs) -> bool {
    let spec = row.id.spec();
    let expected = expectation(row.id, inputs);
    part_of(row.id) == part
        && expected != Expectation::Unexpected
        && row.required == (expected == Expectation::Required)
        && (row.source, row.phase, row.unit, row.scope.covers)
            == (spec.source, spec.phase, spec.unit, spec.covers)
        && status_agrees_with_value(row)
}

/// Every check has the static measures, and a build makes the phase 1 ones required. A launch URL
/// brings the lab measures: phase 1 ones are required, and play ones are when `--spins` asks.
/// `--device` brings the device measures, all required. Model measures are never required, and
/// field measures are never expected.
fn expectation(id: MeasureId, inputs: &Inputs) -> Expectation {
    let spec = id.spec();
    match spec.source {
        Source::Static => required_if(inputs.build && spec.phase == 1),
        Source::Lab if !inputs.launch_url => Expectation::Unexpected,
        Source::Lab if spec.covers == Covers::DemoSpins => required_if(inputs.spins.is_some()),
        Source::Lab => required_if(spec.phase == 1),
        Source::Model => Expectation::Optional,
        Source::Device if inputs.device => Expectation::Required,
        Source::Device | Source::Field => Expectation::Unexpected,
    }
}

fn required_if(required: bool) -> Expectation {
    if required {
        Expectation::Required
    } else {
        Expectation::Optional
    }
}

fn part_of(id: MeasureId) -> Part {
    if id.spec().source == Source::Static {
        Part::Deterministic
    } else {
        Part::Run
    }
}

/// Measured and estimated measures have a value, and missing and unsupported ones do not. An
/// invalid measure may keep the value that failed its check.
fn status_agrees_with_value(row: &Measure) -> bool {
    match row.status {
        MeasureStatus::Measured | MeasureStatus::Estimated => row.value.is_some(),
        MeasureStatus::Missing | MeasureStatus::Unsupported => row.value.is_none(),
        MeasureStatus::Invalid => true,
    }
}

/// A required measure with no row is missing.
fn status_of_required(id: MeasureId, rows: &[Measure]) -> Status {
    match rows.iter().find(|row| row.id == id) {
        Some(row) => status_of_measure(row),
        None => Status::Incomplete,
    }
}

/// A measure with a bad number counts as invalid, as `CheckResult::new` will make it.
fn status_of_measure(measure: &Measure) -> Status {
    if measure.bad_number_reason().is_some() {
        return Status::Invalid;
    }
    match measure.status {
        MeasureStatus::Measured | MeasureStatus::Estimated | MeasureStatus::Unsupported => {
            Status::Pass
        }
        MeasureStatus::Missing => Status::Incomplete,
        MeasureStatus::Invalid => Status::Invalid,
    }
}

fn reads_an_unsupported_measure(finding: &Finding, measures: &[Measure]) -> bool {
    measures.iter().any(|measure| {
        measure.status == MeasureStatus::Unsupported && finding.measures.contains(&measure.id)
    })
}

fn status_of_verdict(verdict: Verdict) -> Status {
    match verdict {
        Verdict::Pass => Status::Pass,
        Verdict::Warn => Status::Warn,
        Verdict::Fail => Status::Fail,
        Verdict::Incomplete => Status::Incomplete,
    }
}

fn status_of_coverage(coverage: RunCoverage) -> Status {
    match coverage {
        RunCoverage::NotRequested | RunCoverage::Complete => Status::Pass,
        RunCoverage::Missing => Status::Incomplete,
        RunCoverage::Invalid => Status::Invalid,
    }
}

/// The precedence of a status: the higher, the worse.
fn severity(status: Status) -> u8 {
    match status {
        Status::Pass => 0,
        Status::Warn => 1,
        Status::Incomplete => 2,
        Status::Invalid => 3,
        Status::Fail => 4,
    }
}

#[cfg(test)]
mod tests {
    use dyno_schema::budget::BudgetLimit;
    use dyno_schema::ids::{BudgetId, RuleId};
    use dyno_schema::measure::{MeasureValue, Unit};
    use dyno_schema::profile::{HostProfile, Scenario};

    use super::*;

    const FILES: MeasureId = MeasureId::StaticInventoryFiles;
    const SOURCE_MAP: MeasureId = MeasureId::StaticBundleSourceMap;
    const HEAP: MeasureId = MeasureId::LabJsHeap;
    const PEAK: MeasureId = MeasureId::PlayPeak;
    const MODEL_FOOTPRINT: MeasureId = MeasureId::ModelFootprint;
    const DEVICE_FOOTPRINT: MeasureId = MeasureId::DevicePhysFootprint;
    /// The measure that the stereo rule reads.
    const STEREO_FILES: MeasureId = MeasureId::StaticAudioStereoFiles;
    const SPINS: u32 = 100;

    /// Why a stereo finding is incomplete: a gap in what dyno read from the build.
    const A_SOUND_DID_NOT_DECODE: &str = "no estimate for 1 file: a.ogg";

    /// A change to one field of a row.
    type RowEdit = fn(&mut Measure);

    fn nothing_asked() -> Inputs {
        Inputs {
            build: false,
            launch_url: false,
            host_profile: HostProfile::reels().reference(),
            budget: None,
            scenario: Scenario::Alone,
            spins: None,
            device: false,
        }
    }

    fn a_build() -> Inputs {
        Inputs {
            build: true,
            ..nothing_asked()
        }
    }

    fn a_launch_url() -> Inputs {
        Inputs {
            launch_url: true,
            ..nothing_asked()
        }
    }

    fn a_device() -> Inputs {
        Inputs {
            device: true,
            ..nothing_asked()
        }
    }

    fn a_launch_url_with_spins() -> Inputs {
        Inputs {
            spins: Some(SPINS),
            ..a_launch_url()
        }
    }

    /// A row that fits the check: estimated with a value when required, and unsupported when
    /// optional, which does not count.
    fn good_row(id: MeasureId, required: bool) -> Measure {
        let (status, value) = if required {
            (MeasureStatus::Estimated, Some(MeasureValue::Integer(1)))
        } else {
            (MeasureStatus::Unsupported, None)
        };
        Measure {
            required,
            value,
            ..Measure::blank(id, status)
        }
    }

    /// One good row for every measure the inputs expect in a part.
    fn good_rows(part: Part, inputs: &Inputs) -> Vec<Measure> {
        MeasureId::ALL
            .iter()
            .filter(|&&id| part_of(id) == part)
            .filter_map(|&id| match expectation(id, inputs) {
                Expectation::Unexpected => None,
                Expectation::Optional => Some(good_row(id, false)),
                Expectation::Required => Some(good_row(id, true)),
            })
            .collect()
    }

    fn edited(
        mut rows: Vec<Measure>,
        id: MeasureId,
        edit: impl FnOnce(&mut Measure),
    ) -> Vec<Measure> {
        let row = rows
            .iter_mut()
            .find(|row| row.id == id)
            .expect("a row for the id");
        edit(row);
        rows
    }

    fn without(rows: Vec<Measure>, id: MeasureId) -> Vec<Measure> {
        rows.into_iter().filter(|row| row.id != id).collect()
    }

    /// The status of a build-only check from its static rows.
    fn static_status(rows: &[Measure]) -> Status {
        overall_status(&a_build(), rows, &[], &[], RunCoverage::NotRequested)
    }

    /// The good static rows of a check with only a build.
    fn static_rows() -> Vec<Measure> {
        good_rows(Part::Deterministic, &a_build())
    }

    fn run_rows(inputs: &Inputs) -> Vec<Measure> {
        good_rows(Part::Run, inputs)
    }

    fn verdict(verdict: Verdict) -> BudgetVerdict {
        BudgetVerdict {
            budget: BudgetId::AudioDecodedMb,
            measure: MeasureId::StaticAudioDecoded48k,
            verdict,
            actual: None,
            limit: BudgetLimit::Bytes {
                warn_above: None,
                fail_above: Some(1),
            },
        }
    }

    /// The status of a build-only check with these static rows and one finding.
    fn status_with_finding(rows: &[Measure], finding: Finding) -> Status {
        overall_status(&a_build(), rows, &[], &[finding], RunCoverage::NotRequested)
    }

    /// A finding of the stereo rule. An incomplete one says that a sound did not decode.
    fn stereo_finding(verdict: Verdict) -> Finding {
        let incomplete = Finding::incomplete(RuleId::AudioStereo, A_SOUND_DID_NOT_DECODE);
        match verdict {
            Verdict::Incomplete => incomplete,
            _ => Finding {
                verdict,
                reason: None,
                ..incomplete
            },
        }
    }

    fn make_unsupported(row: &mut Measure) {
        row.status = MeasureStatus::Unsupported;
        row.value = None;
    }

    #[test]
    fn worst_follows_the_documented_order() {
        let order = [
            Status::Pass,
            Status::Warn,
            Status::Incomplete,
            Status::Invalid,
            Status::Fail,
        ];
        for (index, &better) in order.iter().enumerate() {
            for &worse in &order[index..] {
                assert_eq!(worst([better, worse]), worse);
                assert_eq!(worst([worse, better]), worse);
            }
        }
        assert_eq!(worst([]), Status::Pass);
    }

    #[test]
    fn the_inputs_decide_which_measures_are_required() {
        use Expectation::{Optional, Required, Unexpected};
        let ids = [
            FILES,
            SOURCE_MAP,
            HEAP,
            PEAK,
            MODEL_FOOTPRINT,
            DEVICE_FOOTPRINT,
        ];
        let cases = [
            (
                nothing_asked(),
                [
                    Optional, Optional, Unexpected, Unexpected, Optional, Unexpected,
                ],
            ),
            (
                a_build(),
                [
                    Required, Optional, Unexpected, Unexpected, Optional, Unexpected,
                ],
            ),
            (
                a_launch_url(),
                [Optional, Optional, Required, Optional, Optional, Unexpected],
            ),
            (
                a_launch_url_with_spins(),
                [Optional, Optional, Required, Required, Optional, Unexpected],
            ),
            (
                a_device(),
                [
                    Optional, Optional, Unexpected, Unexpected, Optional, Required,
                ],
            ),
        ];
        for (inputs, expected) in cases {
            let actual = ids.map(|id| expectation(id, &inputs));
            assert_eq!(actual, expected, "{inputs:?}");
        }
    }

    #[test]
    fn every_required_measure_present_and_good_passes() {
        let everything = Inputs {
            build: true,
            device: true,
            ..a_launch_url_with_spins()
        };
        let coverage = run_coverage(&everything, &run_rows(&everything));
        assert_eq!(coverage, RunCoverage::Complete);
        let rows = good_rows(Part::Deterministic, &everything);
        assert_eq!(
            overall_status(&everything, &rows, &[], &[], coverage),
            Status::Pass
        );
    }

    #[test]
    fn a_device_alone_asks_for_a_run() {
        assert_eq!(
            run_coverage(&a_device(), &run_rows(&a_device())),
            RunCoverage::Complete
        );
    }

    #[test]
    fn a_build_with_no_rows_is_incomplete() {
        assert_eq!(static_status(&[]), Status::Incomplete);
    }

    #[test]
    fn a_required_measure_with_no_row_is_incomplete() {
        assert_eq!(
            static_status(&without(static_rows(), FILES)),
            Status::Incomplete
        );
        let lab = without(run_rows(&a_launch_url()), HEAP);
        assert_eq!(run_coverage(&a_launch_url(), &lab), RunCoverage::Missing);
    }

    #[test]
    fn an_optional_measure_with_no_row_does_not_count() {
        assert_eq!(
            static_status(&without(static_rows(), SOURCE_MAP)),
            Status::Pass
        );
    }

    #[test]
    fn a_missing_required_measure_is_incomplete() {
        let rows = edited(static_rows(), FILES, |row| {
            row.status = MeasureStatus::Missing;
            row.value = None;
        });
        assert_eq!(static_status(&rows), Status::Incomplete);
    }

    #[test]
    fn an_invalid_required_measure_is_invalid() {
        let rows = edited(static_rows(), FILES, |row| {
            row.status = MeasureStatus::Invalid;
        });
        assert_eq!(static_status(&rows), Status::Invalid);
    }

    #[test]
    fn a_required_measure_with_a_bad_number_is_invalid() {
        let negative = |row: &mut Measure| row.value = Some(MeasureValue::Integer(-1));
        assert_eq!(
            static_status(&edited(static_rows(), FILES, negative)),
            Status::Invalid
        );
        let lab = edited(run_rows(&a_launch_url()), HEAP, negative);
        assert_eq!(run_coverage(&a_launch_url(), &lab), RunCoverage::Invalid);
    }

    #[test]
    fn an_optional_measure_that_is_missing_or_invalid_does_not_count() {
        for status in [MeasureStatus::Missing, MeasureStatus::Invalid] {
            let rows = edited(static_rows(), SOURCE_MAP, |row| row.status = status);
            assert_eq!(static_status(&rows), Status::Pass, "{status:?}");
        }
    }

    #[test]
    fn an_unsupported_required_measure_does_not_block_a_pass() {
        let rows = edited(static_rows(), FILES, |row| {
            row.status = MeasureStatus::Unsupported;
            row.value = None;
        });
        assert_eq!(static_status(&rows), Status::Pass);
    }

    #[test]
    fn a_repeated_measure_is_invalid() {
        let mut rows = static_rows();
        rows.push(good_row(SOURCE_MAP, false));
        assert_eq!(static_status(&rows), Status::Invalid);
    }

    #[test]
    fn a_status_that_disagrees_with_its_value_is_invalid() {
        let one = Some(MeasureValue::Integer(1));
        let cases = [
            (MeasureStatus::Measured, None),
            (MeasureStatus::Estimated, None),
            (MeasureStatus::Missing, one.clone()),
            (MeasureStatus::Unsupported, one),
        ];
        for (status, value) in cases {
            for id in [FILES, SOURCE_MAP] {
                let rows = edited(static_rows(), id, |row| {
                    row.status = status;
                    row.value = value.clone();
                });
                assert_eq!(static_status(&rows), Status::Invalid, "{id} {status:?}");
            }
        }
    }

    #[test]
    fn an_invalid_measure_may_keep_its_value() {
        let rows = edited(static_rows(), SOURCE_MAP, |row| {
            row.status = MeasureStatus::Invalid;
            row.value = Some(MeasureValue::Integer(1));
        });
        assert_eq!(static_status(&rows), Status::Pass);
    }

    #[test]
    fn a_required_flag_that_disagrees_with_the_inputs_is_invalid() {
        for id in [FILES, SOURCE_MAP] {
            let rows = edited(static_rows(), id, |row| row.required = !row.required);
            assert_eq!(static_status(&rows), Status::Invalid, "{id}");
        }
        let play = edited(run_rows(&a_launch_url()), PEAK, |row| row.required = true);
        assert_eq!(run_coverage(&a_launch_url(), &play), RunCoverage::Invalid);
        let model = edited(run_rows(&a_launch_url()), MODEL_FOOTPRINT, |row| {
            row.required = true;
        });
        assert_eq!(run_coverage(&a_launch_url(), &model), RunCoverage::Invalid);
    }

    #[test]
    fn a_row_that_disagrees_with_its_id_is_invalid() {
        let edits: [(&str, RowEdit); 4] = [
            ("source", |row| row.source = Source::Lab),
            ("phase", |row| row.phase = 2),
            ("unit", |row| row.unit = Unit::BytesChange),
            ("covers", |row| row.scope.covers = Covers::DemoSpins),
        ];
        for (field, edit) in edits {
            assert_eq!(
                static_status(&edited(static_rows(), FILES, edit)),
                Status::Invalid,
                "{field}"
            );
        }
    }

    #[test]
    fn a_row_the_inputs_do_not_expect_is_invalid() {
        for required in [true, false] {
            let lab_without_a_launch_url = [good_row(HEAP, required)];
            assert_eq!(
                run_coverage(&a_build(), &lab_without_a_launch_url),
                RunCoverage::Invalid
            );
            let device_without_the_flag = [good_row(DEVICE_FOOTPRINT, required)];
            assert_eq!(
                run_coverage(&a_launch_url(), &device_without_the_flag),
                RunCoverage::Invalid
            );
        }
    }

    #[test]
    fn a_row_in_the_wrong_part_is_invalid() {
        let mut static_with_lab = static_rows();
        static_with_lab.push(good_row(HEAP, true));
        let inputs = Inputs {
            launch_url: true,
            ..a_build()
        };
        assert_eq!(
            overall_status(
                &inputs,
                &static_with_lab,
                &[],
                &[],
                RunCoverage::NotRequested
            ),
            Status::Invalid
        );
        let mut lab_with_static = run_rows(&a_launch_url());
        lab_with_static.push(good_row(SOURCE_MAP, false));
        assert_eq!(
            run_coverage(&a_launch_url(), &lab_with_static),
            RunCoverage::Invalid
        );
    }

    #[test]
    fn budget_verdicts_set_the_status() {
        for (budget, status) in [
            (Verdict::Pass, Status::Pass),
            (Verdict::Warn, Status::Warn),
            (Verdict::Fail, Status::Fail),
            (Verdict::Incomplete, Status::Incomplete),
        ] {
            let verdicts = [verdict(budget)];
            assert_eq!(
                overall_status(
                    &a_build(),
                    &static_rows(),
                    &verdicts,
                    &[],
                    RunCoverage::NotRequested
                ),
                status
            );
        }
    }

    #[test]
    fn a_finding_that_passes_warns_or_fails_sets_the_status() {
        for (verdict, status) in [
            (Verdict::Pass, Status::Pass),
            (Verdict::Warn, Status::Warn),
            (Verdict::Fail, Status::Fail),
        ] {
            assert_eq!(
                status_with_finding(&static_rows(), stereo_finding(verdict)),
                status,
                "{verdict:?}"
            );
        }
    }

    #[test]
    fn an_incomplete_finding_about_the_build_is_incomplete() {
        assert_eq!(
            status_with_finding(&static_rows(), stereo_finding(Verdict::Incomplete)),
            Status::Incomplete
        );
    }

    #[test]
    fn a_rule_that_is_not_built_yet_does_not_count() {
        let unbuilt = Finding::incomplete(RuleId::FilesUnreferenced, NOT_BUILT_YET);
        assert_eq!(status_with_finding(&static_rows(), unbuilt), Status::Pass);
    }

    #[test]
    fn an_incomplete_finding_that_reads_an_unsupported_measure_does_not_count() {
        let rows = edited(static_rows(), STEREO_FILES, make_unsupported);
        assert_eq!(
            status_with_finding(&rows, stereo_finding(Verdict::Incomplete)),
            Status::Pass
        );
    }

    #[test]
    fn an_unsupported_measure_that_the_rule_does_not_read_does_not_excuse_it() {
        let rows = edited(static_rows(), FILES, make_unsupported);
        assert_eq!(
            status_with_finding(&rows, stereo_finding(Verdict::Incomplete)),
            Status::Incomplete
        );
    }

    #[test]
    fn a_failing_finding_counts_even_when_it_reads_an_unsupported_measure() {
        let rows = edited(static_rows(), STEREO_FILES, make_unsupported);
        assert_eq!(
            status_with_finding(&rows, stereo_finding(Verdict::Fail)),
            Status::Fail
        );
    }

    #[test]
    fn a_fail_beats_a_missing_measure() {
        let verdicts = [verdict(Verdict::Fail)];
        assert_eq!(
            overall_status(&a_build(), &[], &verdicts, &[], RunCoverage::Missing),
            Status::Fail
        );
    }

    #[test]
    fn run_coverage_sets_the_status() {
        let inputs = nothing_asked();
        let rows = good_rows(Part::Deterministic, &inputs);
        for (coverage, status) in [
            (RunCoverage::NotRequested, Status::Pass),
            (RunCoverage::Complete, Status::Pass),
            (RunCoverage::Missing, Status::Incomplete),
            (RunCoverage::Invalid, Status::Invalid),
        ] {
            assert_eq!(overall_status(&inputs, &rows, &[], &[], coverage), status);
        }
    }

    #[test]
    fn coverage_is_not_requested_without_a_launch_url_or_device() {
        let model_only = run_rows(&a_build());
        assert_eq!(
            run_coverage(&a_build(), &model_only),
            RunCoverage::NotRequested
        );
    }

    #[test]
    fn a_launch_url_with_no_run_rows_is_missing() {
        assert_eq!(run_coverage(&a_launch_url(), &[]), RunCoverage::Missing);
    }

    #[test]
    fn play_measures_count_only_when_spins_are_asked_for() {
        let missing_peak = |row: &mut Measure| {
            row.status = MeasureStatus::Missing;
            row.value = None;
        };
        let spins_not_asked = edited(run_rows(&a_launch_url()), PEAK, missing_peak);
        assert_eq!(
            run_coverage(&a_launch_url(), &spins_not_asked),
            RunCoverage::Complete
        );
        let spins_asked = edited(run_rows(&a_launch_url_with_spins()), PEAK, missing_peak);
        assert_eq!(
            run_coverage(&a_launch_url_with_spins(), &spins_asked),
            RunCoverage::Missing
        );
    }

    #[test]
    fn device_measures_count_with_the_device_flag() {
        let rows = without(run_rows(&a_device()), DEVICE_FOOTPRINT);
        assert_eq!(run_coverage(&a_device(), &rows), RunCoverage::Missing);
    }

    #[test]
    fn invalid_coverage_wins_over_missing() {
        let rows = edited(
            without(run_rows(&a_launch_url_with_spins()), HEAP),
            PEAK,
            |row| {
                row.status = MeasureStatus::Invalid;
            },
        );
        assert_eq!(
            run_coverage(&a_launch_url_with_spins(), &rows),
            RunCoverage::Invalid
        );
    }
}
