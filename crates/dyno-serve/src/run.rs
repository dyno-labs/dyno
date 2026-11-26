//! One load-and-idle run in a browser that dyno serves to.
//!
//! The order: serve, ask for the page to be opened in the browser when someone can, and wait for
//! a browser to open the harness page. Its `hello` starts the driver's clock, and the page loads
//! the game. The frame is idle when its document has loaded
//! the probe, no game request is in flight and none started for `idle_after_ms`. Then the run asks
//! the page for the self-check, makes the game active, watches it for a short while, asks the
//! page to send what it still holds, and ends with `done`. A failure after `hello` ends the run
//! with `error`.
//!
//! The run keeps what it saw of the page for the run's record ([`PageSeen`]), and nothing of that
//! changes its messages. It cannot see the game frame's document, only whether a probe's document
//! message reached the servers in the run. A run that the deadline cut short timed out. Of any
//! other that fails after `hello`, it says what it saw: a command that the harness page did not
//! take in the whole time dyno waited, or no request for [`LINK_SILENCE`], longer than the page's
//! poll for its next command waits ([`POLL_WAIT`]). Only requests with the run's key, at the
//! address that took the `hello`, count. A second tab opened at the run's URL has the key, so it
//! can take a command and make requests too.
//!
//! Two clocks meet, as in the CDP driver. The driver's clock is this machine's, from the moment
//! dyno answered `hello`. Probe events keep the game frame's own clock. The page and the frame
//! share the browser's wall clock, so the frame's clock start is placed on the driver's clock by
//! the browser's own times, never by this machine's.
//!
//! Only the page that dyno asked for drives the run. Every request to the harness server must
//! carry the run's key ([`crate::key`]), or it is refused and changes nothing, not even the note
//! that a browser came to that address. The game server serves the page's browser alone: from
//! the `hello` on, it answers the address that the hello came from, and refuses every other, so
//! no other device can add game requests to the run. Before the hello, there is no frame, and it
//! answers no one.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dyno_run::PageSeen;
use dyno_schema::probe::{MessageDirection, ProbeEvent};
use dyno_schema::result::DriverInfo;
use dyno_schema::run_record::{PageEnd, ProbeHeard};
use dyno_schema::runner::{DriverMessage, RUNNER_PROTOCOL_VERSION, RunMark, RunRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::files::{Build, PROBE_PATH, Pages, build_response};
use crate::harness::{EVENTS_PATH, HELLO_PATH, NEXT_PATH, REFERRER_POLICY, page};
use crate::http::{Response, accept, read_body, read_head, refuse, write_response};
use crate::key::{REFUSED_HEADER, RunKey};
use crate::{DRIVER_NAME, Place, RunTimes, ServeDriver, ServeError, browser_name};

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

/// How long the harness page may make no request before a run that fails counts it as gone. A
/// page that still runs asks for its next command again as soon as an ask is answered, and dyno
/// answers each ask within [`POLL_WAIT`]; the rest is room for the network.
const LINK_SILENCE: Duration = Duration::from_secs(25);

/// What the servers saw, in the order they saw it.
#[derive(Debug)]
enum Seen {
    /// A request to the harness server at this place, of any kind: a browser reached it there.
    HarnessRequest {
        place: usize,
        at: Instant,
    },
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
    build: Build,
    /// The place whose harness server took the page's `hello`: its index in the driver's
    /// places. The page that said it is the page that runs.
    hello_at: Mutex<Option<usize>>,
    /// The run's key, which every request to the harness server must carry.
    key: RunKey,
    /// The address the page's `hello` came from: the only one the game server answers.
    page_address: Mutex<Option<IpAddr>>,
}

/// What the two servers at one place share: the run, and what differs between places.
struct At {
    shared: Arc<Shared>,
    /// This place's index in the driver's places.
    place: usize,
    /// The harness page, which loads the game from this place's game server.
    page: String,
    /// This place's game server's origin, which every game request URL there starts with.
    game_origin: String,
}

/// How a run asks for the harness page to be opened in the browser, and hears what became of it.
pub(crate) struct Asks {
    /// The URL to open. The run asks once.
    pub ask: mpsc::UnboundedSender<String>,
    /// What became of the ask. An error is why the run must stop.
    pub answers: mpsc::UnboundedReceiver<Result<(), String>>,
}

/// What a run says once when no request has come to the harness page at its first address
/// `after` it began to wait: `say` is called.
pub(crate) struct NoRequest<'s> {
    pub after: Duration,
    pub say: &'s mut dyn FnMut(),
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
    /// When the harness page that said hello last made a request to dyno, of any kind.
    last_contact: Instant,
    /// The place that took the `hello`. Only requests there are the harness page's.
    hello_place: Option<usize>,
    /// Whether a probe's document message reached dyno in the run.
    probe_heard: bool,
}

