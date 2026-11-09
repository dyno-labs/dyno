//! The iPhone lab: `dyno check --device` and `dyno doctor`.
//!
//! A device run records an Activity Monitor trace of every process on the phone with `xcrun
//! xctrace` (module [`xctrace`]) while the phone's Safari runs a served check (`--driver serve`).
//! The trace ([`trace`]) gives each process's physical footprint, the `phys_footprint` that
//! jetsam reads, about once a second. [`device_measures`] finds the game's WebContent process and
//! reads its footprint when the frame went idle, and whether it left the trace before the run
//! ended, as a process that iOS kills does. Before it records, [`devicectl`] checks that the
//! phone is unlocked and opens the page in its Safari.
//!
//! `doctor` is still a stub: it runs no command and touches no device.

pub mod devicectl;
pub mod trace;
pub mod xctrace;

use dyno_schema::SCHEMA_VERSION;
use dyno_schema::doctor::{DoctorCheck, DoctorCheckId, DoctorReport};
use dyno_schema::ids::{MeasureId, Phase};
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, NOT_BUILT_YET, Uncertainty,
};
use dyno_schema::status::{Status, Verdict};

use crate::trace::Trace;

/// The plan phase that builds `doctor` and the device run.
pub const DEVICE_PHASE: Phase = 2;

/// The measures of a device run. A check has them only when `--device` asks for them, so they
/// are always required.
pub const DEVICE_MEASURES: &[MeasureId] = &[MeasureId::DevicePhysFootprint, MeasureId::DeviceKills];

/// Whether this Mac can run the game on an iPhone.
///
/// Stub: every check is incomplete, and the reason names the phase. The built version checks
/// Xcode, a connected iPhone and trust.
pub fn doctor() -> DoctorReport {
    let checks = DoctorCheckId::ALL
        .iter()
        .map(|&id| DoctorCheck {
            id,
            verdict: Verdict::Incomplete,
            phase: DEVICE_PHASE,
            detail: None,
            reason: Some(format!(
                "{NOT_BUILT_YET}: doctor is a Phase {DEVICE_PHASE} command"
            )),
        })
        .collect();
    DoctorReport {
        schema_version: SCHEMA_VERSION,
        phase: DEVICE_PHASE,
        status: Status::Incomplete,
        checks,
    }
}

/// The name of Safari's process that runs a page.
const WEB_CONTENT: &str = "com.apple.WebKit.WebContent";

/// How far outside the run a sample may be and still count: a sample is taken about once a
/// second, and the trace's clock is placed on the run's to within that.
const SLACK_MS: f64 = 1_500.0;

/// How long before the run ended the game's process may last appear and still count as alive.
/// A busy process gets a sample every second.
const KILL_GAP_MS: f64 = 3_000.0;

/// When the run's steps happened, in ms since the Unix epoch on the Mac's clock, which the trace
/// also uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunWindow {
    /// When the harness page started to load the game.
    pub navigated_epoch_ms: f64,
    /// When the game frame went idle. None when it never did.
    pub idle_epoch_ms: Option<f64>,
    /// When the run ended.
    pub ended_epoch_ms: f64,
}

