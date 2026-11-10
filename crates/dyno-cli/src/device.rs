//! `--device`: a recording of the phone while its Safari runs a served check, read into the
//! device measures.

use std::path::{Path, PathBuf};

use dyno_ios::recording::Link;
use dyno_ios::{Address, DeviceError, OpenedWith, Recorder, RunWindow};
use dyno_schema::environment::{EnvironmentDevice, EnvironmentOs};
use dyno_schema::measure::Measure;
use dyno_serve::RunTimes;

use crate::exit::CliError;
use crate::output;

/// A recording in progress, and where to save it.
pub struct DeviceRun {
    recorder: Recorder,
    save_to: Option<PathBuf>,
}

/// Chooses the phone, starts recording it, and opens `open` in its Safari when given. The
/// recording is saved to `save_to` when given. A problem the person fixes, such as no phone or a
/// locked one, is a usage error. Safari that does not open is not: the person can open the page.
pub fn start(
    address: &Address,
    open: Option<&str>,
    save_to: Option<&Path>,
) -> Result<DeviceRun, CliError> {
    let recorder = Recorder::start(address, open).map_err(|error| match error {
        DeviceError::Record(_) => CliError::internal(error),
        _ => CliError::usage(error),
    })?;
    Ok(DeviceRun {
        recorder,
        save_to: save_to.map(Path::to_owned),
    })
}

impl DeviceRun {
    /// What `--device` prints once the recording is on: whether Safari opened the page, and
    /// what it records.
    pub fn lines(&self) -> Vec<String> {
        let phone = self.recorder.phone();
        let model = phone.model();
        let mut lines = Vec::new();
        match self.recorder.opened() {
            Some(Ok(OpenedWith::CoreDevice)) => {
                lines.push(format!(
                    "dyno check: opened the page in Safari on the {model}."
                ));
            }
            Some(Ok(OpenedWith::Devicectl)) => lines.push(format!(
                "dyno check: opened the page in Safari on the {model}, with devicectl."
            )),
            Some(Err(error)) => lines.push(format!(
                "dyno check: {error}. Open the URL below on the {model} by hand."
            )),
            None => {}
        }
        let link = match phone.link {
            Link::Usb => "on USB",
            Link::Network => "on the network",
        };
        lines.push(format!(
            "dyno check: recording the {model}, iOS {}, {link}.",
            phone.os_version
        ));
        lines
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
        FinishedDeviceRun {
            measures: dyno_ios::device_measures(
                &recording,
                &RunWindow {
                    navigated_epoch_ms: run.navigated_epoch_ms,
                    idle_epoch_ms: run.idle_epoch_ms,
                    ended_epoch_ms: run.ended_epoch_ms,
                },
            ),
            os,
            device,
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
}

impl FinishedDeviceRun {
    /// Every device measure missing for this reason, and no OS or device.
    fn missing(reason: &str) -> FinishedDeviceRun {
        FinishedDeviceRun {
            measures: dyno_ios::missing_device_measures(reason),
            os: None,
            device: None,
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
