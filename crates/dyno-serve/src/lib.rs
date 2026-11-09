//! The served run: dyno serves the build, with the probe, and a harness page to any browser on
//! the network, such as Safari on an iPhone, Safari in the iOS Simulator or a desktop browser.
//!
//! Two servers run on one host, on two ports, so the game frame is cross-origin to the harness
//! page around it. The game server serves the build folder and adds the probe's script tag to
//! each HTML page. Every request it serves, but the probe's own, is a game request. The harness
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

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, UdpSocket};
use std::path::Path;
use std::time::Duration;

use dyno_run::{Driver, DriverError, DriverErrorCode};
use dyno_schema::devices::APPLE;
use dyno_schema::environment::{
    BrowserEngine, DeviceKind, Environment, EnvironmentBrowser, EnvironmentDevice, EnvironmentOs,
    OsName,
};
use dyno_schema::result::DriverInfo;
use dyno_schema::runner::{DriverMessage, RunRequest};
use dyno_schema::scrub::scrub;

use crate::files::Build;

pub use crate::build_server::BuildServer;

mod build_server;
pub mod files;
mod harness;
mod http;
mod run;

/// The driver's name in `hello` and in errors.
pub const DRIVER_NAME: &str = "serve";

/// The harness page's port when none is given. The game's is the next one. A fixed port gives
/// the same URL on every run, so a phone can keep it as a bookmark.
pub const DEFAULT_PORT: u16 = 7370;

/// How long a run waits for a browser to open the harness page when no time is given.
pub const DEFAULT_WAIT_FOR_BROWSER: Duration = Duration::from_secs(10 * 60);

/// Where and how a served run serves.
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// The address a browser reaches this machine at. None takes this machine's address on its
    /// local network.
    pub host: Option<String>,
    /// The harness page's port. The game's is the next one. 0 takes two free ports.
    pub port: u16,
    /// How long the run waits for a browser to open the harness page.
    pub wait_for_browser: Duration,
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
}

/// The served-run driver, with its two servers bound.
#[derive(Debug)]
pub struct ServeDriver {
    build: Build,
    harness: TcpListener,
    game: TcpListener,
    harness_url: String,
    game_url: String,
    wait_for_browser: Duration,
    last_run: Option<RunTimes>,
    /// The user agent of the browser that opened the page in the last run.
    last_user_agent: Option<String>,
}

/// When a served run's steps happened, in ms since the Unix epoch on this machine's clock. A
/// device run places them on its trace, which the same clock times.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunTimes {
    /// When dyno answered `hello`, so the page loaded the game.
    pub navigated_epoch_ms: f64,
    /// When the game frame went idle. None when it never did.
    pub idle_epoch_ms: Option<f64>,
    /// When the run ended, before the page dropped the game.
    pub ended_epoch_ms: f64,
}

impl ServeDriver {
    /// Binds the two servers for this build folder. They serve only while a run is on.
    pub fn bind(build: &Path, options: &ServeOptions) -> Result<ServeDriver, ServeError> {
        let build = Build::open(build)?;
        let host = options
            .host
            .clone()
            .unwrap_or_else(|| local_network_address().to_string());
        // An address is bound as it is. A name could be any of this machine's addresses.
        let bind_to = host
            .parse::<IpAddr>()
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let game_port = match options.port {
            0 => 0,
            port => port.checked_add(1).ok_or_else(|| ServeError::Bind {
                address: format!("{host}:{port}"),
                detail: "the game needs the next port too".to_owned(),
            })?,
        };
        let harness = listen(bind_to, options.port)?;
        let game = listen(bind_to, game_port)?;
        Ok(ServeDriver {
            harness_url: url(&host, &harness)?,
            game_url: url(&host, &game)?,
            build,
            harness,
            game,
            wait_for_browser: options.wait_for_browser,
            last_run: None,
            last_user_agent: None,
        })
    }

    /// The times of the last run that a browser opened. None before one.
    pub fn last_run(&self) -> Option<RunTimes> {
        self.last_run
    }

