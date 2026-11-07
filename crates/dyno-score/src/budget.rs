//! Budget verdicts: each budget the file sets, checked against its measure. Rules check their own
//! thresholds.

use dyno_schema::budget::{Budget, BudgetLimit, BudgetVerdict, Severity};
use dyno_schema::ids::{BudgetId, CheckedBy};
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue};
use dyno_schema::status::Verdict;

/// One verdict per budget the file sets, in the order of `BudgetId::ALL`.
///
/// A budget whose measure has no usable value is incomplete, never a pass. So is one whose measure
/// is partial, unless no bigger value could give a worse verdict. A rule's threshold gives no
/// verdict: the rule checks it.
pub fn budget_verdicts(budget: &Budget, measures: &[Measure]) -> Vec<BudgetVerdict> {
    BudgetId::ALL
        .iter()
        .filter_map(|&id| {
            let spec = id.spec();
            if spec.checked_by != CheckedBy::Score {
                return None;
            }
            let limit = budget.limit(id)?;
            let measure = measures
                .iter()
                .find(|candidate| candidate.id == spec.measure);
            let actual = measure.and_then(usable_value);
            let mut verdict = verdict_for(limit, actual.as_ref());
            if measure.is_some_and(is_partial) && verdict != worst_verdict(limit) {
                // The value is a floor, and the full value could give a worse verdict.
                verdict = Verdict::Incomplete;
            }
            Some(BudgetVerdict {
                budget: id,
                measure: spec.measure,
                verdict,
                actual,
                limit,
            })
        })
        .collect()
}

/// The value of a measured or estimated measure with no bad number. Others give none.
fn usable_value(measure: &Measure) -> Option<MeasureValue> {
    if measure.bad_number_reason().is_some() {
        return None;
    }
    match measure.status {
        MeasureStatus::Measured | MeasureStatus::Estimated => measure.value.clone(),
        MeasureStatus::Unsupported | MeasureStatus::Missing | MeasureStatus::Invalid => None,
    }
}

fn verdict_for(limit: BudgetLimit, actual: Option<&MeasureValue>) -> Verdict {
    match (limit, actual) {
        (
            BudgetLimit::Bytes {
                warn_above,
                fail_above,
            },
            Some(&MeasureValue::Integer(value)),
        ) => {
            if exceeds(value, fail_above) {
                Verdict::Fail
            } else if exceeds(value, warn_above) {
                Verdict::Warn
            } else {
                Verdict::Pass
            }
        }
        (BudgetLimit::Flag { .. }, Some(&MeasureValue::Flag(false))) => Verdict::Pass,
        (BudgetLimit::Flag { when_on }, Some(&MeasureValue::Flag(true))) => {
            verdict_for_severity(when_on)
        }
        // No value, or a value of the wrong kind for the limit.
        _ => Verdict::Incomplete,
    }
}

/// Whether `value` is above the limit. With no limit, nothing exceeds it, and a negative value is
/// below every limit.
fn exceeds(value: i64, limit: Option<u64>) -> bool {
    let Some(limit) = limit else {
        return false;
    };
    u64::try_from(value).is_ok_and(|bytes| bytes > limit)
}

/// Whether the measure is partial: its scope names something its value leaves out.
///
/// A `floor` uncertainty alone does not make it partial. Every decoded audio size is a floor, and
/// its limits are set for that estimate.
fn is_partial(measure: &Measure) -> bool {
    !measure.scope.not_covered.is_empty()
}

/// The worst verdict any value could get: a fail limit can fail, a warn limit can warn, and a flag
/// that is on gets its severity.
fn worst_verdict(limit: BudgetLimit) -> Verdict {
    match limit {
        BudgetLimit::Bytes {
            warn_above,
            fail_above,
        }
        | BudgetLimit::BasisPoints {
            warn_above,
            fail_above,
        } => {
            if fail_above.is_some() {
                Verdict::Fail
            } else if warn_above.is_some() {
                Verdict::Warn
            } else {
                Verdict::Pass
            }
        }
        BudgetLimit::Flag { when_on } => verdict_for_severity(when_on),
    }
}

/// The verdict a flag that is on gets.
fn verdict_for_severity(severity: Severity) -> Verdict {
    match severity {
        Severity::Allow => Verdict::Pass,
        Severity::Warn => Verdict::Warn,
        Severity::Fail => Verdict::Fail,
    }
}

