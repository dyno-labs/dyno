//! One load-and-idle run in a browser that dyno serves to.
//!
//! The order: serve, and wait for a browser to open the harness page. Its `hello` starts the
//! driver's clock, and the page loads the game. The frame is idle when its document has loaded
//! the probe, no game request is in flight and none started for `idle_after_ms`. Then the run asks
//! the page for the self-check, makes the game active, watches it for a short while, asks the
//! page to send what it still holds, and ends with `done`. A failure after `hello` ends the run
//! with `error`.
//!
//! Two clocks meet, as in the CDP driver. The driver's clock is this machine's, from the moment
//! dyno answered `hello`. Probe events keep the game frame's own clock. The page and the frame
//! share the browser's wall clock, so the frame's clock start is placed on the driver's clock by
//! the browser's own times, never by this machine's.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dyno_schema::probe::{MessageDirection, ProbeEvent};
use dyno_schema::result::DriverInfo;
use dyno_schema::runner::{DriverMessage, RUNNER_PROTOCOL_VERSION, RunMark, RunRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::files::{Build, DYNO_PATH, PROBE_PATH, Pages, build_response};
use crate::harness::{EVENTS_PATH, HELLO_PATH, NEXT_PATH, page};
use crate::http::{Response, accept, read_request, write_response};
use crate::{DRIVER_NAME, RunTimes, ServeDriver, ServeError, browser_name};

/// How often the run checks whether the frame is idle.
const TICK: Duration = Duration::from_millis(100);

/// How long the run waits for the self-check's answer.
const SELF_CHECK_WAIT: Duration = Duration::from_secs(15);

/// How long the run watches the game after it makes it active.
const ACTIVE_WATCH: Duration = Duration::from_secs(2);

/// How long one poll for the next command waits before it answers `wait`.
const POLL_WAIT: Duration = Duration::from_secs(20);

/// How long the run waits for the page to send what it still holds. The page sends its events
/// every 250 ms, but a browser can slow that timer in a page it does not show.
const FLUSH_WAIT: Duration = Duration::from_secs(5);

/// How long the run waits for the page to take the `done` command.
const DONE_WAIT: Duration = Duration::from_secs(3);

/// What the servers saw, in the order they saw it.
#[derive(Debug)]
enum Seen {
    Hello {
        user_agent: String,
        at: Instant,
    },
    Events {
        events: Vec<PageEvent>,
        at: Instant,
    },
    RequestStarted {
        at: Instant,
    },
    RequestFinished {
        url: String,
        mime_type: Option<String>,
        bytes: u64,
        at: Instant,
    },
}

/// One thing the harness page relays.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PageEvent {
    /// When the page set the frame's URL, on the browser's wall clock.
    NavigatedAt { epoch_ms: f64 },
    /// A new document in the game frame: when its clock started, on the browser's wall clock.
    Document {
        time_origin: f64,
        device_pixel_ratio: f64,
    },
    /// One probe event, as the probe wrote it.
    Probe { payload: String },
    /// A message the game sent its host, on the browser's wall clock.
    Host { name: String, epoch_ms: f64 },
    /// The self-check's answer from the page.
    SelfCheck { webgl_renderer: Option<String> },
    /// The page sent every event it held when the run asked.
    Flushed,
}

/// What the harness page does next.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum Command {
    /// Nothing yet: poll again.
    Wait,
    SelfCheck,
    /// Post `{type: name, value: true}` to the game frame.
    Send {
        name: String,
    },
    /// Send every event the page holds, then `flushed`.
    Flush,
    Done,
}

#[derive(Deserialize)]
struct Hello {
    user_agent: String,
}

/// What the servers share with the run.
struct Shared {
    seen: mpsc::UnboundedSender<Seen>,
    commands: Mutex<VecDeque<Command>>,
    command_ready: Notify,
    /// Whether a page has said hello in this run.
    greeted: AtomicBool,
    page: String,
    build: Build,
    /// The game server's origin, which every game request URL starts with.
    game_origin: String,
}

impl Shared {
    fn command(&self, command: Command) {
        self.queue().push_back(command);
        self.command_ready.notify_one();
    }

