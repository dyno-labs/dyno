//! Apple's `xcrun xctrace`, run as a subprocess: the connected devices, and an Activity Monitor
//! recording of every process on one of them, read back as a [`Trace`].
//!
//! `DYNO_XCRUN` names another program to run instead of `xcrun`, such as a fake in the tests.

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::trace::{PROCESS_TABLE_XPATH, Trace, read_samples, read_toc};

/// The variable that names another program to run instead of `xcrun`.
pub const XCRUN_VARIABLE: &str = "DYNO_XCRUN";

/// What xctrace prints once it records.
const RECORDING_STARTED: &str = "Starting recording";

/// How long xctrace may take to start recording: a phone can take a while to get ready.
const START_WAIT: Duration = Duration::from_secs(60);

/// How long xctrace may take to save the trace once it is told to stop. On a Mac mini it took
/// from 4 s to over a minute for the same short recording.
const STOP_WAIT: Duration = Duration::from_secs(300);

/// How often a wait checks on xctrace.
const POLL: Duration = Duration::from_millis(200);

/// Why a device step failed.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("cannot run xcrun: {0}. A device run needs a Mac with Xcode")]
    NoXcrun(String),
    #[error("no iPhone is connected. Connect one with a cable, unlock it and trust this Mac. {0}")]
    NoDevice(String),
    #[error("{0} iPhones are connected: choose one with --device-id. {1}")]
    SeveralDevices(usize, String),
    #[error("no connected device is named {0}. {1}")]
    UnknownDevice(String, String),
    #[error("xctrace did not start recording: {0}")]
    Start(String),
    #[error("xctrace did not save the trace: {0}")]
    Stop(String),
    #[error("cannot read the trace: {0}")]
    Read(String),
}

/// A device that xctrace can record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    /// The OS version. None for the Mac itself, which xctrace lists with no version.
    pub os_version: Option<String>,
    pub udid: String,
}

/// An Activity Monitor recording in progress.
pub struct Recording {
    child: Child,
    trace: PathBuf,
}

fn xcrun() -> Command {
    let program = std::env::var_os(XCRUN_VARIABLE).unwrap_or_else(|| OsString::from("xcrun"));
    Command::new(program)
}

/// The connected devices, as `xcrun xctrace list devices` lists them. Offline devices and
/// simulators are left out.
pub fn connected_devices() -> Result<Vec<Device>, DeviceError> {
    let output = xcrun()
        .args(["xctrace", "list", "devices"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| DeviceError::NoXcrun(error.to_string()))?;
    if !output.status.success() {
        return Err(DeviceError::NoXcrun(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(parse_devices(&String::from_utf8_lossy(&output.stdout)))
}

/// The devices of the `== Devices ==` section. A line is `name (version) (udid)`, or
/// `name (udid)` for the Mac itself.
pub fn parse_devices(listing: &str) -> Vec<Device> {
    let mut in_devices = false;
    let mut devices = Vec::new();
    for line in listing.lines().map(str::trim) {
        if line.starts_with("==") {
            in_devices = line == "== Devices ==";
            continue;
        }
        if !in_devices {
            continue;
        }
        let Some((rest, udid)) = last_group(line) else {
            continue;
        };
        let (name, os_version) = match last_group(rest) {
            Some((name, version)) => (name, Some(version.to_owned())),
            None => (rest, None),
        };
        devices.push(Device {
            name: name.to_owned(),
            os_version,
            udid: udid.to_owned(),
        });
    }
    devices
}

/// `rest (group)` split into the rest and the group.
fn last_group(text: &str) -> Option<(&str, &str)> {
    let (rest, group) = text.strip_suffix(')')?.rsplit_once(" (")?;
    Some((rest.trim_end(), group))
}

/// The device to record: the one whose UDID or name is `id`, or else the only connected phone.
pub fn choose_device(devices: &[Device], id: Option<&str>) -> Result<Device, DeviceError> {
    let listed = || {
        let names: Vec<String> = devices
            .iter()
            .map(|device| format!("{} ({})", device.name, device.udid))
            .collect();
        format!("Connected: {}.", names.join(", "))
    };
    if let Some(id) = id {
        return devices
            .iter()
            .find(|device| device.udid == id || device.name == id)
            .cloned()
            .ok_or_else(|| DeviceError::UnknownDevice(id.to_owned(), listed()));
    }
    let phones: Vec<&Device> = devices
        .iter()
        .filter(|device| device.os_version.is_some())
        .collect();
    match phones.as_slice() {
        [phone] => Ok((*phone).clone()),
        [] => Err(DeviceError::NoDevice(listed())),
        several => Err(DeviceError::SeveralDevices(several.len(), listed())),
    }
}

impl Recording {
    /// Starts recording every process on the device into `trace`, which must not exist yet. It
    /// returns once xctrace records. The recording stops by itself after `limit`.
    pub fn start(device: &Device, trace: &Path, limit: Duration) -> Result<Recording, DeviceError> {
        let mut child = xcrun()
            .args([
                "xctrace",
                "record",
                "--template",
                "Activity Monitor",
                "--device",
            ])
            .arg(&device.udid)
            .args(["--all-processes", "--time-limit"])
            .arg(format!("{}s", limit.as_secs().max(1)))
            .arg("--output")
            .arg(trace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| DeviceError::NoXcrun(error.to_string()))?;
        let (lines, received) = mpsc::channel();
        for stream in [
            child
                .stdout
                .take()
                .map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let lines = lines.clone();
            thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let _ = lines.send(line);
                }
            });
        }
        drop(lines);
        let mut said = Vec::new();
        let until = Instant::now() + START_WAIT;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match received.recv_timeout(left.min(POLL)) {
                Ok(line) if line.starts_with(RECORDING_STARTED) => {
                    return Ok(Recording {
                        child,
                        trace: trace.to_owned(),
                    });
                }
                Ok(line) => said.push(line),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        Err(DeviceError::Start(said.join(" ")))
    }

    /// Stops the recording, as Ctrl-C does, waits for xctrace to save it, and reads it back.
    pub fn stop(mut self) -> Result<Trace, DeviceError> {
        // xctrace saves the trace on an interrupt. std has no way to send one, so `kill` does.
        let interrupted = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .is_ok_and(|status| status.success());
        let until = Instant::now() + STOP_WAIT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if interrupted && Instant::now() < until => thread::sleep(POLL),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(DeviceError::Stop(
                        "it did not stop when interrupted".to_owned(),
                    ));
                }
            }
        }
        read_trace(&self.trace)
    }
}