#[cfg(test)]
mod tests {
    use dyno_schema::budget::BYTES_PER_MB;
    use dyno_schema::config::ConfigFormat;
    use dyno_schema::ids::MeasureId;
    use dyno_schema::measure::UncertaintyKind;

    use super::*;

    const AUDIO_100_TO_200_MB: &str =
        "schema_version = 0\n[audio]\ndecoded_mb = { warn = 100, fail = 200 }\n";

    const AUDIO_WARN_ABOVE_100_MB: &str =
        "schema_version = 0\n[audio]\ndecoded_mb = { warn = 100 }\n";

    const RULE_THRESHOLDS: &str = "schema_version = 0\n\
        [audio]\nstereo_saving_mb = { warn = 4 }\n\
        [files]\nduplicate = \"fail\"\n";

    fn audio_budget() -> Budget {
        Budget::parse(AUDIO_100_TO_200_MB, ConfigFormat::Toml).unwrap()
    }

    fn audio_warn_only_budget() -> Budget {
        Budget::parse(AUDIO_WARN_ABOVE_100_MB, ConfigFormat::Toml).unwrap()
    }

    fn estimated(id: MeasureId, value: MeasureValue) -> Measure {
        Measure {
            value: Some(value),
            ..Measure::blank(id, MeasureStatus::Estimated)
        }
    }

    fn audio(mb: u64, extra_bytes: u64) -> Measure {
        let bytes = mb * BYTES_PER_MB + extra_bytes;
        estimated(
            MeasureId::StaticAudioDecoded48k,
            MeasureValue::Integer(i64::try_from(bytes).unwrap()),
        )
    }

    fn audio_verdict(mb: u64, extra_bytes: u64) -> Verdict {
        only_verdict(&audio_budget(), audio(mb, extra_bytes))
    }

    fn only_verdict(budget: &Budget, measure: Measure) -> Verdict {
        let verdicts = budget_verdicts(budget, &[measure]);
        assert_eq!(verdicts.len(), 1);
        verdicts[0].verdict
    }

    /// The measure made partial, as when a file did not decode.
    fn partial(mut measure: Measure) -> Measure {
        measure.scope.not_covered = vec!["one sound that did not decode".to_owned()];
        measure
    }

    fn antialias_budget(when_on: &str) -> Budget {
        let text = format!("schema_version = 0\n[render]\nantialias = \"{when_on}\"\n");
        Budget::parse(&text, ConfigFormat::Toml).unwrap()
    }

    fn antialias(on: bool) -> Measure {
        estimated(MeasureId::StaticRenderAntialias, MeasureValue::Flag(on))
    }

    #[test]
    fn a_size_passes_up_to_the_warn_limit() {
        assert_eq!(audio_verdict(100, 0), Verdict::Pass);
    }

    #[test]
    fn a_size_above_the_warn_limit_warns() {
        assert_eq!(audio_verdict(100, 1), Verdict::Warn);
        assert_eq!(audio_verdict(200, 0), Verdict::Warn);
    }

    #[test]
    fn a_size_above_the_fail_limit_fails() {
        assert_eq!(audio_verdict(200, 1), Verdict::Fail);
    }

    #[test]
    fn a_partial_size_within_the_limits_is_incomplete() {
        let budget = audio_budget();
        assert_eq!(
            only_verdict(&budget, partial(audio(100, 0))),
            Verdict::Incomplete
        );
        assert_eq!(
            only_verdict(&budget, partial(audio(0, 0))),
            Verdict::Incomplete
        );
    }

    #[test]
    fn a_partial_size_over_the_fail_limit_fails() {
        assert_eq!(
            only_verdict(&audio_budget(), partial(audio(200, 1))),
            Verdict::Fail
        );
    }

    #[test]
    fn a_partial_size_over_only_the_warn_limit_is_incomplete() {
        assert_eq!(
            only_verdict(&audio_budget(), partial(audio(100, 1))),
            Verdict::Incomplete
        );
    }

    #[test]
    fn a_partial_size_over_the_warn_limit_warns_when_there_is_no_fail_limit() {
        assert_eq!(
            only_verdict(&audio_warn_only_budget(), partial(audio(100, 1))),
            Verdict::Warn
        );
    }

    #[test]
    fn a_partial_size_under_a_lone_warn_limit_is_incomplete() {
        assert_eq!(
            only_verdict(&audio_warn_only_budget(), partial(audio(100, 0))),
            Verdict::Incomplete
        );
    }

