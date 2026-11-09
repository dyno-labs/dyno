//! The fixed ids: every measure, rule and budget that Phases 1 and 2 name.
//!
//! Each id is a dotted name in JSON. Its first part says where the value comes from, except that
//! the demo spin loop uses `play`. Each table row also gives the spec that the rest of dyno reads,
//! so one row is the whole definition.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::measure::{Covers, Source, Unit};

/// Defines an id enum whose variants serialize as dotted names, with a spec per variant.
macro_rules! dotted_ids {
    (
        $(#[doc = $enum_doc:literal])*
        pub enum $name:ident -> $spec:ty {
            $(
                $(#[doc = $doc:literal])*
                $variant:ident = $id:literal => $spec_value:expr,
            )*
        }
    ) => {
        $(#[doc = $enum_doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
        pub enum $name {
            $(
                $(#[doc = $doc])*
                #[serde(rename = $id)]
                $variant,
            )*
        }

        impl $name {
            /// Every id, in table order.
            pub const ALL: &[$name] = &[$($name::$variant),*];

            /// The id as it appears in JSON. Lists of these ids sort by it.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $id,)*
                }
            }

            /// What dyno knows about this id.
            pub fn spec(self) -> $spec {
                match self {
                    $($name::$variant => $spec_value,)*
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

/// The plan phase that builds a measure, rule or check: 1 or 2.
pub type Phase = u8;

/// What dyno knows about a measure before it measures anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasureSpec {
    /// Where the value comes from.
    pub source: Source,
    /// The plan phase that builds the measure.
    pub phase: Phase,
    /// The unit of the value.
    pub unit: Unit,
    /// What the value covers.
    pub covers: Covers,
    /// What the value counts. Several ids can count one thing, such as textures in the files
    /// and textures a run uploaded.
    pub quantity: Quantity,
    /// Which reading of the quantity the value is.
    pub statistic: Statistic,
}

/// What a measure counts, whatever the source or the reading. A site groups measures by it, so
/// the textures a build holds and the textures a run uploaded sit in one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Quantity {
    /// Files in the build.
    Files,
    /// Bytes to download: the files as they are, or what a run received over the network.
    DownloadBytes,
    /// Gzip bytes of the text files.
    CompressedBytes,
    /// Bytes in files that repeat another file.
    DuplicateBytes,
    /// Texture bytes.
    Textures,
    /// Decoded audio bytes.
    Audio,
    /// Stereo sound groups.
    StereoSounds,
    /// Stereo sound groups that a mono export would not change.
    MonoSafeStereoSounds,
    /// Silence in a Howler sprite map.
    SpriteSilence,
    /// Silence in the decoded sounds.
    AudioSilence,
    /// Whether the renderer turns antialias on.
    Antialias,
    /// Whether the render resolution is above the cap.
    ResolutionUncapped,
    /// The renderer: webgl, webgl2, webgpu or canvas2d.
    RenderBackend,
    /// Canvas buffer bytes.
    Canvas,
    /// Libraries in the bundle.
    Libraries,
    /// URLs that point outside the build.
    ExternalUrls,
    /// JavaScript bytes per source module.
    ScriptBytes,
    /// The time to the first animation frame.
    FirstFrameTime,
    /// The time to the host profile's ready message.
    ReadyTime,
    /// The audio context's sample rate.
    AudioRate,
    /// Canvas rendering contexts.
    CanvasContexts,
    /// The JavaScript heap.
    JsHeap,
    /// Whether an audio context ran.
    AudioRunning,
    /// Frames rendered a second.
    FrameRate,
    /// Audio decode and texture upload bytes.
    AllocatedBytes,
    /// The game frame's device pixel ratio.
    FrameDpr,
    /// Requests the game frame made.
    FrameRequests,
    /// The WebGL renderer string.
    WebglRenderer,
    /// The physical footprint of the game's process: the `phys_footprint` that iOS reads to
    /// kill a process.
    Footprint,
    /// Spins played.
    Spins,
    /// Memory during the spin loop. The play measures are not built yet, so which memory they
    /// read is not settled.
    Memory,
    /// Runs that the phone killed.
    Kills,
}

/// Which reading of a quantity a measure is: over the build's files, by idle, at idle or at its
/// peak, for example.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Statistic {
    /// Every file in the build: what the files hold, not what a run loaded.
    AllFiles,
    /// Every file in the build, with audio decoded at 48 kHz.
    AllFilesAt48k,
    /// Every file in the build, with audio decoded at 24 kHz.
    AllFilesAt24k,
    /// The largest of several, such as the sprite map with the most silence.
    Largest,
    /// A time from the navigation.
    SinceNavigation,
    /// What the run loaded or made from the navigation until idle.
    ByIdle,
    /// What the run loaded before the game's playable hook.
    ToPlayable,
    /// One reading when the frame went idle.
    AtIdle,
    /// While the frame was inactive, before the host made it active.
    WhileInactive,
    /// The largest reading in the run window, from the navigation to the end of the run.
    Peak,
    /// The median of the game runs' peaks, minus the median of the empty build runs' peaks.
    MedianPeakOverBaseline,
    /// The model's value while the game is active.
    ModelledActive,
    /// The model's value while the game is inactive.
    ModelledInactive,
    /// Over the whole spin loop.
    DuringSpins,
    /// The largest reading during the spin loop.
    PeakDuringSpins,
    /// The slope per 100 spins.
    GrowthPer100Spins,
    /// The largest reading during a bonus round.
    PeakDuringBonus,
    /// How many of the runs it happened in.
    OfRuns,
}

impl MeasureSpec {
    const fn static_scan(
        phase: Phase,
        unit: Unit,
        quantity: Quantity,
        statistic: Statistic,
    ) -> MeasureSpec {
        MeasureSpec {
            source: Source::Static,
            phase,
            unit,
            covers: Covers::SuppliedFiles,
            quantity,
            statistic,
        }
    }

    const fn lab(
        phase: Phase,
        unit: Unit,
        quantity: Quantity,
        statistic: Statistic,
    ) -> MeasureSpec {
        MeasureSpec {
            source: Source::Lab,
            phase,
            unit,
            covers: Covers::LoadToIdle,
            quantity,
            statistic,
        }
    }

    /// A lab measure of the demo spin loop.
    const fn play(
        phase: Phase,
        unit: Unit,
        quantity: Quantity,
        statistic: Statistic,
    ) -> MeasureSpec {
        MeasureSpec {
            source: Source::Lab,
            phase,
            unit,
            covers: Covers::DemoSpins,
            quantity,
            statistic,
        }
    }

    const fn model(phase: Phase, unit: Unit, statistic: Statistic) -> MeasureSpec {
        MeasureSpec {
            source: Source::Model,
            phase,
            unit,
            covers: Covers::ModelInputs,
            quantity: Quantity::Footprint,
            statistic,
        }
    }

    const fn device(
        phase: Phase,
        unit: Unit,
        quantity: Quantity,
        statistic: Statistic,
    ) -> MeasureSpec {
        MeasureSpec {
            source: Source::Device,
            phase,
            unit,
            covers: Covers::FreshDeviceRuns,
            quantity,
            statistic,
        }
    }
}

dotted_ids! {
    /// Every measure that Phases 1 and 2 name.
    pub enum MeasureId -> MeasureSpec {
        /// Number of supplied files.
        StaticInventoryFiles = "static.inventory.files" => MeasureSpec::static_scan(1, Unit::Count, Quantity::Files, Statistic::AllFiles),
        /// Bytes of every supplied file, with one item per file kind.
        StaticBytesByType = "static.bytes.by_type" => MeasureSpec::static_scan(1, Unit::Bytes, Quantity::DownloadBytes, Statistic::AllFiles),
        /// Gzip bytes of the text files (js, css, html, data), with one item per file kind.
        StaticBytesCompressed = "static.bytes.compressed" => MeasureSpec::static_scan(1, Unit::Bytes, Quantity::CompressedBytes, Statistic::AllFiles),
        /// Bytes in files that repeat another file byte for byte (same SHA-256), one item per copy.
        StaticBytesDuplicates = "static.bytes.duplicates" => MeasureSpec::static_scan(1, Unit::Bytes, Quantity::DuplicateBytes, Statistic::AllFiles),
        /// Decoded texture bytes of all supplied images, one copy: width x height x 4, plus the mip
        /// levels the engine or the file makes. The uncertainty gives the range up to two copies.
        StaticTexturesDecoded = "static.textures.decoded" => MeasureSpec::static_scan(1, Unit::Bytes, Quantity::Textures, Statistic::AllFiles),
        /// Decoded audio bytes at 48 kHz: seconds x channels x 48,000 x 4, one item per sound
        /// group.
        StaticAudioDecoded48k = "static.audio.decoded_48k" => MeasureSpec::static_scan(1, Unit::Bytes, Quantity::Audio, Statistic::AllFilesAt48k),
        /// Decoded audio bytes at 24 kHz, one item per sound group.
        StaticAudioDecoded24k = "static.audio.decoded_24k" => MeasureSpec::static_scan(1, Unit::Bytes, Quantity::Audio, Statistic::AllFilesAt24k),
        /// Number of stereo sound groups, one item per group's representative file.
        StaticAudioStereoFiles = "static.audio.stereo_files" => MeasureSpec::static_scan(1, Unit::Count, Quantity::StereoSounds, Statistic::AllFiles),
        /// Number of stereo sound groups whose two channels match once decoded, so a mono export
        /// sounds the same. One item per decoded stereo group, with what a mono export does to it:
        /// `safe_to_make_mono`, `changes_the_sound` or `cancels`.
        StaticAudioStereoMonoSafe = "static.audio.stereo_mono_safe" => MeasureSpec::static_scan(1, Unit::Count, Quantity::MonoSafeStereoSounds, Statistic::AllFiles),
        /// Largest share of silence in a Howler sprite map, one item per sprite map.
        StaticAudioSpriteSilence = "static.audio.sprite_silence" => MeasureSpec::static_scan(1, Unit::BasisPoints, Quantity::SpriteSilence, Statistic::Largest),
        /// Share of the decoded time of every sound group that is silent, in 10 ms windows at or
        /// below -60 dBFS. One item per decoded group.
        StaticAudioSilenceByDecode = "static.audio.silence_by_decode" => MeasureSpec::static_scan(1, Unit::BasisPoints, Quantity::AudioSilence, Statistic::AllFiles),
        /// Whether the bundle turns antialias on.
        StaticRenderAntialias = "static.render.antialias" => MeasureSpec::static_scan(1, Unit::Flag, Quantity::Antialias, Statistic::AllFiles),
        /// Whether the bundle renders at a resolution above 2, the cap that a fix suggests.
        StaticRenderResolutionUncapped = "static.render.resolution_uncapped" => MeasureSpec::static_scan(1, Unit::Flag, Quantity::ResolutionUncapped, Statistic::AllFiles),
        /// The renderer the bundle asks for: webgl, webgl2, webgpu or canvas2d.
        StaticRenderBackend = "static.render.backend" => MeasureSpec::static_scan(1, Unit::Text, Quantity::RenderBackend, Statistic::AllFiles),
        /// Canvas bytes: one WebGL context plus the backing store at the bundle's resolution.
        StaticRenderCanvas = "static.render.canvas" => MeasureSpec::static_scan(1, Unit::Bytes, Quantity::Canvas, Statistic::AllFiles),
        /// Number of libraries found in the bundle, one item per library with its version, keyed
        /// `<bucket>.<library>` such as `rendering.pixi` (`library::Library`).
        StaticBundleEngines = "static.bundle.engines" => MeasureSpec::static_scan(1, Unit::Count, Quantity::Libraries, Statistic::AllFiles),
        /// Number of URLs in the bundle that point outside the build, one item per URL.
        /// The build id does not cover them.
        StaticBundleExternalUrls = "static.bundle.external_urls" => MeasureSpec::static_scan(1, Unit::Count, Quantity::ExternalUrls, Statistic::AllFiles),
        /// JavaScript bytes per source module, from source maps.
        StaticBundleSourceMap = "static.bundle.source_map" => MeasureSpec::static_scan(2, Unit::Bytes, Quantity::ScriptBytes, Statistic::AllFiles),

        /// Time from navigation to the game frame's first animation frame.
        LabTimeFirstFrame = "lab.time.first_frame" => MeasureSpec::lab(1, Unit::Ms, Quantity::FirstFrameTime, Statistic::SinceNavigation),
        /// Time from navigation to the host profile's ready message, such as `reels:ready`.
        LabTimeReady = "lab.time.ready" => MeasureSpec::lab(1, Unit::Ms, Quantity::ReadyTime, Statistic::SinceNavigation),
        /// Bytes the game frame loaded by idle, one item per file kind.
        LabBytesLoadedByIdle = "lab.bytes.loaded_by_idle" => MeasureSpec::lab(1, Unit::Bytes, Quantity::DownloadBytes, Statistic::ByIdle),
        /// Bytes the game frame loaded before its playable hook. Unsupported when the game sends no
        /// playable hook. The host's ready message is not that signal.
        LabBytesToPlayable = "lab.bytes.to_playable" => MeasureSpec::lab(1, Unit::Bytes, Quantity::DownloadBytes, Statistic::ToPlayable),
        /// Audio bytes decoded by idle, at the audio context's rate: length x channels x 4.
        LabAudioDecoded = "lab.audio.decoded" => MeasureSpec::lab(1, Unit::Bytes, Quantity::Audio, Statistic::ByIdle),
        /// The audio context's sample rate.
        LabAudioContextRate = "lab.audio.context_rate" => MeasureSpec::lab(1, Unit::Hertz, Quantity::AudioRate, Statistic::AtIdle),
        /// Texture bytes uploaded by idle, re-uploads included. One item per source path, such as
        /// `/assets/a.png`. An ImageBitmap, a `blob:` object URL or a response made from a
        /// fetched file keeps that file's path. Uploads with no http source are `empty` (no
        /// pixels), `canvas` (a canvas the game drew) or `other` (data, `data:` URLs, or a field
        /// probe that does not tell). A path always starts with `/`, so it never meets these.
        LabTexturesUploaded = "lab.textures.uploaded" => MeasureSpec::lab(1, Unit::Bytes, Quantity::Textures, Statistic::ByIdle),
        /// Texture bytes the GPU holds at idle: the last upload of each level of each texture the
        /// game did not delete.
        LabTexturesResident = "lab.textures.resident" => MeasureSpec::lab(1, Unit::Bytes, Quantity::Textures, Statistic::AtIdle),
        /// Number of canvas rendering contexts the game created.
        LabCanvasContexts = "lab.canvas.contexts" => MeasureSpec::lab(1, Unit::Count, Quantity::CanvasContexts, Statistic::ByIdle),
        /// Canvas buffer bytes from the contexts' sizes, DPR and antialias.
        LabCanvasBytes = "lab.canvas.bytes" => MeasureSpec::lab(1, Unit::Bytes, Quantity::Canvas, Statistic::AtIdle),
        /// JavaScript heap in use at idle. Chromium's heap, not the iPhone's.
        LabJsHeap = "lab.js_heap" => MeasureSpec::lab(1, Unit::Bytes, Quantity::JsHeap, Statistic::AtIdle),
        /// Whether an audio context ran while the frame was inactive.
        LabInactiveAudioRunning = "lab.inactive.audio_running" => MeasureSpec::lab(1, Unit::Flag, Quantity::AudioRunning, Statistic::WhileInactive),
        /// Frames per second the game rendered while inactive.
        LabInactiveFrameRate = "lab.inactive.frame_rate" => MeasureSpec::lab(1, Unit::Hertz, Quantity::FrameRate, Statistic::WhileInactive),
        /// Audio decode and texture upload bytes before activation. A diagnostic, never a fail.
        LabInactiveAllocated = "lab.inactive.allocated" => MeasureSpec::lab(1, Unit::Bytes, Quantity::AllocatedBytes, Statistic::WhileInactive),
        /// The game frame's device pixel ratio, in thousandths. The run is invalid unless it is 3
        /// (3,000).
        LabSelfCheckFrameDpr = "lab.self_check.frame_dpr" => MeasureSpec::lab(1, Unit::Milli, Quantity::FrameDpr, Statistic::AtIdle),
        /// Requests the game frame made. The run is invalid when it made none.
        LabSelfCheckFrameRequests = "lab.self_check.frame_requests" => MeasureSpec::lab(1, Unit::Count, Quantity::FrameRequests, Statistic::ByIdle),
        /// The WebGL renderer string. CI runners often fall back to a software renderer.
        LabSelfCheckWebglRenderer = "lab.self_check.webgl_renderer" => MeasureSpec::lab(1, Unit::Text, Quantity::WebglRenderer, Statistic::AtIdle),

        /// Modelled iPhone footprint while active: decoded audio at the context rate, textures,
        /// canvas buffers, decoded images that are not textures, and k times the JS heap, plus a
        /// fixed b. The model fit sets k and b.
        ModelFootprint = "model.footprint" => MeasureSpec::model(1, Unit::Bytes, Statistic::ModelledActive),
        /// Modelled iPhone footprint while inactive, for the preloadable tier.
        ModelFootprintInactive = "model.footprint_inactive" => MeasureSpec::model(2, Unit::Bytes, Statistic::ModelledInactive),

        /// Spins played in the demo-only session.
        PlaySpins = "play.spins" => MeasureSpec::play(2, Unit::Count, Quantity::Spins, Statistic::DuringSpins),
        /// Peak bytes during the spin loop.
        PlayPeak = "play.peak" => MeasureSpec::play(2, Unit::Bytes, Quantity::Memory, Statistic::PeakDuringSpins),
        /// Memory growth per 100 spins (Theil-Sen slope). Negative when memory shrinks.
        PlayGrowthPer100Spins = "play.growth_per_100_spins" => MeasureSpec::play(2, Unit::BytesChange, Quantity::Memory, Statistic::GrowthPer100Spins),
        /// Peak bytes during a bonus round.
        PlayBonusPeak = "play.bonus_peak" => MeasureSpec::play(2, Unit::Bytes, Quantity::Memory, Statistic::PeakDuringBonus),

        /// The `phys_footprint` of the game's process on the iPhone, at the first sample after the
        /// frame went idle. Its peak in the run window is an item too, for readers from before
        /// `device.phys_footprint_peak`.
        DevicePhysFootprint = "device.phys_footprint" => MeasureSpec::device(2, Unit::Bytes, Quantity::Footprint, Statistic::AtIdle),
        /// The largest `phys_footprint` of the game's process on the iPhone in the run window,
        /// from the answer to the page's `hello` to the end of the run, with the slack of a sample
        /// at each end that places the run on the trace's clock.
        DevicePhysFootprintPeak = "device.phys_footprint_peak" => MeasureSpec::device(2, Unit::Bytes, Quantity::Footprint, Statistic::Peak),
        /// The median `phys_footprint` peak of N runs of the game, minus the median peak of N runs
        /// of the empty build, on the same phone. Needs `--runs`. A difference, so a game within
        /// the noise of the empty page can give a value below 0.
        DeviceFootprintOverBaseline = "device.footprint_over_baseline" => MeasureSpec::device(2, Unit::BytesChange, Quantity::Footprint, Statistic::MedianPeakOverBaseline),
        /// Runs that the iPhone killed: the game's process left the trace before the run ended, as
        /// a process that iOS kills does. One item per kill.
        DeviceKills = "device.kills" => MeasureSpec::device(2, Unit::Count, Quantity::Kills, Statistic::OfRuns),
    }
}

/// One row of `packages/schema/json/measures.json`: a measure id and its spec, so a site can
/// label and group measures with no copy of this table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MeasureEntry {
    pub id: MeasureId,
    pub source: Source,
    pub phase: Phase,
    pub unit: Unit,
    pub covers: Covers,
    pub quantity: Quantity,
    pub statistic: Statistic,
}

impl MeasureEntry {
    /// Every measure id with its spec, in table order.
    pub fn all() -> Vec<MeasureEntry> {
        MeasureId::ALL
            .iter()
            .map(|&id| {
                let spec = id.spec();
                MeasureEntry {
                    id,
                    source: spec.source,
                    phase: spec.phase,
                    unit: spec.unit,
                    covers: spec.covers,
                    quantity: spec.quantity,
                    statistic: spec.statistic,
                }
            })
            .collect()
    }
}

/// What dyno knows about a rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleSpec {
    /// Static rules give deterministic findings. Lab rules give run findings.
    pub source: Source,
    /// The plan phase that builds the rule.
    pub phase: Phase,
    /// The measures the rule reads.
    pub measures: &'static [MeasureId],
}

dotted_ids! {
    /// Every rule that Phases 1 and 2 name. Field reports reuse these ids.
    pub enum RuleId -> RuleSpec {
        /// Decoded audio is over the audio budget.
        AudioPcmOverBudget = "audio.pcm_over_budget" => RuleSpec {
            source: Source::Static,
            phase: 1,
            measures: &[MeasureId::StaticAudioDecoded48k, MeasureId::StaticAudioDecoded24k],
        },
        /// An audio file is stereo.
        AudioStereo = "audio.stereo" => RuleSpec {
            source: Source::Static,
            phase: 1,
            measures: &[MeasureId::StaticAudioStereoFiles, MeasureId::StaticAudioStereoMonoSafe],
        },
        /// A Howler sprite map is mostly silence.
        AudioSilenceShare = "audio.silence_share" => RuleSpec {
            source: Source::Static,
            phase: 1,
            measures: &[MeasureId::StaticAudioSpriteSilence],
        },
        /// The bundle turns antialias on or leaves the resolution uncapped.
        RenderAntialiasOrResolution = "render.antialias_or_resolution" => RuleSpec {
            source: Source::Static,
            phase: 1,
            measures: &[MeasureId::StaticRenderAntialias, MeasureId::StaticRenderResolutionUncapped],
        },
        /// Decoded textures are over the texture budget. Names the top 5 files.
        TexturesOverBudget = "textures.over_budget" => RuleSpec {
            source: Source::Static,
            phase: 1,
            measures: &[MeasureId::StaticTexturesDecoded],
        },
        /// Two or more files have the same SHA-256.
        FilesDuplicate = "files.duplicate" => RuleSpec {
            source: Source::Static,
            phase: 1,
            measures: &[MeasureId::StaticBytesDuplicates],
        },
        /// A supplied file that nothing references. Needs the asset graph.
        FilesUnreferenced = "files.unreferenced" => RuleSpec {
            source: Source::Static,
            phase: 2,
            measures: &[MeasureId::StaticInventoryFiles],
        },
        /// Flags a ready message that came after the host profile's limit (5 s for Reels), or
        /// never came.
        ContractReadyInTime = "contract.ready_in_time" => RuleSpec {
            source: Source::Lab,
            phase: 1,
            measures: &[MeasureId::LabTimeReady],
        },
        /// Flags an audio context that ran before activation.
        ContractNoAudioBeforeActive = "contract.no_audio_before_active" => RuleSpec {
            source: Source::Lab,
            phase: 1,
            measures: &[MeasureId::LabInactiveAudioRunning],
        },
        /// Flags rendering that was not throttled while inactive: more than 1 frame a second.
        ContractThrottledWhileInactive = "contract.throttled_while_inactive" => RuleSpec {
            source: Source::Lab,
            phase: 1,
            measures: &[MeasureId::LabInactiveFrameRate],
        },
        /// Flags audio decode or texture upload work before activation. Never a fail.
        DiagnosticAllocBeforeActive = "diagnostic.alloc_before_active" => RuleSpec {
            source: Source::Lab,
            phase: 1,
            measures: &[MeasureId::LabInactiveAllocated],
        },
    }
}

/// What dyno knows about a budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetSpec {
    /// The measure the budget limits. For a rule's threshold, the measure the rule reads.
    pub measure: MeasureId,
    /// Who compares a value with the limit.
    pub checked_by: CheckedBy,
}

/// Who compares a value with a budget's limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedBy {
    /// `dyno-score` compares the measure's value. The verdict can set the status.
    Score,
    /// The limit is the rule's threshold, because `dyno-score` cannot compare it with the measure's
    /// value. The rule's finding gives the outcome, and the finding can set the status.
    Rule(RuleId),
}

