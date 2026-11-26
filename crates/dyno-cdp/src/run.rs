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
//!
//! The run also keeps what it saw of the page for the run's record ([`PageSeen`]): the document
//! that the game frame held at the end, by the URL it committed at and the HTTP status of the
//! response it came from, and how a run that failed ended. A frame whose navigation fails commits
//! the browser's error page, and a 404 page is a document too, which a `frame_document` mark alone
//! does not tell from the game. The frame is followed by its id on its own session and on the
//! page's, where a redirect into the harness page's own site moves it. A frame that leaves both,
//! as when its target detaches before the page's session says where it went, is not observed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dyno_run::PageSeen;
use dyno_run::memory::{self, Reading, Sampler};
use dyno_schema::probe::{MessageDirection, ProbeEvent};
use dyno_schema::profile::HostProfile;
use dyno_schema::result::DriverInfo;
use dyno_schema::run_record::{PageDocument, PageEnd};
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
    /// What the run saw of the game frame's documents, for the record only.
    evidence: FrameEvidence,
}

/// What the run saw of the game frame's documents, for the run's record. Nothing here changes a
/// message.
struct FrameEvidence {
    /// The launch URL, whose scheme, host and port are the build's own.
    launch_url: Url,
    /// The harness page's session, and its own frame id, the parent of the game frame there.
    page_session: Option<String>,
    page_frame_id: String,
    /// The last document that the game frame committed. None until it commits one.
    commit: Option<Commit>,
    /// Whether the game frame's target detached after its last commit.
    frame_left: bool,
    /// The session and target id of the game frame's target, noted as it attached.
    attaching: Option<(String, String)>,
    /// The HTTP status of the last few document responses that no commit has taken yet, by
    /// loader id: a response comes before its commit. At most [`PENDING_LOADERS`], the oldest
    /// going first, so a page of many frames does not hold one for each of them.
    pending_statuses: VecDeque<(String, u64)>,
}

/// How many document responses the run keeps for commits to come.
const PENDING_LOADERS: usize = 8;

/// Where a document that the game frame committed is, by its URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// The browser's error page.
    ErrorPage,
    /// The launch URL's scheme, host and port.
    Launch,
    /// Anywhere else.
    Other,
}

/// A document that the game frame committed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Commit {
    place: Place,
    /// The loader that the document came from, which its response names too.
    loader: Option<String>,
    /// Whether the game frame's own session said so, rather than the page's.
    on_game_session: bool,
    /// The HTTP status of its response. None until the run sees it.
    status: Option<u64>,
}

/// What a run that got its `hello` gave: every message, and what it saw of the page.
pub struct Ran {
    pub messages: Vec<DriverMessage>,
    pub seen: PageSeen,
}

/// URL schemes whose bytes never cross the network.
const LOCAL_SCHEMES: &[&str] = &["data:", "blob:"];

/// Runs the request and returns every message, with what the run saw of the page. An error
/// before `hello` is returned as an error; after it, the run ends with an `error` message.
pub async fn run(request: &RunRequest) -> Result<Ran, CdpError> {
    let launch_url = Url::parse(&request.launch_url).map_err(|_| CdpError::LaunchUrl)?;
    let harness = Harness::serve(&launch_url).await?;
    let browser = Browser::launch(&request.host_profile).await?;
    let sampler = browser.pid().map(Sampler::start);
    let outcome = run_in(&browser, &harness, request, &launch_url, sampler).await;
    browser.close().await;
    outcome
}

