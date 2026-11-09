//! `--runs`: N runs of the game and N runs of the empty build on one phone, each with its own
//! recording, read into one set of device measures.
//!
//! Each run gives its own device measures first, as a single run does. Then
//! `device.phys_footprint` is the median of the game runs' readings at idle,
//! `device.phys_footprint_peak` the median of their peaks, `device.kills` the number of them the
//! phone killed, and `device.footprint_over_baseline` the median game peak minus the median peak
//! of the empty build. Each has an item per run. A run with no reading counts: the measure is
//! missing, and the reason names the run and says why, because the median of fewer runs is not
//! the median of N.

use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Uncertainty, UncertaintyKind,
};

use crate::required;

/// Why a check without `--runs` has no footprint over the empty build.
const NEEDS_RUNS: &str = "needs --runs: the median peak of N runs of the game minus the median \
    peak of N runs of the empty build";

/// What a run is a run of, as the items and reasons name it.
#[derive(Clone, Copy)]
enum Of {
    Game,
    EmptyBuild,
}

/// `device.footprint_over_baseline` for a check without `--runs`: unsupported, and not required.
pub fn over_baseline_unsupported() -> Measure {
    Measure {
        reason: Some(NEEDS_RUNS.to_owned()),
        ..Measure::blank(
            MeasureId::DeviceFootprintOverBaseline,
            MeasureStatus::Unsupported,
        )
    }
}

/// The device measures of N runs of the game and N runs of the empty build, from each run's own
/// device measures, in run order. Every one is required.
pub fn device_runs_measures(
    game_runs: &[Vec<Measure>],
    empty_runs: &[Vec<Measure>],
) -> Vec<Measure> {
    let footprints = readings(game_runs, Of::Game, MeasureId::DevicePhysFootprint);
    let peaks = readings(game_runs, Of::Game, MeasureId::DevicePhysFootprintPeak);
    let kills = readings(game_runs, Of::Game, MeasureId::DeviceKills);
    let empty_peaks = readings(
        empty_runs,
        Of::EmptyBuild,
        MeasureId::DevicePhysFootprintPeak,
    );
    let footprint = match &footprints.failed {
        None => {
            let mut measure = median_measure(MeasureId::DevicePhysFootprint, &footprints.done);
            // Readers from before the peak measure find the peak here, as in a single run.
            if peaks.failed.is_none() {
                measure.items.push(MeasureItem {
                    key: crate::PEAK_ITEM.to_owned(),
                    value: MeasureValue::Integer(median(&values(&peaks.done))),
                });
            }
            measure
        }
        Some(reason) => missing(
            MeasureId::DevicePhysFootprint,
            reason,
            &footprints.done,
            run_items(&footprints.done),
        ),
    };
    let footprint_peak = match &peaks.failed {
        None => median_measure(MeasureId::DevicePhysFootprintPeak, &peaks.done),
        Some(reason) => missing(
            MeasureId::DevicePhysFootprintPeak,
            reason,
            &peaks.done,
            run_items(&peaks.done),
        ),
    };
    let kills = match &kills.failed {
        None => kills_measure(&kills.done),
        Some(reason) => missing(
            MeasureId::DeviceKills,
            reason,
            &kills.done,
            kill_items(&kills.done),
        ),
    };
    let both: Vec<&Reading> = peaks.done.iter().chain(&empty_peaks.done).collect();
    let over_baseline = match peaks.failed.as_ref().or(empty_peaks.failed.as_ref()) {
        None => over_baseline_measure(&peaks.done, &empty_peaks.done),
        Some(reason) => {
            let mut items = run_items(&peaks.done);
            items.extend(run_items(&empty_peaks.done));
            Measure {
                items,
                evidence: run_evidence(both),
                ..required(Measure::missing(
                    MeasureId::DeviceFootprintOverBaseline,
                    reason,
                ))
            }
        }
    };
    vec![footprint, footprint_peak, kills, over_baseline]
}

/// One run's reading of a measure.
struct Reading<'a> {
    /// What the run was a run of, and its number from 1.
    of: Of,
    run: usize,
    value: i64,
    measure: &'a Measure,
}

/// The runs' readings of one measure: those of the runs that gave one, and why the first run
/// that gave none did not.
struct Readings<'a> {
    done: Vec<Reading<'a>>,
    failed: Option<String>,
}