/// A saved trace, read through `xctrace export`.
pub fn read_trace(trace: &Path) -> Result<Trace, DeviceError> {
    let export = |args: &[&str]| -> Result<String, DeviceError> {
        let output = xcrun()
            .args(["xctrace", "export", "--input"])
            .arg(trace)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| DeviceError::Read(error.to_string()))?;
        if !output.status.success() {
            return Err(DeviceError::Read(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let (started_epoch_ms, device) =
        read_toc(&export(&["--toc"])?).map_err(|error| DeviceError::Read(error.to_string()))?;
    let samples = read_samples(&export(&["--xpath", PROCESS_TABLE_XPATH])?)
        .map_err(|error| DeviceError::Read(error.to_string()))?;
    Ok(Trace {
        started_epoch_ms,
        device,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "== Devices ==
Dev’s Mac mini (00000000-0000-0000-0000-00000000000A)
Dev’s iPhone (2) (26.6.2) (00008120-000000000000002E)

== Devices Offline ==
Old iPhone (18.1) (00008030-000000000000002E)

== Simulators ==
iPhone 18 Pro Simulator (27.0) (F5CA8CA4-53D1-4C23-BB65-4BE605CB525F)
";

    fn mac() -> Device {
        Device {
            name: "Dev’s Mac mini".to_owned(),
            os_version: None,
            udid: "00000000-0000-0000-0000-00000000000A".to_owned(),
        }
    }

    fn phone() -> Device {
        Device {
            name: "Dev’s iPhone (2)".to_owned(),
            os_version: Some("26.6.2".to_owned()),
            udid: "00008120-000000000000002E".to_owned(),
        }
    }

    #[test]
    fn only_connected_devices_are_listed_and_a_name_may_hold_brackets() {
        assert_eq!(parse_devices(LISTING), [mac(), phone()]);
    }

    #[test]
    fn the_only_connected_phone_is_chosen() {
        assert_eq!(choose_device(&[mac(), phone()], None).unwrap(), phone());
    }

    #[test]
    fn a_device_is_chosen_by_udid_or_name_the_mac_too() {
        assert_eq!(
            choose_device(
                &[mac(), phone()],
                Some("00000000-0000-0000-0000-00000000000A")
            )
            .unwrap(),
            mac()
        );
        assert_eq!(
            choose_device(&[mac(), phone()], Some("Dev’s iPhone (2)")).unwrap(),
            phone()
        );
        assert!(matches!(
            choose_device(&[mac()], Some("nope")),
            Err(DeviceError::UnknownDevice(..))
        ));
    }

    #[test]
    fn no_phone_or_several_phones_need_the_person() {
        assert!(matches!(
            choose_device(&[mac()], None),
            Err(DeviceError::NoDevice(_))
        ));
        let second = Device {
            udid: "other".to_owned(),
            ..phone()
        };
        assert!(matches!(
            choose_device(&[phone(), second], None),
            Err(DeviceError::SeveralDevices(2, _))
        ));
    }
}