async fn run_in(
    browser: &Browser,
    harness: &Harness,
    request: &RunRequest,
    launch_url: &Url,
    sampler: Option<Sampler>,
) -> Result<Ran, CdpError> {
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
    let (page, page_frame_id) = open_page(&browser_session, &request.host_profile).await?;
    let page_session = page.id().map(str::to_owned);
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
        evidence: FrameEvidence {
            launch_url: launch_url.clone(),
            page_session,
            page_frame_id,
            commit: None,
            frame_left: false,
            attaching: None,
            pending_statuses: VecDeque::new(),
        },
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
    let end = page_end(outcome.as_ref().err(), connection.is_closed());
    // The last message takes its time here, as it always did, before the record reads anything.
    match outcome {
        Ok(()) => run.end(DriverMessage::Done { t_ms: 0.0 }),
        Err(error) => run.end(DriverMessage::Error {
            message: error.to_string(),
            t_ms: 0.0,
        }),
    }
    // An event the run did not take before it ended, as a detach queued behind a call that
    // failed, still counts for the record.
    while let Ok(event) = events.try_recv() {
        let game = run
            .game
            .as_ref()
            .and_then(|game| Some((game.session.id()?.to_owned(), game.target_id.clone())));
        run.evidence
            .late(&event, game.as_ref().map(|(s, t)| (s.as_str(), t.as_str())));
    }
    // The browser may be gone already; the process is killed next either way.
    let _ = browser_session.call("Browser.close", json!({})).await;
    Ok(Ran {
        seen: PageSeen {
            document: Some(run.evidence.document()),
            end,
            ..PageSeen::default()
        },
        messages: run.messages,
    })
}

/// How a run that got its `hello` ended, for its record, when it did not simply end with `done`:
/// the DevTools connection to the browser closed during it, whatever error that gave, even a run
/// that the driver ended with `done` as before, since a watch takes a closed event stream for a
/// quiet one; or else its deadline passed, or any other error, such as a crashed renderer. The
/// run's messages, and so its measures, are as they were: this is for the record only.
fn page_end(error: Option<&CdpError>, connection_closed: bool) -> Option<PageEnd> {
    match error {
        _ if connection_closed => Some(PageEnd::ConnectionClosed),
        None => None,
        Some(CdpError::Timeout) => Some(PageEnd::TimedOut),
        Some(_) => Some(PageEnd::Failed),
    }
}

/// Where a frame committed, from the `frame` of `Page.frameNavigated`: the browser's error page
/// when the navigation failed (it names the URL it could not reach), the launch URL's scheme,
/// host and port, or elsewhere.
///
/// The three are compared one by one, not with `Url::origin`, which recurses into a `blob:` URL
/// and overflows the stack on one nested deep enough (RUSTSEC-2026-0592): the game chooses the
/// URL. A `blob:` document is so elsewhere, though the browser gives it its creator's origin.
fn place_of(frame: &Value, launch_url: &Url) -> Place {
    let url = frame["url"].as_str().and_then(|url| Url::parse(url).ok());
    let failed = frame["unreachableUrl"]
        .as_str()
        .is_some_and(|unreachable| !unreachable.is_empty());
    match url {
        _ if failed => Place::ErrorPage,
        Some(url) if url.scheme() == "chrome-error" => Place::ErrorPage,
        Some(url) if same_place(&url, launch_url) => Place::Launch,
        _ => Place::Other,
    }
}

/// The document that the game frame held at the end of the run. A frame that left the sessions
/// the run follows is not observed, whether it had committed or not. One at the launch URL's place
/// is the build's only when its response's HTTP status is known and below 400, and not observed
/// when the run did not see the response.
fn document_at_end(commit: Option<&Commit>, frame_left: bool) -> PageDocument {
    if frame_left {
        return PageDocument::NotObserved;
    }
    let Some(commit) = commit else {
        return PageDocument::NoDocument;
    };
    match (commit.place, commit.status) {
        (Place::ErrorPage, _) => PageDocument::ErrorPage,
        (Place::Other, _) => PageDocument::OtherUrl,
        (Place::Launch, Some(status)) if status >= 400 => PageDocument::HttpError,
        (Place::Launch, Some(_)) => PageDocument::Build,
        (Place::Launch, None) => PageDocument::NotObserved,
    }
}

