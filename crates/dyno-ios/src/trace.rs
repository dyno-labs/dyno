//! An Activity Monitor trace, read from what `xcrun xctrace export` gives: the table of contents,
//! for when the recording started and on which device, and the `activity-monitor-process-live`
//! table, for each process's physical footprint over time.
//!
//! The table writes each value once with an `id`, and later as `<x ref="id"/>`. A row holds one
//! element per column, in the schema's column order. A process gets a row only when it does
//! work, so a quiet process can be missing from a sample and still run.

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

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

#[cfg(test)]
pub(crate) mod tests {
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
}
