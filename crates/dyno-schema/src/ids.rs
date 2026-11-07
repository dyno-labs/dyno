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
}

impl MeasureSpec {
    const fn static_scan(phase: Phase, unit: Unit) -> MeasureSpec {
        MeasureSpec {
            source: Source::Static,
            phase,
            unit,
            covers: Covers::SuppliedFiles,
        }
    }

    const fn lab(phase: Phase, unit: Unit) -> MeasureSpec {
        MeasureSpec {
            source: Source::Lab,
            phase,
            unit,
            covers: Covers::LoadToIdle,
        }
    }

    /// A lab measure of the demo spin loop.
    const fn play(phase: Phase, unit: Unit) -> MeasureSpec {
        MeasureSpec {
            source: Source::Lab,
            phase,
            unit,
            covers: Covers::DemoSpins,
        }
    }

    const fn model(phase: Phase, unit: Unit) -> MeasureSpec {
        MeasureSpec {
            source: Source::Model,
            phase,
            unit,
            covers: Covers::ModelInputs,
        }
    }

    const fn device(phase: Phase, unit: Unit) -> MeasureSpec {
        MeasureSpec {
            source: Source::Device,
            phase,
            unit,
            covers: Covers::FreshDeviceRuns,
        }
    }
}

dotted_ids! {
    /// Every measure that Phases 1 and 2 name.
    pub enum MeasureId -> MeasureSpec {
        /// Number of supplied files.
        StaticInventoryFiles = "static.inventory.files" => MeasureSpec::static_scan(1, Unit::Count),
        /// Bytes of every supplied file, with one item per file kind.
        StaticBytesByType = "static.bytes.by_type" => MeasureSpec::static_scan(1, Unit::Bytes),
        /// Gzip bytes of the text files (js, css, html, data), with one item per file kind.
        StaticBytesCompressed = "static.bytes.compressed" => MeasureSpec::static_scan(1, Unit::Bytes),
        /// Bytes in files that repeat another file byte for byte (same SHA-256), one item per copy.
        StaticBytesDuplicates = "static.bytes.duplicates" => MeasureSpec::static_scan(1, Unit::Bytes),
        /// Decoded texture bytes of all supplied images, one copy: width x height x 4, plus the mip
        /// levels the engine or the file makes. The uncertainty gives the range up to two copies.
        StaticTexturesDecoded = "static.textures.decoded" => MeasureSpec::static_scan(1, Unit::Bytes),
        /// Decoded audio bytes at 48 kHz: seconds x channels x 48,000 x 4, one item per sound
        /// group.
        StaticAudioDecoded48k = "static.audio.decoded_48k" => MeasureSpec::static_scan(1, Unit::Bytes),
        /// Decoded audio bytes at 24 kHz, one item per sound group.
        StaticAudioDecoded24k = "static.audio.decoded_24k" => MeasureSpec::static_scan(1, Unit::Bytes),
        /// Number of stereo sound groups, one item per group's representative file.
        StaticAudioStereoFiles = "static.audio.stereo_files" => MeasureSpec::static_scan(1, Unit::Count),
        /// Number of stereo sound groups whose two channels match once decoded, so a mono export
        /// sounds the same. One item per decoded stereo group, with what a mono export does to it:
        /// `safe_to_make_mono`, `changes_the_sound` or `cancels`.
        StaticAudioStereoMonoSafe = "static.audio.stereo_mono_safe" => MeasureSpec::static_scan(1, Unit::Count),
        /// Largest share of silence in a Howler sprite map, one item per sprite map.
        StaticAudioSpriteSilence = "static.audio.sprite_silence" => MeasureSpec::static_scan(1, Unit::BasisPoints),
        /// Share of the decoded time of every sound group that is silent, in 10 ms windows at or
        /// below -60 dBFS. One item per decoded group.
        StaticAudioSilenceByDecode = "static.audio.silence_by_decode" => MeasureSpec::static_scan(1, Unit::BasisPoints),
        /// Whether the bundle turns antialias on.
        StaticRenderAntialias = "static.render.antialias" => MeasureSpec::static_scan(1, Unit::Flag),
        /// Whether the bundle renders at a resolution above 2, the cap that a fix suggests.
        StaticRenderResolutionUncapped = "static.render.resolution_uncapped" => MeasureSpec::static_scan(1, Unit::Flag),
        /// The renderer the bundle asks for: webgl, webgl2, webgpu or canvas2d.
        StaticRenderBackend = "static.render.backend" => MeasureSpec::static_scan(1, Unit::Text),
        /// Canvas bytes: one WebGL context plus the backing store at the bundle's resolution.
        StaticRenderCanvas = "static.render.canvas" => MeasureSpec::static_scan(1, Unit::Bytes),
        /// Number of libraries found in the bundle, one item per library with its version, keyed
        /// `<bucket>.<library>` such as `rendering.pixi` (`library::Library`).
        StaticBundleEngines = "static.bundle.engines" => MeasureSpec::static_scan(1, Unit::Count),
        /// Number of URLs in the bundle that point outside the build, one item per URL.
        /// The build id does not cover them.
        StaticBundleExternalUrls = "static.bundle.external_urls" => MeasureSpec::static_scan(1, Unit::Count),
        /// JavaScript bytes per source module, from source maps.
        StaticBundleSourceMap = "static.bundle.source_map" => MeasureSpec::static_scan(2, Unit::Bytes),

        /// Time from navigation to the game frame's first animation frame.
        LabTimeFirstFrame = "lab.time.first_frame" => MeasureSpec::lab(1, Unit::Ms),
        /// Time from navigation to the host profile's ready message, such as `reels:ready`.
        LabTimeReady = "lab.time.ready" => MeasureSpec::lab(1, Unit::Ms),
        /// Bytes the game frame loaded by idle, one item per file kind.
        LabBytesLoadedByIdle = "lab.bytes.loaded_by_idle" => MeasureSpec::lab(1, Unit::Bytes),
        /// Bytes the game frame loaded before its playable hook. Unsupported when the game sends no
        /// playable hook. The host's ready message is not that signal.
        LabBytesToPlayable = "lab.bytes.to_playable" => MeasureSpec::lab(1, Unit::Bytes),
        /// Audio bytes decoded by idle, at the audio context's rate: length x channels x 4.
        LabAudioDecoded = "lab.audio.decoded" => MeasureSpec::lab(1, Unit::Bytes),
        /// The audio context's sample rate.
        LabAudioContextRate = "lab.audio.context_rate" => MeasureSpec::lab(1, Unit::Hertz),
        /// Texture bytes uploaded by idle, re-uploads included. One item per source path, such as
        /// `/assets/a.png`. An ImageBitmap, a `blob:` object URL or a response made from a
        /// fetched file keeps that file's path. Uploads with no http source are `empty` (no
        /// pixels), `canvas` (a canvas the game drew) or `other` (data, `data:` URLs, or a field
        /// probe that does not tell). A path always starts with `/`, so it never meets these.
        LabTexturesUploaded = "lab.textures.uploaded" => MeasureSpec::lab(1, Unit::Bytes),
        /// Texture bytes the GPU holds at idle: the last upload of each level of each texture the
        /// game did not delete.
        LabTexturesResident = "lab.textures.resident" => MeasureSpec::lab(1, Unit::Bytes),
        /// Number of canvas rendering contexts the game created.
        LabCanvasContexts = "lab.canvas.contexts" => MeasureSpec::lab(1, Unit::Count),
        /// Canvas buffer bytes from the contexts' sizes, DPR and antialias.
        LabCanvasBytes = "lab.canvas.bytes" => MeasureSpec::lab(1, Unit::Bytes),
        /// JavaScript heap in use at idle. Chromium's heap, not the iPhone's.
        LabJsHeap = "lab.js_heap" => MeasureSpec::lab(1, Unit::Bytes),
        /// Whether an audio context ran while the frame was inactive.
        LabInactiveAudioRunning = "lab.inactive.audio_running" => MeasureSpec::lab(1, Unit::Flag),
        /// Frames per second the game rendered while inactive.
        LabInactiveFrameRate = "lab.inactive.frame_rate" => MeasureSpec::lab(1, Unit::Hertz),
        /// Audio decode and texture upload bytes before activation. A diagnostic, never a fail.
        LabInactiveAllocated = "lab.inactive.allocated" => MeasureSpec::lab(1, Unit::Bytes),
        /// The game frame's device pixel ratio, in thousandths. The run is invalid unless it is 3
        /// (3,000).
        LabSelfCheckFrameDpr = "lab.self_check.frame_dpr" => MeasureSpec::lab(1, Unit::Milli),
        /// Requests the game frame made. The run is invalid when it made none.
        LabSelfCheckFrameRequests = "lab.self_check.frame_requests" => MeasureSpec::lab(1, Unit::Count),
        /// The WebGL renderer string. CI runners often fall back to a software renderer.
        LabSelfCheckWebglRenderer = "lab.self_check.webgl_renderer" => MeasureSpec::lab(1, Unit::Text),

        /// Modelled iPhone footprint while active: decoded audio at the context rate, textures,
        /// canvas buffers, decoded images that are not textures, and k times the JS heap, plus a
        /// fixed b. The model fit sets k and b.
        ModelFootprint = "model.footprint" => MeasureSpec::model(1, Unit::Bytes),
        /// Modelled iPhone footprint while inactive, for the preloadable tier.
        ModelFootprintInactive = "model.footprint_inactive" => MeasureSpec::model(2, Unit::Bytes),

        /// Spins played in the demo-only session.
        PlaySpins = "play.spins" => MeasureSpec::play(2, Unit::Count),
        /// Peak bytes during the spin loop.
        PlayPeak = "play.peak" => MeasureSpec::play(2, Unit::Bytes),
        /// Memory growth per 100 spins (Theil-Sen slope). Negative when memory shrinks.
        PlayGrowthPer100Spins = "play.growth_per_100_spins" => MeasureSpec::play(2, Unit::BytesChange),
        /// Peak bytes during a bonus round.
        PlayBonusPeak = "play.bonus_peak" => MeasureSpec::play(2, Unit::Bytes),

        /// Median `phys_footprint` of fresh-process runs on the iPhone, minus the empty harness.
        DevicePhysFootprint = "device.phys_footprint" => MeasureSpec::device(2, Unit::Bytes),
        /// Fresh-process runs that the iPhone killed, one item per syslog line.
        DeviceKills = "device.kills" => MeasureSpec::device(2, Unit::Count),
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