impl FrameEvidence {
    /// Keeps a document that the game frame committed, from its `frame` in `Page.frameNavigated`:
    /// on its own session, or on the page's, which says where the frame went.
    fn committed(&mut self, frame: &Value, on_game_session: bool) {
        let loader = frame["loaderId"].as_str().map(str::to_owned);
        let status = loader.as_ref().and_then(|loader| {
            let at = self
                .pending_statuses
                .iter()
                .position(|(pending, _)| pending == loader)?;
            self.pending_statuses.remove(at).map(|(_, status)| status)
        });
        self.commit = Some(Commit {
            place: place_of(frame, &self.launch_url),
            loader,
            on_game_session,
            status,
        });
        self.frame_left = false;
    }

    /// Notes the game frame's target as it attaches, from `Target.attachedToTarget`, before its
    /// set-up: the run may end on a failed set-up call before it takes the frame for the game's.
    fn attaching(&mut self, params: &Value) {
        if let (Some(session), Some(target)) = (
            params["sessionId"].as_str(),
            params["targetInfo"]["targetId"].as_str(),
        ) {
            self.attaching = Some((session.to_owned(), target.to_owned()));
        }
    }

    /// The game frame's target detached: until the page's session says where the frame went,
    /// the run does not know what it holds, nor whether it holds anything.
    fn detached(&mut self) {
        self.frame_left = true;
    }

    /// A document response, with its loader and its HTTP status.
    fn response(&mut self, loader: &str, status: u64) {
        keep_status(
            self.commit.as_mut(),
            &mut self.pending_statuses,
            loader,
            status,
        );
    }

    /// Whether a frame on the page's session is the game frame: the one with the game target's id,
    /// or, with no game target, the harness page's own child, its only frame. `game` is the game
    /// frame's session and target id, once it attached.
    fn is_game_frame_on_page(&self, frame: &Value, game: Option<(&str, &str)>) -> bool {
        match game {
            Some((_, target_id)) => frame["id"].as_str() == Some(target_id),
            None => frame["parentId"].as_str() == Some(&self.page_frame_id),
        }
    }

    /// An event that the run did not take before it ended, as a detach queued behind a call that
    /// failed. It counts for the record as it would have, and changes nothing else.
    fn late(&mut self, event: &Event, game: Option<(&str, &str)>) {
        // A frame whose set-up failed is the game's only by the note taken as it attached.
        let attaching = self.attaching.clone();
        let game = game.or(attaching
            .as_ref()
            .map(|(session, target)| (session.as_str(), target.as_str())));
        let session = event.session_id.as_deref();
        let game_session = game.map(|(session, _)| session);
        let on_game = session.is_some() && session == game_session;
        let on_page = session.is_some() && session == self.page_session.as_deref();
        let params = &event.params;
        match event.method.as_str() {
            "Target.detachedFromTarget"
                if game_session.is_some() && params["sessionId"].as_str() == game_session =>
            {
                self.detached();
            }
            "Page.frameNavigated"
                if on_game && game.map(|(_, target)| target) == params["frame"]["id"].as_str() =>
            {
                self.committed(&params["frame"], true);
            }
            "Page.frameNavigated"
                if on_page && self.is_game_frame_on_page(&params["frame"], game) =>
            {
                self.committed(&params["frame"], false);
            }
            "Network.responseReceived" if params["type"] == "Document" => {
                if let (Some(loader), Some(status)) = (
                    params["loaderId"].as_str(),
                    params["response"]["status"].as_u64(),
                ) {
                    self.response(loader, status);
                }
            }
            _ => {}
        }
    }

    /// The document that the game frame held at the end of the run.
    fn document(&self) -> PageDocument {
        document_at_end(self.commit.as_ref(), self.frame_left)
    }
}

