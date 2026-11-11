//! The iPhone lab: `dyno check --device` and `dyno doctor`.
//!
//! A device run records the phone while its Safari runs a served check (`--driver serve`). It
//! reads the phone with the `idevice` crate, through usbmuxd or straight over the network, so it
//! needs no Xcode and runs on Linux too ([`phone`]). The phone's sysmontap service ([`sysmon`])
//! gives each process's physical footprint, the `phys_footprint` that jetsam reads, about once a
//! second, and its log ([`syslog`]) gives the kernel's line when jetsam kills a process.
//! [`Recorder`] runs the recording on a thread of its own, and opens the page in the phone's
//! Safari once it records. [`device_measures`] finds the game's WebContent process, reads its
//! footprint when the frame went idle and its peak, and counts a kill when the kernel killed it
//! during the run. With `--runs`, [`runs`] reads the runs of the game and of the empty build into
//! one set of device measures.
//!
//! [`device_timeline`] keeps every sample of Safari's processes through the run for `run.timeline`,
//! with the kernel's kills and memory limits as marks.
//!
//! [`trace`] reads an Activity Monitor trace that xctrace exported, and gives the same measures
//! and timeline from it. `--device` does not use it: it is for traces that a device cloud's host
//! records.
//!
//! `doctor` is still a stub: it runs no command and touches no device.

pub mod devicectl;
pub mod fake;
mod phone;
pub mod recorder;
pub mod recording;
pub mod runs;
pub mod syslog;
pub mod sysmon;
pub mod trace;

use dyno_schema::SCHEMA_VERSION;
use dyno_schema::devices::apple_os;
use dyno_schema::doctor::{DoctorCheck, DoctorCheckId, DoctorReport};
use dyno_schema::environment::{EnvironmentDevice, EnvironmentOs};
use dyno_schema::ids::{MeasureId, Phase};
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, NOT_BUILT_YET, Uncertainty,
};
use dyno_schema::runner::RunMark;
use dyno_schema::status::{Status, Verdict};
use dyno_schema::timeline::{
    MemorySample, ProcessRole, Timeline, TimelineMark, TimelineMethod, TimelineProcess,
};

pub use crate::recorder::{Address, DeviceError, OpenedWith, Recorder};
use crate::recording::Recording;

/// The plan phase that builds `doctor` and the device run.
pub const DEVICE_PHASE: Phase = 2;

/// The measures of one device run. A check has them only when `--device` asks for them, so they
/// are always required. `device.footprint_over_baseline` needs `--runs`, so [`runs`] gives it.
pub const DEVICE_MEASURES: &[MeasureId] = &[
    MeasureId::DevicePhysFootprint,
    MeasureId::DevicePhysFootprintPeak,
    MeasureId::DeviceKills,
];

/// Whether this computer can run the game on an iPhone.
///
/// Stub: every check is incomplete, and the reason names the phase. The built version checks a
/// connected iPhone and trust.
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

/// The item of `device.phys_footprint` that holds the peak, for readers from before
/// `device.phys_footprint_peak`.
pub(crate) const PEAK_ITEM: &str = "peak during the run";

/// The name of Safari's process that runs a page.
pub const WEB_CONTENT: &str = "com.apple.WebKit.WebContent";

/// Safari's own processes, which a recording and a timeline keep, with the role each plays when
/// it is not the game's: its page processes, its GPU and network processes, and the app.
pub const SAFARI_PROCESSES: &[(&str, ProcessRole)] = &[
    (WEB_CONTENT, ProcessRole::Other),
    ("com.apple.WebKit.GPU", ProcessRole::Gpu),
    ("com.apple.WebKit.Networking", ProcessRole::Other),
    ("MobileSafari", ProcessRole::Browser),
];

/// How far outside the run a sample may be and still count: a sample comes about once a second.
pub(crate) const SLACK_MS: f64 = 1_500.0;

/// How long before the run ended a source may end and still cover the run.
const ENDED_GAP_MS: f64 = 3_000.0;

