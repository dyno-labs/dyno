//! The served run: dyno serves the build, with the probe, and a harness page to any browser on
//! the network, such as Safari on an iPhone, Safari in the iOS Simulator or a desktop browser.
//!
//! The servers are at an address of this machine, which the browser must reach. There are two
//! paths from a phone to this machine. One is the local network, which a guest Wi-Fi or a cloud
//! device lab often does not give. The other is the device link itself: the tunnel that reads the
//! phone has an address for each end, and the phone's Safari can load a page from this machine's.
//!
//! A tunnel can go away between the moment its address was read and the moment the browser asks
//! for the page. So a run on a phone whose caller has a tunnel address
//! ([`ServeOptions::tunnel_host`]) serves at both addresses, and asks the browser for the tunnel's
//! ([`ServeDriver::run_watched`]). It asks once. A second ask would restart the browser, perhaps
//! under a page that had just begun, so when no request comes to the tunnel address the run only
//! says so, and a person opens the other address. [`ServeDriver::serve_path`] says which address
//! the page that ran said hello at, not which one was asked for.
//!
//! Two servers run on one host, on two ports, so the game frame is cross-origin to the harness
//! page around it. The game server serves the build folder and adds the probe's script tag to
//! each HTML page. It serves the bytes that the static scan read, and nothing else ([`files`]). Every request it serves, but the probe's own, is a game request. The harness
//! server serves the harness page, which relays the probe's events to dyno over HTTP, as the
//! browser has no driver binding. The run gives the same runner protocol messages as the CDP
//! driver, so `dyno_run::lab` reads both the same way.
//!
//! What a served run cannot see: requests to other hosts, such as a CDN or the game's server,
//! and the JS heap, which Safari does not expose. The phone's memory comes from the device tools
//! instead (`dyno-ios`).
//!
//! [`BuildServer`] is the game server alone, with each page as it is, for the CDP driver's
//! Chromium (`dyno check <build> --browser`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dyno_run::{Driver, DriverError, DriverErrorCode};
use dyno_schema::devices::APPLE;
use dyno_schema::environment::{
    BrowserEngine, DeviceKind, Environment, EnvironmentBrowser, EnvironmentDevice, EnvironmentOs,
    OsName,
};
use dyno_schema::result::{DriverInfo, ServePath};
use dyno_schema::runner::{DriverMessage, RunRequest};
use dyno_schema::scrub::scrub;
use tokio::sync::mpsc;

use crate::files::{Build, check_probe_path};
use crate::local::LocalBrowser;

pub use crate::build_server::BuildServer;

mod build_server;
pub mod files;
mod harness;
mod http;
pub mod local;
mod run;

/// The driver's name in `hello` and in errors.
pub const DRIVER_NAME: &str = "serve";

/// The harness page's port when none is given. The game's is the next one. A fixed port gives
/// the same URL on every run, so a phone can keep it as a bookmark.
pub const DEFAULT_PORT: u16 = 7370;

/// How long a run waits for a browser to open the harness page when no time is given.
pub const DEFAULT_WAIT_FOR_BROWSER: Duration = Duration::from_secs(10 * 60);

/// How long a run that serves at a tunnel address and at the local network address waits for a
/// request at the tunnel's before its caller says that none has come. Safari on a phone opens a
/// page in about a second or two, so a page that is on its way is there well before this.
pub const DEFAULT_SAY_NO_REQUEST_AFTER: Duration = Duration::from_secs(30);

/// Opens the harness page of a run in the browser. It gets the page's URL, once, when the
/// servers answer. An error stops the run, with that reason, such as a locked phone.
pub type Opener<'o> = &'o mut (dyn FnMut(&str) -> Result<(), String> + Send);

/// Where and how a served run serves.
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// The one address a browser reaches this machine at. None takes the tunnel address when
    /// there is one, with this machine's address on its local network after it, and else the
    /// local network address alone.
    pub host: Option<String>,
    /// This machine's address inside the device link's tunnel to the phone, when the phone can
    /// reach a server there. With no `host`, the servers are there, and at the local network
    /// address too. When they cannot be bound there, they are at the local network address
    /// alone, as with no tunnel address.
    pub tunnel_host: Option<IpAddr>,
    /// The harness page's port. The game's is the next one. 0 takes two free ports.
    pub port: u16,
    /// How long the run waits for a browser to open the harness page.
    pub wait_for_browser: Duration,
    /// A program that dyno starts on the harness page for each run, reads the memory of, and
    /// stops when the run ends (module `local`). None waits for a browser on the network.
    pub open_with: Option<PathBuf>,
}

