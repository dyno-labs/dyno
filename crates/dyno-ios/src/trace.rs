//! An Activity Monitor trace, read from what `xcrun xctrace export` gives: the table of contents,
//! for when the recording started and on which device, and the `activity-monitor-process-live`
//! table, for each process's physical footprint over time. [`device_measures`] turns a trace and
//! a run's times into the device measures, and [`trace_timeline`] into a run's timeline.
//!
//! `--device` no longer records with xctrace: it reads the phone itself ([`crate::recorder`]).
//! This reader stays for traces that other hosts record, such as a device cloud's phone, where
//! Appium's `mobile: startPerfRecord` with the Activity Monitor template gives the same table.
//!
//! The table writes each value once with an `id`, and later as `<x ref="id"/>`. A row holds one
//! element per column, in the schema's column order. A process gets a row only when it does
//! work, so a quiet process can be missing from a sample and still run.

use std::collections::HashMap;

use dyno_schema::devices::apple_os;
use dyno_schema::environment::{EnvironmentDevice, EnvironmentOs};
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, Uncertainty,
};
use dyno_schema::timeline::{Timeline, TimelineMethod};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::{
    Footprint, KernelEvent, PEAK_ITEM, RunWindow, SLACK_MS, WEB_CONTENT, missing_device_measures,
    required, saturating,
};

/// The table that holds each process's footprint over time.
pub const PROCESS_TABLE: &str = "activity-monitor-process-live";

/// The xpath of that table in an export.
pub const PROCESS_TABLE_XPATH: &str =
    "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"activity-monitor-process-live\"]";

const START_COLUMN: &str = "start";
const PROCESS_COLUMN: &str = "process";
const FOOTPRINT_COLUMN: &str = "memory-physical-footprint";

/// Nanoseconds in a millisecond, for the table's start times.
const NANOS_PER_MILLI: f64 = 1e6;

/// Why an export could not be read.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TraceError {
    #[error("the trace export is not XML: {0}")]
    Xml(String),
    #[error("the trace export has no {0}")]
    Missing(&'static str),
}

/// The device a trace recorded, from its table of contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceDevice {
    pub name: String,
    pub model: String,
    pub os_version: String,
    pub memory_bytes: Option<u64>,
}

/// One process's footprint at one sample.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Milliseconds since the recording started.
    pub ms: f64,
    /// The process's name, such as `com.apple.WebKit.WebContent`.
    pub process: String,
    pub pid: u32,
    pub footprint_bytes: u64,
}

/// A read trace.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    /// When the recording started, in ms since the Unix epoch.
    pub started_epoch_ms: f64,
    pub device: TraceDevice,
    pub samples: Vec<Sample>,
}

/// An element's own values: what a later `ref` to it gives.
#[derive(Clone, Debug, Default)]
struct Value {
    fmt: String,
    text: String,
}

/// An element that is open while the reader is inside it.
struct Open {
    name: String,
    id: Option<String>,
    reference: Option<String>,
    value: Value,
}

/// When the recording started and on which device, from `xctrace export --toc`.
pub fn read_toc(xml: &str) -> Result<(f64, TraceDevice), TraceError> {
    let mut reader = Reader::from_str(xml);
    let mut device = None;
    let mut start_date = None;
    let mut in_start_date = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.name().as_ref() == "device" && device.is_none() =>
            {
                let attributes = attributes(&element)?;
                let get = |key: &str| attributes.get(key).cloned().unwrap_or_default();
                device = Some(TraceDevice {
                    name: get("name"),
                    model: get("model"),
                    os_version: get("os-version"),
                    memory_bytes: get("memory-size").parse().ok(),
                });
            }
            Ok(Event::Start(element)) => in_start_date = element.name().as_ref() == "start-date",
            Ok(Event::Text(text)) if in_start_date => {
                start_date = epoch_ms(text.xml10_content().trim());
                in_start_date = false;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(TraceError::Xml(error.to_string())),
        }
    }
    let started = start_date.ok_or(TraceError::Missing("start date"))?;
    Ok((started, device.ok_or(TraceError::Missing("device"))?))
}

