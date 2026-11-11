//! A recording of the phone: the footprint of each of Safari's processes about once a second,
//! from sysmontap, and the kernel's memory lines, from the log. Each is stamped with this
//! machine's clock as it arrives, which is the clock the served run keeps too.
//!
//! [`record`] reads the two sources at once until it is told to stop. A source that fails, or
//! that sends nothing for [`SILENCE_MS`] before the stop, as one does when the phone leaves the
//! network, covers the recording only up to the last thing it sent.

use std::cell::RefCell;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use dyno_schema::devices::model_name;
use serde::Serialize;
use tokio::sync::oneshot;

use crate::SAFARI_PROCESSES;
use crate::syslog::{MemoryLine, memory_line};
use crate::sysmon::Reading;

/// How long a source may send nothing before the stop and still cover the recording to the end.
/// Samples came at least every 2.7 s over Wi-Fi, and the log sends hundreds of lines a second.
pub const SILENCE_MS: f64 = 10_000.0;

/// The phone a recording is of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Phone {
    /// Apple's model identifier, such as `iPhone15,3`.
    pub identifier: String,
    /// The iOS version and build, such as `26.6.2 (23G90)`.
    pub os_version: String,
    /// How dyno reaches it.
    pub link: Link,
    /// Its UDID, for devicectl. Never in a result or a saved recording.
    #[serde(skip)]
    pub udid: Option<String>,
}

/// How dyno reaches a phone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    /// A cable, through usbmuxd.
    Usb,
    /// The network: through usbmuxd on macOS, or straight to the phone's address.
    Network,
}

impl Phone {
    /// The model's name, such as "iPhone 14 Pro Max", or its identifier when the catalogue does
    /// not list it.
    pub fn model(&self) -> String {
        model_name(&self.identifier)
    }
}

/// One of Safari's processes in one sample.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Sample {
    /// When the sample arrived, in ms since the Unix epoch.
    pub epoch_ms: f64,
    pub pid: u32,
    /// The process's name, one of [`SAFARI_PROCESSES`], such as `com.apple.WebKit.WebContent`.
    pub process: String,
    pub footprint_bytes: u64,
}

/// A kernel memory line, and when it arrived.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LoggedLine {
    pub epoch_ms: f64,
    #[serde(flatten)]
    pub line: MemoryLine,
}

/// How much of the recording a source covers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Coverage {
    /// Until when, in ms since the Unix epoch: the stop, or the last thing the source sent.
    pub until_epoch_ms: f64,
    /// Why the source ended before the stop. None when it covers the recording to the end.
    pub ended: Option<String>,
}

/// A finished recording.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Recording {
    pub phone: Phone,
    /// The memory iOS manages, from the samples.
    pub memory_bytes: Option<u64>,
    pub started_epoch_ms: f64,
    pub stopped_epoch_ms: f64,
    /// Every sample of each of Safari's processes ([`SAFARI_PROCESSES`]): its page processes
    /// (`com.apple.WebKit.WebContent`), its GPU and network processes and the app.
    pub samples: Vec<Sample>,
    pub samples_cover: Coverage,
    /// Every kernel memory line, about any process.
    pub memory_lines: Vec<LoggedLine>,
    pub log_covers: Coverage,
}

impl Recording {
    /// Saves the recording as JSON, to read the footprint over time or the kernel's lines.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        fs::write(path, json + "\n")
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }
}

/// Where the samples come from.
pub(crate) trait SampleFeed {
    /// The next message's reading. None for a message with nothing to read.
    async fn next(&mut self) -> Result<Option<Reading>, String>;
}

/// Where the log's lines come from.
pub(crate) trait LineFeed {
    async fn next(&mut self) -> Result<String, String>;
}

/// The time now, in ms since the Unix epoch.
pub(crate) fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1_000.0)
}

/// What a recording has taken so far.
#[derive(Default)]
struct Taken {
    samples: Vec<Sample>,
    memory_lines: Vec<LoggedLine>,
    memory_bytes: Option<u64>,
    last_sample: Option<f64>,
    last_line: Option<f64>,
    samples_failed: Option<String>,
    log_failed: Option<String>,
}