/// Each run's reading of one measure. A run with none gives its own reason.
fn readings(runs: &[Vec<Measure>], of: Of, id: MeasureId) -> Readings<'_> {
    let mut readings = Readings {
        done: Vec::new(),
        failed: None,
    };
    for (index, measures) in runs.iter().enumerate() {
        let run = index + 1;
        let measure = measures.iter().find(|measure| measure.id == id);
        match measure {
            Some(
                measure @ Measure {
                    status: MeasureStatus::Measured,
                    value: Some(MeasureValue::Integer(value)),
                    ..
                },
            ) => readings.done.push(Reading {
                of,
                run,
                value: *value,
                measure,
            }),
            _ => {
                let why = measure
                    .and_then(|measure| measure.reason.as_deref())
                    .unwrap_or("no reading");
                readings.failed.get_or_insert_with(|| {
                    format!(
                        "{} {run} of {} did not complete: {why}",
                        run_name(of),
                        runs.len()
                    )
                });
            }
        }
    }
    readings
}

fn run_name(of: Of) -> &'static str {
    match of {
        Of::Game => "game run",
        Of::EmptyBuild => "empty build run",
    }
}

fn values(readings: &[Reading]) -> Vec<i64> {
    readings.iter().map(|reading| reading.value).collect()
}

/// The median: the middle value, or the mean of the two middle values, rounded down.
fn median(values: &[i64]) -> i64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        let (low, high) = (sorted[middle - 1], sorted[middle]);
        low + (high - low) / 2
    }
}

/// An item per run, keyed by the run.
fn run_items(readings: &[Reading]) -> Vec<MeasureItem> {
    readings
        .iter()
        .map(|reading| MeasureItem {
            key: format!("{} {}", run_name(reading.of), reading.run),
            value: MeasureValue::Integer(reading.value),
        })
        .collect()
}

/// Each run's evidence, with the run it is from.
fn run_evidence<'a>(readings: impl IntoIterator<Item = &'a Reading<'a>>) -> Vec<Evidence> {
    let mut evidence: Vec<Evidence> = readings
        .into_iter()
        .flat_map(|reading| {
            reading.measure.evidence.iter().map(|evidence| Evidence {
                method: evidence.method,
                detail: format!(
                    "{} {}: {}",
                    run_name(reading.of),
                    reading.run,
                    evidence.detail
                ),
            })
        })
        .collect();
    evidence.dedup();
    evidence
}

/// The median of the runs, with the smallest and largest run as its range.
fn median_measure(id: MeasureId, readings: &[Reading]) -> Measure {
    let values = values(readings);
    Measure {
        value: Some(MeasureValue::Integer(median(&values))),
        items: run_items(readings),
        evidence: run_evidence(readings),
        uncertainty: Uncertainty {
            kind: UncertaintyKind::Range,
            low: values.iter().min().copied(),
            high: values.iter().max().copied(),
            note: Some(format!(
                "the median of {} runs; low and high are the smallest and largest run",
                values.len()
            )),
        },
        ..required(Measure::blank(id, MeasureStatus::Measured))
    }
}

/// The runs the phone killed, with an item per run: 0, or 1 with what the trace saw.
fn kills_measure(readings: &[Reading]) -> Measure {
    Measure {
        value: Some(MeasureValue::Integer(
            readings.iter().map(|reading| reading.value).sum(),
        )),
        items: kill_items(readings),
        evidence: run_evidence(readings),
        uncertainty: Uncertainty::exact(),
        ..required(Measure::blank(
            MeasureId::DeviceKills,
            MeasureStatus::Measured,
        ))
    }
}

/// An item per run: 0, or 1 with what the trace saw.
fn kill_items(readings: &[Reading]) -> Vec<MeasureItem> {
    readings
        .iter()
        .map(|reading| {
            let run = format!("{} {}", run_name(reading.of), reading.run);
            match reading.measure.items.first() {
                Some(seen) if reading.value > 0 => MeasureItem {
                    key: format!("{run}: {}", seen.key),
                    value: MeasureValue::Integer(reading.value),
                },
                _ => MeasureItem {
                    key: run,
                    value: MeasureValue::Integer(reading.value),
                },
            }
        })
        .collect()
}

/// The median game peak minus the median peak of the empty build, with an item per run of each.
fn over_baseline_measure(peaks: &[Reading], empty_peaks: &[Reading]) -> Measure {
    let mut items = run_items(peaks);
    items.extend(run_items(empty_peaks));
    Measure {
        value: Some(MeasureValue::Integer(
            median(&values(peaks)) - median(&values(empty_peaks)),
        )),
        items,
        evidence: run_evidence(peaks.iter().chain(empty_peaks)),
        uncertainty: Uncertainty {
            kind: UncertaintyKind::Unknown,
            low: None,
            high: None,
            note: Some(format!(
                "the median peak of {} game runs minus the median peak of {} runs of the empty build",
                peaks.len(),
                empty_peaks.len()
            )),
        },
        ..required(Measure::blank(
            MeasureId::DeviceFootprintOverBaseline,
            MeasureStatus::Measured,
        ))
    }
}