/// Why a served run failed.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("{0}")]
    Build(String),
    #[error("cannot serve on {address}: {detail}")]
    Bind { address: String, detail: String },
    #[error("no browser opened the harness page within {0} s")]
    NoBrowser(u64),
    #[error("no async runtime: {0}")]
    Runtime(String),
    #[error("cannot start the browser to open the page with: {0}")]
    OpenWith(String),
    /// Whoever opens the page in the browser said to stop the run, and why.
    #[error("{0}")]
    Stopped(String),
}

/// The served-run driver, with its servers bound.
#[derive(Debug)]
pub struct ServeDriver {
    build: Build,
    /// Where the two servers are, in the order the browser is asked: one place, or the tunnel
    /// address and then the local network address.
    places: Vec<Place>,
    /// Why the servers are not at the tunnel address the options gave.
    tunnel_refused: Option<String>,
    wait_for_browser: Duration,
    open_with: Option<PathBuf>,
    last_run: Option<RunTimes>,
    /// The user agent of the browser that opened the page in the last run.
    last_user_agent: Option<String>,
    /// Where the page of the last run said hello.
    last_path: Option<ServePath>,
}

/// When a served run's steps happened, in ms since the Unix epoch on this machine's clock. A
/// device run places them on its trace, which the same clock times.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunTimes {
    /// When dyno answered `hello`, so the page loaded the game.
    pub navigated_epoch_ms: f64,
    /// When the game frame went idle. None when it never did.
    pub idle_epoch_ms: Option<f64>,
    /// When the harness made the game active. None when it never did.
    pub activated_epoch_ms: Option<f64>,
    /// When the run ended, before the page dropped the game.
    pub ended_epoch_ms: f64,
}

impl ServeDriver {
    /// Binds the two servers for this build folder. They serve only while a run is on.
    ///
    /// The address is the first of these: the host the options give; the tunnel address they
    /// give, when the servers can be bound there; this machine's address on its local network.
    pub fn bind(build: &Path, options: &ServeOptions) -> Result<ServeDriver, ServeError> {
        ServeDriver::bind_build(Build::open(build)?, options)
    }

    /// [`ServeDriver::bind`], for a build that is open already, such as the build of a scan
    /// ([`Build::of_scan`]). A build with a file of its own at the probe's path is an error.
    pub fn bind_build(build: Build, options: &ServeOptions) -> Result<ServeDriver, ServeError> {
        check_probe_path(&build)?;
        let mut tunnel_refused = None;
        let places = match (&options.host, options.tunnel_host) {
            (Some(host), _) => vec![Place::bind(host, options.port, ServePath::Given)?],
            (None, tunnel) => {
                let at_tunnel = tunnel.and_then(|tunnel| {
                    // The tunnel can go between the moment its address was read and now, and
                    // then the address is no longer this machine's.
                    Place::bind(&tunnel.to_string(), options.port, ServePath::Tunnel)
                        .map_err(|error| tunnel_refused = Some(error.to_string()))
                        .ok()
                });
                let local = local_network_address();
                let at_local = |path| Place::bind(&local.to_string(), options.port, path);
                match at_tunnel {
                    // A phone cannot reach the loopback address, so it is no second path. A
                    // local network address that cannot be bound leaves the tunnel alone.
                    Some(at_tunnel) if local.is_loopback() => vec![at_tunnel],
                    Some(at_tunnel) => std::iter::once(at_tunnel)
                        .chain(at_local(ServePath::LocalNetwork).ok())
                        .collect(),
                    None if local.is_loopback() => vec![at_local(ServePath::Loopback)?],
                    None => vec![at_local(ServePath::LocalNetwork)?],
                }
            }
        };
        Ok(ServeDriver {
            build,
            places,
            tunnel_refused,
            wait_for_browser: options.wait_for_browser,
            open_with: options.open_with.clone(),
            last_run: None,
            last_user_agent: None,
            last_path: None,
        })
    }

    /// The path the browser is asked for first: which of this machine's addresses the URLs of
    /// [`ServeDriver::harness_url`] and [`ServeDriver::game_url`] name.
    pub fn asked_path(&self) -> ServePath {
        self.first().path
    }

