//! A recording in progress, for a caller with no async runtime: a thread of its own runs the
//! phone's session ([`phone`] or, for tests, [`fake`]) on a tokio runtime, and says when the
//! recording has started and whether Safari opened the page.
//!
//! The recording starts first, and Safari opens the page after, in a step of its own. Between the
//! two the caller knows the phone, and can ask for this machine's address in the device link's
//! tunnel ([`Recorder::tunnel_host`]) to serve the page there. A recording opens one page: a
//! second launch would restart Safari under a run that may have begun.
//!
//! [`phone`]: crate::phone
//! [`fake`]: crate::fake

use std::ffi::OsString;
use std::future::Future;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::devicectl;
use crate::fake::{self, FAKE_PHONE_VARIABLE};
use crate::phone;
use crate::recording::{LineFeed, Phone, Recording, SampleFeed, record};
use crate::tunnel::{self, NoTunnelAddress};

/// How long the phone may take to connect and start sending.
const START_WAIT: Duration = Duration::from_secs(60);

/// How long the phone may take to open Safari.
const OPEN_WAIT: Duration = Duration::from_secs(30);

/// Where the phone is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    /// Through usbmuxd: the phone with this UDID, or the only one it lists.
    Usbmuxd { udid: Option<String> },
    /// Straight at this address on the network, with the pair record in this file.
    Network { ip: IpAddr, pairing_file: PathBuf },
}

/// Why a device step failed.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error(
        "cannot reach usbmuxd, which lists the phones on a cable: {0}. Connect the phone with a cable and run usbmuxd, or give its address with --device-ip and --pairing-file"
    )]
    NoUsbmuxd(String),
    #[error(
        "no iPhone is connected. Connect one with a cable, unlock it and trust this computer, or give its address with --device-ip and --pairing-file"
    )]
    NoDevice,
    #[error("{0} iPhones are connected: choose one with --device-id. Connected: {1}")]
    SeveralDevices(usize, String),
    #[error("no connected iPhone has the UDID {0}. Connected: {1}")]
    UnknownDevice(String, String),
    #[error("cannot read the pairing file {0}: {1}")]
    PairingFile(String, String),
    #[error(
        "cannot connect to the phone: {0}. It needs iOS 17 or later, Developer Mode on, and trust in this computer"
    )]
    Connect(String),
    #[error("cannot start recording the phone: {0}")]
    Record(String),
    #[error(
        "the phone is locked. Unlock it, and set Settings > Display & Brightness > Auto-Lock to Never, so it stays unlocked for the run"
    )]
    Locked,
    #[error("Safari did not open the page: {0}")]
    Open(String),
}

/// What a session says while it starts.
pub(crate) enum Event {
    /// The recording is on.
    Started(Phone),
    /// The recording did not start.
    Failed(DeviceError),
    /// Whether Safari opened the page.
    Opened(Result<(), DeviceError>),
}

/// How Safari opened the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenedWith {
    /// The phone's CoreDevice app service, as `devicectl` uses it.
    CoreDevice,
    /// `xcrun devicectl`, when the app service did not open it.
    Devicectl,
}

/// A recording in progress.
pub struct Recorder {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<Option<Recording>>>,
    phone: Phone,
    opened: Option<Result<OpenedWith, DeviceError>>,
    /// Where the session waits for the page to open in Safari. None once it was asked, and
    /// once the recording has ended.
    open: Option<oneshot::Sender<String>>,
    events: Receiver<Event>,
    open_wait: Duration,
    /// The program that runs `xcrun devicectl` for this phone: a Mac's `xcrun`, or the stub that
    /// `DYNO_XCRUN` names. None on a machine with none.
    xcrun: Option<OsString>,
    /// This machine's address in the fake phone's tunnel, when the fake phone has one.
    fake_tunnel: Option<IpAddr>,
}

impl Recorder {
    /// Connects to the phone and starts recording it. It returns once the recording is on.
    /// `DYNO_FAKE_PHONE` records a fake phone instead.
    pub fn start(address: &Address) -> Result<Recorder, DeviceError> {
        let fake = std::env::var(FAKE_PHONE_VARIABLE).ok();
        Recorder::start_with(address, fake, [START_WAIT, OPEN_WAIT])
    }

