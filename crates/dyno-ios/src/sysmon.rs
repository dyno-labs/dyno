//! The phone's sysmontap service, which Instruments' Activity Monitor reads too: each process's
//! physical footprint, about once a second.
//!
//! The service takes a config only with every process attribute and every system attribute that
//! the phone lists. With fewer, it closes the connection. So dyno asks for all of them, and reads
//! the ones it needs by their place in the lists. A message from the tap holds rows: one with the
//! system's values, in system attribute order, and one with the processes, keyed by pid, each in
//! process attribute order. A row can come alone, and a sample can leave out a process that is
//! still running.

use plist::{Dictionary, Value};

/// The Instruments service that sends the samples.
pub const SERVICE: &str = "com.apple.instruments.server.services.sysmontap";

/// How often the tap samples, in ms.
pub const INTERVAL_MS: u32 = 1_000;

/// The size of a memory page on every iPhone dyno knows: 16 KiB, as on every Apple 64-bit chip
/// since the A9. The tap gives the phone's memory in pages.
pub const PAGE_BYTES: u64 = 16_384;

const PID: &str = "pid";
const NAME: &str = "name";
const FOOTPRINT: &str = "physFootprint";
const MEMORY: &str = "physMemSize";

/// Where the values dyno reads are, in the rows of the attributes it asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Columns {
    pid: usize,
    name: usize,
    footprint: usize,
    memory: Option<usize>,
}

/// One process in a sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub footprint_bytes: u64,
}

/// What one message from the tap gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// The processes of a process row. None when the message had none.
    pub processes: Option<Vec<ProcessRow>>,
    /// The memory iOS manages, in bytes, from a system row.
    pub memory_bytes: Option<u64>,
}

impl Columns {
    /// The places of the values dyno reads in the phone's attribute lists. An error names an
    /// attribute that the phone does not list.
    pub fn find(process: &[String], system: &[String]) -> Result<Columns, String> {
        let at = |attributes: &[String], name: &str| attributes.iter().position(|a| a == name);
        let needed = |name: &str| {
            at(process, name).ok_or_else(|| format!("the phone's sysmontap has no {name}"))
        };
        Ok(Columns {
            pid: needed(PID)?,
            name: needed(NAME)?,
            footprint: needed(FOOTPRINT)?,
            memory: at(system, MEMORY),
        })
    }
}

/// The tap's config: every attribute, a sample each [`INTERVAL_MS`].
pub fn config(process: &[String], system: &[String]) -> Dictionary {
    let names = |attributes: &[String]| {
        Value::Array(attributes.iter().cloned().map(Value::String).collect())
    };
    let mut config = Dictionary::new();
    config.insert("ur".to_owned(), Value::Integer(INTERVAL_MS.into()));
    config.insert("bm".to_owned(), Value::Integer(0.into()));
    config.insert("procAttrs".to_owned(), names(process));
    config.insert("sysAttrs".to_owned(), names(system));
    config.insert("cpuUsage".to_owned(), Value::Boolean(true));
    config.insert("physFootprint".to_owned(), Value::Boolean(true));
    config.insert(
        "sampleInterval".to_owned(),
        Value::Integer((u64::from(INTERVAL_MS) * 1_000_000).into()),
    );
    config
}

/// What a message from the tap holds: every row of it. None for a message with no row, such as
/// the one that answers `start`.
pub fn read(data: &Value, columns: &Columns) -> Option<Reading> {
    let rows = match data {
        Value::Array(rows) => rows.as_slice(),
        row @ Value::Dictionary(_) => std::slice::from_ref(row),
        _ => return None,
    };
    let mut reading = Reading::default();
    let mut any = false;
    for row in rows.iter().filter_map(Value::as_dictionary) {
        if let Some(processes) = row.get("Processes").and_then(Value::as_dictionary) {
            reading.processes.get_or_insert_with(Vec::new).extend(
                processes
                    .values()
                    .filter_map(|values| process_row(values.as_array()?, columns)),
            );
            any = true;
        }
        if let Some(system) = row.get("System").and_then(Value::as_array) {
            reading.memory_bytes = columns
                .memory
                .and_then(|at| unsigned(system.get(at)?))
                .and_then(|pages| pages.checked_mul(PAGE_BYTES));
            any = true;
        }
    }
    any.then_some(reading)
}

fn process_row(values: &[Value], columns: &Columns) -> Option<ProcessRow> {
    Some(ProcessRow {
        pid: u32::try_from(unsigned(values.get(columns.pid)?)?).ok()?,
        name: values.get(columns.name)?.as_string()?.to_owned(),
        footprint_bytes: unsigned(values.get(columns.footprint)?)?,
    })
}