/// Keeps a document response's status for the commit of its loader: the current commit's, when
/// it is that loader's, or else among the few pending ones.
fn keep_status(
    commit: Option<&mut Commit>,
    pending: &mut VecDeque<(String, u64)>,
    loader: &str,
    status: u64,
) {
    match commit {
        Some(commit) if commit.loader.as_deref() == Some(loader) => commit.status = Some(status),
        _ => {
            if pending.len() == PENDING_LOADERS {
                pending.pop_front();
            }
            pending.push_back((loader.to_owned(), status));
        }
    }
}

/// Whether two URLs have the same scheme, host and port.
fn same_place(url: &Url, other: &Url) -> bool {
    url.scheme() == other.scheme()
        && url.host() == other.host()
        && url.port_or_known_default() == other.port_or_known_default()
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
/// It gives the page's session and its frame id, which is its target id.
async fn open_page(
    browser: &Session,
    profile: &HostProfile,
) -> Result<(Session, String), CdpError> {
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
    Ok((page, target_id))
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
        // For the record only: the game frame's target, noted before its set-up, whose calls can
        // fail with its detach still queued behind them.
        if event.method == "Target.attachedToTarget"
            && on_page
            && self.game.is_none()
            && params["targetInfo"]["type"] == "iframe"
        {
            self.evidence.attaching(params);
        }
        match event.method.as_str() {
            "Target.attachedToTarget" => self.on_attached(on_page, in_family, params).await?,
            "Target.detachedFromTarget"
                if params["sessionId"].as_str() == self.game_session_id() =>
            {
                self.evidence.detached();
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
                if params["type"] == "Document"
                    && let (Some(loader), Some(status)) = (
                        params["loaderId"].as_str(),
                        params["response"]["status"].as_u64(),
                    )
                {
                    self.evidence.response(loader, status);
                }
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
                self.evidence.committed(&params["frame"], true);
            }
            // The page's session reports the game frame when it is in the page's own process, as
            // after a redirect into the harness page's site. No probe runs there, so it is no
            // new document for the lab measures.
            "Page.frameNavigated"
                if on_page
                    && self
                        .evidence
                        .is_game_frame_on_page(&params["frame"], self.game_ids()) =>
            {
                self.evidence.committed(&params["frame"], false);
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

    /// The game frame's session and target id, once it attached.
    fn game_ids(&self) -> Option<(&str, &str)> {
        let game = self.game.as_ref()?;
        Some((game.session.id()?, game.target_id.as_str()))
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

    fn launch_url() -> Url {
        Url::parse("http://127.0.0.1:7372/?session=1").unwrap()
    }

    #[test]
    fn a_document_at_the_launch_urls_scheme_host_and_port_is_at_its_place() {
        let frame = json!({ "id": "GAME", "url": "http://127.0.0.1:7372/game/index.html" });
        assert_eq!(place_of(&frame, &launch_url()), Place::Launch);
    }

    #[test]
    fn a_failed_navigation_commits_the_error_page() {
        let frame = json!({
            "id": "GAME",
            "url": "chrome-error://chromewebdata/",
            "unreachableUrl": "http://127.0.0.1:7372/",
        });
        assert_eq!(place_of(&frame, &launch_url()), Place::ErrorPage);
        // The error page names the URL it could not reach, whatever URL it gives itself.
        let frame = json!({
            "id": "GAME",
            "url": "http://127.0.0.1:7372/",
            "unreachableUrl": "http://127.0.0.1:7372/",
        });
        assert_eq!(place_of(&frame, &launch_url()), Place::ErrorPage);
    }

    #[test]
    fn a_document_at_another_place_or_with_no_url_is_elsewhere() {
        let nested_blob = format!("{}http://127.0.0.1:7372/x", "blob:".repeat(100_000));
        for url in [
            "http://127.0.0.1:9999/",
            "https://127.0.0.1:7372/",
            "https://games.example/",
            "about:blank",
            "blob:http://127.0.0.1:7372/0a1b",
            &nested_blob,
        ] {
            let frame = json!({ "id": "GAME", "url": url, "unreachableUrl": "" });
            assert_eq!(
                place_of(&frame, &launch_url()),
                Place::Other,
                "{}",
                &url[..url.len().min(40)]
            );
        }
        assert_eq!(
            place_of(&json!({ "id": "GAME" }), &launch_url()),
            Place::Other
        );
    }

    fn commit(place: Place, status: Option<u64>) -> Commit {
        Commit {
            place,
            loader: Some("L".to_owned()),
            on_game_session: true,
            status,
        }
    }

    #[test]
    fn a_document_at_the_launch_place_is_the_builds_only_with_a_status_below_400() {
        let at = |status| document_at_end(Some(&commit(Place::Launch, status)), false);
        assert_eq!(at(Some(200)), PageDocument::Build);
        assert_eq!(at(Some(404)), PageDocument::HttpError);
        // A response the run did not see says nothing of the document.
        assert_eq!(at(None), PageDocument::NotObserved);
    }

    #[test]
    fn the_document_at_the_end_is_the_last_commit_where_the_run_could_follow_it() {
        assert_eq!(document_at_end(None, false), PageDocument::NoDocument);
        let error = commit(Place::ErrorPage, None);
        assert_eq!(
            document_at_end(Some(&error), false),
            PageDocument::ErrorPage
        );
        let other = commit(Place::Other, Some(200));
        assert_eq!(document_at_end(Some(&other), false), PageDocument::OtherUrl);
        // A frame that left the sessions the run follows holds a document the run did not see,
        // and one that left before any commit is not one with no document.
        assert_eq!(
            document_at_end(Some(&other), true),
            PageDocument::NotObserved
        );
        assert_eq!(document_at_end(None, true), PageDocument::NotObserved);
    }

    #[test]
    fn a_response_goes_to_the_current_commit_or_waits_among_a_few() {
        let mut current = commit(Place::Launch, None);
        let mut pending = VecDeque::new();
        // A late response of the current commit's loader is its status.
        keep_status(Some(&mut current), &mut pending, "L", 404);
        assert_eq!(current.status, Some(404));
        assert!(pending.is_empty());
        // Responses of other frames wait, at most a few, the oldest going first.
        for frame in 0..20 {
            keep_status(None, &mut pending, &format!("other-{frame}"), 200);
        }
        assert_eq!(pending.len(), PENDING_LOADERS);
        assert_eq!(pending.front().unwrap().0, "other-12");
    }

    /// The game frame's session and target id in the tests.
    const GAME_IDS: Option<(&str, &str)> = Some(("GAME", "GAME-FRAME"));

    fn evidence() -> FrameEvidence {
        FrameEvidence {
            launch_url: launch_url(),
            page_session: Some("PAGE".to_owned()),
            page_frame_id: "PAGE-FRAME".to_owned(),
            commit: None,
            frame_left: false,
            attaching: None,
            pending_statuses: VecDeque::new(),
        }
    }

    fn event(session: &str, method: &str, params: Value) -> Event {
        Event {
            session_id: Some(session.to_owned()),
            method: method.to_owned(),
            params,
        }
    }

    fn navigated(url: &str, loader: &str) -> Value {
        json!({ "frame": { "id": "GAME-FRAME", "url": url, "loaderId": loader } })
    }

    fn document_response(loader: &str, status: u64) -> Value {
        json!({ "type": "Document", "loaderId": loader, "response": { "status": status } })
    }

    #[test]
    fn a_detach_queued_behind_a_failed_call_leaves_the_document_not_observed() {
        let mut evidence = evidence();
        evidence.response("L1", 200);
        evidence.committed(&navigated("http://127.0.0.1:7372/", "L1")["frame"], true);
        assert_eq!(evidence.document(), PageDocument::Build);
        // The run ended on a failed call; the detach was still in the queue.
        evidence.late(
            &event(
                "PAGE",
                "Target.detachedFromTarget",
                json!({ "sessionId": "GAME" }),
            ),
            GAME_IDS,
        );
        assert_eq!(evidence.document(), PageDocument::NotObserved);
    }

    #[test]
    fn a_detach_queued_behind_a_failed_set_up_counts_by_the_note_taken_as_the_frame_attached() {
        let mut evidence = evidence();
        evidence.response("L1", 200);
        evidence.committed(&navigated("http://127.0.0.1:7372/", "L1")["frame"], true);
        let detach = event(
            "PAGE",
            "Target.detachedFromTarget",
            json!({ "sessionId": "GAME" }),
        );
        // The run never took the frame for the game's: with no note, the detach is no one's.
        evidence.late(&detach, None);
        assert_eq!(evidence.document(), PageDocument::Build);
        evidence.attaching(&json!({
            "sessionId": "GAME",
            "targetInfo": { "type": "iframe", "targetId": "GAME-FRAME" },
        }));
        evidence.late(&detach, None);
        assert_eq!(evidence.document(), PageDocument::NotObserved);
    }

    #[test]
    fn a_page_commit_after_the_detach_says_where_the_frame_went_and_one_before_does_not() {
        let mut evidence = evidence();
        let detach = event(
            "PAGE",
            "Target.detachedFromTarget",
            json!({ "sessionId": "GAME" }),
        );
        let on_page = event(
            "PAGE",
            "Page.frameNavigated",
            navigated("http://localhost:7372/", "L2"),
        );
        // The page's session saw the frame before it detached: that says nothing of after.
        evidence.late(&on_page, GAME_IDS);
        evidence.late(&detach, GAME_IDS);
        assert_eq!(evidence.document(), PageDocument::NotObserved);
        // And after it: the frame is there.
        evidence.late(&on_page, GAME_IDS);
        assert_eq!(evidence.document(), PageDocument::OtherUrl);
    }

    #[test]
    fn late_events_of_other_sessions_and_frames_change_nothing() {
        let mut evidence = evidence();
        evidence.committed(&navigated("http://127.0.0.1:7372/", "L1")["frame"], true);
        evidence.late(
            &event(
                "PAGE",
                "Network.responseReceived",
                document_response("L1", 404),
            ),
            GAME_IDS,
        );
        assert_eq!(evidence.document(), PageDocument::HttpError);
        for late in [
            event(
                "OTHER",
                "Target.detachedFromTarget",
                json!({ "sessionId": "OTHER" }),
            ),
            event(
                "OTHER",
                "Page.frameNavigated",
                navigated("https://elsewhere.example/", "L9"),
            ),
            event(
                "PAGE",
                "Page.frameNavigated",
                json!({ "frame": { "id": "NESTED", "parentId": "GAME-FRAME", "url": "about:blank" } }),
            ),
        ] {
            evidence.late(&late, GAME_IDS);
        }
        assert_eq!(evidence.document(), PageDocument::HttpError);
    }

    #[test]
    fn a_closed_connection_a_timeout_and_other_errors_end_a_run_apart() {
        assert_eq!(
            page_end(Some(&CdpError::Timeout), false),
            Some(PageEnd::TimedOut)
        );
        // The connection closing wins over whatever error it gave the run, and over a run that
        // ended with `done` because a watch took the closed stream for a quiet one.
        for error in [
            None,
            Some(CdpError::Gone("the browser".to_owned())),
            Some(CdpError::Timeout),
            Some(CdpError::Reply("no result".to_owned())),
        ] {
            assert_eq!(
                page_end(error.as_ref(), true),
                Some(PageEnd::ConnectionClosed)
            );
        }
        assert_eq!(
            page_end(
                Some(&CdpError::Gone("a crashed renderer".to_owned())),
                false
            ),
            Some(PageEnd::Failed)
        );
        assert_eq!(page_end(None, false), None);
    }

    #[test]
    fn the_browser_is_named_by_its_version() {
        assert_eq!(
            browser_name("HeadlessChrome/152.0.7977.82"),
            "Chromium 152.0.7977.82"
        );
    }
}