    /// [`Recorder::start`], with the fake phone's settings when `fake` gives them, and these
    /// waits for the start and for Safari.
    fn start_with(
        address: &Address,
        fake: Option<String>,
        [start_wait, open_wait]: [Duration; 2],
    ) -> Result<Recorder, DeviceError> {
        let (events_to, events) = mpsc::channel();
        let (stop, stopped) = oneshot::channel();
        let (open, to_open) = oneshot::channel();
        // A fake phone has no devicectl to fall back on, but the stub that `DYNO_XCRUN` names.
        let xcrun = devicectl::program(fake.is_some());
        let fake_tunnel = fake.as_deref().and_then(fake::tunnel_host);
        let address = address.clone();
        let thread = thread::Builder::new()
            .name("dyno-phone".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    let error = DeviceError::Record("cannot start an async runtime".to_owned());
                    let _ = events_to.send(Event::Failed(error));
                    return None;
                };
                runtime.block_on(async {
                    match fake {
                        Some(spec) => {
                            fake::session(&spec, &address, to_open, &events_to, stopped).await
                        }
                        None => phone::session(&address, to_open, &events_to, stopped).await,
                    }
                })
            })
            .map_err(|error| DeviceError::Record(error.to_string()))?;
        let phone = match events.recv_timeout(start_wait) {
            Ok(Event::Started(phone)) => phone,
            Ok(Event::Failed(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            // A phone that does not answer can hold the thread in a connect for minutes, so it
            // is left to end on its own.
            _ => {
                return Err(DeviceError::Connect(format!(
                    "it did not answer in {:.0} s",
                    start_wait.as_secs_f64()
                )));
            }
        };
        Ok(Recorder {
            stop: Some(stop),
            thread: Some(thread),
            phone,
            opened: None,
            open: Some(open),
            events,
            open_wait,
            xcrun,
            fake_tunnel,
        })
    }

    /// Opens `url` in the phone's Safari, which restarts Safari, and returns once Safari has
    /// answered. A locked phone is an error: the caller stops the recording. Any other answer of
    /// Safari's is in [`Recorder::opened`], and the recording stays on, because a person can
    /// still open the page.
    ///
    /// A recording opens one page. A second ask is an error, and so is an ask of a recording
    /// that has ended: neither opens anything, by the phone's own service or by devicectl, and
    /// neither changes what [`Recorder::opened`] says of the first.
    pub fn open(&mut self, url: &str) -> Result<(), DeviceError> {
        let asked = self
            .open
            .take()
            .is_some_and(|open| open.send(url.to_owned()).is_ok());
        if !asked {
            return Err(DeviceError::Open(
                "the recording has opened its one page, or has ended".to_owned(),
            ));
        }
        let answer = match self.events.recv_timeout(self.open_wait) {
            Ok(Event::Opened(opened)) => opened,
            _ => Err(DeviceError::Open(format!(
                "the phone did not answer in {:.0} s",
                self.open_wait.as_secs_f64()
            ))),
        };
        self.opened = Some(match answer {
            Ok(()) => Ok(OpenedWith::CoreDevice),
            Err(DeviceError::Locked) => return Err(DeviceError::Locked),
            Err(error) => self.open_with_devicectl(url, error),
        });
        Ok(())
    }

    /// This machine's address inside the device link's tunnel to the phone, where the phone's
    /// Safari reaches a server on this machine with no network path to it. Only a Mac has one,
    /// in its own tunnel to the phone, and `xcrun devicectl` finds it ([`crate::tunnel`]). The
    /// error says why there is none. A fake phone has the address its settings give.
    pub fn tunnel_host(&self) -> Result<IpAddr, NoTunnelAddress> {
        if let Some(host) = self.fake_tunnel {
            return Ok(host);
        }
        let xcrun = self
            .xcrun
            .as_deref()
            .ok_or(NoTunnelAddress::NoSystemTunnel)?;
        tunnel::host_address(xcrun, self.phone.udid.as_deref())
    }

    /// The phone being recorded.
    pub fn phone(&self) -> &Phone {
        &self.phone
    }

    /// Whether Safari opened the page, once the recorder was asked to open it.
    pub fn opened(&self) -> Option<&Result<OpenedWith, DeviceError>> {
        self.opened.as_ref()
    }

    /// Stops the recording and gives it back.
    pub fn stop(mut self) -> Result<Recording, DeviceError> {
        self.end()
            .ok_or_else(|| DeviceError::Record("the recording stopped by itself".to_owned()))
    }

    /// `xcrun devicectl` tries to open the page when the app service did not: on a Mac, or with
    /// `DYNO_XCRUN`. When it fails too, the app service's error stands, with devicectl's. A
    /// machine with no devicectl, or a phone with no UDID, keeps the app service's error.
    fn open_with_devicectl(
        &self,
        url: &str,
        error: DeviceError,
    ) -> Result<OpenedWith, DeviceError> {
        let (Some(xcrun), Some(udid)) = (&self.xcrun, &self.phone.udid) else {
            return Err(error);
        };
        devicectl::open_in_safari(xcrun, udid, url)
            .map(|()| OpenedWith::Devicectl)
            .map_err(|devicectl| match (error, devicectl) {
                (DeviceError::Open(first), DeviceError::Open(then)) => {
                    DeviceError::Open(format!("{first}; {then}"))
                }
                (error, _) => error,
            })
    }

    /// Tells the session to stop, and waits for its recording.
    fn end(&mut self) -> Option<Recording> {
        // A recording that has ended opens no page.
        self.open = None;
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.thread.take()?.join().ok().flatten()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.end();
    }
}

