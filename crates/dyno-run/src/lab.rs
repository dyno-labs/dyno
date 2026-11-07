//! Lab measures and contract findings from one run's driver messages.
//!
//! Two clocks meet here. Requests and marks are on the driver's clock; probe events are on the
//! game frame's own `performance.now()`. So "by idle" is the order of the messages: what arrived
//! before the `idle` mark. "While inactive" is on the frame's clock: what happened before the
//! frame saw the host profile's active message. A frame that commits a new document starts a new
//! clock, so probe events count only from the last `frame_document` mark before idle: they are
//! the document that went idle. Requests count from the start, since they all crossed the network.

use dyno_schema::ids::{MeasureId, RuleId};
use dyno_schema::manifest::FileKind;
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, Uncertainty,
    UncertaintyKind,
};
use dyno_schema::probe::{AudioState, GameHookKind, MessageDirection, ProbeEvent};
use dyno_schema::rule::Finding;
use dyno_schema::runner::{DriverMessage, RunMark, RunRequest};
use dyno_schema::status::Verdict;

/// Bytes a decoded audio sample holds per channel: a 32-bit float.
const BYTES_PER_SAMPLE: u64 = 4;

/// Bytes a canvas colour buffer holds per backing pixel: RGBA, 8 bits each.
const COLOUR_BUFFER_BYTES_PER_PIXEL: u64 = 4;

/// Thousandths in one, for the frame's device pixel ratio.
const MILLI: f64 = 1_000.0;

/// The most frames per second a game may render while inactive and still count as throttled: 1
/// frame a second or less. The finding compares the rate before rounding, so 1.4 frames a second
/// warns.
pub const INACTIVE_FRAME_RATE_LIMIT_HZ: f64 = 1.0;

/// What one run gave, read once.
struct Reading<'m> {
    request: &'m RunRequest,
    /// Every message before the `idle` mark.
    by_idle: &'m [DriverMessage],
    /// Where the document that went idle starts in the messages.
    document_start: usize,
    /// Every probe event, in order.
    events: Vec<&'m ProbeEvent>,
    all: &'m [DriverMessage],
    /// When the frame saw the active message, on its clock.
    activated_at: Option<f64>,
}

/// The lab measures and findings of a run that ended with `done`. None when the run never went
/// idle, so no measure can be read.
pub fn read(
    messages: &[DriverMessage],
    request: &RunRequest,
) -> Option<(Vec<Measure>, Vec<Finding>)> {
    let idle = messages.iter().position(|message| {
        matches!(
            message,
            DriverMessage::Mark {
                mark: RunMark::Idle,
                ..
            }
        )
    })?;
    // The probe events of the document that went idle.
    let document_start = messages[..idle]
        .iter()
        .rposition(|message| {
            matches!(
                message,
                DriverMessage::Mark {
                    mark: RunMark::FrameDocument,
                    ..
                }
            )
        })
        .unwrap_or(0);
    let events: Vec<&ProbeEvent> = messages[document_start..]
        .iter()
        .filter_map(|message| match message {
            DriverMessage::Probe { event } => Some(event),
            _ => None,
        })
        .collect();
    let active = &request.host_profile.active_message;
    let activated_at = events.iter().find_map(|event| match event {
        ProbeEvent::HostMessage {
            direction: MessageDirection::FromHost,
            name,
            t_ms,
        } if name == active => Some(*t_ms),
        _ => None,
    });
    let reading = Reading {
        request,
        by_idle: &messages[..idle],
        document_start,
        events,
        all: messages,
        activated_at,
    };
    let measures = vec![
        reading.first_frame(),
        reading.ready(),
        reading.loaded_by_idle(),
        to_playable(&reading),
        reading.audio_decoded(),
        reading.context_rate(),
        reading.textures_uploaded(),
        reading.textures_resident(),
        reading.canvas_contexts(),
        reading.canvas_bytes(),
        reading.js_heap(),
        reading.inactive_audio_running(),
        reading.inactive_frame_rate(),
        reading.inactive_allocated(),
        reading.frame_dpr(),
        reading.frame_requests(),
        reading.webgl_renderer(),
    ];
    let findings = findings(&measures, request, reading.inactive_rate());
    Some((measures, findings))
}