/// When the run's steps happened, in ms since the Unix epoch on this machine's clock, which the
/// recording also uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunWindow {
    /// When the harness page started to load the game.
    pub navigated_epoch_ms: f64,
    /// When the game frame went idle. None when it never did.
    pub idle_epoch_ms: Option<f64>,
    /// When the harness made the game active. None when it never did.
    pub activated_epoch_ms: Option<f64>,
    /// When the run ended.
    pub ended_epoch_ms: f64,
}

/// The device measures of a run, from its recording.
///
/// The game's process is the WebContent process with the largest footprint during the run.
/// `device.phys_footprint` is its first footprint at or after the frame went idle, with its peak
/// during the run as an item. `device.phys_footprint_peak` is that peak as a measure of its own.
/// `device.kills` is 1 when the kernel's log says that jetsam killed the process during the run.
/// The evidence names the model and iOS version, never the phone's own name, which is often its
/// owner's.
pub fn device_measures(recording: &Recording, run: &RunWindow) -> Vec<Measure> {
    let in_run =
        |at: f64| at >= run.navigated_epoch_ms - SLACK_MS && at <= run.ended_epoch_ms + SLACK_MS;
    let pages: Vec<&recording::Sample> = recording
        .samples
        .iter()
        .filter(|sample| sample.process == WEB_CONTENT && in_run(sample.epoch_ms))
        .collect();
    // The game's process is the one with the largest sample, so that sample is its peak.
    let Some((game_pid, peak)) = pages
        .iter()
        .max_by_key(|sample| sample.footprint_bytes)
        .map(|sample| (sample.pid, sample.footprint_bytes))
    else {
        return missing_device_measures(&match short_by(&recording.samples_cover, run) {
            Some(short) => format!(
                "the phone's samples ended {short:.0} s before the run did: {}",
                ended(&recording.samples_cover)
            ),
            None => {
                "the phone's samples have no Safari WebContent process during the run".to_owned()
            }
        });
    };
    let game: Vec<&recording::Sample> = pages
        .into_iter()
        .filter(|sample| sample.pid == game_pid)
        .collect();
    let phone = format!(
        "{}, iOS {}, process {WEB_CONTENT} ({game_pid})",
        recording.phone.model(),
        recording.phone.os_version
    );
    let evidence = vec![Evidence {
        method: Method::Sysmontap,
        detail: format!("sysmontap samples of the phone: {phone}"),
    }];
    // Samples that ended before the run did, as when the phone left the network, cannot give its
    // peak.
    let samples_short = short_by(&recording.samples_cover, run);
    let peak = match samples_short {
        None => Ok(peak),
        Some(short) => Err(format!(
            "the phone's samples ended {short:.0} s before the run did, so the peak of the run is \
             not known: {}",
            ended(&recording.samples_cover)
        )),
    };
    let at_idle = run.idle_epoch_ms.and_then(|idle| {
        game.iter()
            .find(|sample| sample.epoch_ms >= idle)
            .map(|sample| sample.footprint_bytes)
    });
    let footprint = match at_idle {
        Some(bytes) => Measure {
            value: Some(MeasureValue::Integer(saturating(bytes))),
            items: peak
                .iter()
                .map(|&peak| MeasureItem {
                    key: PEAK_ITEM.to_owned(),
                    value: MeasureValue::Integer(saturating(peak)),
                })
                .collect(),
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
    let footprint_peak = match peak {
        Ok(peak) => Measure {
            value: Some(MeasureValue::Integer(saturating(peak))),
            evidence,
            uncertainty: Uncertainty::exact(),
            ..required(Measure::blank(
                MeasureId::DevicePhysFootprintPeak,
                MeasureStatus::Measured,
            ))
        },
        Err(reason) => required(Measure::missing(
            MeasureId::DevicePhysFootprintPeak,
            &reason,
        )),
    };
    vec![
        footprint,
        footprint_peak,
        kills(recording, run, game_pid, &phone),
    ]
}

/// The timeline of Safari's processes during the run, from its recording: every sample of each
/// of [`SAFARI_PROCESSES`] from the navigation to the end of the run, on the run's clock. The
/// game's process is the WebContent process with the largest footprint, as in
/// [`device_measures`]; a page and its game frame share it, since Safari keeps a cross-site frame
/// in its page's process. The marks are the navigation, the idle and the activation, and the
/// kernel's kill of the game's process or its going over a memory limit. None when the recording
/// has no page process during the run.
pub fn device_timeline(recording: &Recording, run: &RunWindow) -> Option<Timeline> {
    let samples = recording.samples.iter().map(|sample| Footprint {
        epoch_ms: sample.epoch_ms,
        pid: sample.pid,
        process: &sample.process,
        bytes: sample.footprint_bytes,
    });
    let events = recording.memory_lines.iter().map(|logged| KernelEvent {
        epoch_ms: logged.epoch_ms,
        pid: logged.line.pid,
        mark: if logged.line.is_kill() {
            RunMark::Killed
        } else {
            RunMark::OverMemoryLimit
        },
    });
    safari_timeline(
        samples,
        events,
        run,
        TimelineMethod::Sysmontap,
        sysmon::INTERVAL_MS,
    )
}

/// One process's footprint at one moment, from a recording or a trace.
pub(crate) struct Footprint<'a> {
    pub epoch_ms: f64,
    pub pid: u32,
    pub process: &'a str,
    pub bytes: u64,
}

/// What the kernel did to a process, and when.
pub(crate) struct KernelEvent {
    pub epoch_ms: f64,
    pub pid: u32,
    pub mark: RunMark,
}

/// The timeline of Safari's processes from their footprints, for [`device_timeline`] and
/// [`trace::trace_timeline`]. A footprint from before the navigation or after the run is left
/// out. The kernel's events about the game's process during the run are marks.
pub(crate) fn safari_timeline<'a>(
    footprints: impl IntoIterator<Item = Footprint<'a>>,
    events: impl IntoIterator<Item = KernelEvent>,
    run: &RunWindow,
    method: TimelineMethod,
    every_ms: u32,
) -> Option<Timeline> {
    let run_ms = |epoch_ms: f64| (epoch_ms - run.navigated_epoch_ms).max(0.0).round() as u64;
    // The harness drops the game when the run ends, so later samples and lines are not the run's.
    let in_run =
        |epoch_ms: f64| epoch_ms >= run.navigated_epoch_ms && epoch_ms <= run.ended_epoch_ms;
    let mut processes: Vec<TimelineProcess> = Vec::new();
    for footprint in footprints {
        let Some(&(_, role)) = SAFARI_PROCESSES
            .iter()
            .find(|(name, _)| *name == footprint.process)
        else {
            continue;
        };
        if !in_run(footprint.epoch_ms) {
            continue;
        }
        let index = match processes
            .iter()
            .position(|process| process.pid == footprint.pid)
        {
            Some(index) => index,
            None => {
                processes.push(TimelineProcess {
                    pid: footprint.pid,
                    role,
                    kind: footprint.process.to_owned(),
                    samples: Vec::new(),
                });
                processes.len() - 1
            }
        };
        processes[index].samples.push(MemorySample {
            t_ms: run_ms(footprint.epoch_ms),
            private_bytes: footprint.bytes,
            resident_bytes: None,
            shared_bytes: None,
        });
    }
    let game = processes
        .iter()
        .filter(|process| process.kind == WEB_CONTENT)
        .max_by_key(|process| {
            process
                .samples
                .iter()
                .map(|sample| sample.private_bytes)
                .max()
        })
        .map(|process| process.pid)?;
    for process in &mut processes {
        if process.pid == game {
            process.role = ProcessRole::Game;
        }
    }
    let mut marks = vec![TimelineMark {
        mark: RunMark::Navigated,
        t_ms: 0,
    }];
    for (mark, at) in [
        (RunMark::Idle, run.idle_epoch_ms),
        (RunMark::Activated, run.activated_epoch_ms),
    ] {
        if let Some(at) = at {
            marks.push(TimelineMark {
                mark,
                t_ms: run_ms(at),
            });
        }
    }
    marks.extend(
        events
            .into_iter()
            .filter(|event| event.pid == game && in_run(event.epoch_ms))
            .map(|event| TimelineMark {
                mark: event.mark,
                t_ms: run_ms(event.epoch_ms),
            }),
    );
    let mut timeline = Timeline {
        method,
        every_ms,
        marks,
        processes,
    };
    timeline.sort_lists();
    Some(timeline)
}