/// Says that the recording has started, and records until `stop`. Alongside, it waits for the URL
/// that `open` gives, has `launch` open it in Safari, once, and says what Safari answered.
pub(crate) async fn started<Launch: Future<Output = Result<(), DeviceError>>>(
    phone: Phone,
    samples: impl SampleFeed,
    lines: impl LineFeed,
    open: oneshot::Receiver<String>,
    launch: impl FnOnce(String) -> Launch,
    events: &Sender<Event>,
    stop: oneshot::Receiver<()>,
) -> Recording {
    let _ = events.send(Event::Started(phone.clone()));
    let opening = async {
        // A recorder that opens no page drops its end when it stops, and nothing opens.
        if let Ok(url) = open.await {
            let _ = events.send(Event::Opened(launch(url).await));
        }
    };
    record(phone, samples, lines, opening, stop).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(200);
    const LONG: Duration = Duration::from_secs(30);

    const PAGE: &str = "http://192.0.2.1:7380/";

    fn start(fake: &str, waits: [Duration; 2]) -> Result<Recorder, DeviceError> {
        let address = Address::Usbmuxd { udid: None };
        Recorder::start_with(&address, Some(fake.to_owned()), waits)
    }

    #[test]
    fn a_phone_that_never_starts_is_given_up_on() {
        let error = start("fail=hang", [SHORT, LONG]).err().unwrap();
        assert_eq!(
            error.to_string(),
            "cannot connect to the phone: it did not answer in 0 s. It needs iOS 17 or later, \
             Developer Mode on, and trust in this computer"
        );
    }

    #[test]
    fn the_recording_is_on_before_any_page_opens() {
        let folder = tempfile::tempdir().unwrap();
        let launched = folder.path().join("launched");
        let mut recorder =
            start(&format!("launched={}", launched.display()), [LONG, LONG]).unwrap();
        assert_eq!(recorder.phone().identifier, "iPhone15,3");
        assert!(recorder.opened().is_none());
        assert!(!launched.exists());
        recorder.open(PAGE).unwrap();
        assert!(matches!(
            recorder.opened(),
            Some(Ok(OpenedWith::CoreDevice))
        ));
        assert_eq!(std::fs::read_to_string(&launched).unwrap(), PAGE);
        assert_eq!(recorder.stop().unwrap().phone.identifier, "iPhone15,3");
    }

    #[test]
    fn a_recording_that_opens_no_page_still_stops_with_its_recording() {
        let recorder = start("", [LONG, LONG]).unwrap();
        assert_eq!(recorder.stop().unwrap().phone.identifier, "iPhone15,3");
    }

    /// A stub xcrun of this crate's tests (`tests/data`). Its launch makes a file at the path
    /// that the URL gives.
    #[cfg(unix)]
    fn stub_xcrun() -> OsString {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/stub-xcrun.sh")
            .into_os_string()
    }

    #[cfg(unix)]
    #[test]
    fn devicectl_opens_the_page_that_the_phone_s_own_service_did_not() {
        let folder = tempfile::tempdir().unwrap();
        let launched = folder.path().join("launched-by-devicectl");
        let mut recorder = start("fail=launch", [LONG, LONG]).unwrap();
        recorder.xcrun = Some(stub_xcrun());
        recorder.open(launched.to_str().unwrap()).unwrap();
        assert!(matches!(recorder.opened(), Some(Ok(OpenedWith::Devicectl))));
        assert!(launched.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_recording_opens_one_page_and_a_second_ask_opens_nothing() {
        let folder = tempfile::tempdir().unwrap();
        let launched = folder.path().join("launched");
        let by_devicectl = folder.path().join("launched-by-devicectl");
        let mut recorder =
            start(&format!("launched={}", launched.display()), [LONG, LONG]).unwrap();
        recorder.xcrun = Some(stub_xcrun());
        recorder.open(PAGE).unwrap();
        // The second ask is refused before the phone or devicectl is asked for anything.
        let error = recorder.open(by_devicectl.to_str().unwrap()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Safari did not open the page: the recording has opened its one page, or has ended"
        );
        assert_eq!(std::fs::read_to_string(&launched).unwrap(), PAGE);
        assert!(!by_devicectl.exists());
        // What Safari answered for the first page stands.
        assert!(matches!(
            recorder.opened(),
            Some(Ok(OpenedWith::CoreDevice))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_recording_that_has_ended_opens_nothing_with_devicectl_either() {
        let folder = tempfile::tempdir().unwrap();
        let launched = folder.path().join("launched-by-devicectl");
        let mut recorder = start("", [LONG, LONG]).unwrap();
        recorder.xcrun = Some(stub_xcrun());
        // The session ends, as when its thread has gone. The recorder is still held.
        assert!(recorder.end().is_some());
        let error = recorder.open(launched.to_str().unwrap()).unwrap_err();
        assert!(matches!(error, DeviceError::Open(_)), "{error}");
        assert!(recorder.opened().is_none());
        // devicectl was not run: its stub would have made the file.
        assert!(!launched.exists());
    }

    #[test]
    fn safari_that_never_answers_leaves_the_recording_on() {
        let mut recorder = start("fail=launch-hangs", [LONG, SHORT]).unwrap();
        recorder.open(PAGE).unwrap();
        let Some(Err(DeviceError::Open(why))) = recorder.opened() else {
            panic!("Safari did not time out");
        };
        assert_eq!(why, "the phone did not answer in 0 s");
        assert_eq!(recorder.phone().identifier, "iPhone15,3");
        let recording = recorder.stop().unwrap();
        assert_eq!(recording.phone.identifier, "iPhone15,3");
    }

    #[test]
    fn a_locked_phone_is_an_error_for_the_caller_to_stop_on() {
        let mut recorder = start("fail=locked", [LONG, LONG]).unwrap();
        let error = recorder.open(PAGE).unwrap_err();
        assert!(matches!(error, DeviceError::Locked), "{error}");
    }

    #[test]
    fn a_machine_with_no_devicectl_has_no_tunnel_address() {
        let mut recorder = start("", [LONG, LONG]).unwrap();
        // As on Linux, whatever `DYNO_XCRUN` names on the machine that runs this test.
        recorder.xcrun = None;
        assert_eq!(recorder.tunnel_host(), Err(NoTunnelAddress::NoSystemTunnel));
    }

    #[test]
    fn a_fake_phone_has_the_tunnel_address_its_settings_give() {
        let recorder = start("tunnel=fd7b:e5b:6f53::2", [LONG, LONG]).unwrap();
        assert_eq!(
            recorder.tunnel_host(),
            Ok("fd7b:e5b:6f53::2".parse().unwrap())
        );
    }
}
