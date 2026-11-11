//! One load-and-idle run: the harness page embeds the game, the probe runs in the game frame, and
//! the run streams what it saw as runner protocol messages.
//!
//! The order: launch, attach to the page, navigate to the harness, and on the game frame's
//! attach, set it up before its first script (the probe, its binding, its network). The frame is
//! idle when it has no request in flight and none started for `idle_after_ms`. Then the run
//! reads the heap and the self-checks, finds which processes run the game frame and the page,
//! makes the game active, watches it for a short while, purges the garbage and the caches, and
//! ends with `done`. Any failure after `hello` ends the run with `error`. From the launch to the end, a sampler reads the memory of
//! every browser process (`dyno_run::memory`); its readings go in as `memory` messages, with a
//! `roles` message, just before the last one.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dyno_run::memory::{self, Reading, Sampler};
use dyno_schema::probe::{MessageDirection, ProbeEvent};
use dyno_schema::profile::HostProfile;
use dyno_schema::result::DriverInfo;
use dyno_schema::runner::{
    AssignedRole, DriverMessage, RUNNER_PROTOCOL_VERSION, RunMark, RunRequest,
};
use dyno_schema::timeline::ProcessRole;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;
use url::Url;

use crate::browser::Browser;
use crate::connection::{Connection, Event, Session};
use crate::harness::{HOST_BINDING, Harness};
use crate::{CdpError, DRIVER_NAME, PROBE};

/// The binding the probe calls with each event, in the game frame.
const PROBE_BINDING: &str = "__dynoProbe";

/// How long one CDP call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the run watches the game after it makes it active.
const ACTIVE_WATCH: Duration = Duration::from_secs(2);

/// How long the run watches after the purge, so the sampler reads the memory left: 6 samples.
const PURGE_WATCH: Duration = Duration::from_millis(1_500);

/// The trace category whose `TracingStartedInBrowser` event lists each frame of the page with its
/// process id.
const FRAMES_CATEGORY: &str = "disabled-by-default-devtools.timeline";

/// How long the trace that finds the frames' processes may take. It takes about 50 ms.
const FRAMES_TRACE_TIMEOUT: Duration = Duration::from_secs(5);

/// The name of the isolated world the self-checks run in, apart from the game's wrapped
/// prototypes.
const CHECK_WORLD: &str = "dyno-checks";

/// The self-checks, in the isolated world. The WebGL context is lost again at once.
const SELF_CHECKS: &str = r#"(() => {
  const gl = document.createElement("canvas").getContext("webgl");
  let renderer = null;
  if (gl) {
    const info = gl.getExtension("WEBGL_debug_renderer_info");
    renderer = info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
    const lose = gl.getExtension("WEBGL_lose_context");
    if (lose) lose.loseContext();
  }
  return { device_pixel_ratio: devicePixelRatio, webgl_renderer: renderer, time_origin: performance.timeOrigin };
})()"#;

/// What the self-checks give back.
#[derive(Deserialize)]
struct Checks {
    device_pixel_ratio: f64,
    webgl_renderer: Option<String>,
    time_origin: f64,
}

/// A message the game sent the harness page, on the page's clock.
#[derive(Deserialize)]
struct HostMessage {
    name: String,
    epoch_ms: f64,
}

/// A request seen on any session, joined by its id: a frame's document request starts on the
/// page's session and ends on the frame's (CDP feasibility test, surprise 4).
#[derive(Default)]
struct RequestSeen {
    url: String,
    mime_type: Option<String>,
}

/// The game frame's target, once it is attached.
struct GameFrame {
    session: Session,
    target_id: String,
}

/// The processes that run the game frame and the harness page, as the trace at idle names them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FrameProcesses {
    game: Option<u32>,
    page: Option<u32>,
}

/// Everything the run tracks while events arrive.
struct Run {
    started: Instant,
    messages: Vec<DriverMessage>,
    page: Session,
    game: Option<GameFrame>,
    requests: HashMap<String, RequestSeen>,
    in_flight: HashSet<String>,
    finished_on_frame: u32,
    last_activity: Instant,
    /// Messages to the host, kept until the frame's clock is known.
    to_host: Vec<HostMessage>,
    frame_time_origin: Option<f64>,
    /// The wall clock at the navigation, in ms since the epoch, as `performance.timeOrigin` is.
    navigated_at_epoch_ms: f64,
    /// The game frame's session and every session attached under it: nested frames and
    /// workers. Their requests are the game's.
    game_family: HashSet<String>,
    /// The game frame's own default JavaScript contexts, one per document. The probe also runs
    /// in a same-process frame inside it, which numbers its canvases and textures from 0 too, so
    /// its events are left out.
    game_contexts: HashSet<i64>,
    frame_processes: FrameProcesses,
}