/// Every row of the process table, from `xctrace export --xpath PROCESS_TABLE_XPATH`.
pub fn read_samples(xml: &str) -> Result<Vec<Sample>, TraceError> {
    let mut reader = Reader::from_str(xml);
    let mut by_id: HashMap<String, Value> = HashMap::new();
    let mut columns: Vec<String> = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    // The depth of the row being read, and its cells so far.
    let mut row: Option<(usize, Vec<Value>)> = None;
    let mut rows: Vec<Vec<Value>> = Vec::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|error| TraceError::Xml(error.to_string()))?;
        let (element, closes) = match event {
            Event::Start(element) => (Some(element), false),
            Event::Empty(element) => (Some(element), true),
            Event::End(_) => (None, true),
            Event::Text(text) => {
                if let Some(top) = open.last_mut() {
                    top.value.text.push_str(&text.xml10_content());
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        if let Some(element) = element {
            let attributes = attributes(&element)?;
            let name = element.name().as_ref().to_owned();
            if name == "row" {
                row = Some((open.len(), Vec::new()));
            }
            open.push(Open {
                name,
                id: attributes.get("id").cloned(),
                reference: attributes.get("ref").cloned(),
                value: Value {
                    fmt: attributes.get("fmt").cloned().unwrap_or_default(),
                    text: String::new(),
                },
            });
        }
        if !closes {
            continue;
        }
        let Some(closed) = open.pop() else {
            continue;
        };
        let value = match &closed.reference {
            Some(reference) => by_id.get(reference).cloned().unwrap_or_default(),
            None => closed.value.clone(),
        };
        if let Some(id) = &closed.id {
            by_id.insert(id.clone(), value.clone());
        }
        if closed.name == "mnemonic" {
            columns.push(value.text.trim().to_owned());
        }
        match &mut row {
            Some((depth, _)) if closed.name == "row" && open.len() == *depth => {
                if let Some((_, cells)) = row.take() {
                    rows.push(cells);
                }
            }
            Some((depth, cells)) if open.len() == *depth + 1 => cells.push(value),
            _ => {}
        }
    }
    let column = |name: &'static str| {
        columns
            .iter()
            .position(|column| column == name)
            .ok_or(TraceError::Missing(name))
    };
    let (start, process, footprint) = (
        column(START_COLUMN)?,
        column(PROCESS_COLUMN)?,
        column(FOOTPRINT_COLUMN)?,
    );
    Ok(rows
        .iter()
        .filter_map(|cells| {
            let (name, pid) = process_label(&cells.get(process)?.fmt)?;
            Some(Sample {
                ms: cells.get(start)?.text.trim().parse::<f64>().ok()? / NANOS_PER_MILLI,
                process: name,
                pid,
                footprint_bytes: cells.get(footprint)?.text.trim().parse().ok()?,
            })
        })
        .collect())
}

