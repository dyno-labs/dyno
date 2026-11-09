//! `--device`: an xctrace recording of the connected phone while its Safari runs a served check,
//! read into the device measures.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dyno_ios::RunWindow;
use dyno_ios::devicectl::{is_locked, open_in_safari};
use dyno_ios::xctrace::{DeviceError, Recording, choose_device, connected_devices};
use dyno_schema::environment::{EnvironmentDevice, EnvironmentOs};
use dyno_schema::measure::Measure;
use dyno_serve::RunTimes;

use crate::exit::CliError;

/// A recording in progress, and where its trace is.
pub struct DeviceRun {
    recording: Recording,
    name: String,
    /// Whether Safari opened the page, when dyno opened it.
    opened: Option<Result<(), DeviceError>>,
    /// The folder of a trace that is not kept, deleted when the run is dropped.
    _scratch: Option<tempfile::TempDir>,
}

/// Chooses the device, starts recording it, and opens `open` in its Safari when given. The trace
/// goes to `keep_at` when given, and to a folder deleted after the check otherwise. A problem the
/// person fixes, such as no phone or a locked one, is a usage error. Safari that does not open
/// is not: the person can open the page.
pub fn start(
    id: Option<&str>,
    open: Option<&str>,
    keep_at: Option<&Path>,
    limit: Duration,
) -> Result<DeviceRun, CliError> {
    let usage = |error: DeviceError| CliError::usage(error);
    let device = choose_device(&connected_devices().map_err(usage)?, id).map_err(usage)?;
    // A locked phone stops the page and disconnects from xctrace. A lock state that devicectl
    // cannot give, as for the Mac itself, does not stop the run.
    if is_locked(&device) == Some(true) {
        return Err(usage(DeviceError::Locked(device.name)));
    }
    let (trace, scratch) = match keep_at {
        Some(path) => (path.to_owned(), None),
        None => {
            let folder = tempfile::tempdir().map_err(CliError::internal)?;
            (folder.path().join("device.trace"), Some(folder))
        }
    };
    // xctrace will not write over a trace, such as the one an earlier check kept here.
    if trace.exists() {
        fs::remove_dir_all(&trace).map_err(|error| {
            CliError::internal(format!("cannot remove {}: {error}", trace.display()))
        })?;
    }
    let recording = Recording::start(&device, &trace, limit).map_err(CliError::internal)?;
    // Only a phone has Safari to open. The Mac itself has no OS version. On a cable, the launch
    // leaves the recording on.
    let opened = match open {
        Some(url) if device.os_version.is_some() => Some(open_in_safari(&device, url)),
        _ => None,
    };
    Ok(DeviceRun {
        recording,
        name: device.name,
        opened,
        _scratch: scratch,
    })
}

impl DeviceRun {
    /// What `--device` prints once the recording is on: whether Safari opened the page, and
    /// what it records.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match &self.opened {
            Some(Ok(())) => lines.push(format!(
                "dyno check: opened the page in Safari on {}.",
                self.name
            )),
            Some(Err(error)) => lines.push(format!(
                "dyno check: {error}. Open the URL below on {} by hand.",
                self.name
            )),
            None => {}
        }
        lines.push(format!("dyno check: recording {} with xctrace.", self.name));
        lines
    }

    /// What `--device` prints when the run ends: xctrace can take a minute to save.
    pub fn saving_line(&self) -> String {
        format!(
            "dyno check: saving the trace of {}. This can take a minute.",
            self.name
        )
    }

    /// Stops the recording and gives the device measures of the run, and the OS and device that
    /// its trace recorded. A run that never started, or a trace that cannot be read, gives every
    /// device measure as missing, with the reason, and no OS or device.
    pub fn finish(self, run: Option<RunTimes>) -> FinishedDeviceRun {
        let trace = self.recording.stop();
        let Some(run) = run else {
            return FinishedDeviceRun::missing(
                "the run did not start, so the trace has no run to read",
            );
        };
        match trace {
            Ok(trace) => {
                let (os, device) = dyno_ios::trace_environment(&trace.device);
                FinishedDeviceRun {
                    measures: dyno_ios::device_measures(
                        &trace,
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
            Err(error) => FinishedDeviceRun::missing(&without_name(&error.to_string(), &self.name)),
        }
    }
}

/// What a device run gave once its recording stopped.
pub struct FinishedDeviceRun {
    pub measures: Vec<Measure>,
    /// The OS the trace recorded, with its version.
    pub os: Option<EnvironmentOs>,
    /// The device the trace recorded: its model and memory.
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

/// The text with the phone's own name, which is often its owner's, taken out: a reason goes into
/// the result, which can be uploaded.
fn without_name(text: &str, name: &str) -> String {
    if name.is_empty() {
        return text.to_owned();
    }
    text.replace(name, "the phone")
}

/// Where a check that writes its result to `out` keeps its trace: beside it, as `<name>.trace`.
pub fn kept_trace(out: &Path) -> PathBuf {
    out.with_extension("trace")
}

/// Where `--runs` keeps the trace of one run, beside the trace a single run keeps: as
/// `<name>.game-2.trace` for the second run of the game, for example.
pub fn run_trace(kept: &Path, of: &str, run: u32) -> PathBuf {
    kept.with_extension(format!("{of}-{run}.trace"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_never_holds_the_phone_s_own_name() {
        assert_eq!(
            without_name(
                "xctrace did not save the trace: Dev’s iPhone went away",
                "Dev’s iPhone"
            ),
            "xctrace did not save the trace: the phone went away"
        );
        assert_eq!(without_name("no name here", ""), "no name here");
    }

    #[test]
    fn the_trace_is_kept_beside_the_result() {
        assert_eq!(
            kept_trace(Path::new("runs/game-1.json")),
            PathBuf::from("runs/game-1.trace")
        );
    }

    #[test]
    fn each_run_of_runs_keeps_its_own_trace() {
        let kept = kept_trace(Path::new("runs/moon_lander.json"));
        assert_eq!(
            run_trace(&kept, "empty", 3),
            PathBuf::from("runs/moon_lander.empty-3.trace")
        );
        assert_eq!(
            run_trace(&kept, "game", 1),
            PathBuf::from("runs/moon_lander.game-1.trace")
        );
    }
}