impl Reading<'_> {
    /// Probe events of the document that went idle, that arrived before the `idle` mark.
    fn events_by_idle(&self) -> impl Iterator<Item = &ProbeEvent> {
        self.by_idle[self.document_start..]
            .iter()
            .filter_map(|message| match message {
                DriverMessage::Probe { event } => Some(event),
                _ => None,
            })
    }

    /// When the frame's clock started after the navigation: the load time before the document
    /// that went idle, such as a gateway page. 0 when the driver does not say.
    fn frame_clock_at(&self) -> f64 {
        self.all
            .iter()
            .find_map(|message| match message {
                DriverMessage::SelfCheck {
                    frame_clock_at_ms, ..
                } => *frame_clock_at_ms,
                _ => None,
            })
            .unwrap_or(0.0)
    }

    /// Time from the navigation: a frame time plus when the frame's clock started.
    fn since_navigation(&self, frame_t_ms: f64) -> i64 {
        rounded(frame_t_ms + self.frame_clock_at())
    }

    fn first_frame(&self) -> Measure {
        let id = MeasureId::LabTimeFirstFrame;
        match self.events.iter().find_map(|event| match event {
            ProbeEvent::FirstFrame { t_ms } => Some(*t_ms),
            _ => None,
        }) {
            Some(t_ms) => measured(id, self.since_navigation(t_ms), Vec::new(), Method::Probe),
            None => unsupported(id, "the game drew no frame"),
        }
    }

    fn ready(&self) -> Measure {
        let id = MeasureId::LabTimeReady;
        let ready = &self.request.host_profile.ready_message;
        match self.events.iter().find_map(|event| match event {
            ProbeEvent::HostMessage {
                direction: MessageDirection::ToHost,
                name,
                t_ms,
            } if name == ready => Some(*t_ms),
            _ => None,
        }) {
            Some(t_ms) => measured(
                id,
                self.since_navigation(t_ms),
                Vec::new(),
                Method::HostMessage,
            ),
            None => unsupported(id, &format!("the game sent no {ready}")),
        }
    }

    fn loaded_by_idle(&self) -> Measure {
        let mut by_kind: Vec<(FileKind, u64)> = Vec::new();
        for message in self.by_idle {
            let DriverMessage::Request {
                url,
                mime_type,
                bytes,
                ..
            } = message
            else {
                continue;
            };
            let kind = kind_of(mime_type.as_deref(), url);
            match by_kind.iter_mut().find(|(known, _)| *known == kind) {
                Some((_, total)) => *total += bytes,
                None => by_kind.push((kind, *bytes)),
            }
        }
        let items = by_kind
            .iter()
            .map(|(kind, bytes)| item(kind_name(*kind), *bytes))
            .collect();
        let total = by_kind.iter().map(|(_, bytes)| bytes).sum();
        measured(
            MeasureId::LabBytesLoadedByIdle,
            saturating(total),
            items,
            Method::Network,
        )
    }

    fn audio_decoded(&self) -> Measure {
        let bytes = self
            .events_by_idle()
            .filter_map(|event| match event {
                ProbeEvent::AudioDecoded {
                    length, channels, ..
                } => Some(decoded_bytes(*length, *channels)),
                _ => None,
            })
            .sum();
        measured(
            MeasureId::LabAudioDecoded,
            saturating(bytes),
            Vec::new(),
            Method::Probe,
        )
    }

    fn context_rate(&self) -> Measure {
        let id = MeasureId::LabAudioContextRate;
        let rate = self.events.iter().find_map(|event| match event {
            ProbeEvent::AudioContextState { sample_rate, .. }
            | ProbeEvent::AudioDecoded { sample_rate, .. } => Some(*sample_rate),
            _ => None,
        });
        match rate {
            Some(rate) => measured(id, i64::from(rate), Vec::new(), Method::Probe),
            None => unsupported(id, "the game made no audio context"),
        }
    }

    /// Every upload by idle, with one item per source image path, one per kind of upload with no
    /// source (`empty`, `canvas`), and one for the rest.
    fn textures_uploaded(&self) -> Measure {
        let mut by_source: Vec<(String, u64)> = Vec::new();
        for event in self.events_by_idle() {
            let ProbeEvent::TextureUploaded {
                bytes,
                source,
                source_kind,
                ..
            } = event
            else {
                continue;
            };
            let key = match (source, source_kind) {
                (Some(path), _) => path.clone(),
                (None, Some(kind)) => kind.name().to_owned(),
                (None, None) => NO_SOURCE.to_owned(),
            };
            match by_source.iter_mut().find(|(known, _)| *known == key) {
                Some((_, total)) => *total = total.saturating_add(*bytes),
                None => by_source.push((key, *bytes)),
            }
        }
        let total = by_source
            .iter()
            .fold(0_u64, |sum, (_, bytes)| sum.saturating_add(*bytes));
        let items = by_source
            .into_iter()
            .map(|(key, bytes)| item(key, bytes))
            .collect();
        measured(
            MeasureId::LabTexturesUploaded,
            saturating(total),
            items,
            Method::Probe,
        )
    }

    /// The last upload of each level of each texture by idle, without the deleted textures. An
    /// upload the probe did not number counts on its own.
    fn textures_resident(&self) -> Measure {
        let mut levels: Vec<(TextureLevel, u64)> = Vec::new();
        let mut unnumbered: u64 = 0;
        for event in self.events_by_idle() {
            match event {
                ProbeEvent::TextureUploaded {
                    texture: Some(texture),
                    face,
                    level,
                    bytes,
                    ..
                } => {
                    let key = (*texture, *face, *level);
                    match levels.iter_mut().find(|(known, _)| *known == key) {
                        Some((_, held)) => *held = *bytes,
                        None => levels.push((key, *bytes)),
                    }
                }
                ProbeEvent::TextureUploaded { bytes, .. } => {
                    unnumbered = unnumbered.saturating_add(*bytes);
                }
                ProbeEvent::TextureDeleted { texture, .. } => {
                    levels.retain(|((held, _, _), _)| held != texture);
                }
                _ => {}
            }
        }
        let held = levels
            .iter()
            .fold(unnumbered, |sum, (_, bytes)| sum.saturating_add(*bytes));
        measured(
            MeasureId::LabTexturesResident,
            saturating(held),
            Vec::new(),
            Method::Probe,
        )
    }

    /// Each context's canvas at its last backing size by idle. A context the probe did not
    /// number keeps the size it was made at.
    fn canvas_sizes(&self) -> Vec<(u32, u32)> {
        let mut numbered: Vec<(u32, (u32, u32))> = Vec::new();
        let mut unnumbered = Vec::new();
        for event in self.events_by_idle() {
            match event {
                ProbeEvent::ContextCreated {
                    canvas: Some(canvas),
                    width,
                    height,
                    ..
                } => numbered.push((*canvas, (*width, *height))),
                ProbeEvent::ContextCreated { width, height, .. } => {
                    unnumbered.push((*width, *height));
                }
                ProbeEvent::CanvasResized {
                    canvas,
                    width,
                    height,
                    ..
                } => {
                    if let Some((_, size)) = numbered.iter_mut().find(|(known, _)| known == canvas)
                    {
                        *size = (*width, *height);
                    }
                }
                _ => {}
            }
        }
        numbered
            .into_iter()
            .map(|(_, size)| size)
            .chain(unnumbered)
            .collect()
    }

    fn canvas_contexts(&self) -> Measure {
        measured(
            MeasureId::LabCanvasContexts,
            saturating(self.canvas_sizes().len() as u64),
            Vec::new(),
            Method::Probe,
        )
    }

    /// One colour buffer per context, at its canvas's last backing size by idle.
    fn canvas_bytes(&self) -> Measure {
        let bytes = self
            .canvas_sizes()
            .into_iter()
            .map(|(width, height)| {
                u64::from(width) * u64::from(height) * COLOUR_BUFFER_BYTES_PER_PIXEL
            })
            .sum();
        Measure {
            uncertainty: Uncertainty {
                kind: UncertaintyKind::Floor,
                note: Some(
                    "one colour buffer per context at its canvas's last size by idle; antialias \
                     buffers and the GPU context are not included"
                        .to_owned(),
                ),
                ..Uncertainty::unknown()
            },
            ..measured(
                MeasureId::LabCanvasBytes,
                saturating(bytes),
                Vec::new(),
                Method::Probe,
            )
        }
    }

    fn js_heap(&self) -> Measure {
        let id = MeasureId::LabJsHeap;
        match self.all.iter().find_map(|message| match message {
            DriverMessage::Heap { used_bytes, .. } => Some(*used_bytes),
            _ => None,
        }) {
            Some(bytes) => measured(id, saturating(bytes), Vec::new(), Method::DevtoolsProtocol),
            None => Measure::missing(id, "the driver sent no heap reading"),
        }
    }

    /// Events before the frame saw the active message. None when it never saw it.
    fn before_activation(&self) -> Option<Vec<&ProbeEvent>> {
        let activated_at = self.activated_at?;
        Some(
            self.events
                .iter()
                .copied()
                .filter(|event| event_time(event) < activated_at)
                .collect(),
        )
    }

    fn inactive_audio_running(&self) -> Measure {
        let id = MeasureId::LabInactiveAudioRunning;
        let Some(before) = self.before_activation() else {
            return unsupported(id, NOT_ACTIVATED);
        };
        let running = before.iter().any(|event| {
            matches!(
                event,
                ProbeEvent::AudioContextState {
                    state: AudioState::Running,
                    ..
                }
            )
        });
        measured_value(id, MeasureValue::Flag(running), Method::Probe)
    }

    /// Frames per second in the idle time before activation, when the game had loaded and
    /// waited. A report covers the time since the report before it, so only reports that lie
    /// wholly in that time count.
    fn inactive_frame_rate(&self) -> Measure {
        let id = MeasureId::LabInactiveFrameRate;
        match self.inactive_rate() {
            Ok(rate) => measured(id, rounded(rate), Vec::new(), Method::Probe),
            Err(reason) => unsupported(id, reason),
        }
    }

    /// Frames per second in the idle time before activation, before rounding, or why there is no
    /// rate.
    fn inactive_rate(&self) -> Result<f64, &'static str> {
        let Some(activated_at) = self.activated_at else {
            return Err(NOT_ACTIVATED);
        };
        let window_start = activated_at - f64::from(self.request.idle_after_ms);
        let mut covered_since = 0.0;
        let mut frames: u64 = 0;
        let mut covered_ms = 0.0;
        for event in &self.events {
            let ProbeEvent::FrameTimes { counts, t_ms, .. } = event else {
                continue;
            };
            if covered_since >= window_start && *t_ms <= activated_at {
                frames += counts.iter().map(|count| u64::from(*count)).sum::<u64>();
                covered_ms += t_ms - covered_since;
            }
            covered_since = *t_ms;
        }
        if covered_ms <= 0.0 {
            return Err("no frame time report lies wholly in the idle time before activation");
        }
        Ok(frames as f64 / (covered_ms / MILLI))
    }

    fn inactive_allocated(&self) -> Measure {
        let id = MeasureId::LabInactiveAllocated;
        let Some(before) = self.before_activation() else {
            return unsupported(id, NOT_ACTIVATED);
        };
        let mut audio = 0;
        let mut textures = 0;
        for event in before {
            match event {
                ProbeEvent::AudioDecoded {
                    length, channels, ..
                } => audio += decoded_bytes(*length, *channels),
                ProbeEvent::TextureUploaded { bytes, .. } => textures += bytes,
                _ => {}
            }
        }
        measured(
            id,
            saturating(audio + textures),
            vec![
                item("audio".to_owned(), audio),
                item("textures".to_owned(), textures),
            ],
            Method::Probe,
        )
    }

    fn self_check(&self) -> Option<(f64, Option<&str>)> {
        self.all.iter().find_map(|message| match message {
            DriverMessage::SelfCheck {
                device_pixel_ratio,
                webgl_renderer,
                ..
            } => Some((*device_pixel_ratio, webgl_renderer.as_deref())),
            _ => None,
        })
    }

    /// Invalid unless the frame's ratio is the profile's: a cross-site frame reports 1 unless
    /// the browser's screen is forced.
    fn frame_dpr(&self) -> Measure {
        let id = MeasureId::LabSelfCheckFrameDpr;
        let Some((ratio, _)) = self.self_check() else {
            return Measure::missing(id, NO_SELF_CHECK);
        };
        let milli = rounded(ratio * MILLI);
        let expected = i64::from(self.request.host_profile.device_scale_factor) * 1_000;
        let measure = measured(id, milli, Vec::new(), Method::DevtoolsProtocol);
        if milli == expected {
            return measure;
        }
        Measure {
            status: MeasureStatus::Invalid,
            reason: Some(format!(
                "the game frame's device pixel ratio is {ratio}, not {}",
                self.request.host_profile.device_scale_factor
            )),
            ..measure
        }
    }

    /// Invalid when the frame made no request: the run did not load the game.
    fn frame_requests(&self) -> Measure {
        let id = MeasureId::LabSelfCheckFrameRequests;
        let count = self
            .all
            .iter()
            .filter(|message| matches!(message, DriverMessage::Request { .. }))
            .count();
        let measure = measured(id, saturating(count as u64), Vec::new(), Method::Network);
        if count > 0 {
            return measure;
        }
        Measure {
            status: MeasureStatus::Invalid,
            reason: Some("the game frame made no requests".to_owned()),
            ..measure
        }
    }

    fn webgl_renderer(&self) -> Measure {
        let id = MeasureId::LabSelfCheckWebglRenderer;
        match self.self_check() {
            None => Measure::missing(id, NO_SELF_CHECK),
            Some((_, None)) => unsupported(id, "WebGL is off in the browser"),
            Some((_, Some(renderer))) => measured_value(
                id,
                MeasureValue::Text(renderer.to_owned()),
                Method::DevtoolsProtocol,
            ),
        }
    }
}