fn unsigned(value: &Value) -> Option<u64> {
    value.as_unsigned_integer().or_else(|| {
        value
            .as_signed_integer()
            .and_then(|signed| u64::try_from(signed).ok())
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A short version of the phone's lists, with the values dyno reads among others.
    pub(crate) fn attributes() -> (Vec<String>, Vec<String>) {
        let names = |names: &[&str]| names.iter().map(|&name| name.to_owned()).collect();
        (
            names(&["msgRecv", "name", "cpuUsage", "pid", "physFootprint"]),
            names(&["vmSwapOuts", "physMemSize", "vmFreeCount"]),
        )
    }

    const WEB_CONTENT_NAME: &str = "com.apple.WebKit.WebContent";

    pub(crate) fn columns() -> Columns {
        let (process, system) = attributes();
        Columns::find(&process, &system).unwrap()
    }

    /// A message with a system row and a process row of these (pid, name, bytes).
    pub(crate) fn message(processes: &[(u64, &str, u64)], memory_pages: u64) -> Value {
        let mut system = Dictionary::new();
        system.insert(
            "System".to_owned(),
            Value::Array(vec![
                Value::Integer(1.into()),
                Value::Integer(memory_pages.into()),
                Value::Integer(2.into()),
            ]),
        );
        system.insert("Type".to_owned(), Value::Integer(41.into()));
        let mut by_pid = Dictionary::new();
        for &(pid, name, bytes) in processes {
            by_pid.insert(
                pid.to_string(),
                Value::Array(vec![
                    Value::Integer(7.into()),
                    Value::String(name.to_owned()),
                    Value::Real(1.5),
                    Value::Integer(pid.into()),
                    Value::Integer(bytes.into()),
                ]),
            );
        }
        let mut process_row = Dictionary::new();
        process_row.insert("Processes".to_owned(), Value::Dictionary(by_pid));
        process_row.insert("Type".to_owned(), Value::Integer(5.into()));
        Value::Array(vec![
            Value::Dictionary(system),
            Value::Dictionary(process_row),
        ])
    }

    #[test]
    fn every_row_of_a_message_is_read_the_process_row_too() {
        let reading = read(
            &message(
                &[
                    (412, "com.apple.WebKit.WebContent", 337_626_224),
                    (60, "SpringBoard", 9),
                ],
                360_717,
            ),
            &columns(),
        )
        .unwrap();
        let mut processes = reading.processes.unwrap();
        processes.sort_by_key(|row| row.pid);
        assert_eq!(
            processes,
            [
                ProcessRow {
                    pid: 60,
                    name: "SpringBoard".to_owned(),
                    footprint_bytes: 9
                },
                ProcessRow {
                    pid: 412,
                    name: "com.apple.WebKit.WebContent".to_owned(),
                    footprint_bytes: 337_626_224
                },
            ]
        );
        assert_eq!(reading.memory_bytes, Some(360_717 * 16_384));
    }

    #[test]
    fn a_message_with_no_row_gives_nothing() {
        let mut ack = Dictionary::new();
        ack.insert("$class".to_owned(), Value::Integer(8.into()));
        assert_eq!(read(&Value::Dictionary(ack), &columns()), None);
        assert_eq!(read(&Value::String("ok".to_owned()), &columns()), None);
        assert_eq!(read(&Value::Array(Vec::new()), &columns()), None);
    }

    #[test]
    fn a_system_row_alone_gives_the_memory_and_no_processes() {
        let Value::Array(rows) = message(&[], 100) else {
            unreachable!()
        };
        let reading = read(&rows[0], &columns()).unwrap();
        assert_eq!(reading.processes, None);
        assert_eq!(reading.memory_bytes, Some(100 * 16_384));
    }

    #[test]
    fn a_footprint_below_zero_leaves_the_process_out() {
        let Value::Array(mut rows) = message(&[(412, WEB_CONTENT_NAME, 1)], 1) else {
            unreachable!()
        };
        let row = rows[1].as_dictionary_mut().unwrap();
        let processes = row["Processes"].as_dictionary_mut().unwrap();
        processes["412"].as_array_mut().unwrap()[4] = Value::Integer((-1_i64).into());
        let reading = read(&Value::Array(rows), &columns()).unwrap();
        assert_eq!(reading.processes, Some(Vec::new()));
    }

    #[test]
    fn a_process_row_that_cannot_be_read_is_left_out() {
        let mut by_pid = Dictionary::new();
        by_pid.insert(
            "1".to_owned(),
            Value::Array(vec![Value::String("short".to_owned())]),
        );
        let mut row = Dictionary::new();
        row.insert("Processes".to_owned(), Value::Dictionary(by_pid));
        let reading = read(&Value::Dictionary(row), &columns()).unwrap();
        assert_eq!(reading.processes, Some(Vec::new()));
    }

    #[test]
    fn the_columns_need_a_pid_a_name_and_a_footprint() {
        let (process, system) = attributes();
        let found = Columns::find(&process, &system).unwrap();
        assert_eq!(
            found,
            Columns {
                pid: 3,
                name: 1,
                footprint: 4,
                memory: Some(1)
            }
        );
        let error = Columns::find(&process[..4], &system).unwrap_err();
        assert_eq!(error, "the phone's sysmontap has no physFootprint");
        assert_eq!(Columns::find(&process, &[]).unwrap().memory, None);
    }

    #[test]
    fn the_config_asks_for_every_attribute_once_a_second() {
        let (process, system) = attributes();
        let config = config(&process, &system);
        assert_eq!(config["procAttrs"].as_array().unwrap().len(), process.len());
        assert_eq!(config["sysAttrs"].as_array().unwrap().len(), system.len());
        assert_eq!(config["ur"].as_unsigned_integer(), Some(1_000));
        assert_eq!(
            config["sampleInterval"].as_unsigned_integer(),
            Some(1_000_000_000)
        );
    }
}
