//! The Rust CDP driver.
//!
//! It launches Chromium with site isolation on and `--force-device-scale-factor=3`, attaches to
//! the cross-origin game frame, injects the probe before game code runs, and reads the frame's
//! own requests. It speaks the runner protocol in process.
//!
//! [`CdpDriver`] is the [`Driver`] that `dyno check <launch-url>` uses; [`run_cdp`] is the same
//! run as a function.

use dyno_run::{Driver, DriverError, DriverErrorCode};
use dyno_schema::runner::{DriverMessage, RunRequest};

pub mod browser;
pub mod connection;
pub mod harness;
pub mod run;

/// The lab build of the probe, built from `probe/probe.ts` by `probe/build.ts`. dyno injects it
/// over CDP; it never ships in a game.
pub const PROBE: &str = include_str!("../probe/probe.js");

/// The field build of the probe: the lab build without its `lab` blocks. A platform adds it to a
/// game's page. No dyno code injects it yet.
pub const FIELD_PROBE: &str = include_str!("../probe/probe.field.js");

/// The driver's name in `hello` and in errors.
pub const DRIVER_NAME: &str = "cdp";

/// Why a CDP run failed.
#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    #[error("{0}")]
    NoChromium(String),
    #[error("Chromium did not start: {0}")]
    Launch(String),
    #[error("cannot connect to Chromium: {0}")]
    Connect(String),
    #[error("{method} failed: {detail}")]
    Call { method: String, detail: String },
    #[error("an unexpected reply: {0}")]
    Reply(String),
    #[error("the harness server failed: {0}")]
    Harness(String),
    #[error("the launch URL is not a URL")]
    LaunchUrl,
    #[error("the run ended early: {0} went away")]
    Gone(String),
    #[error("the run timed out")]
    Timeout,
    #[error("no async runtime: {0}")]
    Runtime(String),
}

impl CdpError {
    pub(crate) fn call(method: &str, detail: &str) -> CdpError {
        CdpError::Call {
            method: method.to_owned(),
            detail: detail.to_owned(),
        }
    }
}

/// Runs one load-and-idle run with Chromium. An error is scrubbed, since it can quote a URL.
pub fn run_cdp(request: &RunRequest) -> Result<Vec<DriverMessage>, DriverError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| driver_error(&CdpError::Runtime(error.to_string())))?;
    runtime
        .block_on(run::run(request))
        .map_err(|error| driver_error(&error))
}

fn driver_error(error: &CdpError) -> DriverError {
    DriverError::new(DriverErrorCode::Run, &error.to_string())
}

/// The CDP driver.
#[derive(Debug, Default)]
pub struct CdpDriver;

impl Driver for CdpDriver {
    fn name(&self) -> &str {
        DRIVER_NAME
    }

    fn run(&mut self, request: &RunRequest) -> Result<Vec<DriverMessage>, DriverError> {
        run_cdp(request)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;
    use std::process::Command;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    /// The gzip budgets: 3 KiB for the field build, which runs in the process the
    /// platform protects, and 10 KB for the lab build.
    const FIELD_PROBE_GZIP_BUDGET: usize = 3 * 1024;
    const LAB_PROBE_GZIP_BUDGET: usize = 10 * 1024;

    fn gzip_bytes(script: &str) -> usize {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(script.as_bytes()).unwrap();
        encoder.finish().unwrap().len()
    }

    #[test]
    fn the_field_probe_fits_its_gzip_budget() {
        let bytes = gzip_bytes(FIELD_PROBE);
        assert!(bytes <= FIELD_PROBE_GZIP_BUDGET, "{bytes} bytes");
    }

    #[test]
    fn the_lab_probe_fits_its_gzip_budget() {
        let bytes = gzip_bytes(PROBE);
        assert!(bytes <= LAB_PROBE_GZIP_BUDGET, "{bytes} bytes");
    }

    #[test]
    fn the_field_probe_leaves_out_the_lab_blocks() {
        assert!(PROBE.contains("createImageBitmap"));
        assert!(!FIELD_PROBE.contains("createImageBitmap"));
        assert!(!FIELD_PROBE.contains("Worker"));
        // Only the lab build says what an upload with no source held.
        assert!(PROBE.contains("\"empty\""));
        assert!(!FIELD_PROBE.contains("\"empty\""));
    }

    #[test]
    fn the_probe_uses_the_frame_time_edges_and_period_of_the_schema() {
        let edges: Vec<String> = dyno_schema::probe::FRAME_TIME_BUCKETS_MS
            .iter()
            .map(u32::to_string)
            .collect();
        let edges = format!("const BUCKETS_MS = [{}];", edges.join(", "));
        let every = format!(
            "const EVERY_MS = {};",
            dyno_schema::probe::FRAME_TIMES_EVERY_MS
        );
        for probe in [PROBE, FIELD_PROBE] {
            assert!(probe.contains(&edges), "{edges}");
            assert!(probe.contains(&every), "{every}");
        }
    }

    #[test]
    #[ignore = "needs Node 22.18 or later"]
    fn the_probe_is_built_from_its_typescript() {
        let build = Path::new(env!("CARGO_MANIFEST_DIR")).join("probe/build.ts");
        let status = Command::new("node")
            .arg(build)
            .arg("--check")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