/// `device.kills`: 1 when the kernel killed the game's process during the run, 0 when the log
/// covers the run and has no kill of it, and missing when the log ended before the run did.
fn kills(recording: &Recording, run: &RunWindow, game_pid: u32, phone: &str) -> Measure {
    let lines: Vec<&recording::LoggedLine> = recording
        .memory_lines
        .iter()
        .filter(|logged| logged.line.pid == game_pid)
        .filter(|logged| logged.epoch_ms >= run.navigated_epoch_ms - SLACK_MS)
        .filter(|logged| logged.epoch_ms <= run.ended_epoch_ms + SLACK_MS)
        .collect();
    let killed = lines.iter().find(|logged| logged.line.is_kill());
    if let (None, Some(short)) = (killed, short_by(&recording.log_covers, run)) {
        return required(Measure::missing(
            MeasureId::DeviceKills,
            &format!(
                "the phone's log ended {short:.0} s before the run did, so it cannot tell \
                 whether iOS killed the game: {}",
                ended(&recording.log_covers)
            ),
        ));
    }
    // The kernel's other lines about the process, such as a soft memory limit it went over.
    let mut detail = format!("the phone's kernel log: {phone}");
    for logged in lines.iter().filter(|logged| !logged.line.is_kill()) {
        detail.push_str(&format!(". It logged \"{}\"", logged.line.short_text()));
    }
    Measure {
        value: Some(MeasureValue::Integer(i64::from(killed.is_some()))),
        items: killed
            .map(|logged| MeasureItem {
                key: format!(
                    "{}, {:.0} s before the run ended",
                    logged.line.short_text(),
                    ((run.ended_epoch_ms - logged.epoch_ms) / 1_000.0).max(0.0)
                ),
                value: MeasureValue::Integer(1),
            })
            .into_iter()
            .collect(),
        evidence: vec![Evidence {
            method: Method::Syslog,
            detail,
        }],
        uncertainty: Uncertainty::exact(),
        ..required(Measure::blank(
            MeasureId::DeviceKills,
            MeasureStatus::Measured,
        ))
    }
}

