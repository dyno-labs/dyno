//! `--device`: a recording of the phone while its Safari runs a served check, read into the
//! device measures.
//!
//! The recording starts first. Then the check serves the page, at this machine's address in the
//! device link's tunnel when it has one, and only then opens the page in the phone's Safari.
//! It can open a page more than once: at another address, when the first got no request.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use dyno_ios::recording::Link;
use dyno_ios::{Address, DeviceError, NoTunnelAddress, OpenedWith, Recorder, RunWindow};
use dyno_schema::environment::{EnvironmentDevice, EnvironmentOs};
use dyno_schema::measure::Measure;
use dyno_schema::timeline::Timeline;
use dyno_serve::RunTimes;

use crate::exit::CliError;
use crate::output;

/// A recording in progress, and where to save it.
pub struct DeviceRun {
    recorder: Recorder,
    save_to: Option<PathBuf>,
}

/// Chooses the phone and starts recording it. The recording is saved to `save_to` when given.
/// A problem the person fixes, such as no phone, is a usage error.
pub fn start(address: &Address, save_to: Option<&Path>) -> Result<DeviceRun, CliError> {
    let recorder = Recorder::start(address).map_err(device_error)?;
    Ok(DeviceRun {
        recorder,
        save_to: save_to.map(Path::to_owned),
    })
}

/// A problem the person fixes, such as no phone or a locked one, is a usage error. A recording
/// that does not start is not theirs to fix.
fn device_error(error: DeviceError) -> CliError {
    match error {
        DeviceError::Record(_) => CliError::internal(error),
        _ => CliError::usage(error),
    }
}

impl DeviceRun {
    /// What `--device` prints once the recording is on: what it records.
    pub fn recording_line(&self) -> String {
        let phone = self.recorder.phone();
        let link = match phone.link {
            Link::Usb => "on USB",
            Link::Network => "on the network",
        };
        format!(
            "dyno check: recording the {}, iOS {}, {link}.",
            phone.model(),
            phone.os_version
        )
    }

    /// This machine's address inside the device link's tunnel to the phone, to serve the page
    /// at, or why it has none.
    pub fn tunnel_host(&self) -> Result<IpAddr, NoTunnelAddress> {
        self.recorder.tunnel_host()
    }

    /// Opens `url` in the phone's Safari, and gives whether Safari opened it, with what to print.
    /// A locked phone is a usage error. Safari that does not open the page is not an error: the
    /// person can open it.
    pub fn open(&mut self, url: &str) -> Result<(bool, String), CliError> {
        self.recorder.open(url).map_err(device_error)?;
        let model = self.recorder.phone().model();
        Ok(match self.recorder.opened() {
            Some(Ok(OpenedWith::CoreDevice)) => (
                true,
                format!("dyno check: opened the page in Safari on the {model}."),
            ),
            Some(Ok(OpenedWith::Devicectl)) => (
                true,
                format!("dyno check: opened the page in Safari on the {model}, with devicectl."),
            ),
            Some(Err(error)) => (
                false,
                format!("dyno check: {error}. Open the URL above on the {model} by hand."),
            ),
            // `open` always leaves an answer.
            None => (
                false,
                format!("dyno check: open the URL above on the {model} by hand."),
            ),
        })
    }

    /// Stops the recording, saves it when asked, and gives the device measures of the run with the
    /// OS and device of the phone. A run that never started, or a recording that did not end
    /// well, gives every device measure as missing, with the reason, and no OS or device.
    pub fn finish(self, run: Option<RunTimes>) -> FinishedDeviceRun {
        let recording = match self.recorder.stop() {
            Ok(recording) => recording,
            Err(error) => return FinishedDeviceRun::missing(&error.to_string()),
        };
        if let Some(path) = &self.save_to
            && let Err(error) = recording.save(path)
        {
            output::print_summary(&[format!("dyno check: the recording was not saved: {error}.")]);
        }
        let Some(run) = run else {
            return FinishedDeviceRun::missing(
                "the run did not start, so the recording has no run to read",
            );
        };
        let (os, device) = dyno_ios::phone_environment(&recording);
        let window = RunWindow {
            navigated_epoch_ms: run.navigated_epoch_ms,
            idle_epoch_ms: run.idle_epoch_ms,
            activated_epoch_ms: run.activated_epoch_ms,
            ended_epoch_ms: run.ended_epoch_ms,
        };
        FinishedDeviceRun {
            measures: dyno_ios::device_measures(&recording, &window),
            os,
            device,
            timeline: dyno_ios::device_timeline(&recording, &window),
        }
    }
}

/// What a device run gave once its recording stopped.
pub struct FinishedDeviceRun {
    pub measures: Vec<Measure>,
    /// The phone's OS, with its version.
    pub os: Option<EnvironmentOs>,
    /// The phone: its model and memory.
    pub device: Option<EnvironmentDevice>,
    /// Safari's processes through the run, from the recording.
    pub timeline: Option<Timeline>,
}

impl FinishedDeviceRun {
    /// Every device measure missing for this reason, and no OS or device.
    fn missing(reason: &str) -> FinishedDeviceRun {
        FinishedDeviceRun {
            measures: dyno_ios::missing_device_measures(reason),
            os: None,
            device: None,
            timeline: None,
        }
    }
}

/// Where a check that writes its result to `out` saves its recording: beside it, as
/// `<name>.device.json`.
pub fn saved_recording(out: &Path) -> PathBuf {
    out.with_extension("device.json")
}

/// Where `--runs` saves the recording of one run, beside the one a single run saves: as
/// `<name>.game-2.device.json` for the second run of the game, for example.
pub fn run_recording(saved: &Path, of: &str, run: u32) -> PathBuf {
    let name = saved
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".device.json"))
        .unwrap_or("result");
    saved.with_file_name(format!("{name}.{of}-{run}.device.json"))
}

#[cfg(test)]
mod tests {
    use dyno_schema::measure::MeasureStatus;

    use super::*;

    #[test]
    fn the_recording_is_saved_beside_the_result() {
        assert_eq!(
            saved_recording(Path::new("runs/game-1.json")),
            PathBuf::from("runs/game-1.device.json")
        );
    }

    #[test]
    fn a_run_with_nothing_to_read_gives_every_device_measure_missing_and_no_phone() {
        let finished = FinishedDeviceRun::missing("the run did not start");
        assert!(!finished.measures.is_empty());
        for measure in &finished.measures {
            assert_eq!(measure.status, MeasureStatus::Missing, "{}", measure.id);
            assert_eq!(measure.reason.as_deref(), Some("the run did not start"));
            assert!(measure.required, "{}", measure.id);
        }
        assert!(finished.os.is_none());
        assert!(finished.device.is_none());
    }

    #[test]
    fn each_run_of_runs_saves_its_own_recording() {
        let saved = saved_recording(Path::new("runs/moon_lander.json"));
        assert_eq!(
            run_recording(&saved, "empty", 3),
            PathBuf::from("runs/moon_lander.empty-3.device.json")
        );
        assert_eq!(
            run_recording(&saved, "game", 1),
            PathBuf::from("runs/moon_lander.game-1.device.json")
        );
    }
}