    /// The path the browser took in the last run: the address that the page which ran said
    /// hello at. A request that no run came of, at this address or another, is no path. None
    /// before a run, and when no page said hello.
    pub fn serve_path(&self) -> Option<ServePath> {
        self.last_path
    }

    /// The harness page's URL at the local network address, when the servers are there as well
    /// as at the tunnel address. No run asks the browser for it: a person opens it when the
    /// tunnel address gets no request.
    pub fn fallback_harness_url(&self) -> Option<&str> {
        self.places.get(1).map(|place| place.harness_url.as_str())
    }

    /// Why the servers are not at the tunnel address that the options gave. None when they are
    /// there, when the options gave no tunnel address, or when they gave a host, which wins.
    pub fn tunnel_refused(&self) -> Option<&str> {
        self.tunnel_refused.as_deref()
    }

    /// The first place: the one the browser is asked for first. A driver has one at least.
    fn first(&self) -> &Place {
        &self.places[0]
    }

    /// The times of the last run that a browser opened. None before one.
    pub fn last_run(&self) -> Option<RunTimes> {
        self.last_run
    }

    /// The URL to open in the browser: the harness page, at the address asked for first.
    pub fn harness_url(&self) -> &str {
        &self.first().harness_url
    }

    /// The game server's URL: the build's root, at the address asked for first.
    pub fn game_url(&self) -> &str {
        &self.first().game_url
    }

    /// The launch URL of a run: the game server's URL with this query string, such as a demo
    /// session's `sessionID` and `rgs_url`. The harness page loads the run request's launch URL
    /// in its frame, so a run request names this one.
    pub fn launch_url(&self, query: Option<&str>) -> String {
        match query.map(|query| query.trim_start_matches('?')) {
            Some(query) if !query.is_empty() => format!("{}?{query}", self.game_url()),
            _ => self.game_url().to_owned(),
        }
    }

    /// Runs the request as [`Driver::run`] does, and tells the caller two things while it waits
    /// for a page.
    ///
    /// `open` gets the harness page's URL once the servers serve, to open it in the browser, so
    /// the browser's first request is answered at once. It is asked once, and runs on a thread
    /// of its own, because it can take as long as the phone does.
    ///
    /// `say_no_request` is called once when no request has come to the harness page at the
    /// first address, the one of [`ServeDriver::harness_url`], that long after the run began to
    /// wait, and `open` has returned: it can still restart the browser until then, as by a
    /// launch of its own after the phone did not open the page. The run waits on.
    pub fn run_watched(
        &mut self,
        request: &RunRequest,
        open: Option<Opener<'_>>,
        say_no_request: Option<(Duration, &mut dyn FnMut())>,
    ) -> Result<Vec<DriverMessage>, DriverError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| driver_error(&ServeError::Runtime(error.to_string())))?;
        let local = self
            .open_with
            .as_deref()
            .map(|program| LocalBrowser::open(program, self.harness_url()))
            .transpose()
            .map_err(|error| driver_error(&error))?;
        let no_request = say_no_request.map(|(after, say)| run::NoRequest { after, say });
        let outcome = match open {
            None => runtime.block_on(run::run(self, request, None, no_request)),
            Some(open) => std::thread::scope(|scope| {
                let (ask, mut asked) = mpsc::unbounded_channel::<String>();
                let (answer, answers) = mpsc::unbounded_channel();
                // The thread ends when the run drops its end, and the scope waits for it.
                scope.spawn(move || {
                    if let Some(url) = asked.blocking_recv() {
                        let _ = answer.send(open(&url));
                    }
                });
                let asks = run::Asks { ask, answers };
                runtime.block_on(run::run(self, request, Some(asks), no_request))
            }),
        };
        self.last_path = outcome.hello_at.map(|place| self.places[place].path);
        // An error drops the local browser, which stops it.
        let served = outcome.served.map_err(|error| driver_error(&error))?;
        let mut messages = served.messages;
        if let Some(local) = local {
            let times = served.times;
            let last = messages.pop();
            messages.extend(local.close(
                instant_at(times.navigated_epoch_ms),
                times.idle_epoch_ms.map(instant_at),
                instant_at(times.ended_epoch_ms),
                times.ended_epoch_ms - times.navigated_epoch_ms,
            ));
            messages.extend(last);
        }
        self.last_run = Some(served.times);
        self.last_user_agent = Some(served.user_agent);
        Ok(messages)
    }
}