/// One level of one face of one texture: (texture, face, level). A level of None is every level
/// that `texStorage2D` allocated at once.
type TextureLevel = (u32, Option<u32>, Option<u32>);

/// The item key of uploads with no http source and no kind the probe could tell. A source path
/// starts with `/`, so it never meets this key or a `SourceKind` name.
const NO_SOURCE: &str = "other";

const NOT_ACTIVATED: &str = "the game frame did not see the active message";
const NO_SELF_CHECK: &str = "the driver sent no self-check";

/// Unsupported either way in Phase 1: without a playable hook there is no signal, and with one
/// its time is on the frame's clock while requests are on the driver's.
fn to_playable(reading: &Reading<'_>) -> Measure {
    let has_hook = reading.events.iter().any(|event| {
        matches!(
            event,
            ProbeEvent::GameHook {
                hook: GameHookKind::Playable,
                ..
            }
        )
    });
    let reason = if has_hook {
        "joining the playable hook to the requests is not built yet"
    } else {
        "the game sent no playable hook"
    };
    unsupported(MeasureId::LabBytesToPlayable, reason)
}

/// The lab findings. Each reads its one measure; one with no value leaves its finding
/// incomplete.
fn findings(
    measures: &[Measure],
    request: &RunRequest,
    inactive_rate: Result<f64, &str>,
) -> Vec<Finding> {
    let value = |id: MeasureId| {
        measures
            .iter()
            .find(|measure| measure.id == id && measure.status == MeasureStatus::Measured)
            .and_then(|measure| measure.value.clone())
    };
    let reason = |id: MeasureId| {
        measures
            .iter()
            .find(|measure| measure.id == id)
            .and_then(|measure| measure.reason.clone())
            .unwrap_or_default()
    };
    vec![
        ready_in_time(measures, request),
        flag_finding(
            RuleId::ContractNoAudioBeforeActive,
            value(MeasureId::LabInactiveAudioRunning),
            &reason(MeasureId::LabInactiveAudioRunning),
            |value| value == MeasureValue::Flag(true),
        ),
        throttled_while_inactive(inactive_rate),
        flag_finding(
            RuleId::DiagnosticAllocBeforeActive,
            value(MeasureId::LabInactiveAllocated),
            &reason(MeasureId::LabInactiveAllocated),
            |value| matches!(value, MeasureValue::Integer(bytes) if bytes > 0),
        ),
    ]
}