impl BudgetSpec {
    const fn score(measure: MeasureId) -> BudgetSpec {
        BudgetSpec {
            measure,
            checked_by: CheckedBy::Score,
        }
    }

    const fn rule(rule: RuleId, measure: MeasureId) -> BudgetSpec {
        BudgetSpec {
            measure,
            checked_by: CheckedBy::Rule(rule),
        }
    }
}

dotted_ids! {
    /// Every budget a budget file can set. Each id is the key path in the file.
    pub enum BudgetId -> BudgetSpec {
        /// Decoded audio at 48 kHz.
        AudioDecodedMb = "audio.decoded_mb" => BudgetSpec::score(MeasureId::StaticAudioDecoded48k),
        /// Decoded bytes at 48 kHz that a mono export of every stereo sound would save.
        AudioStereoSavingMb = "audio.stereo_saving_mb" => BudgetSpec::rule(
            RuleId::AudioStereo,
            MeasureId::StaticAudioStereoFiles,
        ),
        /// A sprite map's share of time that no sprite covers.
        AudioSilenceShareBp = "audio.silence_share_bp" => BudgetSpec::rule(
            RuleId::AudioSilenceShare,
            MeasureId::StaticAudioSpriteSilence,
        ),
        /// Decoded bytes at 48 kHz in a sprite map's time that no sprite covers.
        AudioSilenceSavingMb = "audio.silence_saving_mb" => BudgetSpec::rule(
            RuleId::AudioSilenceShare,
            MeasureId::StaticAudioSpriteSilence,
        ),
        /// Decoded textures, one copy.
        TexturesDecodedMb = "textures.decoded_mb" => BudgetSpec::score(
            MeasureId::StaticTexturesDecoded,
        ),
        /// Antialias on.
        RenderAntialias = "render.antialias" => BudgetSpec::score(MeasureId::StaticRenderAntialias),
        /// Resolution uncapped.
        RenderResolutionUncapped = "render.resolution_uncapped" => BudgetSpec::score(
            MeasureId::StaticRenderResolutionUncapped,
        ),
        /// Two or more files with the same SHA-256.
        FilesDuplicate = "files.duplicate" => BudgetSpec::rule(
            RuleId::FilesDuplicate,
            MeasureId::StaticBytesDuplicates,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_name<T: Serialize>(id: T) -> String {
        serde_json::to_value(id)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn as_str_matches_the_json_name() {
        for id in MeasureId::ALL {
            assert_eq!(id.as_str(), json_name(id));
        }
        for id in RuleId::ALL {
            assert_eq!(id.as_str(), json_name(id));
        }
        for id in BudgetId::ALL {
            assert_eq!(id.as_str(), json_name(id));
        }
    }

    #[test]
    fn measure_ids_start_with_their_source_or_play() {
        for id in MeasureId::ALL {
            let spec = id.spec();
            let prefix = match spec.source {
                Source::Static => "static.",
                Source::Lab if spec.covers == Covers::DemoSpins => "play.",
                Source::Lab => "lab.",
                Source::Model => "model.",
                Source::Device => "device.",
                Source::Field => "field.",
            };
            assert!(
                id.as_str().starts_with(prefix),
                "{id} does not start with {prefix}"
            );
        }
    }

    #[test]
    fn a_source_reads_each_quantity_once_per_statistic() {
        let mut seen = std::collections::HashSet::new();
        for id in MeasureId::ALL {
            let spec = id.spec();
            assert!(
                seen.insert((spec.source, spec.quantity, spec.statistic)),
                "{id} repeats the quantity and statistic of another {:?} measure",
                spec.source
            );
        }
    }

    #[test]
    fn the_device_footprint_has_a_reading_at_idle_and_a_peak() {
        let footprint = MeasureId::DevicePhysFootprint.spec();
        let peak = MeasureId::DevicePhysFootprintPeak.spec();
        assert_eq!(footprint.quantity, Quantity::Footprint);
        assert_eq!(peak.quantity, Quantity::Footprint);
        assert_eq!(footprint.statistic, Statistic::AtIdle);
        assert_eq!(peak.statistic, Statistic::Peak);
    }

    #[test]
    fn every_measure_has_an_entry_in_table_order() {
        let entries = MeasureEntry::all();
        let ids: Vec<MeasureId> = entries.iter().map(|entry| entry.id).collect();
        assert_eq!(ids, MeasureId::ALL);
        let json = serde_json::to_value(&entries[0]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": "static.inventory.files",
                "source": "static",
                "phase": 1,
                "unit": "count",
                "covers": "supplied_files",
                "quantity": "files",
                "statistic": "all_files",
            })
        );
    }

    #[test]
    fn budgets_limit_static_measures() {
        for id in BudgetId::ALL {
            assert_eq!(id.spec().measure.spec().source, Source::Static, "{id}");
        }
    }

    #[test]
    fn a_rule_threshold_belongs_to_a_static_rule_that_reads_its_measure() {
        for id in BudgetId::ALL {
            let spec = id.spec();
            if let CheckedBy::Rule(rule) = spec.checked_by {
                assert_eq!(rule.spec().source, Source::Static, "{id}");
                assert!(rule.spec().measures.contains(&spec.measure), "{id}");
            }
        }
    }
}