    fn queue(&self) -> std::sync::MutexGuard<'_, VecDeque<Command>> {
        self.commands.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The game frame's latest document.
#[derive(Clone, Copy, Debug)]
struct Document {
    time_origin: f64,
    device_pixel_ratio: f64,
}

/// Everything the run tracks while the servers report.
struct Run {
    /// When dyno answered `hello`: the start of the driver's clock.
    started: Instant,
    /// The same moment on this machine's wall clock, in ms since the Unix epoch.
    started_epoch_ms: f64,
    /// When the frame went idle.
    idle_at: Option<Instant>,
    activated_at: Option<Instant>,
    messages: Vec<DriverMessage>,
    in_flight: u32,
    last_started: Instant,
    navigated_at_epoch_ms: Option<f64>,
    document: Option<Document>,
    /// Messages to the host, kept until the frame's clock is known.
    to_host: Vec<(String, f64)>,
    frame_time_origin: Option<f64>,
    /// The self-check's answer, once it came.
    webgl_renderer: Option<Option<String>>,
    /// How many times the page said it sent everything it held.
    flushes: u32,
}

/// Aborts the server tasks when the run ends.
struct Servers(Vec<JoinHandle<()>>);

impl Drop for Servers {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// What one served run gave: every message, when its steps happened, and the user agent of the
/// browser that opened the page.
pub(crate) struct Served {
    pub messages: Vec<DriverMessage>,
    pub times: RunTimes,
    pub user_agent: String,
}

/// Runs the request and returns every message. An error before `hello` is returned as an error;
/// after it, the run ends with an `error` message.
pub(crate) async fn run(driver: &ServeDriver, request: &RunRequest) -> Result<Served, ServeError> {
    let harness = tokio_listener(&driver.harness)?;
    let game = tokio_listener(&driver.game)?;
    let (seen_sender, mut seen) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        seen: seen_sender,
        commands: Mutex::new(VecDeque::new()),
        command_ready: Notify::new(),
        greeted: AtomicBool::new(false),
        page: page(&request.launch_url),
        build: driver.build.clone(),
        game_origin: driver.game_url.trim_end_matches('/').to_owned(),
    });
    let _servers = Servers(vec![
        tokio::spawn(accept(harness, Arc::clone(&shared), serve_harness)),
        tokio::spawn(accept(game, Arc::clone(&shared), serve_game)),
    ]);

    let (user_agent, started) = wait_for_hello(&mut seen, driver.wait_for_browser).await?;
    let since_hello = Instant::now().saturating_duration_since(started);
    let started_epoch_ms = epoch_ms_now() - since_hello.as_secs_f64() * 1_000.0;
    let mut run = Run {
        started,
        started_epoch_ms,
        idle_at: None,
        activated_at: None,
        messages: vec![DriverMessage::Hello {
            protocol_version: RUNNER_PROTOCOL_VERSION,
            driver: DriverInfo {
                name: DRIVER_NAME.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                browser: browser_name(&user_agent),
            },
        }],
        in_flight: 0,
        last_started: started,
        navigated_at_epoch_ms: None,
        document: None,
        to_host: Vec::new(),
        frame_time_origin: None,
        webgl_renderer: None,
        flushes: 0,
    };
    run.mark(RunMark::Navigated, started);
    if let Err(message) = run_in(&mut run, &mut seen, &shared, request).await {
        let t_ms = run.t_ms(Instant::now());
        run.messages.push(DriverMessage::Error { message, t_ms });
    }
    let ended_epoch_ms = run.started_epoch_ms + run.t_ms(Instant::now());
    end_page(&shared).await;
    let times = RunTimes {
        navigated_epoch_ms: run.started_epoch_ms,
        idle_epoch_ms: run
            .idle_at
            .map(|idle| run.started_epoch_ms + run.t_ms(idle)),
        activated_epoch_ms: run
            .activated_at
            .map(|activated| run.started_epoch_ms + run.t_ms(activated)),
        ended_epoch_ms,
    };
    Ok(Served {
        messages: run.messages,
        times,
        user_agent,
    })
}

/// Load, idle, self-check, activate and watch. An error is the run's error message.
async fn run_in(
    run: &mut Run,
    seen: &mut mpsc::UnboundedReceiver<Seen>,
    shared: &Shared,
    request: &RunRequest,
) -> Result<(), String> {
    let deadline = run.started + Duration::from_millis(u64::from(request.timeout_ms));
    let idle_after = Duration::from_millis(u64::from(request.idle_after_ms));
    if !take_until(run, seen, deadline, |run| run.is_idle(idle_after)).await {
        return Err("the run timed out before the game frame was idle".to_owned());
    }
    run.mark(RunMark::Idle, Instant::now());

    shared.command(Command::SelfCheck);
    let answer_by = deadline.min(Instant::now() + SELF_CHECK_WAIT);
    if !take_until(run, seen, answer_by, |run| run.webgl_renderer.is_some()).await {
        return Err("the harness page did not answer the self-check".to_owned());
    }
    run.self_check(Instant::now());

    shared.command(Command::Send {
        name: request.host_profile.active_message.clone(),
    });
    run.mark(RunMark::Activated, Instant::now());
    take_until(run, seen, Instant::now() + ACTIVE_WATCH, |_| false).await;
    // What the game did in the watch reaches dyno only when the page sends it.
    let flushes = run.flushes;
    shared.command(Command::Flush);
    if !take_until(run, seen, Instant::now() + FLUSH_WAIT, |run| {
        run.flushes > flushes
    })
    .await
    {
        return Err("the harness page did not send the events it held".to_owned());
    }
    let t_ms = run.t_ms(Instant::now());
    run.messages.push(DriverMessage::Done { t_ms });
    Ok(())
}