impl Driver for ServeDriver {
    fn name(&self) -> &str {
        DRIVER_NAME
    }

    /// With `open_with`, the browser starts before the run waits for it, and its processes'
    /// memory goes in just before the run's last message.
    fn run(&mut self, request: &RunRequest) -> Result<Vec<DriverMessage>, DriverError> {
        self.run_watched(request, None, None)
    }

    /// Safari gives the page no heap reading, and the harness page has no other way to read it.
    fn reads_heap(&self) -> bool {
        false
    }

    /// What the user agent of the browser that opened the page says.
    fn environment(&self, _hello: &DriverInfo) -> Environment {
        user_agent_environment(self.last_user_agent.as_deref().unwrap_or_default())
    }
}

/// The monotonic instant of a time in ms since the Unix epoch on this machine's clock, which the
/// served run's times use. A time in the future is now.
fn instant_at(epoch_ms: f64) -> Instant {
    let now = Instant::now();
    let now_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1_000.0);
    let ago = Duration::from_secs_f64(((now_epoch_ms - epoch_ms) / 1_000.0).max(0.0));
    now.checked_sub(ago).unwrap_or(now)
}

fn driver_error(error: &ServeError) -> DriverError {
    DriverError::new(DriverErrorCode::Run, &error.to_string())
}

/// The two servers, bound at one address of this machine, with their URLs.
#[derive(Debug)]
struct Place {
    harness: TcpListener,
    game: TcpListener,
    harness_url: String,
    game_url: String,
    /// The path a browser takes to this address.
    path: ServePath,
}

impl Place {
    /// Binds the harness page's server on `port` of `host`, and the game's on the next port.
    /// Port 0 takes two free ports.
    fn bind(host: &str, port: u16, path: ServePath) -> Result<Place, ServeError> {
        // An address is bound as it is. A name could be any of this machine's addresses.
        let bind_to = host
            .parse::<IpAddr>()
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let game_port = match port {
            0 => 0,
            port => port.checked_add(1).ok_or_else(|| ServeError::Bind {
                address: format!("{host}:{port}"),
                detail: "the game needs the next port too".to_owned(),
            })?,
        };
        let harness = listen(bind_to, port)?;
        let game = listen(bind_to, game_port)?;
        Ok(Place {
            harness_url: url(host, &harness)?,
            game_url: url(host, &game)?,
            harness,
            game,
            path,
        })
    }
}

/// A listener on this address and port, ready for tokio.
fn listen(address: IpAddr, port: u16) -> Result<TcpListener, ServeError> {
    let bind_error = |error: std::io::Error| ServeError::Bind {
        // With its brackets when it is IPv6, as a tunnel address is.
        address: SocketAddr::new(address, port).to_string(),
        detail: error.to_string(),
    };
    let listener = TcpListener::bind((address, port)).map_err(bind_error)?;
    listener.set_nonblocking(true).map_err(bind_error)?;
    Ok(listener)
}

/// The URL of a listener, by the host a browser reaches it at.
fn url(host: &str, listener: &TcpListener) -> Result<String, ServeError> {
    let port = listener
        .local_addr()
        .map_err(|error| ServeError::Bind {
            address: host.to_owned(),
            detail: error.to_string(),
        })?
        .port();
    let host = match host.parse::<Ipv6Addr>() {
        Ok(_) => format!("[{host}]"),
        Err(_) => host.to_owned(),
    };
    Ok(format!("http://{host}:{port}/"))
}

/// This machine's address on the network that reaches the internet: the address a phone on the
/// same Wi-Fi reaches it at. Connecting a UDP socket sends nothing. With no such network, the
/// loopback address, which the iOS Simulator and a browser on this machine reach.
pub fn local_network_address() -> IpAddr {
    let found = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).and_then(|socket| {
        // 192.0.2.1 is a documentation address (RFC 5737): it is only used to pick a route.
        socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?;
        socket.local_addr()
    });
    match found {
        Ok(address) if !address.ip().is_unspecified() => address.ip(),
        _ => IpAddr::V4(Ipv4Addr::LOCALHOST),
    }
}