/// URL schemes whose bytes never cross the network.
const LOCAL_SCHEMES: &[&str] = &["data:", "blob:"];

/// Runs the request and returns every message. An error before `hello` is returned as an error;
/// after it, the run ends with an `error` message.
pub async fn run(request: &RunRequest) -> Result<Vec<DriverMessage>, CdpError> {
    let launch_url = Url::parse(&request.launch_url).map_err(|_| CdpError::LaunchUrl)?;
    let harness = Harness::serve(&launch_url).await?;
    let browser = Browser::launch(&request.host_profile).await?;
    let sampler = browser.pid().map(Sampler::start);
    let outcome = run_in(&browser, &harness, request, sampler).await;
    browser.close().await;
    outcome
}

async fn run_in(
    browser: &Browser,
    harness: &Harness,
    request: &RunRequest,
    sampler: Option<Sampler>,
) -> Result<Vec<DriverMessage>, CdpError> {
    let (connection, mut events) = Connection::open(browser.web_socket_url(), CALL_TIMEOUT).await?;
    let browser_session = Session::browser(&connection);
    let version = browser_session
        .call("Browser.getVersion", json!({}))
        .await?;
    let hello = DriverMessage::Hello {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        driver: DriverInfo {
            name: DRIVER_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            browser: version["product"].as_str().map(browser_name),
        },
    };
    let page = open_page(&browser_session, &request.host_profile).await?;
    let mut run = Run {
        started: Instant::now(),
        messages: vec![hello],
        page,
        game: None,
        requests: HashMap::new(),
        in_flight: HashSet::new(),
        finished_on_frame: 0,
        last_activity: Instant::now(),
        to_host: Vec::new(),
        frame_time_origin: None,
        navigated_at_epoch_ms: 0.0,
        game_family: HashSet::new(),
        game_contexts: HashSet::new(),
        frame_processes: FrameProcesses::default(),
    };
    let deadline = run.started + Duration::from_millis(u64::from(request.timeout_ms));
    // The deadline bounds every call too, not only the waits between events.
    let outcome =
        tokio::time::timeout_at(deadline, run.drive(&mut events, harness, request, deadline))
            .await
            .unwrap_or(Err(CdpError::Timeout));
    if let (Some(sampler), Some(browser_pid)) = (sampler, browser.pid()) {
        run.add_memory(sampler.stop(), browser_pid);
    }
    match outcome {
        Ok(()) => run.end(DriverMessage::Done { t_ms: 0.0 }),
        Err(error) => run.end(DriverMessage::Error {
            message: error.to_string(),
            t_ms: 0.0,
        }),
    }
    // The browser may be gone already; the process is killed next either way.
    let _ = browser_session.call("Browser.close", json!({})).await;
    Ok(run.messages)
}

/// "Chromium 152.0.7977.82" from a product such as "HeadlessChrome/152.0.7977.82".
fn browser_name(product: &str) -> String {
    let version = product
        .split_once('/')
        .map_or(product, |(_, version)| version);
    format!("Chromium {version}")
}

/// A new page, attached flat, with the profile's screen, that auto-attaches each frame paused
/// before its first script.
async fn open_page(browser: &Session, profile: &HostProfile) -> Result<Session, CdpError> {
    let target = browser
        .call("Target.createTarget", json!({ "url": "about:blank" }))
        .await?;
    let target_id = text(&target, "targetId")?;
    let attached = browser
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await?;
    let page = browser.child(&text(&attached, "sessionId")?);
    for method in ["Page.enable", "Runtime.enable", "Network.enable"] {
        page.call(method, json!({})).await?;
    }
    // A window size sets the outer window, which Chromium clamps, so the page gets the
    // profile's viewport by emulation. The frame's pixel ratio still comes from the launch flag.
    let viewport = profile.viewport;
    page.call(
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": viewport.width,
            "height": viewport.height,
            "screenWidth": viewport.width,
            "screenHeight": viewport.height,
            "deviceScaleFactor": profile.device_scale_factor,
            "mobile": true,
        }),
    )
    .await?;
    page.call("Runtime.addBinding", json!({ "name": HOST_BINDING }))
        .await?;
    page.call("Target.setAutoAttach", auto_attach()).await?;
    Ok(page)
}