/// Why a run that got its `hello` ended before `done`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// The deadline passed before the game frame was idle.
    NeverIdle,
    /// The page did not answer the self-check. `full_wait` says whether dyno waited the whole
    /// [`SELF_CHECK_WAIT`], or the deadline cut it short.
    NoSelfCheck { full_wait: bool },
    /// The page did not send the events it held, in the whole [`FLUSH_WAIT`].
    NoFlush,
}

impl Failure {
    /// The run's `error` message.
    fn message(self) -> &'static str {
        match self {
            Failure::NeverIdle => "the run timed out before the game frame was idle",
            Failure::NoSelfCheck { .. } => "the harness page did not answer the self-check",
            Failure::NoFlush => "the harness page did not send the events it held",
        }
    }

    /// Whether the page had the whole of its step's wait to take the command.
    fn waited_in_full(self) -> bool {
        match self {
            Failure::NeverIdle => false,
            Failure::NoSelfCheck { full_wait } => full_wait,
            Failure::NoFlush => true,
        }
    }
}

/// How a run that failed after `hello` ended, in this order:
///
/// 1. A step that the run's deadline cut short timed out, whatever the silence says: the page
///    may simply not have asked yet.
/// 2. A command that dyno gave the page stayed untaken for the whole of the step's wait.
/// 3. No request came from the address that took the `hello` for `silent`, longer than
///    [`LINK_SILENCE`].
/// 4. A page that never went idle met the deadline: the wait for idle has no length of its own,
///    so the deadline is its end, not a cut.
/// 5. One that took a command and did not answer it failed.
fn page_end(failure: Failure, command_untaken: bool, silent: Duration) -> PageEnd {
    if failure == (Failure::NoSelfCheck { full_wait: false }) {
        return PageEnd::TimedOut;
    }
    if command_untaken && failure.waited_in_full() {
        return PageEnd::CommandNotTaken;
    }
    if silent > LINK_SILENCE {
        return PageEnd::NoRequest;
    }
    match failure {
        Failure::NeverIdle | Failure::NoSelfCheck { full_wait: false } => PageEnd::TimedOut,
        Failure::NoSelfCheck { full_wait: true } | Failure::NoFlush => PageEnd::Failed,
    }
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
    /// What the run saw of the page.
    pub seen: PageSeen,
}

/// What a run gave, and the place whose harness server took the page's `hello`: its index in
/// the driver's places. None when no page said hello.
pub(crate) struct Outcome {
    pub served: Result<Served, ServeError>,
    pub hello_at: Option<usize>,
}

/// Runs the request and returns every message. An error before `hello` is returned as an error;
/// after it, the run ends with an `error` message. With `asks`, the run asks for the harness
/// page to be opened once its servers serve.
pub(crate) async fn run(
    driver: &ServeDriver,
    request: &RunRequest,
    asks: Option<Asks>,
    no_request: Option<NoRequest<'_>>,
) -> Outcome {
    let (seen_sender, seen) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        seen: seen_sender,
        commands: Mutex::new(VecDeque::new()),
        command_ready: Notify::new(),
        greeted: AtomicBool::new(false),
        build: driver.build.clone(),
        hello_at: Mutex::new(None),
        key: driver.key.clone(),
        page_address: Mutex::new(None),
    });
    let served = run_at(driver, request, asks, no_request, &shared, seen).await;
    let hello_at = *shared
        .hello_at
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    Outcome { served, hello_at }
}

/// The launch URL at a place: the request's, which names the first place's game server, with
/// this place's game server in its stead.
fn launch_url_at(launch_url: &str, first: &Place, place: &Place) -> String {
    match launch_url.strip_prefix(first.game_url.as_str()) {
        Some(rest) => format!("{}{rest}", place.game_url),
        None => launch_url.to_owned(),
    }
}

