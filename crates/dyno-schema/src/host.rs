//! The host's unclean exit record and exposure checkpoints, written by a page that embeds games.
//!
//! A killed frame sends nothing. Only the host page sees the kill, on its next load, as an
//! unclean exit. Any host can write these records: Reels is the first, and the `dyno-host`
//! script (Phase 2) makes them generic. The term is "observed unclean exit": the cause is
//! unknown, so these records never say "memory kill".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::profile::ProfileRef;

/// An observed unclean exit: the last page died with game frames live. Never sampled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UncleanExit {
    pub schema_version: u32,
    /// The host profile, such as reels version 0.
    pub host_profile: ProfileRef,
    /// The game frames that were live when the page died.
    pub frames: Vec<LiveFrame>,
}

/// A game frame that was live when the page died.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LiveFrame {
    pub game_id: String,
    /// From the frame URL, such as "v12" from `/v12/`. Null when the URL has none.
    pub build_version: Option<String>,
    pub frame_state: FrameState,
    /// Time since the frame was mounted.
    pub mount_age_ms: u64,
    /// Whether the host saw the game's ready message.
    pub ready_seen: bool,
}

/// Whether a game frame is the one on screen. Reels calls an inactive frame preloaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FrameState {
    /// On screen, after the host's active message.
    Active,
    /// Mounted but not on screen.
    Inactive,
}

/// Sampled exposure, written to storage every 30 s or so and sent on the next load. A page
/// that dies loses only the time since its last checkpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ExposureCheckpoint {
    pub schema_version: u32,
    pub host_profile: ProfileRef,
    pub samples: Vec<ExposureSample>,
}

/// Live time for one game frame in one state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ExposureSample {
    pub game_id: String,
    pub build_version: Option<String>,
    pub frame_state: FrameState,
    /// Time the frame was live in this state.
    pub live_ms: u64,
    /// The share of sessions that log exposure, from above 0 to 1.
    pub sampled_share: f64,
}
