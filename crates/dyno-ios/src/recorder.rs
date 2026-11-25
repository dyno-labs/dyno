//! A recording in progress, for a caller with no async runtime: a thread of its own runs the
//! phone's session ([`phone`] or, for tests, [`fake`]) on a tokio runtime, and says when the
//! recording has started and whether Safari opened the page.
//!
//! The recording starts first, and Safari opens the page after, in a step of its own. Between the
//! two the caller knows the phone, and can ask for this machine's address in the device link's
//! tunnel ([`Recorder::tunnel_host`]) to serve the page there.
//!
//! [`phone`]: crate::phone
//! [`fake`]: crate::fake

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
    /// Where the session waits for the page to open in Safari. None once it has one.
    open: Option<oneshot::Sender<String>>,
    events: Receiver<Event>,
    open_wait: Duration,
    /// Whether this machine has `xcrun devicectl` for this phone: a Mac's, or the stub that
    /// `DYNO_XCRUN` names.
    devicectl: bool,
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
        let devicectl = std::env::var_os(devicectl::XCRUN_VARIABLE).is_some()
            || (cfg!(target_os = "macos") && fake.is_none());
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
            devicectl,
        })
    }

    /// Opens `url` in the phone's Safari, and returns once Safari has answered. A recording opens
    /// one page. A locked phone is the only error: the caller stops the recording. Any other
    /// answer is in [`Recorder::opened`], and the recording stays on, because a person can still
    /// open the page.
    pub fn open(&mut self, url: &str) -> Result<(), DeviceError> {
        let asked = self
            .open
            .take()
            .is_some_and(|open| open.send(url.to_owned()).is_ok());
        let answer = if asked {
            match self.events.recv_timeout(self.open_wait) {
                Ok(Event::Opened(opened)) => opened,
                _ => Err(DeviceError::Open(format!(
                    "the phone did not answer in {:.0} s",
                    self.open_wait.as_secs_f64()
                ))),
            }
        } else {
            Err(DeviceError::Open(
                "the recording has ended, or has opened its page".to_owned(),
            ))
        };
        self.opened = Some(match answer {
            Ok(()) => Ok(OpenedWith::CoreDevice),
            Err(DeviceError::Locked) => return Err(DeviceError::Locked),
            Err(error) if self.devicectl => self.open_with_devicectl(url, error),
            Err(error) => Err(error),
        });
        Ok(())
    }

    /// This machine's address inside the device link's tunnel to the phone, where the phone's
    /// Safari reaches a server on this machine with no network path to it. Only a Mac has one,
    /// in its own tunnel to the phone, and `xcrun devicectl` finds it ([`crate::tunnel`]). The
    /// error says why there is none.
    pub fn tunnel_host(&self) -> Result<IpAddr, NoTunnelAddress> {
        if !self.devicectl {
            return Err(NoTunnelAddress::NoSystemTunnel);
        }
        tunnel::host_address(self.phone.udid.as_deref())
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
    /// `DYNO_XCRUN`. When it fails too, the app service's error stands, with devicectl's.
    fn open_with_devicectl(
        &self,
        url: &str,
        error: DeviceError,
    ) -> Result<OpenedWith, DeviceError> {
        match &self.phone.udid {
            Some(udid) => devicectl::open_in_safari(udid, url)
                .map(|()| OpenedWith::Devicectl)
                .map_err(|devicectl| match (error, devicectl) {
                    (DeviceError::Open(first), DeviceError::Open(then)) => {
                        DeviceError::Open(format!("{first}; {then}"))
                    }
                    (error, _) => error,
                }),
            None => Err(error),
        }
    }

    /// Tells the session to stop, and waits for its recording.
    fn end(&mut self) -> Option<Recording> {
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
/// that `open` gives, has `launch` open it in Safari, and says what Safari answered.
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

    #[test]
    fn a_recording_opens_one_page() {
        let mut recorder = start("", [LONG, LONG]).unwrap();
        recorder.open(PAGE).unwrap();
        recorder.open(PAGE).unwrap();
        let Some(Err(DeviceError::Open(why))) = recorder.opened() else {
            panic!("the second page opened: {:?}", recorder.opened());
        };
        assert_eq!(why, "the recording has ended, or has opened its page");
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
        // The fake phone has no devicectl unless `DYNO_XCRUN` names one, which no test here sets.
        let recorder = start("", [LONG, LONG]).unwrap();
        if std::env::var_os(devicectl::XCRUN_VARIABLE).is_none() {
            assert_eq!(recorder.tunnel_host(), Err(NoTunnelAddress::NoSystemTunnel));
        }
    }
}