/// How many seconds before the run ended a source ended. None when it covers the run.
fn short_by(cover: &recording::Coverage, run: &RunWindow) -> Option<f64> {
    (cover.until_epoch_ms < run.ended_epoch_ms - ENDED_GAP_MS)
        .then(|| (run.ended_epoch_ms - cover.until_epoch_ms) / 1_000.0)
}

fn ended(cover: &recording::Coverage) -> &str {
    cover.ended.as_deref().unwrap_or("it stopped sending")
}

/// The OS and the device of a recording: the model, its iOS version and the memory iOS manages,
/// never the phone's own name. Both are None when the phone did not give its model.
pub fn phone_environment(
    recording: &Recording,
) -> (Option<EnvironmentOs>, Option<EnvironmentDevice>) {
    let model = recording.phone.model();
    let Some(device) = EnvironmentDevice::of_apple_model(&model, recording.memory_bytes) else {
        return (None, None);
    };
    let version = recording.phone.os_version.trim();
    let os = EnvironmentOs {
        name: apple_os(&model),
        version: (!version.is_empty()).then(|| version.to_owned()),
    };
    (Some(os), Some(device))
}

/// Every device measure, missing for this reason.
pub fn missing_device_measures(reason: &str) -> Vec<Measure> {
    DEVICE_MEASURES
        .iter()
        .map(|&id| required(Measure::missing(id, reason)))
        .collect()
}

