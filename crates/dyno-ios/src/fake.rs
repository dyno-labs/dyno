//! A fake phone, for tests of a device run with no iPhone. `DYNO_FAKE_PHONE` chooses it, and
//! holds `key=value` settings, separated by `;`, each one optional:
//!
//! - `phones=0`: usbmuxd lists no phone.
//! - `fail=<step>`: that step fails. `connect` and `record` stop the start, and `hang` never
//!   starts. `launch` and `locked` are Safari's answer, and `launch-hangs` never answers.
//!   `samples` and `log` end that source as soon as it starts.
//! - `footprints=<bytes>,<bytes>,...`: the page process's footprint in each recording, in turn.
//!   Default: 200 MiB.
//! - `kill-after-ms=<ms>`: jetsam kills the page process this long after the recording starts.
//! - `launched=<file>`: where a launch writes the URL that Safari opens.
//!
//! The phone is an iPhone 14 Pro Max (`iPhone15,3`) on iOS 26.6.2 (23G90) with 6 GiB of memory,
//! with the UDID [`FAKE_UDID`]. Its page process is pid 412. It sends a sample and a log line
//! every 100 ms.

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::{Instant, Interval, interval, sleep_until};

use crate::WEB_CONTENT;
use crate::recorder::{Address, DeviceError, Event, started};
use crate::recording::{LineFeed, Link, Phone, Recording, SampleFeed};
use crate::sysmon::{PAGE_BYTES, ProcessRow, Reading};

/// The variable that chooses the fake phone.
pub const FAKE_PHONE_VARIABLE: &str = "DYNO_FAKE_PHONE";

/// The fake phone's UDID.
pub const FAKE_UDID: &str = "00008120-000000000000001E";

const PAGE_PID: u32 = 412;
const FOOTPRINT: u64 = 200 * 1_048_576;
/// 6 GiB, in pages.
const MEMORY_PAGES: u64 = 393_216;
const EVERY: Duration = Duration::from_millis(100);

/// How many recordings of the fake phone this process has made, for `footprints`.
static RECORDINGS: AtomicUsize = AtomicUsize::new(0);

/// The settings of `DYNO_FAKE_PHONE`.
#[derive(Debug, Default, PartialEq, Eq)]
struct Settings {
    no_phone: bool,
    fail: Option<String>,
    footprints: Vec<u64>,
    kill_after: Option<Duration>,
    launched: Option<String>,
}

impl Settings {
    fn parse(spec: &str) -> Settings {
        let mut settings = Settings::default();
        for (key, value) in spec
            .split(';')
            .filter_map(|setting| setting.split_once('='))
        {
            let value = value.trim();
            match key.trim() {
                "phones" => settings.no_phone = value == "0",
                "fail" => settings.fail = Some(value.to_owned()),
                "footprints" => {
                    settings.footprints = value.split(',').filter_map(|b| b.parse().ok()).collect();
                }
                "kill-after-ms" => {
                    settings.kill_after = value.parse().ok().map(Duration::from_millis);
                }
                "launched" => settings.launched = Some(value.to_owned()),
                _ => {}
            }
        }
        settings
    }

    fn fails(&self, step: &str) -> bool {
        self.fail.as_deref() == Some(step)
    }
}