/// Tells the page the run is over, so it drops the game, and gives it a moment to hear it.
async fn end_page(shared: &Shared) {
    shared.command(Command::Done);
    let until = Instant::now() + DONE_WAIT;
    while !shared.queue().is_empty() && Instant::now() < until {
        tokio::time::sleep(TICK).await;
    }
    // The poll that took `done` still has to write its answer.
    tokio::time::sleep(TICK).await;
}

/// Waits for the first `hello`. Anything the servers saw before it is not part of the run.
async fn wait_for_hello(
    seen: &mut mpsc::UnboundedReceiver<Seen>,
    wait: Duration,
) -> Result<(String, Instant), ServeError> {
    let hello = async {
        while let Some(item) = seen.recv().await {
            if let Seen::Hello { user_agent, at } = item {
                return Some((user_agent, at));
            }
        }
        None
    };
    match tokio::time::timeout(wait, hello).await {
        Ok(Some(hello)) => Ok(hello),
        _ => Err(ServeError::NoBrowser(wait.as_secs())),
    }
}

/// Takes what the servers report until `done` says so (true) or `until` passes (false).
async fn take_until(
    run: &mut Run,
    seen: &mut mpsc::UnboundedReceiver<Seen>,
    until: Instant,
    done: impl Fn(&Run) -> bool,
) -> bool {
    loop {
        if done(run) {
            return true;
        }
        let now = Instant::now();
        if now >= until {
            return false;
        }
        let tick = until.min(now + TICK);
        tokio::select! {
            item = seen.recv() => match item {
                Some(item) => run.take(item),
                // The servers hold a sender for as long as the run, so this does not happen.
                None => return false,
            },
            () = tokio::time::sleep_until(tick) => {}
        }
    }
}

impl Run {
    /// A time on the driver's clock.
    fn t_ms(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.started).as_secs_f64() * 1_000.0
    }

    fn mark(&mut self, mark: RunMark, at: Instant) {
        match mark {
            RunMark::Idle => self.idle_at = Some(at),
            RunMark::Activated => self.activated_at = Some(at),
            _ => {}
        }
        let t_ms = self.t_ms(at);
        self.messages.push(DriverMessage::Mark { mark, t_ms });
    }

    /// The frame is idle once a document with the probe has loaded, no game request is in flight,
    /// and none started for `idle_after`.
    fn is_idle(&self, idle_after: Duration) -> bool {
        self.document.is_some()
            && self.in_flight == 0
            && Instant::now().saturating_duration_since(self.last_started) >= idle_after
    }

    fn take(&mut self, item: Seen) {
        match item {
            // A page that says hello again is turned away by the server.
            Seen::Hello { .. } => {}
            Seen::RequestStarted { at } => {
                self.in_flight += 1;
                self.last_started = at;
            }
            Seen::RequestFinished {
                url,
                mime_type,
                bytes,
                at,
            } => {
                self.in_flight = self.in_flight.saturating_sub(1);
                let t_ms = self.t_ms(at);
                self.messages.push(DriverMessage::Request {
                    url,
                    mime_type,
                    bytes,
                    t_ms,
                });
            }
            Seen::Events { events, at } => {
                for event in events {
                    self.take_event(event, at);
                }
            }
        }
    }

    fn take_event(&mut self, event: PageEvent, at: Instant) {
        match event {
            PageEvent::NavigatedAt { epoch_ms } => self.navigated_at_epoch_ms = Some(epoch_ms),
            PageEvent::Document {
                time_origin,
                device_pixel_ratio,
            } => {
                self.document = Some(Document {
                    time_origin,
                    device_pixel_ratio,
                });
                self.mark(RunMark::FrameDocument, at);
            }
            PageEvent::Probe { payload } => {
                // A payload that is not an event is dropped, as in the CDP driver.
                if let Ok(event) = serde_json::from_str::<ProbeEvent>(&payload) {
                    self.messages.push(DriverMessage::Probe { event });
                }
            }
            PageEvent::Host { name, epoch_ms } => {
                self.to_host.push((name, epoch_ms));
                self.send_host_messages();
            }
            PageEvent::SelfCheck { webgl_renderer } => self.webgl_renderer = Some(webgl_renderer),
            PageEvent::Flushed => self.flushes += 1,
        }
    }

    /// The self-check: the frame's own pixel ratio, the page's WebGL renderer, and where the
    /// frame's clock starts on the driver's clock.
    fn self_check(&mut self, at: Instant) {
        let Some(document) = self.document else {
            return;
        };
        let t_ms = self.t_ms(at);
        self.messages.push(DriverMessage::SelfCheck {
            device_pixel_ratio: document.device_pixel_ratio,
            webgl_renderer: self.webgl_renderer.clone().flatten(),
            frame_clock_at_ms: self
                .navigated_at_epoch_ms
                .map(|navigated| document.time_origin - navigated),
            t_ms,
        });
        self.frame_time_origin = Some(document.time_origin);
        self.send_host_messages();
    }

    /// Each message to the host as a probe event on the frame's clock, once that clock is known.
    fn send_host_messages(&mut self) {
        let Some(time_origin) = self.frame_time_origin else {
            return;
        };
        for (name, epoch_ms) in self.to_host.drain(..) {
            self.messages.push(DriverMessage::Probe {
                event: ProbeEvent::HostMessage {
                    direction: MessageDirection::ToHost,
                    name,
                    t_ms: epoch_ms - time_origin,
                },
            });
        }
    }
}