/// Reads both sources, and runs `beside` alongside them, until `stop` says to stop or the
/// sources and `beside` all end.
pub(crate) async fn record(
    phone: Phone,
    mut samples: impl SampleFeed,
    mut lines: impl LineFeed,
    beside: impl Future<Output = ()>,
    stop: oneshot::Receiver<()>,
) -> Recording {
    let started = epoch_ms();
    let taken = RefCell::new(Taken::default());
    let sampling = async {
        loop {
            match samples.next().await {
                Ok(reading) => taken.borrow_mut().reading(reading),
                Err(why) => {
                    taken.borrow_mut().samples_failed = Some(why);
                    return;
                }
            }
        }
    };
    let logging = async {
        loop {
            match lines.next().await {
                Ok(line) => taken.borrow_mut().line(&line),
                Err(why) => {
                    taken.borrow_mut().log_failed = Some(why);
                    return;
                }
            }
        }
    };
    tokio::select! {
        _ = stop => {}
        _ = async { tokio::join!(sampling, logging, beside) } => {}
    }
    taken.into_inner().finish(phone, started, epoch_ms())
}

impl Taken {
    fn reading(&mut self, reading: Option<Reading>) {
        let now = epoch_ms();
        self.last_sample = Some(now);
        let Some(reading) = reading else {
            return;
        };
        self.memory_bytes = self.memory_bytes.or(reading.memory_bytes);
        let pages = reading.processes.into_iter().flatten();
        self.samples.extend(
            pages
                .filter(|row| SAFARI_PROCESSES.iter().any(|(name, _)| *name == row.name))
                .map(|row| Sample {
                    epoch_ms: now,
                    pid: row.pid,
                    process: row.name,
                    footprint_bytes: row.footprint_bytes,
                }),
        );
    }

    fn line(&mut self, line: &str) {
        let now = epoch_ms();
        self.last_line = Some(now);
        self.memory_lines
            .extend(line.lines().filter_map(memory_line).map(|line| LoggedLine {
                epoch_ms: now,
                line,
            }));
    }

    fn finish(self, phone: Phone, started: f64, stopped: f64) -> Recording {
        Recording {
            phone,
            memory_bytes: self.memory_bytes,
            started_epoch_ms: started,
            stopped_epoch_ms: stopped,
            samples: self.samples,
            samples_cover: coverage(
                "sample",
                self.last_sample,
                self.samples_failed,
                started,
                stopped,
            ),
            memory_lines: self.memory_lines,
            log_covers: coverage(
                "log line",
                self.last_line,
                self.log_failed,
                started,
                stopped,
            ),
        }
    }
}