    /// The URL to open in the browser: the harness page.
    pub fn harness_url(&self) -> &str {
        &self.harness_url
    }

    /// The game server's URL: the build's root.
    pub fn game_url(&self) -> &str {
        &self.game_url
    }

    /// The launch URL of a run: the game server's URL with this query string, such as a demo
    /// session's `sessionID` and `rgs_url`. The harness page loads the run request's launch URL
    /// in its frame, so a run request names this one.
    pub fn launch_url(&self, query: Option<&str>) -> String {
        match query.map(|query| query.trim_start_matches('?')) {
            Some(query) if !query.is_empty() => format!("{}?{query}", self.game_url),
            _ => self.game_url.clone(),
        }
    }
}

impl Driver for ServeDriver {
    fn name(&self) -> &str {
        DRIVER_NAME
    }

    fn run(&mut self, request: &RunRequest) -> Result<Vec<DriverMessage>, DriverError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| driver_error(&ServeError::Runtime(error.to_string())))?;
        let served = runtime
            .block_on(run::run(self, request))
            .map_err(|error| driver_error(&error))?;
        self.last_run = Some(served.times);
        self.last_user_agent = Some(served.user_agent);
        Ok(served.messages)
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

fn driver_error(error: &ServeError) -> DriverError {
    DriverError::new(DriverErrorCode::Run, &error.to_string())
}

/// A listener on this address and port, ready for tokio.
fn listen(address: IpAddr, port: u16) -> Result<TcpListener, ServeError> {
    let bind_error = |error: std::io::Error| ServeError::Bind {
        address: format!("{address}:{port}"),
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
    let browser = format!("{name} {version}");
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
fn browser_and_version(user_agent: &str) -> Option<(&'static str, String)> {
    let version_after = |marker: &str| {
        let rest = user_agent.split(marker).nth(1)?;
        let version = rest.split([' ', ')', ';']).next().unwrap_or_default();
        (!version.is_empty()).then(|| scrub(version))
    };
    if let Some(version) = version_after("CriOS/").or_else(|| version_after("Chrome/")) {
        Some(("Chrome", version))
    } else if let Some(version) = version_after("FxiOS/").or_else(|| version_after("Firefox/")) {
        Some(("Firefox", version))
    } else if user_agent.contains("Safari/") {
        version_after("Version/").map(|version| ("Safari", version))
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
            version: Some(version),
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
            port: 0,
            wait_for_browser: Duration::from_secs(1),
        };
        let driver = ServeDriver::bind(folder.path(), &options).unwrap();
        let address = local_network_address().to_string();
        assert!(
            driver.harness_url().contains(&address),
            "{}",
            driver.harness_url()
        );
        assert_eq!(driver.name(), DRIVER_NAME);
    }

    #[test]
    fn a_port_in_use_cannot_be_served() {
        let folder = tempfile::tempdir().unwrap();
        let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let options = ServeOptions {
            host: Some("127.0.0.1".to_owned()),
            port: taken.local_addr().unwrap().port(),
            wait_for_browser: Duration::from_secs(1),
        };
        let error = ServeDriver::bind(folder.path(), &options).unwrap_err();
        assert!(matches!(error, ServeError::Bind { .. }), "{error}");
    }

    #[test]
    fn the_servers_take_two_ports_and_the_urls_name_the_host() {
        let folder = tempfile::tempdir().unwrap();
        let options = ServeOptions {
            host: Some("127.0.0.1".to_owned()),
            port: 0,
            wait_for_browser: Duration::from_secs(1),
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
            port: 0,
            wait_for_browser: Duration::from_secs(1),
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
            port: 0,
            wait_for_browser: Duration::from_secs(1),
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
            port: u16::MAX,
            wait_for_browser: Duration::from_secs(1),
        };
        assert!(matches!(
            ServeDriver::bind(folder.path(), &options),
            Err(ServeError::Bind { .. })
        ));
    }
}