/// The browser and the kind of device, from a user agent, such as "Safari 26.2 on iPhone". None
/// for a browser it does not know. The iOS Simulator says iPhone too.
pub fn browser_name(user_agent: &str) -> Option<String> {
    let (name, version) = browser_and_version(user_agent)?;
    let browser = match version {
        Some(version) => format!("{name} {version}"),
        None => name.to_owned(),
    };
    let device = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Macintosh", "Mac"),
    ]
    .iter()
    .find(|(marker, _)| user_agent.contains(marker))
    .map(|(_, device)| device);
    Some(match device {
        Some(device) => format!("{browser} on {device}"),
        None => browser,
    })
}

/// The browser's name and version in a user agent. None for a browser it does not know.
///
/// WebKitGTK and WPE on Linux say they are Safari, with a made-up `Version/`, so a Safari on a
/// Linux desktop is WebKitGTK with no version.
fn browser_and_version(user_agent: &str) -> Option<(&'static str, Option<String>)> {
    let version_after = |marker: &str| {
        let rest = user_agent.split(marker).nth(1)?;
        let version = rest.split([' ', ')', ';']).next().unwrap_or_default();
        (!version.is_empty()).then(|| scrub(version))
    };
    let linux_desktop = user_agent.contains("Linux") && !user_agent.contains("Android");
    if let Some(version) = version_after("CriOS/").or_else(|| version_after("Chrome/")) {
        Some(("Chrome", Some(version)))
    } else if let Some(version) = version_after("FxiOS/").or_else(|| version_after("Firefox/")) {
        Some(("Firefox", Some(version)))
    } else if user_agent.contains("Safari/") && linux_desktop {
        Some(("WebKitGTK", None))
    } else if user_agent.contains("Safari/") {
        version_after("Version/").map(|version| ("Safari", Some(version)))
    } else {
        None
    }
}