/// What a source covers, from the last thing it sent and why it failed, if it did.
fn coverage(
    what: &str,
    last: Option<f64>,
    failed: Option<String>,
    started: f64,
    stopped: f64,
) -> Coverage {
    let last = last.unwrap_or(started);
    match failed {
        Some(why) => Coverage {
            until_epoch_ms: last,
            ended: Some(why),
        },
        None if stopped - last > SILENCE_MS => Coverage {
            until_epoch_ms: last,
            ended: Some(format!(
                "no {what} came in the last {:.0} s",
                (stopped - last) / 1_000.0
            )),
        },
        None => Coverage {
            until_epoch_ms: stopped,
            ended: None,
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::*;
    use crate::WEB_CONTENT;
    use crate::sysmon::ProcessRow;

    pub(crate) fn phone() -> Phone {
        Phone {
            identifier: "iPhone15,3".to_owned(),
            os_version: "26.6.2 (23G90)".to_owned(),
            link: Link::Network,
            udid: Some("00008120-000000000000001E".to_owned()),
        }
    }

    /// Gives these readings, then ends with `end`, or waits for ever.
    struct Readings(VecDeque<Option<Reading>>, Option<&'static str>);

    impl SampleFeed for Readings {
        async fn next(&mut self) -> Result<Option<Reading>, String> {
            match (self.0.pop_front(), self.1) {
                (Some(reading), _) => Ok(reading),
                (None, Some(end)) => Err(end.to_owned()),
                (None, None) => std::future::pending().await,
            }
        }
    }

    /// Gives these lines, then ends with `end`, or waits for ever.
    struct Lines(VecDeque<&'static str>, Option<&'static str>);

    impl LineFeed for Lines {
        async fn next(&mut self) -> Result<String, String> {
            match (self.0.pop_front(), self.1) {
                (Some(line), _) => Ok(line.to_owned()),
                (None, Some(end)) => Err(end.to_owned()),
                (None, None) => std::future::pending().await,
            }
        }
    }

    fn page(pid: u32, bytes: u64) -> ProcessRow {
        ProcessRow {
            pid,
            name: WEB_CONTENT.to_owned(),
            footprint_bytes: bytes,
        }
    }

    fn run<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_recording_keeps_safaris_processes_the_memory_and_the_kernel_memory_lines() {
        let readings = Readings(
            VecDeque::from([
                None,
                Some(Reading {
                    processes: Some(vec![
                        page(412, 300),
                        ProcessRow {
                            pid: 60,
                            name: "SpringBoard".to_owned(),
                            footprint_bytes: 9,
                        },
                        ProcessRow {
                            pid: 88,
                            name: "MobileSafari".to_owned(),
                            footprint_bytes: 40,
                        },
                    ]),
                    memory_bytes: Some(6_000),
                }),
                Some(Reading {
                    processes: None,
                    memory_bytes: None,
                }),
            ]),
            Some("the tap closed"),
        );
        let lines = Lines(
            VecDeque::from([
                "kernel[0] <Notice>: vm: segments queued",
                "kernel[0] <Notice>: memorystatus: killing process 412 \
                 [com.apple.WebKit.WebContent] in high band FOREGROUND (100) - x\nnext",
            ]),
            Some("the relay closed"),
        );
        let (_stop, stop) = oneshot::channel();
        let recording = run(record(phone(), readings, lines, async {}, stop));
        // Safari's processes stay, with their names; SpringBoard is not Safari's.
        let kept: Vec<(u32, &str, u64)> = recording
            .samples
            .iter()
            .map(|sample| (sample.pid, sample.process.as_str(), sample.footprint_bytes))
            .collect();
        assert_eq!(kept, [(412, WEB_CONTENT, 300), (88, "MobileSafari", 40)]);
        assert_eq!(recording.memory_bytes, Some(6_000));
        assert_eq!(recording.memory_lines.len(), 1);
        assert_eq!(recording.memory_lines[0].line.pid, 412);
        assert_eq!(
            recording.samples_cover.ended.as_deref(),
            Some("the tap closed")
        );
        assert_eq!(
            recording.log_covers.ended.as_deref(),
            Some("the relay closed")
        );
        assert!(recording.samples_cover.until_epoch_ms >= recording.started_epoch_ms);
    }

    #[test]
    fn sources_that_still_send_cover_the_recording_to_the_stop() {
        let readings = Readings(VecDeque::from([None]), None);
        let lines = Lines(VecDeque::from(["a line"]), None);
        let (stopper, stop) = oneshot::channel();
        let beside = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            stopper.send(()).unwrap();
        };
        let recording = run(record(phone(), readings, lines, beside, stop));
        assert_eq!(recording.samples_cover.ended, None);
        assert_eq!(
            recording.samples_cover.until_epoch_ms,
            recording.stopped_epoch_ms
        );
        assert_eq!(recording.log_covers.ended, None);
        assert!(recording.stopped_epoch_ms >= recording.started_epoch_ms + 20.0);
    }

    #[test]
    fn a_source_quiet_for_too_long_before_the_stop_ends_at_its_last_message() {
        let cover = coverage("sample", Some(1_000.0), None, 0.0, 12_000.0);
        assert_eq!(cover.until_epoch_ms, 1_000.0);
        assert_eq!(
            cover.ended.as_deref(),
            Some("no sample came in the last 11 s")
        );
        let never = coverage("log line", None, None, 0.0, 30_000.0);
        assert_eq!(never.until_epoch_ms, 0.0);
        let recent = coverage("sample", Some(5_000.0), None, 0.0, 12_000.0);
        assert_eq!(recent.until_epoch_ms, 12_000.0);
        assert_eq!(recent.ended, None);
    }

    #[test]
    fn a_saved_recording_has_the_model_identifier_and_never_the_udid() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("result.device.json");
        let recording = Recording {
            phone: phone(),
            memory_bytes: Some(1),
            started_epoch_ms: 0.0,
            stopped_epoch_ms: 1.0,
            samples: vec![Sample {
                epoch_ms: 0.5,
                pid: 412,
                process: crate::WEB_CONTENT.to_owned(),
                footprint_bytes: 300,
            }],
            samples_cover: coverage("sample", None, None, 0.0, 1.0),
            memory_lines: Vec::new(),
            log_covers: coverage("log line", None, None, 0.0, 1.0),
        };
        recording.save(&path).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("\"identifier\": \"iPhone15,3\""), "{saved}");
        assert!(saved.contains("\"footprint_bytes\": 300"), "{saved}");
        assert!(!saved.contains("00008120"), "{saved}");
        assert!(
            recording
                .save(folder.path())
                .unwrap_err()
                .starts_with("cannot write")
        );
        assert_eq!(phone().model(), "iPhone 14 Pro Max");
    }
}