async fn run_at(
    driver: &ServeDriver,
    request: &RunRequest,
    asks: Option<Asks>,
    no_request: Option<NoRequest<'_>>,
    shared: &Arc<Shared>,
    mut seen: mpsc::UnboundedReceiver<Seen>,
) -> Result<Served, ServeError> {
    // Every place serves before anyone is asked to open the page, so the browser's first
    // request is answered at once.
    let mut servers = Servers(Vec::new());
    for (index, place) in driver.places.iter().enumerate() {
        let harness = tokio_listener(&place.harness)?;
        let game = tokio_listener(&place.game)?;
        let at = Arc::new(At {
            shared: Arc::clone(shared),
            place: index,
            page: page(
                &launch_url_at(&request.launch_url, driver.first(), place),
                &shared.key,
            ),
            game_origin: place.game_url.trim_end_matches('/').to_owned(),
        });
        servers.0.push(tokio::spawn(accept(
            harness,
            Arc::clone(&at),
            serve_harness,
        )));
        servers.0.push(tokio::spawn(accept(game, at, serve_game)));
    }

    let (user_agent, started) = wait_for_hello(&mut seen, driver, asks, no_request).await?;
    let hello_place = *shared
        .hello_at
        .lock()
        .unwrap_or_else(PoisonError::into_inner);

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
        last_contact: started,
        hello_place,
        probe_heard: false,
    };
    run.mark(RunMark::Navigated, started);
    let mut failed = None;
    if let Err(failure) = run_in(&mut run, &mut seen, shared, request).await {
        let at = Instant::now();
        let t_ms = run.t_ms(at);
        run.messages.push(DriverMessage::Error {
            message: failure.message().to_owned(),
            t_ms,
        });
        failed = Some(Failed { failure, at });
    }
    let command_untaken = || !shared.queue().is_empty();
    let closed = close(
        &mut run,
        &mut seen,
        failed,
        |_| Instant::now(),
        command_untaken,
    );
    let ended_epoch_ms = run.started_epoch_ms + run.t_ms(closed.ended_at);
    end_page(shared).await;
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
    drop(servers);
    Ok(Served {
        messages: run.messages,
        times,
        user_agent,
        seen: PageSeen {
            // The run hears the probe; it cannot see which document the frame holds.
            document: None,
            end: closed.end,
            probe: Some(closed.probe),
            hello_before_error: false,
        },
    })
}

/// A run that failed after `hello`: the step it failed, and when it failed.
#[derive(Clone, Copy, Debug)]
struct Failed {
    failure: Failure,
    at: Instant,
}

/// The end of a run: when it ended, on `now`, and what it saw of the page.
#[derive(Debug, PartialEq)]
struct Closed {
    ended_at: Instant,
    end: Option<PageEnd>,
    probe: ProbeHeard,
}

/// Ends the run. Its end time is taken first, at the point it always was, so that what the
/// record reads next moves no cutoff of a measure. Then the record reads what the servers
/// received in the run that the run did not take from the queue: a document message, and a
/// request at the address that took the `hello` before the run failed. That goes nowhere else.
/// `now` is the clock, which tests move while the record reads. `command_untaken` says whether a
/// command that dyno gave the page is still untaken: the record asks it of a run that failed,
/// after the end time is taken.
fn close(
    run: &mut Run,
    seen: &mut mpsc::UnboundedReceiver<Seen>,
    failed: Option<Failed>,
    now: impl Fn(&Run) -> Instant,
    command_untaken: impl FnOnce() -> bool,
) -> Closed {
    let ended_at = now(run);
    let command_untaken = failed.is_some() && command_untaken();
    let contact_by = failed.map_or(ended_at, |failed| failed.at);
    while let Ok(item) = seen.try_recv() {
        run.note_unread(&item, ended_at, contact_by);
    }
    let end = failed.map(|failed| {
        page_end(
            failed.failure,
            command_untaken,
            failed.at.saturating_duration_since(run.last_contact),
        )
    });
    let probe = match run.probe_heard {
        true => ProbeHeard::Heard,
        false => ProbeHeard::NotHeard,
    };
    Closed {
        ended_at,
        end,
        probe,
    }
}

