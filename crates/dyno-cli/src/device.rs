//! `--device`: an xctrace recording of the connected phone while its Safari runs a served check,
//! read into the device measures.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dyno_ios::RunWindow;
use dyno_ios::devicectl::{is_locked, open_in_safari};
use dyno_ios::xctrace::{DeviceError, Recording, choose_device, connected_devices};
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

    /// Stops the recording and gives the device measures of the run. A run that never started,
    /// or a trace that cannot be read, gives every device measure as missing, with the reason.
    pub fn finish(self, run: Option<RunTimes>) -> Vec<Measure> {
        let trace = self.recording.stop();
        let Some(run) = run else {
            return dyno_ios::missing_device_measures(
                "the run did not start, so the trace has no run to read",
            );
        };
        match trace {
            Ok(trace) => dyno_ios::device_measures(
                &trace,
                &RunWindow {
                    navigated_epoch_ms: run.navigated_epoch_ms,
                    idle_epoch_ms: run.idle_epoch_ms,
                    ended_epoch_ms: run.ended_epoch_ms,
                },
            ),
            Err(error) => dyno_ios::missing_device_measures(&error.to_string()),
        }
    }
}

/// Where a check that writes its result to `out` keeps its trace: beside it, as `<name>.trace`.
pub fn kept_trace(out: &Path) -> PathBuf {
    out.with_extension("trace")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trace_is_kept_beside_the_result() {
        assert_eq!(
            kept_trace(Path::new("runs/game-1.json")),
            PathBuf::from("runs/game-1.trace")
        );
    }
}