/// The device measures of a run, from its trace.
///
/// The game's process is the WebContent process with the largest footprint during the run.
/// `device.phys_footprint` is its first footprint at or after the frame went idle, with its peak
/// during the run as an item. `device.kills` is 1 when the process left the trace while the run
/// was still on, as a process that iOS kills does. The evidence names the model and iOS version,
/// never the phone's own name, which is often its owner's.
pub fn device_measures(trace: &Trace, run: &RunWindow) -> Vec<Measure> {
    let in_run: Vec<(f64, u32, u64)> = trace
        .samples
        .iter()
        .filter(|sample| sample.process == WEB_CONTENT)
        .map(|sample| {
            (
                trace.started_epoch_ms + sample.ms,
                sample.pid,
                sample.footprint_bytes,
            )
        })
        .filter(|(at, _, _)| {
            *at >= run.navigated_epoch_ms - SLACK_MS && *at <= run.ended_epoch_ms + SLACK_MS
        })
        .collect();
    let Some(&(_, game_pid, peak)) = in_run.iter().max_by_key(|(_, _, bytes)| *bytes) else {
        return missing_device_measures(
            "the trace of the phone has no Safari WebContent process during the run",
        );
    };
    let game: Vec<(f64, u64)> = in_run
        .iter()
        .filter(|(_, pid, _)| *pid == game_pid)
        .map(|&(at, _, bytes)| (at, bytes))
        .collect();
    let evidence = vec![Evidence {
        method: Method::Xctrace,
        detail: format!(
            "an Activity Monitor trace of the phone: {}, iOS {}, process {WEB_CONTENT} ({game_pid})",
            trace.device.model, trace.device.os_version
        ),
    }];
    let at_idle = run.idle_epoch_ms.and_then(|idle| {
        game.iter()
            .find(|(at, _)| *at >= idle)
            .map(|&(_, bytes)| bytes)
    });
    let footprint = match at_idle {
        Some(bytes) => Measure {
            value: Some(MeasureValue::Integer(saturating(bytes))),
            items: vec![MeasureItem {
                key: "peak during the run".to_owned(),
                value: MeasureValue::Integer(saturating(peak)),
            }],
            evidence: evidence.clone(),
            uncertainty: Uncertainty::exact(),
            ..required(Measure::blank(
                MeasureId::DevicePhysFootprint,
                MeasureStatus::Measured,
            ))
        },
        None => required(Measure::missing(
            MeasureId::DevicePhysFootprint,
            "the game's process has no sample after the frame went idle",
        )),
    };
    // A trace that ended before the run did, as one that lost the phone, cannot tell a kill.
    let trace_ended = trace
        .samples
        .iter()
        .map(|sample| trace.started_epoch_ms + sample.ms)
        .fold(f64::MIN, f64::max);
    let kills = if trace_ended < run.ended_epoch_ms - KILL_GAP_MS {
        required(Measure::missing(
            MeasureId::DeviceKills,
            &format!(
                "the trace ended {:.0} s before the run did, so it cannot tell whether iOS killed the game",
                (run.ended_epoch_ms - trace_ended) / 1_000.0
            ),
        ))
    } else {
        let last_seen = game.iter().map(|(at, _)| *at).fold(f64::MIN, f64::max);
        let killed = last_seen < run.ended_epoch_ms - KILL_GAP_MS;
        Measure {
            value: Some(MeasureValue::Integer(i64::from(killed))),
            items: if killed {
                vec![MeasureItem {
                    key: format!(
                        "{WEB_CONTENT} ({game_pid}) left the trace {:.0} s before the run ended",
                        (run.ended_epoch_ms - last_seen) / 1_000.0
                    ),
                    value: MeasureValue::Integer(1),
                }]
            } else {
                Vec::new()
            },
            evidence,
            uncertainty: Uncertainty::exact(),
            ..required(Measure::blank(
                MeasureId::DeviceKills,
                MeasureStatus::Measured,
            ))
        }
    };
    vec![footprint, kills]
}

/// Every device measure, missing for this reason.
pub fn missing_device_measures(reason: &str) -> Vec<Measure> {
    DEVICE_MEASURES
        .iter()
        .map(|&id| required(Measure::missing(id, reason)))
        .collect()
}

/// A device measure is required: a check has it only when `--device` asks for it.
fn required(measure: Measure) -> Measure {
    Measure {
        required: true,
        ..measure
    }
}