    #[test]
    fn a_floor_uncertainty_alone_still_gets_a_verdict() {
        let mut floor = audio(100, 0);
        floor.uncertainty.kind = UncertaintyKind::Floor;
        assert_eq!(only_verdict(&audio_budget(), floor), Verdict::Pass);
    }

    #[test]
    fn a_partial_flag_is_incomplete_unless_it_is_on_or_allowed() {
        let cases = [
            ("fail", false, Verdict::Incomplete),
            ("warn", false, Verdict::Incomplete),
            ("warn", true, Verdict::Warn),
            ("fail", true, Verdict::Fail),
            ("allow", false, Verdict::Pass),
            ("allow", true, Verdict::Pass),
        ];
        for (when_on, on, verdict) in cases {
            let budget = antialias_budget(when_on);
            assert_eq!(
                only_verdict(&budget, partial(antialias(on))),
                verdict,
                "{when_on} {on}"
            );
        }
    }

    #[test]
    fn a_missing_measure_makes_the_budget_incomplete() {
        let audio = Measure::missing(MeasureId::StaticAudioDecoded48k, "not built yet");
        let verdicts = budget_verdicts(&audio_budget(), &[audio]);
        assert_eq!(verdicts[0].verdict, Verdict::Incomplete);
        assert_eq!(verdicts[0].actual, None);
        assert_eq!(
            budget_verdicts(&audio_budget(), &[])[0].verdict,
            Verdict::Incomplete
        );
    }

    #[test]
    fn an_invalid_value_is_not_used() {
        let audio = Measure {
            value: Some(MeasureValue::Integer(1)),
            ..Measure::blank(MeasureId::StaticAudioDecoded48k, MeasureStatus::Invalid)
        };
        assert_eq!(
            budget_verdicts(&audio_budget(), &[audio])[0].verdict,
            Verdict::Incomplete
        );
    }

    #[test]
    fn a_value_with_a_bad_number_is_not_used() {
        let negative = estimated(MeasureId::StaticAudioDecoded48k, MeasureValue::Integer(-1));
        let verdicts = budget_verdicts(&audio_budget(), &[negative]);
        assert_eq!(verdicts[0].verdict, Verdict::Incomplete);
        assert_eq!(verdicts[0].actual, None);
    }

    #[test]
    fn a_flag_that_is_on_gives_its_severity() {
        let text =
            "schema_version = 0\n[render]\nantialias = \"fail\"\nresolution_uncapped = \"allow\"\n";
        let budget = Budget::parse(text, ConfigFormat::Toml).unwrap();
        let measures = [
            estimated(MeasureId::StaticRenderAntialias, MeasureValue::Flag(true)),
            estimated(
                MeasureId::StaticRenderResolutionUncapped,
                MeasureValue::Flag(true),
            ),
        ];
        let verdicts: Vec<(BudgetId, Verdict)> = budget_verdicts(&budget, &measures)
            .iter()
            .map(|verdict| (verdict.budget, verdict.verdict))
            .collect();
        assert_eq!(
            verdicts,
            [
                (BudgetId::RenderAntialias, Verdict::Fail),
                (BudgetId::RenderResolutionUncapped, Verdict::Pass),
            ]
        );
    }

    #[test]
    fn a_flag_that_is_off_passes() {
        assert_eq!(
            only_verdict(&antialias_budget("fail"), antialias(false)),
            Verdict::Pass
        );
    }

    #[test]
    fn rule_thresholds_give_no_verdict() {
        let budget = Budget::parse(RULE_THRESHOLDS, ConfigFormat::Toml).unwrap();
        let measures = [
            estimated(MeasureId::StaticAudioStereoFiles, MeasureValue::Integer(9)),
            estimated(MeasureId::StaticBytesDuplicates, MeasureValue::Integer(9)),
        ];
        assert_eq!(budget_verdicts(&budget, &measures), []);
    }

    #[test]
    fn budgets_the_file_does_not_set_give_no_verdict() {
        let audio = estimated(MeasureId::StaticAudioDecoded48k, MeasureValue::Integer(0));
        let budgets: Vec<BudgetId> = budget_verdicts(&audio_budget(), &[audio])
            .iter()
            .map(|verdict| verdict.budget)
            .collect();
        assert_eq!(budgets, [BudgetId::AudioDecodedMb]);
    }
}
