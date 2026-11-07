//! Events from the in-page probe, which runs inside the game frame before game code.
//!
//! The lab build and the field build send the same events. The probe wraps only calls that run
//! at load or once per frame, never per-draw calls. It records raw facts and never scores: Rust
//! turns these events into measures.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One probe event. `t_ms` is `performance.now()` in the game frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProbeEvent {
    /// The first `requestAnimationFrame` callback ran.
    FirstFrame { t_ms: f64 },
    /// A message crossed between the game frame and the host page, such as `reels:ready`.
    HostMessage {
        direction: MessageDirection,
        name: String,
        t_ms: f64,
    },
    /// `decodeAudioData` finished. Decoded bytes are length x channels x 4.
    AudioDecoded {
        length: u64,
        channels: u32,
        sample_rate: u32,
        t_ms: f64,
    },
    /// An audio context changed state.
    AudioContextState {
        state: AudioState,
        sample_rate: u32,
        t_ms: f64,
    },
    /// A texture upload, such as `texImage2D` or `compressedTexImage2D`.
    TextureUploaded {
        /// The probe's number for the texture, which a later upload or `texture_deleted`
        /// repeats. None from a probe that does not number textures.
        #[serde(default)]
        texture: Option<u32>,
        /// The cube map face, 0 to 5; 0 for a 2D texture. None for `texStorage2D` on a cube
        /// map, which allocates every face at once.
        #[serde(default)]
        face: Option<u32>,
        /// The mip level. None for `texStorage2D`, which allocates every level at once.
        #[serde(default)]
        level: Option<u32>,
        width: u32,
        height: u32,
        bytes: u64,
        /// The path of the http image or video it came from, without host, query or fragment.
        /// The lab probe follows a fetched file to an ImageBitmap, a `blob:` object URL or a
        /// response made from it (also through Pixi 8's worker). None for data, a render target,
        /// a canvas, or a `data:` URL. An upload by `texSubImage2D` into `texStorage2D` storage is
        /// not seen, so its source is not either.
        #[serde(default)]
        source: Option<String>,
        /// What an upload with no source held, when the lab probe can tell: `empty` for an
        /// allocation with no pixels, `canvas` for a canvas the game drew. None otherwise, and
        /// always from the field probe.
        #[serde(default)]
        source_kind: Option<SourceKind>,
        t_ms: f64,
    },
    /// The game deleted a texture.
    TextureDeleted { texture: u32, t_ms: f64 },
    /// A canvas got a rendering context.
    ContextCreated {
        /// The probe's number for the canvas, which its `canvas_resized` events repeat. None from
        /// a probe that does not number canvases.
        #[serde(default)]
        canvas: Option<u32>,
        kind: ContextKind,
        /// The canvas's backing size in pixels when it got its context.
        width: u32,
        height: u32,
        antialias: Option<bool>,
        device_pixel_ratio: f64,
        t_ms: f64,
    },
    /// A canvas with a rendering context got a new backing size, as when Pixi sizes its canvas
    /// to the screen after it starts.
    CanvasResized {
        canvas: u32,
        width: u32,
        height: u32,
        t_ms: f64,
    },
    /// A WebGL context was lost.
    ContextLost { t_ms: f64 },
    /// Frame times since the last histogram, sent every `FRAME_TIMES_EVERY_MS`. `counts[i]`
    /// counts frames that took at most `bucket_upper_ms[i]`, and more than the bucket before it.
    /// The edges are `FRAME_TIME_BUCKETS_MS`.
    FrameTimes {
        bucket_upper_ms: Vec<u32>,
        counts: Vec<u32>,
        t_ms: f64,
    },
    /// An optional game hook (Phase 2). Only the game knows these events.
    GameHook {
        hook: GameHookKind,
        detail: Option<String>,
        t_ms: f64,
    },
}

/// The upper edges of the frame time histogram, in ms. The last edge holds every longer frame.
/// The probe repeats these values, and a test in `dyno-cdp` checks that they match.
pub const FRAME_TIME_BUCKETS_MS: [u32; 7] = [17, 34, 50, 100, 250, 1_000, u32::MAX];

/// How often the probe sends a frame time histogram, in ms.
pub const FRAME_TIMES_EVERY_MS: u32 = 1_000;

/// Which way a host message went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    ToHost,
    FromHost,
}

/// An audio context's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AudioState {
    Running,
    Suspended,
    Closed,
}

/// What a texture upload with no source path held. Facts only: an allocation
/// with no pixels is often a render target, but the probe does not see what fills it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// No pixels: `texStorage2D`, or `texImage2D` with null pixels.
    Empty,
    /// A canvas the game drew, such as text.
    Canvas,
}

impl SourceKind {
    /// The item key in `lab.textures.uploaded`.
    pub fn name(self) -> &'static str {
        match self {
            SourceKind::Empty => "empty",
            SourceKind::Canvas => "canvas",
        }
    }
}

/// A canvas rendering context's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Webgl,
    Webgl2,
    Webgpu,
    Canvas2d,
}

/// An optional game hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum GameHookKind {
    /// The game can take a spin. Bytes to playable stop here.
    Playable,
    SpinStart,
    RoundSettled,
    /// The game moved to a new phase, named in `detail`.
    GamePhase,
}