/// A measure missing for this reason, with the items and evidence of the runs that did complete.
fn missing(id: MeasureId, reason: &str, done: &[Reading], items: Vec<MeasureItem>) -> Measure {
    Measure {
        items,
        evidence: run_evidence(done),
        ..required(Measure::missing(id, reason))
    }
}

#[cfg(test)]
mod tests {
    use dyno_schema::measure::Method;

    use super::*;
    use crate::missing_device_measures;

    const MIB: i64 = 1_048_576;

    fn measured(id: MeasureId, value: i64) -> Measure {
        Measure {
            value: Some(MeasureValue::Integer(value)),
            evidence: vec![Evidence {
                method: Method::Xctrace,
                detail: "an Activity Monitor trace of the phone: iPhone 14 Pro Max".to_owned(),
            }],
            ..required(Measure::blank(id, MeasureStatus::Measured))
        }
    }

    /// One run's device measures: its footprint at idle, its peak, and whether it was killed.
    fn run(idle: i64, peak: i64, killed: bool) -> Vec<Measure> {
        let mut kills = measured(MeasureId::DeviceKills, i64::from(killed));
        if killed {
            kills.items = vec![MeasureItem {
                key: "com.apple.WebKit.WebContent (412) left the trace 7 s before the run ended"
                    .to_owned(),
                value: MeasureValue::Integer(1),
            }];
        }
        vec![
            measured(MeasureId::DevicePhysFootprint, idle),
            measured(MeasureId::DevicePhysFootprintPeak, peak),
            kills,
        ]
    }

    fn empty_run(peak: i64) -> Vec<Measure> {
        run(peak - MIB, peak, false)
    }

    fn by_id(measures: &[Measure], id: MeasureId) -> &Measure {
        measures.iter().find(|measure| measure.id == id).unwrap()
    }