fn saturating(bytes: u64) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use dyno_schema::measure::Source;

    use super::*;
    use crate::trace::TraceDevice;

    #[test]
    fn a_stub_doctor_is_incomplete_and_names_its_phase() {
        let report = doctor();
        assert_eq!(report.status, Status::Incomplete);
        assert_eq!(report.phase, 2);
        assert_eq!(report.checks.len(), DoctorCheckId::ALL.len());
        for check in &report.checks {
            assert_eq!(check.verdict, Verdict::Incomplete);
            assert!(check.reason.as_deref().unwrap().contains("Phase 2"));
        }
    }

    #[test]
    fn missing_device_measures_cover_every_device_measure_and_are_required() {
        let measures = missing_device_measures("no phone");
        let device: Vec<MeasureId> = MeasureId::ALL
            .iter()
            .copied()
            .filter(|id| id.spec().source == Source::Device)
            .collect();
        let ids: Vec<MeasureId> = measures.iter().map(|measure| measure.id).collect();
        assert_eq!(ids, device);
        for measure in &measures {
            assert!(measure.required);
            assert_eq!(measure.status, MeasureStatus::Missing);
            assert_eq!(measure.reason.as_deref(), Some("no phone"));
        }
    }

    const START: f64 = 1_000_000.0;
    const GAME: &str = "com.apple.WebKit.WebContent (412)";
    const OTHER_TAB: &str = "com.apple.WebKit.WebContent (377)";

    /// A trace that started at START, with these samples: (ms since START, process, bytes).
    fn trace(samples: &[(u64, &str, u64)]) -> Trace {
        let xml = crate::trace::tests::process_table(samples);
        Trace {
            started_epoch_ms: START,
            device: TraceDevice {
                name: "Dev’s iPhone".to_owned(),
                model: "iPhone 14 Pro Max".to_owned(),
                os_version: "26.6.2".to_owned(),
                memory_bytes: Some(6_442_450_944),
            },
            samples: crate::trace::read_samples(&xml).unwrap(),
        }
    }

    /// The game loads from 1 s, goes idle at 10.4 s, and the run ends at 13 s.
    fn run() -> RunWindow {
        RunWindow {
            navigated_epoch_ms: START + 1_000.0,
            idle_epoch_ms: Some(START + 10_400.0),
            ended_epoch_ms: START + 13_000.0,
        }
    }

    fn value(measure: &Measure) -> Option<i64> {
        match measure.value {
            Some(MeasureValue::Integer(value)) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn the_footprint_is_the_game_process_at_idle_with_its_peak() {
        let mut samples = vec![(500, OTHER_TAB, 50)];
        // Idle is at 10.4 s: the 10 s sample is before it, so the 11 s one is the footprint.
        for (second, bytes) in [
            (1, 80),
            (5, 400),
            (10, 333),
            (11, 300),
            (12, 310),
            (13, 200),
        ] {
            samples.push((second * 1_000, GAME, bytes));
            samples.push((second * 1_000, OTHER_TAB, 50));
        }
        let [footprint, kills] = device_measures(&trace(&samples), &run())
            .try_into()
            .unwrap();
        assert_eq!(footprint.id, MeasureId::DevicePhysFootprint);
        assert_eq!(footprint.status, MeasureStatus::Measured);
        assert_eq!(value(&footprint), Some(300));
        assert_eq!(footprint.items[0].value, MeasureValue::Integer(400));
        assert_eq!(
            footprint.evidence[0].detail,
            "an Activity Monitor trace of the phone: iPhone 14 Pro Max, iOS 26.6.2, process \
             com.apple.WebKit.WebContent (412)"
        );
        assert!(footprint.required);
        assert_eq!(value(&kills), Some(0));
        assert!(kills.items.is_empty());
    }

    #[test]
    fn a_game_process_that_leaves_the_trace_during_the_run_is_a_kill() {
        let samples = [
            (2_000, GAME, 900),
            (6_000, GAME, 1_800),
            (11_000, OTHER_TAB, 60),
        ];
        let [footprint, kills] = device_measures(&trace(&samples), &run())
            .try_into()
            .unwrap();
        assert_eq!(footprint.status, MeasureStatus::Missing);
        assert_eq!(value(&kills), Some(1));
        assert!(
            kills.items[0]
                .key
                .contains("left the trace 7 s before the run ended")
        );
    }

    #[test]
    fn a_trace_that_ended_before_the_run_cannot_tell_a_kill() {
        // The phone disconnected at 8 s, 5 s before the run ended. Idle at 5 s is still read.
        let samples = [
            (2_000, GAME, 900),
            (6_000, GAME, 1_800),
            (8_000, OTHER_TAB, 60),
        ];
        let window = RunWindow {
            idle_epoch_ms: Some(START + 5_000.0),
            ..run()
        };
        let [footprint, kills] = device_measures(&trace(&samples), &window)
            .try_into()
            .unwrap();
        assert_eq!(value(&footprint), Some(1_800));
        assert_eq!(kills.status, MeasureStatus::Missing);
        assert!(
            kills
                .reason
                .as_deref()
                .unwrap()
                .starts_with("the trace ended 5 s before the run did")
        );
    }

    #[test]
    fn a_trace_with_no_safari_process_in_the_run_gives_missing_measures() {
        let samples = [(0, "SpringBoard (60)", 900), (60_000, GAME, 500)];
        let measures = device_measures(&trace(&samples), &run());
        assert!(
            measures
                .iter()
                .all(|measure| measure.status == MeasureStatus::Missing)
        );
        assert!(
            measures[0]
                .reason
                .as_deref()
                .unwrap()
                .contains("no Safari WebContent process")
        );
    }

    #[test]
    fn a_run_that_never_went_idle_has_no_footprint_but_still_counts_kills() {
        let samples = [(2_000, GAME, 900), (12_500, GAME, 950)];
        let window = RunWindow {
            idle_epoch_ms: None,
            ..run()
        };
        let [footprint, kills] = device_measures(&trace(&samples), &window)
            .try_into()
            .unwrap();
        assert_eq!(footprint.status, MeasureStatus::Missing);
        assert_eq!(value(&kills), Some(0));
    }
}
