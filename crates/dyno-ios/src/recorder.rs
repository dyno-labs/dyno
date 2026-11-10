//! A recording in progress, for a caller with no async runtime: a thread of its own runs the
//! phone's session ([`phone`] or, for tests, [`fake`]) on a tokio runtime, and says when the
//! recording has started and whether Safari opened the page.
//!
//! [`phone`]: crate::phone
//! [`fake`]: crate::fake

use std::future::Future;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::devicectl;
use crate::fake::{self, FAKE_PHONE_VARIABLE};
use crate::phone;
use crate::recording::{LineFeed, Phone, Recording, SampleFeed, record};

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
}

impl Recorder {
    /// Connects to the phone and starts recording it, then opens `open` in its Safari when
    /// given. It returns once the recording is on and Safari has answered. A locked phone stops
    /// the recording. `DYNO_FAKE_PHONE` records a fake phone instead.
    pub fn start(address: &Address, open: Option<&str>) -> Result<Recorder, DeviceError> {
        let fake = std::env::var(FAKE_PHONE_VARIABLE).ok();
        Recorder::start_with(address, open, fake, [START_WAIT, OPEN_WAIT])
    }

    /// [`Recorder::start`], with the fake phone's settings when `fake` gives them, and these
    /// waits for the start and for Safari.
    fn start_with(
        address: &Address,
        open: Option<&str>,
        fake: Option<String>,
        [start_wait, open_wait]: [Duration; 2],
    ) -> Result<Recorder, DeviceError> {
        let (events_to, events) = mpsc::channel();
        let (stop, stopped) = oneshot::channel();
        // A fake phone has no devicectl to fall back on, but the stub that `DYNO_XCRUN` names.
        let devicectl = std::env::var_os(devicectl::XCRUN_VARIABLE).is_some()
            || (cfg!(target_os = "macos") && fake.is_none());
        let address = address.clone();
        let url = open.map(str::to_owned);
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
                            fake::session(&spec, &address, url, &events_to, stopped).await
                        }
                        None => phone::session(&address, url, &events_to, stopped).await,
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
        let mut recorder = Recorder {
            stop: Some(stop),
            thread: Some(thread),
            phone,
            opened: None,
        };
        if let Some(url) = open {
            let opened = match events.recv_timeout(open_wait) {
                Ok(Event::Opened(opened)) => opened,
                _ => Err(DeviceError::Open(format!(
                    "the phone did not answer in {:.0} s",
                    open_wait.as_secs_f64()
                ))),
            };
            recorder.opened = Some(match opened {
                Ok(()) => Ok(OpenedWith::CoreDevice),
                Err(DeviceError::Locked) => return Err(DeviceError::Locked),
                Err(error) if devicectl => recorder.open_with_devicectl(url, error),
                Err(error) => Err(error),
            });
        }
        Ok(recorder)
    }

    /// The phone being recorded.
    pub fn phone(&self) -> &Phone {
        &self.phone
    }

    /// Whether Safari opened the page, when the recorder opened it.
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

/// Says that the recording has started, and records until `stop`, with `launch` opening Safari
/// alongside. It says what Safari answered.
pub(crate) async fn started(
    phone: Phone,
    samples: impl SampleFeed,
    lines: impl LineFeed,
    launch: Option<impl Future<Output = Result<(), DeviceError>>>,
    events: &Sender<Event>,
    stop: oneshot::Receiver<()>,
) -> Recording {
    let _ = events.send(Event::Started(phone.clone()));
    let opening = async {
        if let Some(launch) = launch {
            let _ = events.send(Event::Opened(launch.await));
        }
    };
    record(phone, samples, lines, opening, stop).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(200);
    const LONG: Duration = Duration::from_secs(30);

    fn start(
        fake: &str,
        open: Option<&str>,
        waits: [Duration; 2],
    ) -> Result<Recorder, DeviceError> {
        let address = Address::Usbmuxd { udid: None };
        Recorder::start_with(&address, open, Some(fake.to_owned()), waits)
    }

    #[test]
    fn a_phone_that_never_starts_is_given_up_on() {
        let error = start("fail=hang", None, [SHORT, LONG]).err().unwrap();
        assert_eq!(
            error.to_string(),
            "cannot connect to the phone: it did not answer in 0 s. It needs iOS 17 or later, \
             Developer Mode on, and trust in this computer"
        );
    }

    #[test]
    fn safari_that_never_answers_leaves_the_recording_on() {
        let recorder = start(
            "fail=launch-hangs",
            Some("http://192.0.2.1:7380/"),
            [LONG, SHORT],
        )
        .unwrap();
        let Some(Err(DeviceError::Open(why))) = recorder.opened() else {
            panic!("Safari did not time out");
        };
        assert_eq!(why, "the phone did not answer in 0 s");
        assert_eq!(recorder.phone().identifier, "iPhone15,3");
        let recording = recorder.stop().unwrap();
        assert_eq!(recording.phone.identifier, "iPhone15,3");
    }

    #[test]
    fn a_locked_phone_stops_the_recording() {
        match start("fail=locked", Some("http://192.0.2.1:7380/"), [LONG, LONG]) {
            Err(error) => assert!(matches!(error, DeviceError::Locked), "{error}"),
            Ok(recorder) => panic!("{:?}", recorder.opened()),
        }
    }
}