    fn value(measure: &Measure) -> Option<i64> {
        match measure.value {
            Some(MeasureValue::Integer(value)) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn three_runs_give_the_medians_the_kills_and_the_footprint_over_the_empty_build() {
        let game = [
            run(280 * MIB, 420 * MIB, false),
            run(270 * MIB, 430 * MIB, false),
            run(290 * MIB, 425 * MIB, true),
        ];
        let empty = [
            empty_run(60 * MIB),
            empty_run(62 * MIB),
            empty_run(61 * MIB),
        ];
        let measures = device_runs_measures(&game, &empty);
        let ids: Vec<MeasureId> = measures.iter().map(|measure| measure.id).collect();
        assert_eq!(
            ids,
            [
                MeasureId::DevicePhysFootprint,
                MeasureId::DevicePhysFootprintPeak,
                MeasureId::DeviceKills,
                MeasureId::DeviceFootprintOverBaseline,
            ]
        );
        assert!(measures.iter().all(|measure| measure.required));

        let footprint = by_id(&measures, MeasureId::DevicePhysFootprint);
        assert_eq!(value(footprint), Some(280 * MIB));
        assert_eq!(footprint.uncertainty.low, Some(270 * MIB));
        assert_eq!(footprint.uncertainty.high, Some(290 * MIB));
        assert!(footprint.items.contains(&MeasureItem {
            key: "game run 2".to_owned(),
            value: MeasureValue::Integer(270 * MIB),
        }));
        assert!(footprint.items.contains(&MeasureItem {
            key: "peak during the run".to_owned(),
            value: MeasureValue::Integer(425 * MIB),
        }));

        let peak = by_id(&measures, MeasureId::DevicePhysFootprintPeak);
        assert_eq!(value(peak), Some(425 * MIB));
        assert_eq!(peak.items.len(), 3);

        let kills = by_id(&measures, MeasureId::DeviceKills);
        assert_eq!(value(kills), Some(1));
        assert_eq!(kills.items[0].key, "game run 1");
        assert!(
            kills.items[2]
                .key
                .starts_with("game run 3: com.apple.WebKit.WebContent (412) left the trace")
        );

        let over = by_id(&measures, MeasureId::DeviceFootprintOverBaseline);
        assert_eq!(value(over), Some(425 * MIB - 61 * MIB));
        assert_eq!(over.items.len(), 6);
        assert!(over.items.contains(&MeasureItem {
            key: "empty build run 3".to_owned(),
            value: MeasureValue::Integer(61 * MIB),
        }));
        assert_eq!(
            over.evidence[0].detail,
            "game run 1: an Activity Monitor trace of the phone: iPhone 14 Pro Max"
        );
    }

    #[test]
    fn an_even_number_of_runs_takes_the_mean_of_the_middle_two_rounded_down() {
        assert_eq!(median(&[4, 1]), 2);
        assert_eq!(median(&[10, 1, 7, 3]), 5);
        assert_eq!(median(&[9]), 9);
    }

    #[test]
    fn a_game_smaller_than_the_noise_of_the_empty_page_gives_a_value_below_0() {
        let game = [run(40 * MIB, 50 * MIB, false)];
        let empty = [empty_run(60 * MIB)];
        let mut over = device_runs_measures(&game, &empty).pop().unwrap();
        over.invalidate_bad_numbers();
        assert_eq!(over.status, MeasureStatus::Measured);
        assert_eq!(value(&over), Some(-10 * MIB));
    }

    #[test]
    fn a_run_that_did_not_complete_leaves_its_medians_missing_and_says_which() {
        let game = [
            run(280 * MIB, 420 * MIB, false),
            missing_device_measures("cannot read the trace: xctrace export failed"),
        ];
        let empty = [empty_run(60 * MIB), empty_run(62 * MIB)];
        let measures = device_runs_measures(&game, &empty);
        // The run that did complete keeps its reading as an item.
        let peak = by_id(&measures, MeasureId::DevicePhysFootprintPeak);
        assert_eq!(
            peak.items,
            [MeasureItem {
                key: "game run 1".to_owned(),
                value: MeasureValue::Integer(420 * MIB),
            }]
        );
        assert_eq!(
            by_id(&measures, MeasureId::DeviceKills).items[0].key,
            "game run 1"
        );
        let over = by_id(&measures, MeasureId::DeviceFootprintOverBaseline);
        assert_eq!(over.items.len(), 3);
        for measure in &measures {
            assert_eq!(measure.status, MeasureStatus::Missing, "{}", measure.id);
            assert_eq!(measure.value, None);
            assert!(measure.required);
            assert_eq!(
                measure.reason.as_deref(),
                Some(
                    "game run 2 of 2 did not complete: cannot read the trace: xctrace export failed"
                ),
                "{}",
                measure.id
            );
        }
    }

    #[test]
    fn a_game_killed_before_idle_still_gives_its_peak_and_its_kill() {
        let mut killed = run(0, 900 * MIB, true);
        killed[0] = required(Measure::missing(
            MeasureId::DevicePhysFootprint,
            "the game's process has no sample after the frame went idle",
        ));
        let game = [run(280 * MIB, 420 * MIB, false), killed];
        let empty = [empty_run(60 * MIB), empty_run(60 * MIB)];
        let measures = device_runs_measures(&game, &empty);
        let footprint = by_id(&measures, MeasureId::DevicePhysFootprint);
        assert_eq!(footprint.status, MeasureStatus::Missing);
        assert!(
            footprint
                .reason
                .as_deref()
                .unwrap()
                .starts_with("game run 2 of 2 did not complete: the game's process has no sample")
        );
        assert_eq!(
            value(by_id(&measures, MeasureId::DevicePhysFootprintPeak)),
            Some(660 * MIB)
        );
        assert_eq!(value(by_id(&measures, MeasureId::DeviceKills)), Some(1));
    }

    #[test]
    fn an_empty_build_run_that_did_not_complete_leaves_only_the_footprint_over_it_missing() {
        let game = [run(280 * MIB, 420 * MIB, false)];
        let empty = [missing_device_measures("the run did not start")];
        let measures = device_runs_measures(&game, &empty);
        let over = by_id(&measures, MeasureId::DeviceFootprintOverBaseline);
        assert_eq!(over.status, MeasureStatus::Missing);
        assert_eq!(
            over.reason.as_deref(),
            Some("empty build run 1 of 1 did not complete: the run did not start")
        );
        assert_eq!(
            by_id(&measures, MeasureId::DevicePhysFootprint).status,
            MeasureStatus::Measured
        );
    }

    #[test]
    fn without_runs_the_footprint_over_the_empty_build_is_unsupported_and_not_required() {
        let measure = over_baseline_unsupported();
        assert_eq!(measure.status, MeasureStatus::Unsupported);
        assert!(!measure.required);
        assert!(
            measure
                .reason
                .as_deref()
                .unwrap()
                .starts_with("needs --runs")
        );
    }
}