/// Warns when the game rendered more than `INACTIVE_FRAME_RATE_LIMIT_HZ` while inactive, by the
/// rate before rounding. No rate leaves the finding incomplete with the reason.
fn throttled_while_inactive(inactive_rate: Result<f64, &str>) -> Finding {
    let rule = RuleId::ContractThrottledWhileInactive;
    match inactive_rate {
        Err(reason) => Finding::incomplete(rule, reason),
        Ok(rate) => Finding {
            verdict: if rate > INACTIVE_FRAME_RATE_LIMIT_HZ {
                Verdict::Warn
            } else {
                Verdict::Pass
            },
            reason: None,
            ..Finding::incomplete(rule, "")
        },
    }
}

/// Warns when the ready message came after the host profile's limit, or never came. A run with
/// no reading of it at all leaves the finding incomplete.
fn ready_in_time(measures: &[Measure], request: &RunRequest) -> Finding {
    let rule = RuleId::ContractReadyInTime;
    let Some(ready) = measures
        .iter()
        .find(|measure| measure.id == MeasureId::LabTimeReady)
    else {
        return Finding::incomplete(rule, "no ready reading");
    };
    let within = i64::from(request.host_profile.ready_within_ms);
    let verdict = match (ready.status, &ready.value) {
        (MeasureStatus::Measured, Some(MeasureValue::Integer(t_ms))) if *t_ms <= within => {
            Verdict::Pass
        }
        (MeasureStatus::Measured | MeasureStatus::Unsupported, _) => Verdict::Warn,
        _ => {
            let reason = ready.reason.as_deref().unwrap_or("no ready reading");
            return Finding::incomplete(rule, reason);
        }
    };
    Finding {
        verdict,
        reason: None,
        ..Finding::incomplete(rule, "")
    }
}

/// A finding that warns when `warns` holds for its measure's value, and is incomplete with the
/// measure's reason when the measure has no value.
fn flag_finding(
    rule: RuleId,
    value: Option<MeasureValue>,
    reason: &str,
    warns: impl Fn(MeasureValue) -> bool,
) -> Finding {
    match value {
        None => Finding::incomplete(rule, reason),
        Some(value) => Finding {
            verdict: if warns(value) {
                Verdict::Warn
            } else {
                Verdict::Pass
            },
            reason: None,
            ..Finding::incomplete(rule, reason)
        },
    }
}

fn event_time(event: &ProbeEvent) -> f64 {
    match event {
        ProbeEvent::FirstFrame { t_ms }
        | ProbeEvent::HostMessage { t_ms, .. }
        | ProbeEvent::AudioDecoded { t_ms, .. }
        | ProbeEvent::AudioContextState { t_ms, .. }
        | ProbeEvent::TextureUploaded { t_ms, .. }
        | ProbeEvent::ContextCreated { t_ms, .. }
        | ProbeEvent::CanvasResized { t_ms, .. }
        | ProbeEvent::TextureDeleted { t_ms, .. }
        | ProbeEvent::ContextLost { t_ms }
        | ProbeEvent::FrameTimes { t_ms, .. }
        | ProbeEvent::GameHook { t_ms, .. } => *t_ms,
    }
}

fn decoded_bytes(length: u64, channels: u32) -> u64 {
    length
        .saturating_mul(u64::from(channels))
        .saturating_mul(BYTES_PER_SAMPLE)
}

/// The file kind of a response, from its MIME type, else its URL's extension.
fn kind_of(mime_type: Option<&str>, url: &str) -> FileKind {
    let mime = mime_type.unwrap_or_default();
    let (family, subtype) = mime.split_once('/').unwrap_or((mime, ""));
    match (family, subtype) {
        ("text", "html") => FileKind::Html,
        ("text", "css") => FileKind::Css,
        (_, "javascript" | "x-javascript" | "ecmascript") => FileKind::Js,
        (_, "json") => FileKind::Data,
        ("image", _) => FileKind::Image,
        ("audio", _) => FileKind::Audio,
        ("video", _) => FileKind::Video,
        ("font", _) | (_, "font-woff" | "font-woff2") => FileKind::Font,
        _ => kind_of_extension(url),
    }
}

