//! The runner protocol between dyno and a browser driver.
//!
//! The Rust CDP driver and the exec driver, an outside program such as a Playwright script,
//! speak the same messages, so the driver choice does not change the result. The program gets one
//! [`RunRequest`] line on stdin and writes [`DriverMessage`] lines on stdout until `done` or
//! `error`. Each line is one JSON object.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::probe::ProbeEvent;
use crate::profile::{HostProfile, Scenario};
use crate::result::DriverInfo;

/// What dyno asks a driver to run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RunRequest {
    pub protocol_version: u32,
    /// The full launch URL, query string included. It never goes into a result.
    pub launch_url: String,
    pub host_profile: HostProfile,
    pub scenario: Scenario,
    /// Demo spins to play after load (Phase 2). Null for a load-and-idle run.
    pub spins: Option<u32>,
    /// The game frame is idle after this long with no new requests.
    pub idle_after_ms: u32,
    /// The driver stops and sends an error after this long.
    pub timeout_ms: u32,
}

/// One line from a driver. `t_ms` is the time since the driver started the navigation.
///
/// There are two clocks. A driver message's `t_ms` is on the driver's clock. A probe event's
/// `t_ms` is `performance.now()` in the game frame, which starts at a different moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DriverMessage {
    /// The first line: the driver and the protocol version it speaks.
    Hello {
        protocol_version: u32,
        driver: DriverInfo,
    },
    /// One event from the probe in the game frame. The event keeps the frame's own clock.
    Probe { event: ProbeEvent },
    /// One finished request on the game frame's own session.
    Request {
        url: String,
        /// The response's MIME type. Null when it had none.
        mime_type: Option<String>,
        /// Bytes received over the network for this request.
        bytes: u64,
        t_ms: f64,
    },
    /// The game frame's JS heap in use.
    Heap { used_bytes: u64, t_ms: f64 },
    /// Self-check readings from inside the game frame.
    SelfCheck {
        device_pixel_ratio: f64,
        /// The unmasked WebGL renderer. Null when WebGL is off.
        webgl_renderer: Option<String>,
        /// When the game frame's document clock started, on the driver's clock: a probe event
        /// at frame time `t` happened at `t + frame_clock_at_ms` after the navigation. Null from
        /// a driver that does not read it.
        #[serde(default)]
        frame_clock_at_ms: Option<f64>,
        t_ms: f64,
    },
    /// A step in the run.
    Mark { mark: RunMark, t_ms: f64 },
    /// The last line of a run that finished.
    Done { t_ms: f64 },
    /// The last line of a run that failed.
    Error { message: String, t_ms: f64 },
}

/// The steps of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunMark {
    /// The harness page started loading the game frame.
    Navigated,
    /// The game frame went idle.
    Idle,
    /// The harness made the game active, such as by sending `reels:active` `true`.
    Activated,
    /// The harness made the game inactive.
    Deactivated,
    /// The game frame committed a new document, as a script redirect does. Probe events after
    /// it are on the new document's clock.
    FrameDocument,
}

/// The runner protocol version. It bumps on any change a driver must handle.
pub const RUNNER_PROTOCOL_VERSION: u32 = 0;