/// A device measure is required: a check has it only when `--device` asks for it.
pub(crate) fn required(measure: Measure) -> Measure {
    Measure {
        required: true,
        ..measure
    }
}

pub(crate) fn saturating(bytes: u64) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use dyno_schema::environment::OsName;
    use dyno_schema::measure::Source;

    use super::*;
    use crate::recording::tests::phone;
    use crate::recording::{Coverage, LoggedLine, Sample};
    use crate::syslog::memory_line;

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
    fn missing_device_measures_cover_every_measure_of_one_run_and_are_required() {
        let measures = missing_device_measures("no phone");
        let device: Vec<MeasureId> = MeasureId::ALL
            .iter()
            .copied()
            .filter(|id| id.spec().source == Source::Device)
            .filter(|&id| id != MeasureId::DeviceFootprintOverBaseline)
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
    const GAME: u32 = 412;
    const OTHER_TAB: u32 = 377;

    /// A recording that started at START and stopped at 14 s, with these samples of page
    /// processes, (ms since START, pid, bytes), and these log lines, (ms since START, line). Both
    /// sources cover it to the end.
    fn recording(samples: &[(u64, u32, u64)], lines: &[(u64, &str)]) -> Recording {
        let at = |ms: u64| START + ms as f64;
        let to_end = Coverage {
            until_epoch_ms: at(14_000),
            ended: None,
        };
        Recording {
            phone: phone(),
            memory_bytes: Some(5_910_052_864),
            started_epoch_ms: START,
            stopped_epoch_ms: at(14_000),
            samples: samples
                .iter()
                .map(|&(ms, pid, footprint_bytes)| Sample {
                    epoch_ms: at(ms),
                    pid,
                    process: WEB_CONTENT.to_owned(),
                    footprint_bytes,
                })
                .collect(),
            samples_cover: to_end.clone(),
            memory_lines: lines
                .iter()
                .map(|&(ms, line)| LoggedLine {
                    epoch_ms: at(ms),
                    line: memory_line(line).unwrap(),
                })
                .collect(),
            log_covers: to_end,
        }
    }

    /// The game loads from 1 s, goes idle at 10.4 s, is made active at 10.6 s, and the run ends
    /// at 13 s.
    fn run() -> RunWindow {
        RunWindow {
            navigated_epoch_ms: START + 1_000.0,
            idle_epoch_ms: Some(START + 10_400.0),
            activated_epoch_ms: Some(START + 10_600.0),
            ended_epoch_ms: START + 13_000.0,
        }
    }

    /// A sample of one of Safari's other processes, at ms since START.
    fn other_process(ms: u64, pid: u32, process: &str, bytes: u64) -> Sample {
        Sample {
            epoch_ms: START + ms as f64,
            pid,
            process: process.to_owned(),
            footprint_bytes: bytes,
        }
    }

    fn value(measure: &Measure) -> Option<i64> {
        match measure.value {
            Some(MeasureValue::Integer(value)) => Some(value),
            _ => None,
        }
    }

    const KILL: &str = "memorystatus: killing process 412 [com.apple.WebKit.WebContent] in high \
        band FOREGROUND (100) - memorystatus_available_pages: 53907";

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
        let [footprint, peak, kills] = device_measures(&recording(&samples, &[]), &run())
            .try_into()
            .unwrap();
        assert_eq!(footprint.id, MeasureId::DevicePhysFootprint);
        assert_eq!(footprint.status, MeasureStatus::Measured);
        assert_eq!(value(&footprint), Some(300));
        assert_eq!(footprint.items[0].value, MeasureValue::Integer(400));
        assert_eq!(peak.id, MeasureId::DevicePhysFootprintPeak);
        assert_eq!(peak.status, MeasureStatus::Measured);
        assert_eq!(value(&peak), Some(400));
        assert!(peak.required);
        assert!(peak.items.is_empty());
        assert_eq!(footprint.evidence[0].method, Method::Sysmontap);
        assert_eq!(
            footprint.evidence[0].detail,
            "sysmontap samples of the phone: iPhone 14 Pro Max, iOS 26.6.2 (23G90), process \
             com.apple.WebKit.WebContent (412)"
        );
        assert!(footprint.required);
        assert_eq!(value(&kills), Some(0));
        assert!(kills.items.is_empty());
        assert_eq!(kills.evidence[0].method, Method::Syslog);
    }

    #[test]
    fn the_game_is_a_page_process_even_when_another_of_safaris_processes_is_larger() {
        let mut recording = recording(&[(5_000, GAME, 400), (11_000, GAME, 300)], &[]);
        recording
            .samples
            .push(other_process(5_000, 90, "com.apple.WebKit.GPU", 900));
        let [footprint, peak, _] = device_measures(&recording, &run()).try_into().unwrap();
        assert_eq!(value(&footprint), Some(300));
        assert_eq!(value(&peak), Some(400));
    }

    #[test]
    fn the_timeline_keeps_safaris_processes_from_the_navigation_with_roles_and_marks() {
        const SOFT_LIMIT: &str = "memorystatus: com.apple.WebKit.WebContent [412] exceeded mem \
            limit: ActiveSoft 1536 MB (non-fatal)";
        const OTHER_KILL: &str = "memorystatus: killing process 377 [com.apple.WebKit.WebContent] \
            in high band FOREGROUND (100) - memorystatus_available_pages: 53907";
        let mut recording = recording(
            &[
                (500, GAME, 10),
                (1_000, GAME, 80),
                (1_000, OTHER_TAB, 50),
                (5_000, GAME, 400),
                (30_000, GAME, 300),
            ],
            &[
                (6_000, SOFT_LIMIT),
                (8_000, OTHER_KILL),
                (12_000, KILL),
                (13_500, KILL),
            ],
        );
        // The run ends at 13 s: a sample and a kill just after it are the harness dropping the
        // game, not the run.
        recording
            .samples
            .push(other_process(13_500, GAME, WEB_CONTENT, 5));
        recording.samples.extend([
            other_process(1_000, 90, "com.apple.WebKit.GPU", 20),
            other_process(1_000, 88, "MobileSafari", 30),
            other_process(1_000, 12, "SpringBoard", 99),
        ]);
        let timeline = device_timeline(&recording, &run()).unwrap();
        assert_eq!(timeline.method, TimelineMethod::Sysmontap);
        assert_eq!(timeline.every_ms, sysmon::INTERVAL_MS);
        let marks: Vec<(RunMark, u64)> = timeline
            .marks
            .iter()
            .map(|mark| (mark.mark, mark.t_ms))
            .collect();
        // The kill of another tab is not the game's.
        assert_eq!(
            marks,
            [
                (RunMark::Navigated, 0),
                (RunMark::OverMemoryLimit, 5_000),
                (RunMark::Idle, 9_400),
                (RunMark::Activated, 9_600),
                (RunMark::Killed, 11_000),
            ]
        );
        let processes: Vec<(u32, ProcessRole, &str)> = timeline
            .processes
            .iter()
            .map(|process| (process.pid, process.role, process.kind.as_str()))
            .collect();
        assert_eq!(
            processes,
            [
                (88, ProcessRole::Browser, "MobileSafari"),
                (90, ProcessRole::Gpu, "com.apple.WebKit.GPU"),
                (OTHER_TAB, ProcessRole::Other, WEB_CONTENT),
                (GAME, ProcessRole::Game, WEB_CONTENT),
            ]
        );
        // The sample before the navigation and the one long after the run are left out.
        let game: Vec<(u64, u64)> = timeline.processes[3]
            .samples
            .iter()
            .map(|sample| (sample.t_ms, sample.private_bytes))
            .collect();
        assert_eq!(game, [(0, 80), (4_000, 400)]);
        assert_eq!(timeline.processes[3].samples[0].resident_bytes, None);
    }

    #[test]
    fn a_recording_with_no_page_process_in_the_run_has_no_timeline() {
        let mut recording = recording(&[(30_000, GAME, 300)], &[]);
        recording
            .samples
            .push(other_process(2_000, 88, "MobileSafari", 30));
        assert_eq!(device_timeline(&recording, &run()), None);
    }

    #[test]
    fn a_kernel_kill_of_the_game_process_during_the_run_is_a_kill() {
        let samples = [
            (2_000, GAME, 900),
            (6_000, GAME, 1_800),
            (11_000, OTHER_TAB, 60),
        ];
        let lines = [
            (
                5_000,
                "memorystatus: com.apple.WebKit.WebContent [412] exceeded mem limit: \
                     ActiveSoft 1536 MB (non-fatal)",
            ),
            (6_500, KILL),
            (
                6_600,
                "memorystatus: killing_highwater_process pid 412 \
                     [com.apple.WebKit.WebContent] (highwater 100 10s rf:- type:app) 2371666KB",
            ),
        ];
        let [footprint, peak, kills] = device_measures(&recording(&samples, &lines), &run())
            .try_into()
            .unwrap();
        assert_eq!(footprint.status, MeasureStatus::Missing);
        // The peak in the run window is still read when the process died before idle.
        assert_eq!(value(&peak), Some(1_800));
        assert_eq!(value(&kills), Some(1));
        assert_eq!(
            kills.items[0].key,
            "memorystatus: killing process 412 [com.apple.WebKit.WebContent] in high band \
             FOREGROUND (100), 6 s before the run ended"
        );
        // The soft limit it went over first is kept in the evidence.
        assert!(
            kills.evidence[0]
                .detail
                .contains("exceeded mem limit: ActiveSoft 1536 MB (non-fatal)"),
            "{}",
            kills.evidence[0].detail
        );
    }

    #[test]
    fn a_process_that_leaves_the_samples_with_no_kill_in_the_log_is_not_a_kill() {
        // Sysmon can leave a live process out of samples, and a process can end for other reasons.
        let samples = [
            (2_000, GAME, 900),
            (6_000, GAME, 1_800),
            (11_000, OTHER_TAB, 60),
        ];
        let [_, _, kills] = device_measures(&recording(&samples, &[]), &run())
            .try_into()
            .unwrap();
        assert_eq!(value(&kills), Some(0));
    }

    #[test]
    fn kills_of_other_processes_or_outside_the_run_do_not_count() {
        let samples = [
            (2_000, GAME, 900),
            (12_000, GAME, 900),
            (2_000, OTHER_TAB, 60),
        ];
        let other = "memorystatus: killing process 377 [com.apple.WebKit.WebContent] in high \
            band FOREGROUND (100)";
        let lines = [(5_000, other), (20_000, KILL)];
        let [_, _, kills] = device_measures(&recording(&samples, &lines), &run())
            .try_into()
            .unwrap();
        assert_eq!(value(&kills), Some(0));
    }

    #[test]
    fn a_log_that_ended_before_the_run_cannot_tell_a_kill_but_a_kill_it_saw_counts() {
        let samples = [(2_000, GAME, 900), (12_000, GAME, 950)];
        let mut short = recording(&samples, &[]);
        short.log_covers = Coverage {
            until_epoch_ms: START + 8_000.0,
            ended: Some("the relay closed".to_owned()),
        };
        let [footprint, _, kills] = device_measures(&short, &run()).try_into().unwrap();
        assert_eq!(value(&footprint), Some(950));
        assert_eq!(kills.status, MeasureStatus::Missing);
        assert_eq!(
            kills.reason.as_deref(),
            Some(
                "the phone's log ended 5 s before the run did, so it cannot tell whether iOS \
                 killed the game: the relay closed"
            )
        );
        let mut seen = recording(&samples, &[(6_000, KILL)]);
        seen.log_covers = short.log_covers.clone();
        let [_, _, kills] = device_measures(&seen, &run()).try_into().unwrap();
        assert_eq!(value(&kills), Some(1));
    }

    #[test]
    fn samples_that_ended_before_the_run_give_no_peak() {
        // The phone left at 8 s, 5 s before the run ended. Idle at 5 s is still read.
        let samples = [(2_000, GAME, 900), (6_000, GAME, 1_800)];
        let mut short = recording(&samples, &[]);
        short.samples_cover = Coverage {
            until_epoch_ms: START + 8_000.0,
            ended: None,
        };
        let window = RunWindow {
            idle_epoch_ms: Some(START + 5_000.0),
            ..run()
        };
        let [footprint, peak, _] = device_measures(&short, &window).try_into().unwrap();
        assert_eq!(value(&footprint), Some(1_800));
        assert!(footprint.items.is_empty());
        assert_eq!(peak.status, MeasureStatus::Missing);
        assert_eq!(
            peak.reason.as_deref(),
            Some(
                "the phone's samples ended 5 s before the run did, so the peak of the run is not \
                 known: it stopped sending"
            )
        );
    }

    #[test]
    fn the_peak_leaves_out_samples_outside_the_run_window() {
        // A 9,000 byte sample 3 s before the navigation is outside the slack, so the peak is the
        // largest one in the run.
        let samples = [
            (0, OTHER_TAB, 50),
            (0, GAME, 9_000),
            (4_000, GAME, 700),
            (11_000, GAME, 650),
            (13_000, GAME, 640),
        ];
        let window = RunWindow {
            navigated_epoch_ms: START + 3_000.0,
            ..run()
        };
        let [_, peak, _] = device_measures(&recording(&samples, &[]), &window)
            .try_into()
            .unwrap();
        assert_eq!(value(&peak), Some(700));
    }

    #[test]
    fn a_recording_names_the_model_its_os_version_and_its_memory_but_not_the_phone() {
        let (os, device) = phone_environment(&recording(&[], &[]));
        let os = os.unwrap();
        assert_eq!(os.name, OsName::Ios);
        assert_eq!(os.version.as_deref(), Some("26.6.2 (23G90)"));
        let device = device.unwrap();
        assert_eq!(device.key.as_deref(), Some("iphone-14-pro-max"));
        assert_eq!(device.name, "iPhone 14 Pro Max");
        assert_eq!(device.memory_bytes, Some(5_910_052_864));
        let mut unnamed = recording(&[], &[]);
        unnamed.phone.identifier = String::new();
        assert_eq!(phone_environment(&unnamed), (None, None));
        let mut no_version = recording(&[], &[]);
        no_version.phone.os_version = String::new();
        assert_eq!(phone_environment(&no_version).0.unwrap().version, None);
    }

    #[test]
    fn a_recording_with_no_safari_process_in_the_run_gives_missing_measures() {
        let samples = [(60_000, GAME, 500)];
        let measures = device_measures(&recording(&samples, &[]), &run());
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
    fn samples_that_ended_before_the_game_process_appeared_say_so() {
        let mut short = recording(&[], &[]);
        short.samples_cover = Coverage {
            until_epoch_ms: START,
            ended: Some("the tap closed".to_owned()),
        };
        let measures = device_measures(&short, &run());
        assert_eq!(
            measures[2].reason.as_deref(),
            Some("the phone's samples ended 13 s before the run did: the tap closed")
        );
    }

    #[test]
    fn a_run_that_never_went_idle_has_no_footprint_but_still_counts_kills() {
        let samples = [(2_000, GAME, 900), (12_500, GAME, 950)];
        let window = RunWindow {
            idle_epoch_ms: None,
            ..run()
        };
        let [footprint, peak, kills] = device_measures(&recording(&samples, &[]), &window)
            .try_into()
            .unwrap();
        assert_eq!(footprint.status, MeasureStatus::Missing);
        assert_eq!(value(&peak), Some(950));
        assert_eq!(value(&kills), Some(0));
    }
}