/// An element's attributes, unescaped.
fn attributes(element: &BytesStart<'_>) -> Result<HashMap<String, String>, TraceError> {
    element
        .attributes()
        .map(|attribute| {
            let attribute = attribute.map_err(|error| TraceError::Xml(error.to_string()))?;
            let key = attribute.key.as_ref().to_owned();
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|error| TraceError::Xml(error.to_string()))?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

/// A process label such as `com.apple.WebKit.WebContent (412)`, split into its name and pid.
fn process_label(label: &str) -> Option<(String, u32)> {
    let (name, pid) = label.strip_suffix(')')?.rsplit_once(" (")?;
    Some((name.to_owned(), pid.parse().ok()?))
}

/// Milliseconds since the Unix epoch of a time such as `2026-11-08T14:18:40.970+11:00`, as a
/// trace's table of contents writes it. None for any other form.
pub fn epoch_ms(text: &str) -> Option<f64> {
    let (date, rest) = text.split_once('T')?;
    let mut date_parts = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (
        date_parts.next()?.ok()?,
        date_parts.next()?.ok()?,
        date_parts.next()?.ok()?,
    );
    let (time, offset_minutes) = match rest.rfind(['+', '-']) {
        Some(at) => {
            let (time, offset) = rest.split_at(at);
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let (hours, minutes) = offset[1..].split_once(':')?;
            (
                time,
                sign * (hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?),
            )
        }
        None => (rest.strip_suffix('Z')?, 0),
    };
    let mut time_parts = time.splitn(3, ':');
    let hours = time_parts.next()?.parse::<i64>().ok()?;
    let minutes = time_parts.next()?.parse::<i64>().ok()?;
    let seconds = time_parts.next()?.parse::<f64>().ok()?;
    let days = days_from_civil(year, month, day);
    let whole_minutes = days * 24 * 60 + hours * 60 + minutes - offset_minutes;
    Some(whole_minutes as f64 * 60_000.0 + seconds * 1_000.0)
}

/// Days from 1970-01-01 to a date in the proleptic Gregorian calendar (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// How long before the run ended the game's process may last appear and still count as alive.
/// A busy process gets a sample every second.
const KILL_GAP_MS: f64 = 3_000.0;

/// The device measures of a run, from its trace.
///
/// The game's process is the WebContent process with the largest footprint during the run.
/// `device.phys_footprint` is its first footprint at or after the frame went idle, with its peak
/// during the run as an item. `device.phys_footprint_peak` is that peak as a measure of its own.
/// `device.kills` is 1 when the process left the trace while the run was still on, as a process
/// that iOS kills does. The evidence names the model and iOS version, never the phone's own name,
/// which is often its owner's.
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
                key: PEAK_ITEM.to_owned(),
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
    let footprint_peak = Measure {
        value: Some(MeasureValue::Integer(saturating(peak))),
        evidence: evidence.clone(),
        uncertainty: Uncertainty::exact(),
        ..required(Measure::blank(
            MeasureId::DevicePhysFootprintPeak,
            MeasureStatus::Measured,
        ))
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
    vec![footprint, footprint_peak, kills]
}

/// About how often the Activity Monitor template samples each process.
pub const XCTRACE_EVERY_MS: u32 = 1_000;

/// The timeline of Safari's processes during the run, from a trace, as
/// [`crate::device_timeline`] gives it from a recording. A trace has no kernel log, so the marks
/// are the run's steps only.
pub fn trace_timeline(trace: &Trace, run: &RunWindow) -> Option<Timeline> {
    let footprints = trace.samples.iter().map(|sample| Footprint {
        epoch_ms: trace.started_epoch_ms + sample.ms,
        pid: sample.pid,
        process: &sample.process,
        bytes: sample.footprint_bytes,
    });
    crate::safari_timeline(
        footprints,
        std::iter::empty::<KernelEvent>(),
        run,
        TimelineMethod::Xctrace,
        XCTRACE_EVERY_MS,
    )
}

/// The OS and the device that a trace recorded: the model, its OS version and its memory, never
/// the device's own name. Both are None when the trace does not name the model.
pub fn trace_environment(
    device: &TraceDevice,
) -> (Option<EnvironmentOs>, Option<EnvironmentDevice>) {
    let Some(environment_device) =
        EnvironmentDevice::of_apple_model(&device.model, device.memory_bytes)
    else {
        return (None, None);
    };
    let version = device.os_version.trim();
    let os = EnvironmentOs {
        name: apple_os(&device.model),
        version: (!version.is_empty()).then(|| version.to_owned()),
    };
    (Some(os), Some(environment_device))
}

#[cfg(test)]
pub(crate) mod tests {
    use dyno_schema::environment::OsName;
    use dyno_schema::runner::RunMark;
    use dyno_schema::timeline::ProcessRole;

    use super::*;

    /// The process table of an export, with these rows: (ms since the start, process label,
    /// footprint bytes). A value seen before is written as a `ref`, as xctrace does.
    pub(crate) fn process_table(rows: &[(u64, &str, u64)]) -> String {
        let mut ids: HashMap<String, usize> = HashMap::new();
        let mut cell = |name: &str, fmt: &str, text: &str| {
            let key = format!("{name}|{fmt}|{text}");
            if let Some(id) = ids.get(&key) {
                return format!(r#"<{name} ref="{id}"/>"#);
            }
            let id = ids.len() + 1;
            ids.insert(key, id);
            format!(r#"<{name} id="{id}" fmt="{fmt}">{text}</{name}>"#)
        };
        let body: String = rows
            .iter()
            .map(|(ms, label, bytes)| {
                let nanos = (ms * 1_000_000).to_string();
                let pid = process_label(label).map_or(0, |(_, pid)| pid).to_string();
                format!(
                    "<row>{}{}{}<sentinel/>{}</row>\n",
                    cell("start-time", &nanos, &nanos),
                    cell("process", label, ""),
                    cell("pid", &pid, &pid),
                    cell("size-in-bytes", &bytes.to_string(), &bytes.to_string()),
                )
            })
            .collect();
        let schema: String = [
            "start",
            "process",
            "pid",
            "cpu-percent",
            "memory-physical-footprint",
        ]
        .iter()
        .map(|name| format!("<col><mnemonic>{name}</mnemonic></col>"))
        .collect();
        format!(
            "<?xml version=\"1.0\"?>\n<trace-query-result>\n<node xpath='//trace-toc[1]/run[1]/data[1]/table[10]'><schema name=\"{PROCESS_TABLE}\">{schema}</schema>\n{body}</node></trace-query-result>\n"
        )
    }

    pub(crate) const TOC: &str = r#"<?xml version="1.0"?>
<trace-toc>
    <run number="1">
        <info>
            <target>
                <device platform="iOS" model="iPhone 14 Pro Max" memory-size="6442450944" name="Dev’s iPhone" os-version="26.6.2 (23G100)" uuid="00008120-000000000000002E"/>
                <all-processes/>
            </target>
            <summary>
                <start-date>2026-11-08T14:18:40.970+11:00</start-date>
                <end-date>2026-11-08T14:18:45.876+11:00</end-date>
            </summary>
        </info>
        <processes>
            <process name="kernel" pid="0" path="/System/Library/Kernels/kernel"/>
        </processes>
    </run>
</trace-toc>
"#;

    #[test]
    fn the_toc_gives_the_start_and_the_device() {
        let (started, device) = read_toc(TOC).unwrap();
        assert_eq!(started, epoch_ms("2026-11-08T03:18:40.970Z").unwrap());
        assert_eq!(
            device,
            TraceDevice {
                name: "Dev’s iPhone".to_owned(),
                model: "iPhone 14 Pro Max".to_owned(),
                os_version: "26.6.2 (23G100)".to_owned(),
                memory_bytes: Some(6_442_450_944),
            }
        );
    }

    #[test]
    fn a_toc_without_a_start_date_or_device_is_refused() {
        assert_eq!(
            read_toc("<trace-toc><device name='x'/></trace-toc>"),
            Err(TraceError::Missing("start date"))
        );
        assert_eq!(
            read_toc("<trace-toc><start-date>2026-11-08T03:18:40Z</start-date></trace-toc>"),
            Err(TraceError::Missing("device"))
        );
    }

    #[test]
    fn each_row_is_a_sample_with_refs_resolved() {
        let xml = process_table(&[
            (0, "com.apple.WebKit.WebContent (412)", 100),
            (0, "SpringBoard (60)", 900),
            (1_000, "com.apple.WebKit.WebContent (412)", 100),
            (2_000, "com.apple.WebKit.WebContent (412)", 300),
        ]);
        let samples = read_samples(&xml).unwrap();
        assert_eq!(samples.len(), 4);
        assert_eq!(
            samples[2],
            Sample {
                ms: 1_000.0,
                process: "com.apple.WebKit.WebContent".to_owned(),
                pid: 412,
                footprint_bytes: 100,
            }
        );
        assert_eq!(samples[1].process, "SpringBoard");
        assert_eq!(samples[3].footprint_bytes, 300);
    }

    #[test]
    fn a_table_without_the_footprint_column_is_refused() {
        let xml = "<schema><col><mnemonic>start</mnemonic></col><col><mnemonic>process</mnemonic></col></schema>";
        assert_eq!(
            read_samples(xml),
            Err(TraceError::Missing(FOOTPRINT_COLUMN))
        );
    }

    #[test]
    fn text_that_is_not_xml_is_refused() {
        assert!(matches!(read_samples("<a></b>"), Err(TraceError::Xml(_))));
        assert!(matches!(read_toc("<a></b>"), Err(TraceError::Xml(_))));
    }

    #[test]
    fn a_time_with_an_offset_is_placed_in_utc() {
        assert_eq!(epoch_ms("1970-01-01T00:00:01.5Z"), Some(1_500.0));
        assert_eq!(
            epoch_ms("2026-11-08T14:18:40.970+11:00"),
            epoch_ms("2026-11-08T03:18:40.970Z")
        );
        assert_eq!(
            epoch_ms("2026-11-07T22:18:40.970-05:00"),
            epoch_ms("2026-11-08T03:18:40.970Z")
        );
        assert_eq!(epoch_ms("2000-03-01T00:00:00Z"), Some(951_868_800_000.0));
        assert_eq!(epoch_ms("not a time"), None);
    }

    const START: f64 = 1_000_000.0;
    const GAME: &str = "com.apple.WebKit.WebContent (412)";
    const OTHER_TAB: &str = "com.apple.WebKit.WebContent (377)";

    /// A trace that started at START, with these samples: (ms since START, process, bytes).
    fn trace(samples: &[(u64, &str, u64)]) -> Trace {
        let xml = process_table(samples);
        Trace {
            started_epoch_ms: START,
            device: TraceDevice {
                name: "Dev’s iPhone".to_owned(),
                model: "iPhone 14 Pro Max".to_owned(),
                os_version: "26.6.2".to_owned(),
                memory_bytes: Some(6_442_450_944),
            },
            samples: read_samples(&xml).unwrap(),
        }
    }

    /// The game loads from 1 s, goes idle at 10.4 s, and the run ends at 13 s. The trace's tests
    /// have no activation.
    fn run() -> RunWindow {
        RunWindow {
            navigated_epoch_ms: START + 1_000.0,
            idle_epoch_ms: Some(START + 10_400.0),
            activated_epoch_ms: None,
            ended_epoch_ms: START + 13_000.0,
        }
    }

    #[test]
    fn a_trace_gives_the_same_timeline_with_the_runs_steps_only() {
        let samples = [
            (1_000, GAME, 80),
            (1_000, "com.apple.WebKit.GPU (90)", 20),
            (1_000, "MobileSafari (88)", 30),
            (5_000, GAME, 400),
            (5_000, OTHER_TAB, 50),
        ];
        let timeline = trace_timeline(&trace(&samples), &run()).unwrap();
        assert_eq!(timeline.method, TimelineMethod::Xctrace);
        assert_eq!(timeline.every_ms, XCTRACE_EVERY_MS);
        let marks: Vec<RunMark> = timeline.marks.iter().map(|mark| mark.mark).collect();
        assert_eq!(marks, [RunMark::Navigated, RunMark::Idle]);
        let roles: Vec<(u32, ProcessRole)> = timeline
            .processes
            .iter()
            .map(|process| (process.pid, process.role))
            .collect();
        assert_eq!(
            roles,
            [
                (88, ProcessRole::Browser),
                (90, ProcessRole::Gpu),
                (377, ProcessRole::Other),
                (412, ProcessRole::Game),
            ]
        );
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
        let [footprint, peak, kills] = device_measures(&trace(&samples), &run())
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
        let [footprint, peak, kills] = device_measures(&trace(&samples), &run())
            .try_into()
            .unwrap();
        assert_eq!(footprint.status, MeasureStatus::Missing);
        // The peak in the run window is still read when the process died before idle.
        assert_eq!(value(&peak), Some(1_800));
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
        let [footprint, _, kills] = device_measures(&trace(&samples), &window)
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
        let [_, peak, _] = device_measures(&trace(&samples), &window)
            .try_into()
            .unwrap();
        assert_eq!(value(&peak), Some(700));
    }

    #[test]
    fn a_trace_names_the_model_its_os_version_and_its_memory_but_not_the_phone() {
        let (os, device) = trace_environment(&trace(&[]).device);
        let os = os.unwrap();
        assert_eq!(os.name, OsName::Ios);
        assert_eq!(os.version.as_deref(), Some("26.6.2"));
        let device = device.unwrap();
        assert_eq!(device.key.as_deref(), Some("iphone-14-pro-max"));
        assert_eq!(device.name, "iPhone 14 Pro Max");
        assert_eq!(device.memory_bytes, Some(6_442_450_944));
        let unnamed = TraceDevice {
            model: String::new(),
            ..trace(&[]).device
        };
        assert_eq!(trace_environment(&unnamed), (None, None));
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
        let [footprint, peak, kills] = device_measures(&trace(&samples), &window)
            .try_into()
            .unwrap();
        assert_eq!(footprint.status, MeasureStatus::Missing);
        assert_eq!(value(&peak), Some(950));
        assert_eq!(value(&kills), Some(0));
    }
}