fn auto_attach() -> Value {
    json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true })
}

fn text(value: &Value, key: &str) -> Result<String, CdpError> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CdpError::Reply(format!("no {key}")))
}

impl Run {
    fn t_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1_000.0
    }

    fn mark(&mut self, mark: RunMark) {
        let t_ms = self.t_ms();
        self.messages.push(DriverMessage::Mark { mark, t_ms });
    }

    /// Adds the last message with the run's time.
    fn end(&mut self, last: DriverMessage) {
        let t_ms = self.t_ms();
        let last = match last {
            DriverMessage::Done { .. } => DriverMessage::Done { t_ms },
            DriverMessage::Error { message, .. } => DriverMessage::Error { message, t_ms },
            other => other,
        };
        self.messages.push(last);
    }

    async fn drive(
        &mut self,
        events: &mut mpsc::UnboundedReceiver<Event>,
        harness: &Harness,
        request: &RunRequest,
        deadline: Instant,
    ) -> Result<(), CdpError> {
        self.page
            .call("Page.navigate", json!({ "url": harness.url() }))
            .await?;
        self.started = Instant::now();
        self.navigated_at_epoch_ms = epoch_ms();
        self.last_activity = self.started;
        self.mark(RunMark::Navigated);
        let idle_after = Duration::from_millis(u64::from(request.idle_after_ms));
        loop {
            let wake = self.idle_at(idle_after).unwrap_or(deadline).min(deadline);
            tokio::select! {
                event = events.recv() => {
                    let event = event.ok_or_else(|| CdpError::Gone("the browser".to_owned()))?;
                    self.on_event(event).await?;
                }
                () = tokio::time::sleep_until(wake) => {
                    if Instant::now() >= deadline {
                        return Err(CdpError::Timeout);
                    }
                    if self.is_idle(idle_after) {
                        break;
                    }
                }
            }
        }
        self.at_idle(deadline).await?;
        self.find_frame_processes(events, deadline).await?;
        let active = &request.host_profile.active_message;
        self.page
            .call(
                "Runtime.evaluate",
                json!({ "expression": format!("__dyno.send({})", json!(active)) }),
            )
            .await?;
        self.mark(RunMark::Activated);
        // A deadline inside the watch ends the run as a timeout, not as a shorter watch.
        let watch_until = Instant::now() + ACTIVE_WATCH;
        while let Ok(Some(event)) = tokio::time::timeout_at(watch_until, events.recv()).await {
            self.on_event(event).await?;
        }
        self.purge(events).await
    }

    /// Sends a critical memory pressure signal to the game frame's renderer and the page's, and
    /// forces a garbage collection in the game frame, then watches for a moment, so the timeline
    /// shows the memory the game holds once garbage and caches go. It comes after the active
    /// watch, so no lab measure sees it. A call that fails changes nothing.
    async fn purge(&mut self, events: &mut mpsc::UnboundedReceiver<Event>) -> Result<(), CdpError> {
        let critical = json!({ "level": "critical" });
        let game = self.game.as_ref().map(|game| game.session.clone());
        for session in game.iter().chain([&self.page]) {
            let _ = session
                .call("Memory.simulatePressureNotification", critical.clone())
                .await;
        }
        if let Some(game) = &game {
            let _ = game.call("HeapProfiler.collectGarbage", json!({})).await;
        }
        self.mark(RunMark::Purged);
        let watch_until = Instant::now() + PURGE_WATCH;
        while let Ok(Some(event)) = tokio::time::timeout_at(watch_until, events.recv()).await {
            self.on_event(event).await?;
        }
        Ok(())
    }

    /// When the frame becomes idle if nothing else happens, once it has loaded something.
    fn idle_at(&self, idle_after: Duration) -> Option<Instant> {
        let loaded = self.game.is_some() && self.finished_on_frame > 0;
        (loaded && self.in_flight.is_empty()).then_some(self.last_activity + idle_after)
    }

    fn is_idle(&self, idle_after: Duration) -> bool {
        self.idle_at(idle_after)
            .is_some_and(|idle_at| Instant::now() >= idle_at)
    }

    async fn on_event(&mut self, event: Event) -> Result<(), CdpError> {
        let session_id = event.session_id.as_deref();
        let on_game = session_id.is_some() && session_id == self.game_session_id();
        let on_page = session_id.is_some() && session_id == self.page.id();
        let in_family = session_id.is_some_and(|id| self.game_family.contains(id));
        let params = &event.params;
        match event.method.as_str() {
            "Target.attachedToTarget" => self.on_attached(on_page, in_family, params).await?,
            "Target.detachedFromTarget"
                if params["sessionId"].as_str() == self.game_session_id() =>
            {
                return Err(CdpError::Gone("the game frame".to_owned()));
            }
            "Inspector.targetCrashed" if on_game || on_page => {
                return Err(CdpError::Gone("a crashed renderer".to_owned()));
            }
            "Network.requestWillBeSent" => {
                let id = request_id(params);
                self.requests.entry(id.clone()).or_default().url = params["request"]["url"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if in_family {
                    self.in_flight.insert(id);
                    self.last_activity = Instant::now();
                }
            }
            "Network.responseReceived" => {
                let seen = self.requests.entry(request_id(params)).or_default();
                seen.mime_type = params["response"]["mimeType"].as_str().map(str::to_owned);
                if seen.url.is_empty() {
                    seen.url = params["response"]["url"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                }
            }
            "Network.loadingFinished" if in_family => {
                let id = request_id(params);
                self.in_flight.remove(&id);
                self.last_activity = Instant::now();
                self.finished_on_frame += 1;
                let seen = self.requests.remove(&id).unwrap_or_default();
                if LOCAL_SCHEMES
                    .iter()
                    .any(|scheme| seen.url.starts_with(scheme))
                {
                    return Ok(());
                }
                let t_ms = self.t_ms();
                self.messages.push(DriverMessage::Request {
                    url: seen.url,
                    mime_type: seen.mime_type,
                    bytes: params["encodedDataLength"]
                        .as_f64()
                        .map_or(0, |bytes| bytes as u64),
                    t_ms,
                });
            }
            "Page.frameNavigated" if on_game && self.is_game_document(params) => {
                self.mark(RunMark::FrameDocument);
            }
            "Network.loadingFailed" if in_family => {
                self.in_flight.remove(&request_id(params));
                self.last_activity = Instant::now();
            }
            "Runtime.executionContextCreated" if on_game => {
                let context = &params["context"];
                let is_default = context["auxData"]["isDefault"] == true;
                if is_default
                    && self.is_game_frame(&context["auxData"]["frameId"])
                    && let Some(id) = context["id"].as_i64()
                {
                    self.game_contexts.insert(id);
                }
            }
            "Runtime.bindingCalled"
                if on_game
                    && params["name"] == PROBE_BINDING
                    && params["executionContextId"]
                        .as_i64()
                        .is_some_and(|id| self.game_contexts.contains(&id)) =>
            {
                // A payload that is not an event is the game's, not the probe's: it is dropped.
                if let Some(event) = params["payload"]
                    .as_str()
                    .and_then(|payload| serde_json::from_str::<ProbeEvent>(payload).ok())
                {
                    self.messages.push(DriverMessage::Probe { event });
                }
            }
            "Runtime.bindingCalled" if on_page && params["name"] == HOST_BINDING => {
                if let Some(message) = params["payload"]
                    .as_str()
                    .and_then(|payload| serde_json::from_str::<HostMessage>(payload).ok())
                {
                    self.to_host.push(message);
                    self.send_host_messages();
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Whether a navigation is the game frame's own document, not a frame inside it. The game
    /// frame's id is its target's id; its `parentId` names the harness page's frame.
    fn is_game_document(&self, params: &Value) -> bool {
        self.is_game_frame(&params["frame"]["id"])
    }

    /// Whether a frame id is the game frame's: its target's id.
    fn is_game_frame(&self, frame_id: &Value) -> bool {
        let target_id = self.game.as_ref().map(|game| game.target_id.as_str());
        target_id.is_some() && frame_id.as_str() == target_id
    }

    fn game_session_id(&self) -> Option<&str> {
        self.game.as_ref().and_then(|game| game.session.id())
    }

    /// Sets up the game frame, the first frame the page attaches, before its first script runs.
    /// A target under the game frame, such as a worker or a nested frame, joins its family: its
    /// network counts. Any other target just runs.
    async fn on_attached(
        &mut self,
        on_page: bool,
        in_family: bool,
        params: &Value,
    ) -> Result<(), CdpError> {
        let session_id = text(params, "sessionId")?;
        let session = self.page.child(&session_id);
        let info = &params["targetInfo"];
        let is_game = on_page && self.game.is_none() && info["type"] == "iframe";
        if in_family {
            self.game_family.insert(session_id.clone());
            // A worker has no Page domain, and some targets refuse these; the run goes on.
            let _ = session.call("Network.enable", json!({})).await;
            let _ = session.call("Target.setAutoAttach", auto_attach()).await;
        }
        if is_game {
            for method in ["Network.enable", "Page.enable", "Runtime.enable"] {
                session.call(method, json!({})).await?;
            }
            session
                .call("Runtime.addBinding", json!({ "name": PROBE_BINDING }))
                .await?;
            session
                .call(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({ "source": PROBE }),
                )
                .await?;
            session.call("Target.setAutoAttach", auto_attach()).await?;
            self.game = Some(GameFrame {
                session: session.clone(),
                target_id: text(info, "targetId")?,
            });
            self.game_family.insert(session_id);
            self.last_activity = Instant::now();
        }
        if params["waitingForDebugger"] == true {
            session
                .call("Runtime.runIfWaitingForDebugger", json!({}))
                .await?;
        }
        Ok(())
    }

    /// The heap and the self-checks, read once the frame is idle. The game's own thread answers
    /// these calls, so each waits until the run's deadline.
    async fn at_idle(&mut self, deadline: Instant) -> Result<(), CdpError> {
        self.mark(RunMark::Idle);
        let game = self
            .game
            .as_ref()
            .ok_or_else(|| CdpError::Gone("the game frame".to_owned()))?;
        let session = game.session.clone();
        let heap = session
            .call_until("Runtime.getHeapUsage", json!({}), deadline)
            .await?;
        let used_bytes = heap["usedSize"].as_f64().map_or(0, |bytes| bytes as u64);
        let t_ms = self.t_ms();
        self.messages.push(DriverMessage::Heap { used_bytes, t_ms });
        let world = session
            .call_until(
                "Page.createIsolatedWorld",
                json!({ "frameId": game.target_id, "worldName": CHECK_WORLD }),
                deadline,
            )
            .await?;
        let checked = session
            .call_until(
                "Runtime.evaluate",
                json!({
                    "expression": SELF_CHECKS,
                    "contextId": world["executionContextId"],
                    "returnByValue": true,
                }),
                deadline,
            )
            .await?;
        let checks: Checks = serde_json::from_value(checked["result"]["value"].clone())
            .map_err(|_| CdpError::Reply("the self-checks gave no result".to_owned()))?;
        let t_ms = self.t_ms();
        self.messages.push(DriverMessage::SelfCheck {
            device_pixel_ratio: checks.device_pixel_ratio,
            webgl_renderer: checks.webgl_renderer,
            frame_clock_at_ms: Some(checks.time_origin - self.navigated_at_epoch_ms),
            t_ms,
        });
        self.frame_time_origin = Some(checks.time_origin);
        self.send_host_messages();
        Ok(())
    }

    /// Finds the processes of the game frame and of the harness page with a short trace on the
    /// page's session. CDP gives no process ids otherwise. A trace that cannot start or end
    /// leaves them unknown, and the run goes on; other events that arrive meanwhile are handled
    /// as usual.
    async fn find_frame_processes(
        &mut self,
        events: &mut mpsc::UnboundedReceiver<Event>,
        deadline: Instant,
    ) -> Result<(), CdpError> {
        let Some(game_frame) = self.game.as_ref().map(|game| game.target_id.clone()) else {
            return Ok(());
        };
        let config = json!({
            "traceConfig": { "includedCategories": [FRAMES_CATEGORY] },
            "transferMode": "ReportEvents",
        });
        if self.page.call("Tracing.start", config).await.is_err()
            || self.page.call("Tracing.end", json!({})).await.is_err()
        {
            return Ok(());
        }
        let until = (Instant::now() + FRAMES_TRACE_TIMEOUT).min(deadline);
        // A trace that does not complete in time leaves the processes unknown.
        while let Ok(event) = tokio::time::timeout_at(until, events.recv()).await {
            let event = event.ok_or_else(|| CdpError::Gone("the browser".to_owned()))?;
            let on_page =
                event.session_id.is_some() && event.session_id.as_deref() == self.page.id();
            match event.method.as_str() {
                "Tracing.dataCollected" if on_page => {
                    if let Some(found) = frame_processes(&event.params["value"], &game_frame) {
                        self.frame_processes = found;
                    }
                }
                "Tracing.tracingComplete" if on_page => break,
                _ => self.on_event(event).await?,
            }
        }
        Ok(())
    }

    /// Adds the sampler's readings since the navigation as `memory` messages, and the roles of the
    /// browser, the GPU process, the game frame's process and the page's.
    fn add_memory(&mut self, readings: Vec<Reading>, browser_pid: u32) {
        let mut roles = vec![AssignedRole {
            pid: browser_pid,
            role: ProcessRole::Browser,
        }];
        roles.extend(
            memory::pids_of_kind(&readings, memory::GPU_KIND)
                .into_iter()
                .map(|pid| AssignedRole {
                    pid,
                    role: ProcessRole::Gpu,
                }),
        );
        let FrameProcesses { game, page } = self.frame_processes;
        // The game's role comes last, so it wins if the page and the game share a process.
        for (pid, role) in [(page, ProcessRole::Page), (game, ProcessRole::Game)] {
            if let Some(pid) = pid {
                roles.push(AssignedRole { pid, role });
            }
        }
        self.messages
            .extend(memory::memory_messages(readings, self.started.into_std()));
        let t_ms = self.t_ms();
        self.messages.push(DriverMessage::Roles { roles, t_ms });
    }

    /// Each message to the host as a probe event on the frame's clock, once that clock is known.
    fn send_host_messages(&mut self) {
        let Some(time_origin) = self.frame_time_origin else {
            return;
        };
        for message in self.to_host.drain(..) {
            self.messages.push(DriverMessage::Probe {
                event: ProbeEvent::HostMessage {
                    direction: MessageDirection::ToHost,
                    name: message.name,
                    t_ms: message.epoch_ms - time_origin,
                },
            });
        }
    }
}

/// The wall clock in ms since the epoch.
fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1_000.0)
}

/// The processes of the game frame and of the page's main frame from a chunk of trace events:
/// `TracingStartedInBrowser` lists each frame of the page with its `processId`, and the game
/// frame's id is its target's id. None when the chunk does not hold that event.
fn frame_processes(events: &Value, game_frame: &str) -> Option<FrameProcesses> {
    let started = events
        .as_array()?
        .iter()
        .find(|event| event["name"] == "TracingStartedInBrowser")?;
    let frames = started["args"]["data"]["frames"].as_array()?;
    let process_of = |is_it: &dyn Fn(&Value) -> bool| {
        frames
            .iter()
            .find(|frame| is_it(frame))
            .and_then(|frame| frame["processId"].as_u64())
            .and_then(|pid| u32::try_from(pid).ok())
    };
    Some(FrameProcesses {
        game: process_of(&|frame| frame["frame"] == game_frame),
        page: process_of(&|frame| frame["isOutermostMainFrame"] == true),
    })
}

fn request_id(params: &Value) -> String {
    params["requestId"].as_str().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trace_names_the_game_frames_process_and_the_pages() {
        let events = json!([
            { "name": "thread_name", "args": {} },
            {
                "name": "TracingStartedInBrowser",
                "args": { "data": { "frames": [
                    { "frame": "PAGE", "isOutermostMainFrame": true, "processId": 11 },
                    { "frame": "GAME", "isOutermostMainFrame": false, "parent": "PAGE", "processId": 22 },
                ] } },
            },
        ]);
        assert_eq!(
            frame_processes(&events, "GAME"),
            Some(FrameProcesses {
                game: Some(22),
                page: Some(11)
            })
        );
        assert_eq!(
            frame_processes(&events, "OTHER"),
            Some(FrameProcesses {
                game: None,
                page: Some(11)
            })
        );
        assert_eq!(frame_processes(&json!([{ "name": "x" }]), "GAME"), None);
    }

    #[test]
    fn the_browser_is_named_by_its_version() {
        assert_eq!(
            browser_name("HeadlessChrome/152.0.7977.82"),
            "Chromium 152.0.7977.82"
        );
    }
}