/// This machine's wall clock, in ms since the Unix epoch.
fn epoch_ms_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1_000.0)
}

fn tokio_listener(listener: &std::net::TcpListener) -> Result<TcpListener, ServeError> {
    let bind_error = |error: std::io::Error| ServeError::Bind {
        address: listener
            .local_addr()
            .map_or_else(|_| "?".to_owned(), |address| address.to_string()),
        detail: error.to_string(),
    };
    TcpListener::from_std(listener.try_clone().map_err(bind_error)?).map_err(bind_error)
}

async fn serve_harness(shared: Arc<Shared>, mut socket: TcpStream) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => Response::new(
            "200 OK",
            "text/html; charset=utf-8",
            shared.page.clone().into_bytes(),
        ),
        ("POST", HELLO_PATH) => hello(&shared, &request.body),
        ("POST", EVENTS_PATH) => events(&shared, &request.body),
        ("GET", NEXT_PATH) => next_command(&shared).await,
        _ => Response::error("404 Not Found"),
    };
    write_response(&mut socket, &response, false).await;
}

/// The first `hello` starts the run. A later one is turned away.
fn hello(shared: &Shared, body: &[u8]) -> Response {
    let Ok(hello) = serde_json::from_slice::<Hello>(body) else {
        return Response::error("400 Bad Request");
    };
    if shared.greeted.swap(true, Ordering::SeqCst) {
        return Response::error("409 Conflict");
    }
    let _ = shared.seen.send(Seen::Hello {
        user_agent: hello.user_agent,
        at: Instant::now(),
    });
    json_response(&serde_json::json!({}))
}

/// A batch of events. An event this version does not know is dropped.
fn events(shared: &Shared, body: &[u8]) -> Response {
    let Ok(batch) = serde_json::from_slice::<Vec<Value>>(body) else {
        return Response::error("400 Bad Request");
    };
    let events = batch
        .into_iter()
        .filter_map(|event| serde_json::from_value::<PageEvent>(event).ok())
        .collect();
    let _ = shared.seen.send(Seen::Events {
        events,
        at: Instant::now(),
    });
    json_response(&serde_json::json!({}))
}

/// The next command, as soon as there is one, or `wait` after a while.
async fn next_command(shared: &Shared) -> Response {
    let until = Instant::now() + POLL_WAIT;
    loop {
        if let Some(command) = shared.queue().pop_front() {
            return json_response(&command);
        }
        if tokio::time::timeout_at(until, shared.command_ready.notified())
            .await
            .is_err()
        {
            return json_response(&Command::Wait);
        }
    }
}

fn json_response(value: &impl Serialize) -> Response {
    let body = serde_json::to_vec(value).unwrap_or_default();
    Response::new("200 OK", "application/json", body)
}

async fn serve_game(shared: Arc<Shared>, mut socket: TcpStream) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    let head_only = request.method == "HEAD";
    if request.path.starts_with(DYNO_PATH) {
        // dyno's own files are not game requests.
        let readable = head_only || request.method == "GET";
        let response = if readable && request.path == PROBE_PATH {
            Response::new(
                "200 OK",
                "text/javascript; charset=utf-8",
                dyno_cdp::PROBE.as_bytes().to_vec(),
            )
        } else {
            Response::error("404 Not Found")
        };
        write_response(&mut socket, &response, head_only).await;
        return;
    }
    let _ = shared
        .seen
        .send(Seen::RequestStarted { at: Instant::now() });
    let response = build_response(&shared.build, &request, Pages::WithProbe);
    let bytes = write_response(&mut socket, &response, head_only).await;
    let mime_type = response
        .content_type
        .split(';')
        .next()
        .map(|mime| mime.trim().to_owned());
    let _ = shared.seen.send(Seen::RequestFinished {
        url: format!("{}{}", shared.game_origin, request.path),
        mime_type,
        bytes,
        at: Instant::now(),
    });
}