fn kind_of_extension(url: &str) -> FileKind {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("ktx2" | "basis" | "dds" | "pvr" | "astc") => FileKind::Texture,
        Some("json" | "atlas" | "fnt" | "skel") => FileKind::Data,
        _ => FileKind::Other,
    }
}

/// A file kind as the manifest names it.
fn kind_name(kind: FileKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn measured(id: MeasureId, value: i64, items: Vec<MeasureItem>, method: Method) -> Measure {
    Measure {
        items,
        ..measured_value(id, MeasureValue::Integer(value), method)
    }
}

fn measured_value(id: MeasureId, value: MeasureValue, method: Method) -> Measure {
    Measure {
        value: Some(value),
        evidence: vec![Evidence {
            method,
            detail: "one Chromium run in the harness page".to_owned(),
        }],
        uncertainty: Uncertainty::exact(),
        ..Measure::blank(id, MeasureStatus::Measured)
    }
}

fn unsupported(id: MeasureId, reason: &str) -> Measure {
    Measure {
        reason: Some(reason.to_owned()),
        ..Measure::blank(id, MeasureStatus::Unsupported)
    }
}

fn item(key: String, bytes: u64) -> MeasureItem {
    MeasureItem {
        key,
        value: MeasureValue::Integer(saturating(bytes)),
    }
}

fn saturating(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn rounded(value: f64) -> i64 {
    value.round() as i64
}

#[cfg(test)]
mod tests {
    use dyno_schema::probe::{ContextKind, SourceKind};
    use dyno_schema::profile::{HostProfile, Scenario};
    use dyno_schema::result::DriverInfo;
    use dyno_schema::runner::RUNNER_PROTOCOL_VERSION;

    use super::*;

    const IDLE_AFTER_MS: u32 = 1_000;
    /// When the frame sees the active message, on its clock.
    const ACTIVE_AT: f64 = 1_500.0;

    fn request() -> RunRequest {
        RunRequest {
            protocol_version: RUNNER_PROTOCOL_VERSION,
            launch_url: "http://127.0.0.1:1/".to_owned(),
            host_profile: HostProfile::reels(),
            scenario: Scenario::Alone,
            spins: None,
            idle_after_ms: IDLE_AFTER_MS,
            timeout_ms: 30_000,
        }
    }

    fn probe(event: ProbeEvent) -> DriverMessage {
        DriverMessage::Probe { event }
    }

    fn mark(mark: RunMark) -> DriverMessage {
        DriverMessage::Mark { mark, t_ms: 0.0 }
    }

    fn request_of(url: &str, mime_type: &str, bytes: u64) -> DriverMessage {
        DriverMessage::Request {
            url: url.to_owned(),
            mime_type: Some(mime_type.to_owned()),
            bytes,
            t_ms: 10.0,
        }
    }

    fn host_message(direction: MessageDirection, name: &str, t_ms: f64) -> DriverMessage {
        probe(ProbeEvent::HostMessage {
            direction,
            name: name.to_owned(),
            t_ms,
        })
    }

    fn frame_times(frames: u32, t_ms: f64) -> DriverMessage {
        probe(ProbeEvent::FrameTimes {
            bucket_upper_ms: vec![17, u32::MAX],
            counts: vec![frames, 0],
            t_ms,
        })
    }

    /// A run like the Chromium test's: a page, a script, a WebGL canvas of 402 x 874, a texture,
    /// 4,800 frames of mono audio, ready at 180 ms, idle, the self-checks, then activation.
    fn run() -> Vec<DriverMessage> {
        vec![
            DriverMessage::Hello {
                protocol_version: RUNNER_PROTOCOL_VERSION,
                driver: DriverInfo {
                    name: "cdp".to_owned(),
                    version: "0".to_owned(),
                    browser: None,
                },
            },
            mark(RunMark::Navigated),
            request_of("http://127.0.0.1:1/", "text/html", 181),
            request_of("http://127.0.0.1:1/game.js", "text/javascript", 1_198),
            probe(ProbeEvent::ContextCreated {
                canvas: Some(0),
                kind: ContextKind::Webgl,
                width: 402,
                height: 874,
                antialias: Some(false),
                device_pixel_ratio: 3.0,
                t_ms: 130.0,
            }),
            probe(ProbeEvent::TextureUploaded {
                texture: None,
                face: None,
                level: None,
                source: None,
                source_kind: None,
                width: 64,
                height: 32,
                bytes: 8_192,
                t_ms: 131.0,
            }),
            probe(ProbeEvent::AudioContextState {
                state: AudioState::Suspended,
                sample_rate: 48_000,
                t_ms: 160.0,
            }),
            probe(ProbeEvent::FirstFrame { t_ms: 170.4 }),
            probe(ProbeEvent::AudioDecoded {
                length: 4_800,
                channels: 1,
                sample_rate: 48_000,
                t_ms: 178.0,
            }),
            host_message(MessageDirection::ToHost, "reels:ready", 180.0),
            frame_times(0, 1_000.0),
            mark(RunMark::Idle),
            DriverMessage::Heap {
                used_bytes: 1_245_136,
                t_ms: 1_187.0,
            },
            DriverMessage::SelfCheck {
                device_pixel_ratio: 3.0,
                webgl_renderer: Some("SwiftShader".to_owned()),
                frame_clock_at_ms: Some(0.0),
                t_ms: 1_190.0,
            },
            mark(RunMark::Activated),
            host_message(MessageDirection::FromHost, "reels:active", ACTIVE_AT),
            probe(ProbeEvent::TextureUploaded {
                texture: None,
                face: None,
                level: None,
                source: None,
                source_kind: None,
                width: 16,
                height: 16,
                bytes: 1_024,
                t_ms: 1_600.0,
            }),
            DriverMessage::Done { t_ms: 3_500.0 },
        ]
    }

    fn read_run(messages: &[DriverMessage]) -> (Vec<Measure>, Vec<Finding>) {
        read(messages, &request()).unwrap()
    }

    fn measure(measures: &[Measure], id: MeasureId) -> &Measure {
        measures.iter().find(|measure| measure.id == id).unwrap()
    }

    fn integer(measures: &[Measure], id: MeasureId) -> i64 {
        match measure(measures, id).value {
            Some(MeasureValue::Integer(value)) => value,
            ref other => panic!("{id}: {other:?}"),
        }
    }

    fn verdict(findings: &[Finding], rule: RuleId) -> Verdict {
        findings
            .iter()
            .find(|finding| finding.rule == rule)
            .unwrap()
            .verdict
    }

    #[test]
    fn a_run_gives_every_load_measure_once() {
        let (measures, findings) = read_run(&run());
        let ids: Vec<MeasureId> = measures.iter().map(|measure| measure.id).collect();
        assert_eq!(ids, crate::LOAD_MEASURES);
        let rules: Vec<RuleId> = findings.iter().map(|finding| finding.rule).collect();
        assert_eq!(rules, crate::LAB_RULES);
    }

    #[test]
    fn a_run_with_no_idle_mark_gives_nothing() {
        let without_idle: Vec<DriverMessage> = run()
            .into_iter()
            .filter(|message| {
                !matches!(
                    message,
                    DriverMessage::Mark {
                        mark: RunMark::Idle,
                        ..
                    }
                )
            })
            .collect();
        assert!(read(&without_idle, &request()).is_none());
    }

    #[test]
    fn times_are_on_the_frames_clock_rounded_to_whole_ms() {
        let (measures, _) = read_run(&run());
        assert_eq!(integer(&measures, MeasureId::LabTimeFirstFrame), 170);
        assert_eq!(integer(&measures, MeasureId::LabTimeReady), 180);
    }

    #[test]
    fn bytes_loaded_by_idle_are_summed_by_file_kind() {
        let (measures, _) = read_run(&run());
        let loaded = measure(&measures, MeasureId::LabBytesLoadedByIdle);
        assert_eq!(loaded.value, Some(MeasureValue::Integer(1_379)));
        let keys: Vec<&str> = loaded.items.iter().map(|item| item.key.as_str()).collect();
        assert_eq!(keys, ["html", "js"]);
    }

    #[test]
    fn only_work_before_idle_counts_toward_the_load() {
        let (measures, _) = read_run(&run());
        assert_eq!(integer(&measures, MeasureId::LabTexturesUploaded), 8_192);
        assert_eq!(integer(&measures, MeasureId::LabAudioDecoded), 4_800 * 4);
        assert_eq!(integer(&measures, MeasureId::LabCanvasContexts), 1);
        assert_eq!(integer(&measures, MeasureId::LabCanvasBytes), 402 * 874 * 4);
    }

    #[test]
    fn a_canvas_counts_at_its_last_size_by_idle() {
        let mut messages = run();
        let resized = |width, height| {
            probe(ProbeEvent::CanvasResized {
                canvas: 0,
                width,
                height,
                t_ms: 140.0,
            })
        };
        messages.insert(5, resized(1_206, 2_622));
        let idle = messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    DriverMessage::Mark {
                        mark: RunMark::Idle,
                        ..
                    }
                )
            })
            .unwrap();
        messages.insert(idle + 1, resized(10, 10));
        let (measures, _) = read_run(&messages);
        assert_eq!(
            integer(&measures, MeasureId::LabCanvasBytes),
            1_206 * 2_622 * 4
        );
        assert_eq!(integer(&measures, MeasureId::LabCanvasContexts), 1);
    }

    #[test]
    fn only_the_document_that_went_idle_gives_probe_events() {
        let mut messages = run();
        // An earlier document drew a frame at 5 ms and uploaded a texture, then redirected.
        messages.insert(2, probe(ProbeEvent::FirstFrame { t_ms: 5.0 }));
        messages.insert(
            3,
            probe(ProbeEvent::TextureUploaded {
                texture: None,
                face: None,
                level: None,
                source: None,
                source_kind: None,
                width: 8,
                height: 8,
                bytes: 256,
                t_ms: 6.0,
            }),
        );
        messages.insert(4, mark(RunMark::FrameDocument));
        let (measures, _) = read_run(&messages);
        assert_eq!(integer(&measures, MeasureId::LabTimeFirstFrame), 170);
        assert_eq!(integer(&measures, MeasureId::LabTexturesUploaded), 8_192);
        assert_eq!(integer(&measures, MeasureId::LabBytesLoadedByIdle), 1_379);
    }

    #[test]
    fn times_count_the_load_before_the_document_that_went_idle() {
        let messages: Vec<DriverMessage> = run()
            .into_iter()
            .map(|message| match message {
                DriverMessage::SelfCheck {
                    device_pixel_ratio,
                    webgl_renderer,
                    t_ms,
                    ..
                } => DriverMessage::SelfCheck {
                    device_pixel_ratio,
                    webgl_renderer,
                    frame_clock_at_ms: Some(6_000.0),
                    t_ms,
                },
                other => other,
            })
            .collect();
        let (measures, findings) = read_run(&messages);
        assert_eq!(integer(&measures, MeasureId::LabTimeReady), 6_180);
        assert_eq!(integer(&measures, MeasureId::LabTimeFirstFrame), 6_170);
        assert_eq!(
            verdict(&findings, RuleId::ContractReadyInTime),
            Verdict::Warn
        );
    }

    /// An upload of one level of a numbered texture, at 100 ms.
    fn upload(texture: u32, level: Option<u32>, bytes: u64, source: Option<&str>) -> DriverMessage {
        probe(ProbeEvent::TextureUploaded {
            texture: Some(texture),
            face: Some(0),
            level,
            width: 16,
            height: 16,
            bytes,
            source: source.map(str::to_owned),
            source_kind: None,
            t_ms: 100.0,
        })
    }

    /// The run with these probe events before idle.
    fn with_events(events: Vec<DriverMessage>) -> Vec<DriverMessage> {
        let mut messages = run();
        for (index, event) in events.into_iter().enumerate() {
            messages.insert(3 + index, event);
        }
        messages
    }

    #[test]
    fn uploads_are_counted_by_their_source_image() {
        let messages = with_events(vec![
            upload(1, Some(0), 1_000, Some("/assets/a.png")),
            upload(2, Some(0), 500, Some("/assets/a.png")),
            upload(3, Some(0), 200, Some("/assets/b.png")),
        ]);
        let (measures, _) = read_run(&messages);
        let uploaded = measure(&measures, MeasureId::LabTexturesUploaded);
        assert_eq!(uploaded.value, Some(MeasureValue::Integer(1_700 + 8_192)));
        let items: Vec<(&str, &MeasureValue)> = uploaded
            .items
            .iter()
            .map(|item| (item.key.as_str(), &item.value))
            .collect();
        assert_eq!(
            items,
            [
                ("/assets/a.png", &MeasureValue::Integer(1_500)),
                ("/assets/b.png", &MeasureValue::Integer(200)),
                ("other", &MeasureValue::Integer(8_192)),
            ]
        );
    }

    #[test]
    fn uploads_with_no_source_are_counted_by_what_they_held() {
        let with_kind = |bytes, source_kind| {
            probe(ProbeEvent::TextureUploaded {
                texture: None,
                face: Some(0),
                level: Some(0),
                width: 16,
                height: 16,
                bytes,
                source: None,
                source_kind,
                t_ms: 100.0,
            })
        };
        let messages = with_events(vec![
            with_kind(3_000, Some(SourceKind::Empty)),
            with_kind(1_000, Some(SourceKind::Empty)),
            with_kind(700, Some(SourceKind::Canvas)),
            upload(1, Some(0), 200, Some("/assets/a.png")),
        ]);
        let (measures, _) = read_run(&messages);
        let uploaded = measure(&measures, MeasureId::LabTexturesUploaded);
        let items: Vec<(&str, &MeasureValue)> = uploaded
            .items
            .iter()
            .map(|item| (item.key.as_str(), &item.value))
            .collect();
        assert_eq!(
            items,
            [
                ("empty", &MeasureValue::Integer(4_000)),
                ("canvas", &MeasureValue::Integer(700)),
                ("/assets/a.png", &MeasureValue::Integer(200)),
                ("other", &MeasureValue::Integer(8_192)),
            ]
        );
    }

    #[test]
    fn a_re_upload_replaces_its_level_and_a_delete_frees_the_texture() {
        let messages = with_events(vec![
            upload(1, Some(0), 1_000, None),
            upload(1, Some(0), 400, None),
            upload(1, Some(1), 100, None),
            upload(2, None, 3_000, None),
            probe(ProbeEvent::TextureDeleted {
                texture: 2,
                t_ms: 120.0,
            }),
        ]);
        let (measures, _) = read_run(&messages);
        // Texture 1 holds 400 + 100; texture 2 is gone; the run's own upload has no number.
        assert_eq!(
            integer(&measures, MeasureId::LabTexturesResident),
            400 + 100 + 8_192
        );
        assert_eq!(
            integer(&measures, MeasureId::LabTexturesUploaded),
            1_000 + 400 + 100 + 3_000 + 8_192
        );
    }

    #[test]
    fn the_canvas_is_a_floor() {
        let (measures, _) = read_run(&run());
        let canvas = measure(&measures, MeasureId::LabCanvasBytes);
        assert_eq!(canvas.uncertainty.kind, UncertaintyKind::Floor);
    }

    #[test]
    fn the_heap_and_the_context_rate_are_read() {
        let (measures, _) = read_run(&run());
        assert_eq!(integer(&measures, MeasureId::LabJsHeap), 1_245_136);
        assert_eq!(integer(&measures, MeasureId::LabAudioContextRate), 48_000);
    }

    #[test]
    fn work_before_activation_is_on_the_frames_clock() {
        let (measures, findings) = read_run(&run());
        assert_eq!(
            integer(&measures, MeasureId::LabInactiveAllocated),
            8_192 + 4_800 * 4
        );
        assert_eq!(
            verdict(&findings, RuleId::DiagnosticAllocBeforeActive),
            Verdict::Warn
        );
    }

    #[test]
    fn audio_that_runs_before_activation_breaks_the_contract() {
        let mut messages = run();
        let (measures, findings) = read_run(&messages);
        assert_eq!(
            measure(&measures, MeasureId::LabInactiveAudioRunning).value,
            Some(MeasureValue::Flag(false))
        );
        assert_eq!(
            verdict(&findings, RuleId::ContractNoAudioBeforeActive),
            Verdict::Pass
        );
        messages.insert(
            3,
            probe(ProbeEvent::AudioContextState {
                state: AudioState::Running,
                sample_rate: 48_000,
                t_ms: 900.0,
            }),
        );
        let (measures, findings) = read_run(&messages);
        assert_eq!(
            measure(&measures, MeasureId::LabInactiveAudioRunning).value,
            Some(MeasureValue::Flag(true))
        );
        assert_eq!(
            verdict(&findings, RuleId::ContractNoAudioBeforeActive),
            Verdict::Warn
        );
    }

    /// The run with only these frame time reports.
    fn with_frame_times(reports: &[(u32, f64)]) -> Vec<DriverMessage> {
        let mut messages = run();
        messages.retain(|message| {
            !matches!(
                message,
                DriverMessage::Probe {
                    event: ProbeEvent::FrameTimes { .. }
                }
            )
        });
        for (index, (frames, t_ms)) in reports.iter().enumerate() {
            messages.insert(3 + index, frame_times(*frames, *t_ms));
        }
        messages
    }

    #[test]
    fn the_inactive_frame_rate_counts_the_reports_wholly_in_the_idle_time() {
        // The idle time before activation at 1,500 ms is 500 to 1,500 ms. The report at 600 ms
        // covers 0 to 600 ms, so only the 48 frames of 600 to 1,400 ms count: 60 a second.
        let messages = with_frame_times(&[(30, 600.0), (48, 1_400.0)]);
        let (measures, findings) = read_run(&messages);
        assert_eq!(integer(&measures, MeasureId::LabInactiveFrameRate), 60);
        assert_eq!(
            verdict(&findings, RuleId::ContractThrottledWhileInactive),
            Verdict::Warn
        );
    }

    #[test]
    fn throttled_means_1_frame_a_second_or_less_before_rounding() {
        // The idle time is 500 to 1,500 ms. 1 frame in 1,000 ms is 1 a second: throttled.
        let at_limit = with_frame_times(&[(0, 500.0), (1, 1_500.0)]);
        let (measures, findings) = read_run(&at_limit);
        assert_eq!(integer(&measures, MeasureId::LabInactiveFrameRate), 1);
        assert_eq!(
            verdict(&findings, RuleId::ContractThrottledWhileInactive),
            Verdict::Pass
        );
        // 1 frame in 800 ms is 1.25 a second: shown as 1, but above the limit, so it warns.
        let above = with_frame_times(&[(0, 600.0), (1, 1_400.0)]);
        let (measures, findings) = read_run(&above);
        assert_eq!(integer(&measures, MeasureId::LabInactiveFrameRate), 1);
        assert_eq!(
            verdict(&findings, RuleId::ContractThrottledWhileInactive),
            Verdict::Warn
        );
    }

    #[test]
    fn frames_before_the_idle_time_do_not_count() {
        let messages = with_frame_times(&[(42, 600.0), (0, 1_400.0)]);
        let (measures, findings) = read_run(&messages);
        assert_eq!(integer(&measures, MeasureId::LabInactiveFrameRate), 0);
        assert_eq!(
            verdict(&findings, RuleId::ContractThrottledWhileInactive),
            Verdict::Pass
        );
    }

    #[test]
    fn with_no_report_wholly_in_the_idle_time_the_frame_rate_is_unsupported() {
        let messages = with_frame_times(&[(60, 1_000.0)]);
        let (measures, _) = read_run(&messages);
        let rate = measure(&measures, MeasureId::LabInactiveFrameRate);
        assert_eq!(rate.status, MeasureStatus::Unsupported);
    }

    #[test]
    fn without_the_active_message_the_inactive_measures_are_unsupported() {
        let messages: Vec<DriverMessage> = run()
            .into_iter()
            .filter(|message| {
                !matches!(
                    message,
                    DriverMessage::Probe {
                        event: ProbeEvent::HostMessage {
                            direction: MessageDirection::FromHost,
                            ..
                        }
                    }
                )
            })
            .collect();
        let (measures, findings) = read_run(&messages);
        for id in [
            MeasureId::LabInactiveAudioRunning,
            MeasureId::LabInactiveFrameRate,
            MeasureId::LabInactiveAllocated,
        ] {
            let inactive = measure(&measures, id);
            assert_eq!(inactive.status, MeasureStatus::Unsupported, "{id}");
            assert_eq!(inactive.reason.as_deref(), Some(NOT_ACTIVATED), "{id}");
        }
        assert_eq!(
            verdict(&findings, RuleId::ContractNoAudioBeforeActive),
            Verdict::Incomplete
        );
    }

    #[test]
    fn a_ready_message_after_the_limit_or_never_warns() {
        let (_, findings) = read_run(&run());
        assert_eq!(
            verdict(&findings, RuleId::ContractReadyInTime),
            Verdict::Pass
        );
        let late: Vec<DriverMessage> = run()
            .into_iter()
            .map(|message| match message {
                DriverMessage::Probe {
                    event:
                        ProbeEvent::HostMessage {
                            direction: MessageDirection::ToHost,
                            name,
                            ..
                        },
                } => host_message(MessageDirection::ToHost, &name, 5_001.0),
                other => other,
            })
            .collect();
        let (_, findings) = read_run(&late);
        assert_eq!(
            verdict(&findings, RuleId::ContractReadyInTime),
            Verdict::Warn
        );
        let never: Vec<DriverMessage> = run()
            .into_iter()
            .filter(|message| {
                !matches!(
                    message,
                    DriverMessage::Probe {
                        event: ProbeEvent::HostMessage {
                            direction: MessageDirection::ToHost,
                            ..
                        }
                    }
                )
            })
            .collect();
        let (measures, findings) = read_run(&never);
        assert_eq!(
            measure(&measures, MeasureId::LabTimeReady).status,
            MeasureStatus::Unsupported
        );
        assert_eq!(
            verdict(&findings, RuleId::ContractReadyInTime),
            Verdict::Warn
        );
    }

    #[test]
    fn a_frame_at_another_pixel_ratio_or_with_no_requests_is_invalid() {
        let at_ratio_1: Vec<DriverMessage> = run()
            .into_iter()
            .map(|message| match message {
                DriverMessage::SelfCheck {
                    webgl_renderer,
                    frame_clock_at_ms,
                    t_ms,
                    ..
                } => DriverMessage::SelfCheck {
                    device_pixel_ratio: 1.0,
                    webgl_renderer,
                    frame_clock_at_ms,
                    t_ms,
                },
                other => other,
            })
            .collect();
        let (measures, _) = read_run(&at_ratio_1);
        let ratio = measure(&measures, MeasureId::LabSelfCheckFrameDpr);
        assert_eq!(ratio.status, MeasureStatus::Invalid);
        assert_eq!(ratio.value, Some(MeasureValue::Integer(1_000)));
        let no_requests: Vec<DriverMessage> = run()
            .into_iter()
            .filter(|message| !matches!(message, DriverMessage::Request { .. }))
            .collect();
        let (measures, _) = read_run(&no_requests);
        assert_eq!(
            measure(&measures, MeasureId::LabSelfCheckFrameRequests).status,
            MeasureStatus::Invalid
        );
    }

    #[test]
    fn bytes_to_playable_is_unsupported_in_phase_1() {
        let (measures, _) = read_run(&run());
        assert_eq!(
            measure(&measures, MeasureId::LabBytesToPlayable).status,
            MeasureStatus::Unsupported
        );
    }

    #[test]
    fn a_response_is_kinded_by_its_mime_type_else_its_extension() {
        assert_eq!(kind_of(Some("image/webp"), "a"), FileKind::Image);
        assert_eq!(kind_of(Some("application/javascript"), "a"), FileKind::Js);
        assert_eq!(kind_of(Some("application/json"), "a"), FileKind::Data);
        assert_eq!(
            kind_of(Some("application/octet-stream"), "/t.ktx2?v=1"),
            FileKind::Texture
        );
        assert_eq!(kind_of(None, "/x.bin"), FileKind::Other);
    }
}