/// What a user agent says ran the page: the browser, the OS and, on an iPhone or an iPad, the kind
/// of device. A user agent's OS version is frozen, so the OS has none, and it names no model, so
/// the device has no key. Every browser on iOS uses WebKit. Safari on iOS and Chrome say whether
/// they are headless; other browsers do not.
pub fn user_agent_environment(user_agent: &str) -> Environment {
    let mobile_device = if user_agent.contains("iPhone") {
        Some(("iPhone", DeviceKind::Phone, OsName::Ios))
    } else if user_agent.contains("iPad") {
        Some(("iPad", DeviceKind::Tablet, OsName::Ipados))
    } else {
        None
    };
    let os = match mobile_device {
        Some((_, _, os)) => Some(os),
        None if user_agent.contains("Android") => Some(OsName::Android),
        None if user_agent.contains("Macintosh") => Some(OsName::Macos),
        None if user_agent.contains("Windows") => Some(OsName::Windows),
        None if user_agent.contains("Linux") || user_agent.contains("X11") => Some(OsName::Linux),
        None => None,
    };
    let browser = browser_and_version(user_agent).map(|(name, version)| {
        let engine = match (name, mobile_device) {
            (_, Some(_)) => BrowserEngine::Webkit,
            ("Chrome", None) => BrowserEngine::Blink,
            ("Firefox", None) => BrowserEngine::Gecko,
            _ => BrowserEngine::Webkit,
        };
        let headless = match (name, mobile_device) {
            ("Safari", Some(_)) => Some(false),
            ("Chrome", None) => Some(user_agent.contains("HeadlessChrome")),
            _ => None,
        };
        EnvironmentBrowser {
            name: name.to_owned(),
            version,
            engine: Some(engine),
            headless,
        }
    });
    Environment {
        browser,
        os: os.map(|name| EnvironmentOs {
            name,
            version: None,
        }),
        device: mobile_device.map(|(name, kind, _)| EnvironmentDevice {
            key: None,
            name: name.to_owned(),
            vendor: APPLE.to_owned(),
            kind,
            memory_bytes: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPHONE_SAFARI: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) \
        AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.2 Mobile/15E148 Safari/604.1";

    #[test]
    fn safari_is_named_by_its_own_version_and_the_device() {
        assert_eq!(
            browser_name(IPHONE_SAFARI).as_deref(),
            Some("Safari 26.2 on iPhone")
        );
        let mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
            (KHTML, like Gecko) Version/27.0 Safari/605.1.15";
        assert_eq!(browser_name(mac).as_deref(), Some("Safari 27.0 on Mac"));
    }

    #[test]
    fn chrome_is_chrome_on_any_engine() {
        let headless = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
            HeadlessChrome/152.0.7977.82 Safari/537.36";
        assert_eq!(
            browser_name(headless).as_deref(),
            Some("Chrome 152.0.7977.82")
        );
        let ios = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 \
            (KHTML, like Gecko) CriOS/150.0.1 Mobile/15E148 Safari/604.1";
        assert_eq!(
            browser_name(ios).as_deref(),
            Some("Chrome 150.0.1 on iPhone")
        );
    }

    #[test]
    fn firefox_is_firefox_on_any_engine() {
        let android = "Mozilla/5.0 (Android 15; Mobile; rv:140.0) Gecko/140.0 Firefox/140.0";
        assert_eq!(
            browser_name(android).as_deref(),
            Some("Firefox 140.0 on Android")
        );
        let linux = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
        assert_eq!(browser_name(linux).as_deref(), Some("Firefox 140.0"));
    }

    #[test]
    fn a_safari_on_a_linux_desktop_is_webkitgtk_with_no_version() {
        let gtk = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) \
            Version/60.5 Safari/605.1.15";
        assert_eq!(browser_name(gtk).as_deref(), Some("WebKitGTK"));
        let browser = user_agent_environment(gtk).browser.unwrap();
        assert_eq!(browser.name, "WebKitGTK");
        assert_eq!(browser.version, None);
        assert_eq!(browser.engine, Some(BrowserEngine::Webkit));
    }

    #[test]
    fn an_unknown_browser_has_no_name() {
        assert_eq!(browser_name("curl/8.0"), None);
    }

    #[test]
    fn safari_on_an_iphone_is_webkit_on_ios_with_no_version_or_model() {
        let environment = user_agent_environment(IPHONE_SAFARI);
        assert_eq!(
            environment.browser,
            Some(EnvironmentBrowser {
                name: "Safari".to_owned(),
                version: Some("26.2".to_owned()),
                engine: Some(BrowserEngine::Webkit),
                headless: Some(false),
            })
        );
        // The user agent says iOS 18.6, which Safari 26 freezes, so the version is not read.
        assert_eq!(
            environment.os,
            Some(EnvironmentOs {
                name: OsName::Ios,
                version: None,
            })
        );
        assert_eq!(
            environment.device,
            Some(EnvironmentDevice {
                key: None,
                name: "iPhone".to_owned(),
                vendor: "Apple".to_owned(),
                kind: DeviceKind::Phone,
                memory_bytes: None,
            })
        );
    }

    #[test]
    fn chrome_on_ios_is_webkit_and_an_ipad_is_a_tablet() {
        let ipad = "Mozilla/5.0 (iPad; CPU OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, \
            like Gecko) CriOS/150.0.1 Mobile/15E148 Safari/604.1";
        let environment = user_agent_environment(ipad);
        let browser = environment.browser.unwrap();
        assert_eq!(browser.name, "Chrome");
        assert_eq!(browser.engine, Some(BrowserEngine::Webkit));
        assert_eq!(browser.headless, None);
        assert_eq!(environment.os.unwrap().name, OsName::Ipados);
        assert_eq!(environment.device.unwrap().kind, DeviceKind::Tablet);
    }

    #[test]
    fn a_desktop_browser_has_no_device() {
        let headless = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
            HeadlessChrome/152.0.7977.82 Safari/537.36";
        let environment = user_agent_environment(headless);
        let browser = environment.browser.unwrap();
        assert_eq!(browser.engine, Some(BrowserEngine::Blink));
        assert_eq!(browser.headless, Some(true));
        assert_eq!(environment.os.unwrap().name, OsName::Linux);
        assert_eq!(environment.device, None);

        let android = "Mozilla/5.0 (Android 15; Mobile; rv:140.0) Gecko/140.0 Firefox/140.0";
        let environment = user_agent_environment(android);
        assert_eq!(
            environment.browser.unwrap().engine,
            Some(BrowserEngine::Gecko)
        );
        assert_eq!(environment.os.unwrap().name, OsName::Android);
        assert_eq!(environment.device, None);

        let windows = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, \
            like Gecko) Chrome/150.0.0.0 Safari/537.36";
        let environment = user_agent_environment(windows);
        assert_eq!(environment.browser.unwrap().headless, Some(false));
        assert_eq!(environment.os.unwrap().name, OsName::Windows);

        let mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
            (KHTML, like Gecko) Version/27.0 Safari/605.1.15";
        let environment = user_agent_environment(mac);
        assert_eq!(
            environment.browser.unwrap().engine,
            Some(BrowserEngine::Webkit)
        );
        assert_eq!(environment.os.unwrap().name, OsName::Macos);
    }

    #[test]
    fn an_unknown_user_agent_gives_nothing_known() {
        assert_eq!(
            user_agent_environment("curl/8.0"),
            Environment {
                browser: None,
                os: None,
                device: None,
            }
        );
    }

    #[test]
    fn with_no_host_the_servers_are_on_the_local_network_address() {
        let folder = tempfile::tempdir().unwrap();
        let options = ServeOptions {
            host: None,
            tunnel_host: None,
            port: 0,
            wait_for_browser: Duration::from_secs(1),
            open_with: None,
        };
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        let address = local_network_address();
        assert!(
            driver.harness_url().contains(&address.to_string()),
            "{}",
            driver.harness_url()
        );
        assert_eq!(driver.asked_path(), local_path(address));
        assert_eq!(driver.tunnel_refused(), None);
        assert_eq!(driver.fallback_harness_url(), None);
        // No browser has asked for the page yet, so no path was taken.
        assert_eq!(driver.serve_path(), None);
        assert_eq!(driver.name(), DRIVER_NAME);
    }

    /// The path of a run that serves at this machine's own choice of address.
    fn local_path(address: IpAddr) -> ServePath {
        if address.is_loopback() {
            ServePath::Loopback
        } else {
            ServePath::LocalNetwork
        }
    }

    /// The options of a run on a phone whose device link has this tunnel address, with this
    /// `--serve-host`.
    fn options_with_tunnel(tunnel: &str, host: Option<&str>) -> ServeOptions {
        ServeOptions {
            host: host.map(str::to_owned),
            tunnel_host: Some(tunnel.parse().unwrap()),
            port: 0,
            wait_for_browser: Duration::from_secs(1),
            open_with: None,
        }
    }

    /// An address that is not this machine's, so no server can be bound there (RFC 5737).
    const NOT_THIS_MACHINE: &str = "192.0.2.9";

    #[test]
    fn with_a_tunnel_address_the_browser_is_asked_for_it_first() {
        let folder = tempfile::tempdir().unwrap();
        // The loopback address stands in for the tunnel's: this machine has it.
        let options = options_with_tunnel("127.0.0.1", None);
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        assert!(driver.harness_url().starts_with("http://127.0.0.1:"));
        assert!(driver.game_url().starts_with("http://127.0.0.1:"));
        assert_eq!(driver.asked_path(), ServePath::Tunnel);
        assert_eq!(driver.tunnel_refused(), None);
    }

    #[test]
    fn with_a_tunnel_address_the_servers_are_at_the_local_network_address_too() {
        let folder = tempfile::tempdir().unwrap();
        let options = options_with_tunnel("127.0.0.1", None);
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        let local = local_network_address();
        match driver.fallback_harness_url() {
            Some(url) => assert!(url.starts_with(&format!("http://{local}:")), "{url}"),
            // A phone cannot reach the loopback address, so a machine with no network has no
            // second place.
            None => assert!(local.is_loopback()),
        }
    }

    #[test]
    fn a_tunnel_address_that_cannot_be_bound_falls_back_to_the_local_network() {
        let folder = tempfile::tempdir().unwrap();
        let options = options_with_tunnel(NOT_THIS_MACHINE, None);
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        let address = local_network_address();
        assert!(
            driver.harness_url().contains(&address.to_string()),
            "{}",
            driver.harness_url()
        );
        assert_eq!(driver.asked_path(), local_path(address));
        assert_eq!(driver.fallback_harness_url(), None);
        let refused = driver.tunnel_refused().unwrap();
        assert!(
            refused.starts_with(&format!("cannot serve on {NOT_THIS_MACHINE}:0: ")),
            "{refused}"
        );
    }

    #[test]
    fn a_given_host_wins_over_the_tunnel_address() {
        let folder = tempfile::tempdir().unwrap();
        let options = options_with_tunnel(NOT_THIS_MACHINE, Some("127.0.0.1"));
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        assert!(driver.harness_url().starts_with("http://127.0.0.1:"));
        assert_eq!(driver.asked_path(), ServePath::Given);
        // The tunnel address was never tried, and the given address is the only one.
        assert_eq!(driver.tunnel_refused(), None);
        assert_eq!(driver.fallback_harness_url(), None);
    }

    #[test]
    fn a_given_host_that_cannot_be_bound_is_an_error_with_no_fallback() {
        let folder = tempfile::tempdir().unwrap();
        let options = options_with_tunnel("127.0.0.1", Some(NOT_THIS_MACHINE));
        assert!(matches!(
            ServeDriver::bind(folder.path(), &options),
            Err(ServeError::Bind { .. })
        ));
    }

    #[test]
    fn an_ipv6_tunnel_address_is_bracketed_in_the_urls() {
        let folder = tempfile::tempdir().unwrap();
        let options = options_with_tunnel("::1", None);
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        // A machine with no IPv6 loopback falls back; the URL shape is tested where it has one.
        if driver.asked_path() == ServePath::Tunnel {
            assert!(driver.harness_url().starts_with("http://[::1]:"));
            assert!(driver.game_url().starts_with("http://[::1]:"));
        } else {
            assert!(driver.tunnel_refused().is_some());
        }
    }

    #[test]
    fn a_build_with_a_file_at_the_probe_s_path_cannot_be_served_to_a_run() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join("__dyno")).unwrap();
        std::fs::write(folder.path().join("__dyno/probe.js"), "the build's").unwrap();
        let options = options_with_tunnel("127.0.0.1", Some("127.0.0.1"));
        let error = ServeDriver::bind(folder.path(), &options).unwrap_err();
        assert!(matches!(error, ServeError::Build(_)), "{error}");
        assert!(error.to_string().contains("__dyno/probe.js"), "{error}");
        // The build server adds no probe, so the file is the build's there.
        assert!(BuildServer::start(folder.path()).is_ok());
    }

    #[test]
    fn a_port_in_use_cannot_be_served() {
        let folder = tempfile::tempdir().unwrap();
        let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let options = ServeOptions {
            host: Some("127.0.0.1".to_owned()),
            tunnel_host: None,
            port: taken.local_addr().unwrap().port(),
            wait_for_browser: Duration::from_secs(1),
            open_with: None,
        };
        let error = ServeDriver::bind(folder.path(), &options).unwrap_err();
        assert!(matches!(error, ServeError::Bind { .. }), "{error}");
    }

    #[test]
    fn the_servers_take_two_ports_and_the_urls_name_the_host() {
        let folder = tempfile::tempdir().unwrap();
        let options = ServeOptions {
            host: Some("127.0.0.1".to_owned()),
            tunnel_host: None,
            port: 0,
            wait_for_browser: Duration::from_secs(1),
            open_with: None,
        };
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        assert!(driver.harness_url().starts_with("http://127.0.0.1:"));
        assert!(driver.game_url().starts_with("http://127.0.0.1:"));
        assert_ne!(driver.harness_url(), driver.game_url());
    }

    #[test]
    fn a_launch_url_adds_the_query_to_the_game_url() {
        let folder = tempfile::tempdir().unwrap();
        let options = ServeOptions {
            host: Some("127.0.0.1".to_owned()),
            tunnel_host: None,
            port: 0,
            wait_for_browser: Duration::from_secs(1),
            open_with: None,
        };
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        let game = driver.game_url().to_owned();
        assert_eq!(driver.launch_url(None), game);
        assert_eq!(driver.launch_url(Some("")), game);
        assert_eq!(
            driver.launch_url(Some("?sessionID=s&rgs_url=r")),
            format!("{game}?sessionID=s&rgs_url=r")
        );
    }

    #[test]
    fn an_ipv6_host_is_bracketed() {
        let folder = tempfile::tempdir().unwrap();
        let options = ServeOptions {
            host: Some("::1".to_owned()),
            tunnel_host: None,
            port: 0,
            wait_for_browser: Duration::from_secs(1),
            open_with: None,
        };
        // A machine with no IPv6 loopback cannot bind it; the URL shape is what is tested.
        if let Ok(driver) = ServeDriver::bind(folder.path(), &options) {
            assert!(driver.harness_url().starts_with("http://[::1]:"));
        }
    }

    #[test]
    fn the_last_port_leaves_no_port_for_the_game() {
        let folder = tempfile::tempdir().unwrap();
        let options = ServeOptions {
            host: Some("127.0.0.1".to_owned()),
            tunnel_host: None,
            port: u16::MAX,
            wait_for_browser: Duration::from_secs(1),
            open_with: None,
        };
        assert!(matches!(
            ServeDriver::bind(folder.path(), &options),
            Err(ServeError::Bind { .. })
        ));
    }
}