/// Records the fake phone, as [`crate::phone::session`] records a real one.
pub(crate) async fn session(
    spec: &str,
    address: &Address,
    open: Option<String>,
    events: &Sender<Event>,
    stop: oneshot::Receiver<()>,
) -> Option<Recording> {
    let settings = Settings::parse(spec);
    let failed = match address {
        Address::Usbmuxd { .. } if settings.no_phone => Some(DeviceError::NoDevice),
        Address::Usbmuxd { udid: Some(udid) } if udid != FAKE_UDID => Some(
            DeviceError::UnknownDevice(udid.clone(), format!("{FAKE_UDID} on USB")),
        ),
        _ if settings.fails("connect") => Some(DeviceError::Connect(
            "the fake phone did not connect".to_owned(),
        )),
        _ if settings.fails("record") => Some(DeviceError::Record(
            "the fake phone did not start its tap".to_owned(),
        )),
        _ => None,
    };
    if let Some(error) = failed {
        let _ = events.send(Event::Failed(error));
        return None;
    }
    if settings.fails("hang") {
        let _ = stop.await;
        return None;
    }
    let phone = Phone {
        identifier: "iPhone15,3".to_owned(),
        os_version: "26.6.2 (23G90)".to_owned(),
        link: match address {
            Address::Usbmuxd { .. } => Link::Usb,
            Address::Network { .. } => Link::Network,
        },
        udid: Some(FAKE_UDID.to_owned()),
    };
    let recording = RECORDINGS.fetch_add(1, Ordering::Relaxed);
    let footprint = match settings.footprints.as_slice() {
        [] => FOOTPRINT,
        footprints => footprints[recording % footprints.len()],
    };
    let begun = Instant::now();
    let killed_at = settings.kill_after.map(|after| begun + after);
    let samples = Samples {
        every: interval(EVERY),
        footprint,
        killed_at,
        ended: settings.fails("samples"),
    };
    let lines = Lines {
        every: interval(EVERY),
        killed_at,
        ended: settings.fails("log"),
    };
    let launch = open.map(|url| async move {
        if settings.fails("launch-hangs") {
            std::future::pending::<()>().await;
        }
        if settings.fails("launch") {
            return Err(DeviceError::Open(
                "the fake phone did not open it".to_owned(),
            ));
        }
        if settings.fails("locked") {
            return Err(DeviceError::Locked);
        }
        if let Some(file) = &settings.launched {
            let _ = fs::write(file, &url);
        }
        Ok(())
    });
    Some(started(phone, samples, lines, launch, events, stop).await)
}

struct Samples {
    every: Interval,
    footprint: u64,
    killed_at: Option<Instant>,
    ended: bool,
}

impl SampleFeed for Samples {
    async fn next(&mut self) -> Result<Option<Reading>, String> {
        if self.ended {
            return Err("the fake phone stopped sending samples".to_owned());
        }
        self.every.tick().await;
        let alive = self.killed_at.is_none_or(|at| Instant::now() < at);
        let page = ProcessRow {
            pid: PAGE_PID,
            name: WEB_CONTENT.to_owned(),
            footprint_bytes: self.footprint,
        };
        Ok(Some(Reading {
            processes: Some(alive.then_some(page).into_iter().collect()),
            memory_bytes: Some(MEMORY_PAGES * PAGE_BYTES),
        }))
    }
}

struct Lines {
    every: Interval,
    killed_at: Option<Instant>,
    ended: bool,
}

impl LineFeed for Lines {
    async fn next(&mut self) -> Result<String, String> {
        if self.ended {
            return Err("the fake phone's log closed".to_owned());
        }
        if let Some(at) = self.killed_at.filter(|&at| at <= Instant::now() + EVERY) {
            sleep_until(at).await;
            self.killed_at = None;
            return Ok(format!(
                "kernel[0] <Notice>: memorystatus: killing process {PAGE_PID} [{WEB_CONTENT}] \
                 in high band FOREGROUND (100) - memorystatus_available_pages: 50000"
            ));
        }
        self.every.tick().await;
        Ok("kernel[0] <Notice>: a line of the fake phone's log".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_is_read_and_unknown_ones_are_left_out() {
        let settings = Settings::parse(
            "phones=0; fail=launch;footprints=1,2,x;kill-after-ms=1500;launched=/tmp/a;what=1;bare",
        );
        assert_eq!(
            settings,
            Settings {
                no_phone: true,
                fail: Some("launch".to_owned()),
                footprints: vec![1, 2],
                kill_after: Some(Duration::from_millis(1_500)),
                launched: Some("/tmp/a".to_owned()),
            }
        );
        assert_eq!(Settings::parse(""), Settings::default());
    }
}