/// Load, idle, self-check, activate and watch. An error says which step the page did not meet.
async fn run_in(
    run: &mut Run,
    seen: &mut mpsc::UnboundedReceiver<Seen>,
    shared: &Shared,
    request: &RunRequest,
) -> Result<(), Failure> {
    let deadline = run.started + Duration::from_millis(u64::from(request.timeout_ms));
    let idle_after = Duration::from_millis(u64::from(request.idle_after_ms));
    if !take_until(run, seen, deadline, |run| run.is_idle(idle_after)).await {
        return Err(Failure::NeverIdle);
    }
    run.mark(RunMark::Idle, Instant::now());

    shared.command(Command::SelfCheck);
    let full_wait_until = Instant::now() + SELF_CHECK_WAIT;
    let answer_by = deadline.min(full_wait_until);
    if !take_until(run, seen, answer_by, |run| run.webgl_renderer.is_some()).await {
        return Err(Failure::NoSelfCheck {
            full_wait: answer_by == full_wait_until,
        });
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
        return Err(Failure::NoFlush);
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
///
/// With `asks`, it asks once for the harness page to be opened at the first place. Nothing is
/// asked for again: a second ask would restart the browser under a page that may be loading.
/// With `no_request`, it says so once when no request has come to the first place's harness
/// server in time, and waits on. It says so only once the opener has answered, because until
/// then the opener can still be trying: a launch that a person's own opening of the page came
/// before would restart the browser under it.
async fn wait_for_hello(
    seen: &mut mpsc::UnboundedReceiver<Seen>,
    driver: &ServeDriver,
    mut asks: Option<Asks>,
    mut no_request: Option<NoRequest<'_>>,
) -> Result<(String, Instant), ServeError> {
    let no_browser = || ServeError::NoBrowser(driver.wait_for_browser.as_secs());
    let give_up_at = Instant::now() + driver.wait_for_browser;
    if let Some(asks) = &asks {
        let _ = asks.ask.send(driver.harness_url().to_owned());
    }
    // When it is time to say that no request has come to the first place. None once that time
    // has passed, and when there is nothing to say.
    let mut say_at = no_request
        .as_ref()
        .map(|no_request| Instant::now() + no_request.after);
    // Whether it is time to say so and nothing has said otherwise: the time has passed, and no
    // request has come to the first place.
    let mut due = false;
    // Whether a request has come to the first place, after which there is nothing to say.
    let mut requested = false;
    loop {
        // The opener has answered, or there is none.
        if due && asks.is_none() {
            due = false;
            if let Some(no_request) = no_request.take() {
                (no_request.say)();
            }
        }
        let answer = async {
            match &mut asks {
                Some(asks) => asks.answers.recv().await,
                None => std::future::pending().await,
            }
        };
        let time_to_say = async {
            match say_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            item = seen.recv() => match item {
                Some(Seen::Hello { user_agent, at }) => return Ok((user_agent, at)),
                Some(Seen::HarnessRequest { place: 0, .. }) => {
                    requested = true;
                    due = false;
                    say_at = None;
                }
                Some(_) => {}
                // The servers hold a sender for as long as the run, so this does not happen.
                None => return Err(no_browser()),
            },
            answer = answer => match answer {
                // The opener asks once, so this is its last answer.
                Some(Ok(())) | None => asks = None,
                Some(Err(reason)) => return Err(ServeError::Stopped(reason)),
            },
            () = time_to_say => {
                say_at = None;
                due = !requested;
            }
            () = tokio::time::sleep_until(give_up_at) => return Err(no_browser()),
        }
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

    /// Notes, for the record only, what the servers saw that the run did not take: a document
    /// message that they received by `ended_at`, and a request at the address that took the
    /// `hello` that they received by `contact_by`.
    fn note_unread(&mut self, item: &Seen, ended_at: Instant, contact_by: Instant) {
        match item {
            Seen::Events { events, at }
                if *at <= ended_at
                    && events
                        .iter()
                        .any(|event| matches!(event, PageEvent::Document { .. })) =>
            {
                self.probe_heard = true;
            }
            Seen::HarnessRequest { place, at } if *at <= contact_by => self.contact(*place, *at),
            _ => {}
        }
    }

    /// A request at `place`, received at `at`. Only one at the address that took the `hello` is
    /// the harness page's: a request at the other address of a run served at two is not.
    fn contact(&mut self, place: usize, at: Instant) {
        if Some(place) == self.hello_place {
            self.last_contact = self.last_contact.max(at);
        }
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
            // When the server received it, not when the run took it from the queue.
            Seen::HarnessRequest { place, at } => self.contact(place, at),
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
                    // For the record only: the event goes on as it always did.
                    if matches!(event, PageEvent::Document { .. }) {
                        self.probe_heard = true;
                    }
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

/// What a request to the harness server without the run's key gets: no route, and no change.
const NO_KEY: &str = "dyno: this run needs the URL that dyno printed for it.\n";

async fn serve_harness(at: Arc<At>, mut socket: TcpStream) {
    let Some(head) = read_head(&mut socket).await else {
        return;
    };
    let shared = &at.shared;
    let head_only = head.request.method == "HEAD";
    // The key is checked on the head, so a request without it gets its 403 whatever body it
    // sends, and dyno reads none of it.
    let keyed = head
        .request
        .key
        .as_deref()
        .is_some_and(|key| shared.key.matches(key));
    if !keyed {
        let mut refused = Response::shared(
            "403 Forbidden",
            "text/plain; charset=utf-8",
            NO_KEY.as_bytes(),
        );
        refused
            .headers
            .push((REFUSED_HEADER.0, REFUSED_HEADER.1.to_owned()));
        refuse(&mut socket, &refused, head_only).await;
        return;
    }
    let Some(request) = read_body(&mut socket, head).await else {
        return;
    };
    let _ = shared.seen.send(Seen::HarnessRequest {
        place: at.place,
        at: Instant::now(),
    });
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET" | "HEAD", "/") => {
            let mut page =
                Response::shared("200 OK", "text/html; charset=utf-8", at.page.as_bytes());
            page.headers
                .push((REFERRER_POLICY.0, REFERRER_POLICY.1.to_owned()));
            page
        }
        ("POST", HELLO_PATH) => hello(&at, &request.body, peer_address(&socket)),
        ("POST", EVENTS_PATH) => events(shared, &request.body),
        ("GET", NEXT_PATH) => next_command(shared).await,
        _ => Response::error("404 Not Found"),
    };
    write_response(&mut socket, &response, head_only).await;
}

/// The first `hello` starts the run, and its place is the path the run took. A later one is
/// turned away, and so is one that is not a hello: neither sets the path. The address it came
/// from is the one the game server answers from then on.
fn hello(at: &At, body: &[u8], from: Option<IpAddr>) -> Response<'static> {
    let shared = &at.shared;
    let Ok(hello) = serde_json::from_slice::<Hello>(body) else {
        return Response::error("400 Bad Request");
    };
    // A socket with no peer address has gone, and its page with it.
    let Some(from) = from else {
        return Response::error("400 Bad Request");
    };
    if shared.greeted.swap(true, Ordering::SeqCst) {
        return Response::error("409 Conflict");
    }
    *shared
        .page_address
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(from);
    *shared
        .hello_at
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(at.place);
    let _ = shared.seen.send(Seen::Hello {
        user_agent: hello.user_agent,
        at: Instant::now(),
    });
    json_response(&serde_json::json!({}))
}

/// A batch of events. An event this version does not know is dropped.
fn events(shared: &Shared, body: &[u8]) -> Response<'static> {
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
async fn next_command(shared: &Shared) -> Response<'static> {
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

fn json_response(value: &impl Serialize) -> Response<'static> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    Response::new("200 OK", "application/json", body)
}

/// The address a connection comes from, as one form for IPv4 and IPv6: an IPv4 client of an
/// IPv6 socket is its IPv4 address.
fn peer_address(socket: &TcpStream) -> Option<IpAddr> {
    socket
        .peer_addr()
        .ok()
        .map(|address| address.ip().to_canonical())
}

async fn serve_game(at: Arc<At>, mut socket: TcpStream) {
    let Some(head) = read_head(&mut socket).await else {
        return;
    };
    let shared = &at.shared;
    let head_only = head.request.method == "HEAD";
    let page_address = *shared
        .page_address
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // The game's requests come from the browser whose page said hello. One from anywhere else,
    // or from before the hello, is no request of this run: it is refused, and not counted.
    if page_address.is_none() || peer_address(&socket) != page_address {
        refuse(&mut socket, &Response::error("403 Forbidden"), head_only).await;
        return;
    }
    let Some(request) = read_body(&mut socket, head).await else {
        return;
    };
    // The probe's path is the one path here that is dyno's. Every other path is the build's, and
    // a build with a file of its own at the probe's path does not get this far.
    if request.path == PROBE_PATH {
        // dyno's own file is not a game request.
        let readable = head_only || request.method == "GET";
        let response = if readable {
            Response::shared(
                "200 OK",
                "text/javascript; charset=utf-8",
                dyno_cdp::PROBE.as_bytes(),
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
        url: format!("{}{}", at.game_origin, request.path),
        mime_type,
        bytes,
        at: Instant::now(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const A_MOMENT: Duration = Duration::from_secs(1);
    const FULL: Failure = Failure::NoSelfCheck { full_wait: true };
    const CUT_SHORT: Failure = Failure::NoSelfCheck { full_wait: false };

    #[test]
    fn a_page_that_still_asks_and_never_went_idle_met_the_deadline() {
        assert_eq!(
            page_end(Failure::NeverIdle, false, A_MOMENT),
            PageEnd::TimedOut
        );
    }

    #[test]
    fn a_command_untaken_for_the_whole_wait_is_said_as_that() {
        for failure in [FULL, Failure::NoFlush] {
            assert_eq!(page_end(failure, true, A_MOMENT), PageEnd::CommandNotTaken);
        }
    }

    #[test]
    fn a_step_that_the_deadline_cut_short_timed_out_whatever_the_silence_says() {
        // The frame went idle just before the deadline: the page may simply not have asked yet.
        let silent = LINK_SILENCE + A_MOMENT;
        for (untaken, silence) in [
            (true, A_MOMENT),
            (false, A_MOMENT),
            (true, silent),
            (false, silent),
        ] {
            assert_eq!(page_end(CUT_SHORT, untaken, silence), PageEnd::TimedOut);
        }
    }

    #[test]
    fn no_request_for_longer_than_the_poll_is_said_as_that() {
        let silent = LINK_SILENCE + A_MOMENT;
        assert_eq!(
            page_end(Failure::NeverIdle, false, silent),
            PageEnd::NoRequest
        );
        // Silence of up to the limit is a page that may still be there.
        assert_eq!(
            page_end(Failure::NeverIdle, false, LINK_SILENCE),
            PageEnd::TimedOut
        );
        assert!(LINK_SILENCE > POLL_WAIT);
    }

    #[test]
    fn a_page_that_took_a_command_and_did_not_answer_failed() {
        assert_eq!(page_end(FULL, false, A_MOMENT), PageEnd::Failed);
        assert_eq!(page_end(Failure::NoFlush, false, A_MOMENT), PageEnd::Failed);
    }

    const HELLO_PLACE: usize = 0;

    /// A run that a page said hello in, at place 0, a moment ago.
    fn run() -> Run {
        let started = Instant::now();
        Run {
            started,
            started_epoch_ms: 0.0,
            idle_at: None,
            activated_at: None,
            messages: Vec::new(),
            in_flight: 0,
            last_started: started,
            navigated_at_epoch_ms: None,
            document: None,
            to_host: Vec::new(),
            frame_time_origin: None,
            webgl_renderer: None,
            flushes: 0,
            last_contact: started,
            hello_place: Some(HELLO_PLACE),
            probe_heard: false,
        }
    }

    #[test]
    fn a_probe_is_heard_once_a_document_message_reaches_the_run() {
        let mut run = run();
        run.take(Seen::Events {
            events: vec![PageEvent::Flushed],
            at: Instant::now(),
        });
        assert!(!run.probe_heard);
        run.take(Seen::Events {
            events: vec![PageEvent::Document {
                time_origin: 1_050.0,
                device_pixel_ratio: 3.0,
            }],
            at: Instant::now(),
        });
        assert!(run.probe_heard);
        // The run takes the document as it always did.
        assert!(run.document.is_some());
    }

    fn document() -> PageEvent {
        PageEvent::Document {
            time_origin: 1_050.0,
            device_pixel_ratio: 3.0,
        }
    }

    /// A queue that holds these, as the servers left it when the run ended.
    fn queued(items: Vec<Seen>) -> mpsc::UnboundedReceiver<Seen> {
        let (sender, queue) = mpsc::unbounded_channel();
        for item in items {
            sender.send(item).unwrap();
        }
        queue
    }

    #[test]
    fn a_document_batch_that_the_servers_took_before_the_end_is_heard_and_goes_no_further() {
        let mut run = run();
        let end = run.started + Duration::from_secs(13);
        let mut queue = queued(vec![Seen::Events {
            events: vec![document()],
            at: end,
        }]);
        let closed = close(&mut run, &mut queue, None, |_| end, || false);
        assert_eq!(closed.probe, ProbeHeard::Heard);
        assert!(run.document.is_none());
        assert!(run.messages.is_empty());
    }

    #[test]
    fn a_document_batch_that_the_servers_took_after_the_end_is_not_heard() {
        let mut run = run();
        let end = run.started + Duration::from_secs(13);
        let mut queue = queued(vec![Seen::Events {
            events: vec![document()],
            at: end + A_MOMENT,
        }]);
        assert_eq!(
            close(&mut run, &mut queue, None, |_| end, || false).probe,
            ProbeHeard::NotHeard
        );
    }

    /// The device measures cut off at the run's end, plus their slack, so a later end can take in
    /// a kill line that the run's own end leaves out. The end is the same whether or not the
    /// record has queued evidence to read, however long it reads.
    #[test]
    fn the_end_of_a_run_is_taken_before_the_record_reads_what_is_queued() {
        let started = run().started;
        let end = started + Duration::from_secs(13);
        // A clock that has moved on by 2 s once the record has read the queued document message:
        // more than the 1.5 s of slack of the device measures.
        let clock = |run: &Run| match run.probe_heard {
            true => end + Duration::from_secs(2),
            false => end,
        };
        let failed = Failed {
            failure: Failure::NoFlush,
            at: end,
        };
        for failed in [None, Some(failed)] {
            let mut with = run();
            let mut evidence = queued(vec![Seen::Events {
                events: vec![document()],
                at: end - A_MOMENT,
            }]);
            // The record asks about the command queue only after the end is taken.
            let end_taken = std::cell::Cell::new(false);
            let clock = |run: &Run| {
                end_taken.set(true);
                clock(run)
            };
            let untaken = || {
                assert!(end_taken.get());
                false
            };
            let with = close(&mut with, &mut evidence, failed, clock, untaken);
            let without = close(&mut run(), &mut queued(Vec::new()), failed, clock, untaken);
            assert_eq!(with.probe, ProbeHeard::Heard);
            assert_eq!(without.probe, ProbeHeard::NotHeard);
            assert_eq!(with.ended_at, end);
            assert_eq!(without.ended_at, end);
        }
    }

    #[test]
    fn a_request_still_queued_when_the_run_failed_counts_as_contact() {
        let started = run().started;
        let failed_at = started + LINK_SILENCE * 2;
        let failed = Failed {
            failure: Failure::NoFlush,
            at: failed_at,
        };
        let end_with = |items: Vec<Seen>| {
            close(
                &mut run(),
                &mut queued(items),
                Some(failed),
                |_| failed_at,
                || false,
            )
            .end
        };
        // The page made no request since the hello: it stopped reaching dyno.
        assert_eq!(end_with(Vec::new()), Some(PageEnd::NoRequest));
        // A request at the hello's address just before the run failed, that the run had not yet
        // taken from the queue: the page still reached dyno.
        assert_eq!(
            end_with(vec![Seen::HarnessRequest {
                place: HELLO_PLACE,
                at: failed_at - A_MOMENT,
            }]),
            Some(PageEnd::Failed)
        );
        // One at the other address, or one after the run failed, is no contact.
        for (place, at) in [
            (1, failed_at - A_MOMENT),
            (HELLO_PLACE, failed_at + A_MOMENT),
        ] {
            assert_eq!(
                end_with(vec![Seen::HarnessRequest { place, at }]),
                Some(PageEnd::NoRequest)
            );
        }
    }

    #[test]
    fn only_requests_at_the_place_that_took_the_hello_count_as_contact() {
        let mut run = run();
        let before = run.last_contact;
        let later = before + A_MOMENT;
        run.take(Seen::HarnessRequest {
            place: 1,
            at: later,
        });
        assert_eq!(run.last_contact, before);
        // Contact is when the server received the request, not when the run took it.
        run.take(Seen::HarnessRequest {
            place: HELLO_PLACE,
            at: later,
        });
        assert_eq!(run.last_contact, later);
    }
}
