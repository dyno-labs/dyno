//! Host profiles: how a host page embeds a game, and the contract the game must keep there.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How a host page embeds a game, and the device facts the static formulas use. Reels is the
/// first host profile. It holds no assumed cost: the device run measures those.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HostProfile {
    pub name: String,
    /// Bumps when any value below changes.
    pub version: u32,
    /// The message the game sends when it is ready, such as `reels:ready`.
    pub ready_message: String,
    /// The message the host sends when the game becomes active, such as `reels:active`.
    pub active_message: String,
    /// The contract's limit on time to the ready message.
    pub ready_within_ms: u32,
    /// The game frame's size in CSS pixels.
    pub viewport: Viewport,
    /// The device pixel ratio the run forces. The reference iPhone's is 3.
    pub device_scale_factor: u32,
    /// Whether the reference browser has WebGPU on. When it is off, a game that asks for WebGPU
    /// gets WebGL.
    pub webgpu: bool,
    /// The audio context's sample rate on the reference iPhone. Decoded audio is held at this
    /// rate. The measure ids name 48 kHz, so a change needs new ids.
    pub pcm_rate_hz: u32,
    /// The lower context rate that fixes suggest. The measure ids name 24 kHz, so a change needs
    /// new ids.
    pub pcm_low_rate_hz: u32,
}

/// A size in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// Which host profile, and which version of it, a result used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProfileRef {
    pub name: String,
    pub version: u32,
}

/// The harness scenario of a browser run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    /// The game alone in a fresh harness page (Phase 1).
    Alone,
    /// The harness page after 10 earlier games (Phase 2).
    Aged,
    /// One game active while the next one preloads (Phase 2).
    Overlap,
}

/// The name of the Reels host profile.
const REELS: &str = "reels";

/// The host profile `--host-profile` uses when it is not given.
pub const DEFAULT_HOST_PROFILE: &str = REELS;

impl HostProfile {
    /// Every host profile this dyno knows, by name.
    pub const NAMES: &[&str] = &[REELS];

    /// The host profile with this name, if dyno knows it.
    pub fn named(name: &str) -> Option<HostProfile> {
        match name {
            REELS => Some(HostProfile::reels()),
            _ => None,
        }
    }

    /// Stake Reels: one game per full-screen frame.
    pub fn reels() -> HostProfile {
        HostProfile {
            name: REELS.to_owned(),
            version: 2,
            ready_message: "reels:ready".to_owned(),
            active_message: "reels:active".to_owned(),
            ready_within_ms: 5_000,
            // A standard late-model iPhone's screen, the iPhone 17's. The real frame is a
            // little smaller under the browser bars.
            viewport: Viewport {
                width: 402,
                height: 874,
            },
            device_scale_factor: 3,
            // The reference iPhone runs iOS 26, whose Safari has WebGPU on. Version 1 assumed
            // iOS 18, with WebGPU off.
            webgpu: true,
            pcm_rate_hz: 48_000,
            pcm_low_rate_hz: 24_000,
        }
    }

    /// The name and version a result records for this profile.
    pub fn reference(&self) -> ProfileRef {
        ProfileRef {
            name: self.name.clone(),
            version: self.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_profile_resolves_to_itself() {
        for name in HostProfile::NAMES {
            assert_eq!(HostProfile::named(name).unwrap().name, *name);
        }
        assert!(HostProfile::NAMES.contains(&DEFAULT_HOST_PROFILE));
    }
}
