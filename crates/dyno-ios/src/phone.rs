//! The real phone, through the `idevice` crate: it chooses the phone, connects, and reads it.
//!
//! dyno reaches the phone through usbmuxd, which lists the phones on a cable (and on macOS the
//! ones on Wi-Fi too) and holds their pair records, or straight at its address with a pair record
//! from a file. Either way it opens the phone's CoreDevice tunnel, which iOS 17 and later have,
//! finds the services with RSD, and opens two in it: Instruments for the sysmontap samples, and
//! CoreDevice's app service to open Safari. The syslog relay, for the kernel's memory lines, it
//! opens through lockdown instead. Through the tunnel, the relay serves no one for about a
//! minute after a client goes, so a second run gets no lines. Through lockdown it serves the next
//! client at once, even after one that was killed.
//!
//! idevice's own sysmontap client keeps the first row of each message and drops the rest, so it
//! loses the processes. dyno reads the tap's channel itself ([`crate::sysmon`]).

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use idevice::core_device::CoreDeviceServiceClient;
use idevice::core_device_proxy::CoreDeviceProxy;
use idevice::dvt::device_info::DeviceInfoClient;
use idevice::dvt::message::AuxValue;
use idevice::dvt::remote_server::{Channel, RemoteServerClient};
use idevice::pairing_file::PairingFile;
use idevice::provider::{IdeviceProvider, RsdProvider, TcpProvider};
use idevice::rsd::RsdHandshake;
use idevice::syslog_relay::SyslogRelayClient;
use idevice::tcp::handle::AdapterHandle;
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdDevice};
use idevice::{IdeviceError, IdeviceService, ReadWrite, RsdService};
use plist::{Dictionary, Value};
use tokio::sync::oneshot;

use crate::recorder::{Address, DeviceError, Event, started};
use crate::recording::{LineFeed, Link, Phone, Recording, SampleFeed};
use crate::sysmon::{self, Columns, Reading};

/// What dyno calls itself to the phone.
const LABEL: &str = "dyno";

/// Safari's bundle identifier.
const SAFARI: &str = "com.apple.mobilesafari";

/// The CoreDevice service that launches apps, and its feature that does.
const APP_SERVICE: &str = "com.apple.coredevice.appservice";
const LAUNCH: &str = "com.apple.coredevice.feature.launchapplication";

type Stream = Box<dyn ReadWrite>;

/// The phone's tunnel and its services.
struct Tunnel {
    provider: Box<dyn IdeviceProvider>,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    phone: Phone,
}

/// Everything a recording reads, opened.
struct Opened {
    tunnel: Tunnel,
    instruments: RemoteServerClient<Stream>,
    columns: Columns,
    config: Dictionary,
    log: SyslogRelayClient,
}

/// Records the phone at `address` until `stop`, and opens `open` in its Safari once the recording
/// is on. It says on `events` when the recording starts, or why it did not. usbmuxd is where
/// `USBMUXD_SOCKET_ADDRESS` says, or in its usual place.
pub(crate) async fn session(
    address: &Address,
    open: Option<String>,
    events: &Sender<Event>,
    stop: oneshot::Receiver<()>,
) -> Option<Recording> {
    let usbmuxd = UsbmuxdAddr::from_env_var().unwrap_or_default();
    session_through(&usbmuxd, address, open, events, stop).await
}

/// [`session`], with usbmuxd at this address.
async fn session_through(
    usbmuxd: &UsbmuxdAddr,
    address: &Address,
    open: Option<String>,
    events: &Sender<Event>,
    stop: oneshot::Receiver<()>,
) -> Option<Recording> {
    let failed = |error| {
        let _ = events.send(Event::Failed(error));
        None
    };
    let Opened {
        mut tunnel,
        mut instruments,
        columns,
        config,
        log,
    } = match open_services(usbmuxd, address).await {
        Ok(opened) => opened,
        Err(error) => return failed(error),
    };
    let channel = match start_tap(&mut instruments, config).await {
        Ok(channel) => channel,
        Err(error) => return failed(DeviceError::Record(error.to_string())),
    };
    let phone = tunnel.phone.clone();
    let app_service = tunnel.handshake.services.get(APP_SERVICE).map(|s| s.port);
    let launch = open.map(|url| launch_safari(&mut tunnel.adapter, app_service, url));
    let samples = Tap { channel, columns };
    Some(started(phone, samples, Log(log), launch, events, stop).await)
}

async fn open_services(usbmuxd: &UsbmuxdAddr, address: &Address) -> Result<Opened, DeviceError> {
    let failed = |error: IdeviceError| DeviceError::Record(error.to_string());
    let mut tunnel = connect(usbmuxd, address).await?;
    let mut instruments =
        RemoteServerClient::connect_rsd(&mut tunnel.adapter, &mut tunnel.handshake)
            .await
            .map_err(failed)?;
    let (process, system) = {
        let mut info = DeviceInfoClient::new(&mut instruments)
            .await
            .map_err(failed)?;
        let process = info.sysmon_process_attributes().await.map_err(failed)?;
        (
            process,
            info.sysmon_system_attributes().await.map_err(failed)?,
        )
    };
    let columns = Columns::find(&process, &system).map_err(DeviceError::Record)?;
    let log = SyslogRelayClient::connect(&*tunnel.provider)
        .await
        .map_err(failed)?;
    Ok(Opened {
        tunnel,
        instruments,
        columns,
        config: sysmon::config(&process, &system),
        log,
    })
}

/// Opens the phone's CoreDevice tunnel and reads what the phone is.
async fn connect(usbmuxd: &UsbmuxdAddr, address: &Address) -> Result<Tunnel, DeviceError> {
    let (provider, link): (Box<dyn IdeviceProvider>, Link) = match address {
        Address::Usbmuxd { udid } => {
            let unreachable = |error: IdeviceError| DeviceError::NoUsbmuxd(error.to_string());
            let mut connection = usbmuxd.connect(0).await.map_err(unreachable)?;
            let devices = connection.get_devices().await.map_err(unreachable)?;
            let device = choose(&devices, udid.as_deref())?;
            let link = match device.connection_type {
                Connection::Usb => Link::Usb,
                _ => Link::Network,
            };
            (Box::new(device.to_provider(usbmuxd.clone(), LABEL)), link)
        }
        Address::Network { ip, pairing_file } => {
            let pairing = PairingFile::read_from_file(pairing_file).map_err(|error| {
                DeviceError::PairingFile(pairing_file.display().to_string(), error.to_string())
            })?;
            let provider = TcpProvider {
                addr: *ip,
                scope_id: None,
                pairing_file: pairing,
                label: LABEL.to_owned(),
            };
            (Box::new(provider), Link::Network)
        }
    };
    let failed = |error: IdeviceError| DeviceError::Connect(error.to_string());
    let proxy = CoreDeviceProxy::connect(&*provider).await.map_err(failed)?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let mut adapter = proxy
        .create_software_tunnel()
        .map_err(failed)?
        .to_async_handle();
    let stream = adapter
        .connect_to_service_port(rsd_port)
        .await
        .map_err(failed)?;
    let handshake = RsdHandshake::new(stream).await.map_err(failed)?;
    let phone = phone_of(&handshake.properties, link);
    Ok(Tunnel {
        provider,
        adapter,
        handshake,
        phone,
    })
}

/// Starts the tap: every attribute, as the phone needs, a sample each second.
async fn start_tap(
    instruments: &mut RemoteServerClient<Stream>,
    config: Dictionary,
) -> Result<Channel<'_, Stream>, IdeviceError> {
    let mut channel = instruments.make_channel(sysmon::SERVICE).await?;
    let config = AuxValue::archived_value(Value::Dictionary(config));
    channel
        .call_method(Some("setConfig:"), Some(vec![config]), false)
        .await?;
    channel.call_method(Some("start"), None, false).await?;
    Ok(channel)
}

/// Asks the phone's app service to open `url` in a fresh Safari, as `devicectl device process
/// launch --terminate-existing --payload-url` does.
async fn launch_safari(
    adapter: &mut AdapterHandle,
    app_service: Option<u16>,
    url: String,
) -> Result<(), DeviceError> {
    let port = app_service
        .ok_or_else(|| DeviceError::Open("the phone lists no CoreDevice app service".to_owned()))?;
    let refused = |error: IdeviceError| refusal(&error.to_string());
    let stream = adapter
        .connect_to_service_port(port)
        .await
        .map_err(refused)?;
    let mut service = CoreDeviceServiceClient::new(stream)
        .await
        .map_err(refused)?;
    service
        .invoke_with_plist(LAUNCH, launch_request(&url))
        .await
        .map(drop)
        .map_err(refused)
}

/// The samples, from the tap's channel.
struct Tap<'a> {
    channel: Channel<'a, Stream>,
    columns: Columns,
}

impl SampleFeed for Tap<'_> {
    async fn next(&mut self) -> Result<Option<Reading>, String> {
        let message = self
            .channel
            .read_message()
            .await
            .map_err(|e| e.to_string())?;
        Ok(message
            .data
            .and_then(|data| sysmon::read(&data, &self.columns)))
    }
}

/// The lines of the phone's log.
struct Log(SyslogRelayClient);

impl LineFeed for Log {
    async fn next(&mut self) -> Result<String, String> {
        self.0.next().await.map_err(|error| error.to_string())
    }
}

/// The phone to record: the one with this UDID, or else the only one. usbmuxd lists a phone
/// twice when it is on a cable and on Wi-Fi, and the cable is the one to use.
fn choose<'d>(
    devices: &'d [UsbmuxdDevice],
    udid: Option<&str>,
) -> Result<&'d UsbmuxdDevice, DeviceError> {
    let mut phones: Vec<&UsbmuxdDevice> = Vec::new();
    for device in devices {
        match phones.iter_mut().find(|phone| phone.udid == device.udid) {
            Some(kept) if matches!(device.connection_type, Connection::Usb) => *kept = device,
            Some(_) => {}
            None => phones.push(device),
        }
    }
    let listed = || {
        let phones: Vec<String> = phones
            .iter()
            .map(|phone| match phone.connection_type {
                Connection::Usb => format!("{} on USB", phone.udid),
                _ => format!("{} on the network", phone.udid),
            })
            .collect();
        phones.join(", ")
    };
    if let Some(udid) = udid {
        return phones
            .iter()
            .find(|phone| phone.udid == udid)
            .copied()
            .ok_or_else(|| DeviceError::UnknownDevice(udid.to_owned(), listed()));
    }
    match phones.as_slice() {
        [phone] => Ok(phone),
        [] => Err(DeviceError::NoDevice),
        several => Err(DeviceError::SeveralDevices(several.len(), listed())),
    }
}

/// What the phone is, from the properties of its RSD handshake: its model identifier, its iOS
/// version with the build, and its UDID.
fn phone_of(properties: &HashMap<String, Value>, link: Link) -> Phone {
    let text = |key: &str| {
        properties
            .get(key)
            .and_then(Value::as_string)
            .map_or("", str::trim)
    };
    let os_version = match (text("OSVersion"), text("BuildVersion")) {
        ("", _) => String::new(),
        (version, "") => version.to_owned(),
        (version, build) => format!("{version} ({build})"),
    };
    Phone {
        identifier: text("ProductType").to_owned(),
        os_version,
        link,
        udid: Some(text("UniqueDeviceID"))
            .filter(|udid| !udid.is_empty())
            .map(str::to_owned),
    }
}

/// The app service's request to open `url` in a fresh Safari. A URL goes as a dictionary with
/// the URL as `relative`, the way Swift encodes one: the phone refuses a string.
fn launch_request(url: &str) -> Dictionary {
    let dictionary = |entries: Vec<(&str, Value)>| {
        Value::Dictionary(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    };
    let mut no_options = Vec::new();
    // Writing an empty dictionary to a Vec cannot fail.
    let _ = plist::to_writer_xml(&mut no_options, &Dictionary::new());
    let request = dictionary(vec![
        (
            "applicationSpecifier",
            dictionary(vec![(
                "bundleIdentifier",
                dictionary(vec![("_0", Value::String(SAFARI.to_owned()))]),
            )]),
        ),
        (
            "options",
            dictionary(vec![
                ("arguments", Value::Array(Vec::new())),
                ("environmentVariables", dictionary(Vec::new())),
                ("standardIOUsesPseudoterminals", Value::Boolean(true)),
                ("startStopped", Value::Boolean(false)),
                ("terminateExisting", Value::Boolean(true)),
                ("user", dictionary(vec![("active", Value::Boolean(true))])),
                (
                    "payloadURL",
                    dictionary(vec![("relative", Value::String(url.to_owned()))]),
                ),
                ("platformSpecificOptions", Value::Data(no_options)),
            ]),
        ),
        ("standardIOIdentifiers", dictionary(Vec::new())),
    ]);
    request.into_dictionary().unwrap_or_default()
}

/// Why the phone did not open Safari, from its error. A locked phone says `Locked`.
fn refusal(said: &str) -> DeviceError {
    if said.contains("Locked") || said.contains("could not be, unlocked") {
        return DeviceError::Locked;
    }
    DeviceError::Open(reason(said))
}

/// The phone's own words in a CoreDevice error, which comes as the text of its NSError: the
/// failure reason, the description or the debug description, or else the whole text.
fn reason(said: &str) -> String {
    for key in [
        "NSLocalizedFailureReason",
        "NSLocalizedDescription",
        "NSDebugDescription",
    ] {
        let words = said
            .split_once(&format!("\"{key}\": String(\""))
            .and_then(|(_, rest)| rest.split_once("\")"));
        if let Some((words, _)) = words {
            return words.to_owned();
        }
    }
    said.to_owned()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{IpAddr, Ipv4Addr};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;

    use super::*;

    /// A usbmuxd for tests, on a socket in `folder`. It lists these phones, (UDID, connection
    /// type), and refuses every connection to one.
    fn fake_usbmuxd(folder: &Path, phones: &'static [(&'static str, &'static str)]) -> UsbmuxdAddr {
        let path: PathBuf = folder.join("usbmuxd");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            for mut client in listener.incoming().map_while(Result::ok) {
                let mut header = [0; 16];
                if client.read_exact(&mut header).is_err() {
                    continue;
                }
                let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize - 16;
                let mut body = vec![0; size];
                client.read_exact(&mut body).unwrap();
                let request: Dictionary = plist::from_bytes(&body).unwrap();
                let mut answer = Dictionary::new();
                match request["MessageType"].as_string() {
                    Some("ListDevices") => {
                        let list = phones
                            .iter()
                            .enumerate()
                            .map(|(at, (udid, connection))| {
                                let mut properties = Dictionary::new();
                                properties
                                    .insert("ConnectionType".to_owned(), (*connection).into());
                                properties.insert("SerialNumber".to_owned(), (*udid).into());
                                let mut device = Dictionary::new();
                                device.insert("DeviceID".to_owned(), (at as u64 + 1).into());
                                device.insert("Properties".to_owned(), properties.into());
                                Value::Dictionary(device)
                            })
                            .collect();
                        answer.insert("DeviceList".to_owned(), Value::Array(list));
                    }
                    // Connection refused.
                    _ => {
                        answer.insert("MessageType".to_owned(), "Result".into());
                        answer.insert("Number".to_owned(), 3.into());
                    }
                }
                let mut xml = Vec::new();
                plist::to_writer_xml(&mut xml, &answer).unwrap();
                let mut packet = Vec::new();
                for word in [
                    xml.len() as u32 + 16,
                    1,
                    8,
                    u32::from_le_bytes(header[12..].try_into().unwrap()),
                ] {
                    packet.extend(word.to_le_bytes());
                }
                packet.extend(xml);
                let _ = client.write_all(&packet);
            }
        });
        UsbmuxdAddr::UnixSocket(path.display().to_string())
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_phone_that_usbmuxd_lists_but_that_does_not_answer_does_not_start() {
        let folder = tempfile::tempdir().unwrap();
        let usbmuxd = fake_usbmuxd(folder.path(), &[(PHONE, "USB")]);
        let (events_to, events) = mpsc::channel();
        let (_stop, stop) = oneshot::channel();
        let address = Address::Usbmuxd { udid: None };
        let recording = block_on(session_through(&usbmuxd, &address, None, &events_to, stop));
        assert!(recording.is_none());
        let Ok(Event::Failed(error)) = events.try_recv() else {
            panic!("the session did not say it failed");
        };
        assert!(matches!(error, DeviceError::Connect(_)), "{error}");
    }

    #[test]
    fn usbmuxd_that_lists_no_phone_or_is_not_there_needs_the_person() {
        let folder = tempfile::tempdir().unwrap();
        let usbmuxd = fake_usbmuxd(folder.path(), &[]);
        let address = Address::Usbmuxd { udid: None };
        let error = block_on(connect(&usbmuxd, &address)).err().unwrap();
        assert!(matches!(error, DeviceError::NoDevice), "{error}");
        let nowhere = UsbmuxdAddr::UnixSocket(folder.path().join("none").display().to_string());
        let error = block_on(connect(&nowhere, &address)).err().unwrap();
        assert!(matches!(error, DeviceError::NoUsbmuxd(_)), "{error}");
    }

    #[test]
    fn a_phone_that_does_not_answer_at_its_address_does_not_connect() {
        let folder = tempfile::tempdir().unwrap();
        let pairing_file = folder.path().join("phone.plist");
        // Not a real pair record: its certificates only need to be PEM to be read.
        let pem =
            Value::Data(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_vec());
        let mut record = Dictionary::new();
        for key in [
            "DeviceCertificate",
            "HostCertificate",
            "RootCertificate",
            "HostPrivateKey",
            "RootPrivateKey",
        ] {
            record.insert(key.to_owned(), pem.clone());
        }
        for key in ["SystemBUID", "HostID", "WiFiMACAddress"] {
            record.insert(key.to_owned(), "test".into());
        }
        plist::to_file_xml(&pairing_file, &record).unwrap();
        // Nothing listens for lockdown on this machine.
        let address = Address::Network {
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            pairing_file,
        };
        let error = block_on(connect(&UsbmuxdAddr::default(), &address))
            .err()
            .unwrap();
        assert!(matches!(error, DeviceError::Connect(_)), "{error}");
    }

    #[test]
    fn a_pairing_file_that_cannot_be_read_names_the_file() {
        let folder = tempfile::tempdir().unwrap();
        let pairing_file = folder.path().join("phone.plist");
        let address = Address::Network {
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            pairing_file: pairing_file.clone(),
        };
        let usbmuxd = UsbmuxdAddr::default();
        let error = block_on(connect(&usbmuxd, &address)).err().unwrap();
        let DeviceError::PairingFile(file, _) = error else {
            panic!("{error}");
        };
        assert_eq!(file, pairing_file.display().to_string());
    }

    fn device(udid: &str, connection_type: Connection) -> UsbmuxdDevice {
        UsbmuxdDevice {
            connection_type,
            udid: udid.to_owned(),
            device_id: 1,
        }
    }

    const PHONE: &str = "00008120-000000000000001E";
    const OTHER: &str = "00008030-000000000000002E";

    fn wifi() -> Connection {
        Connection::Network(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 4)))
    }

    #[test]
    fn the_only_phone_is_chosen_on_its_cable_when_it_is_also_on_wifi() {
        let devices = [device(PHONE, wifi()), device(PHONE, Connection::Usb)];
        let chosen = choose(&devices, None).unwrap();
        assert!(matches!(chosen.connection_type, Connection::Usb));
        let devices = [device(PHONE, Connection::Usb), device(PHONE, wifi())];
        let chosen = choose(&devices, None).unwrap();
        assert!(matches!(chosen.connection_type, Connection::Usb));
    }

    #[test]
    fn a_phone_is_chosen_by_its_udid() {
        let devices = [device(PHONE, Connection::Usb), device(OTHER, wifi())];
        assert_eq!(choose(&devices, Some(OTHER)).unwrap().udid, OTHER);
        let error = choose(&devices, Some("nope")).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "no connected iPhone has the UDID nope. Connected: {PHONE} on USB, {OTHER} on the network"
            )
        );
    }

    #[test]
    fn no_phone_or_several_phones_need_the_person() {
        assert!(matches!(choose(&[], None), Err(DeviceError::NoDevice)));
        let devices = [device(PHONE, Connection::Usb), device(OTHER, wifi())];
        assert!(matches!(
            choose(&devices, None),
            Err(DeviceError::SeveralDevices(2, _))
        ));
    }

    #[test]
    fn the_handshake_gives_the_model_the_version_with_its_build_and_the_udid() {
        let properties: HashMap<String, Value> = [
            ("ProductType", "iPhone15,3"),
            ("OSVersion", "26.6.2"),
            ("BuildVersion", "23G90"),
            ("UniqueDeviceID", PHONE),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), Value::String(value.to_owned())))
        .collect();
        let phone = phone_of(&properties, Link::Usb);
        assert_eq!(phone.identifier, "iPhone15,3");
        assert_eq!(phone.os_version, "26.6.2 (23G90)");
        assert_eq!(phone.udid.as_deref(), Some(PHONE));
        assert_eq!(phone.link, Link::Usb);
        let mut no_build = properties.clone();
        no_build.remove("BuildVersion");
        no_build.remove("UniqueDeviceID");
        let phone = phone_of(&no_build, Link::Network);
        assert_eq!(phone.os_version, "26.6.2");
        assert_eq!(phone.udid, None);
        assert_eq!(phone_of(&HashMap::new(), Link::Network).os_version, "");
    }

    #[test]
    fn the_launch_opens_the_url_in_a_fresh_safari_with_the_url_as_a_dictionary() {
        let request = launch_request("http://192.0.2.1:7380/");
        let bundle = &request["applicationSpecifier"].as_dictionary().unwrap()["bundleIdentifier"];
        assert_eq!(
            bundle.as_dictionary().unwrap()["_0"].as_string(),
            Some(SAFARI)
        );
        let options = request["options"].as_dictionary().unwrap();
        assert_eq!(options["terminateExisting"].as_boolean(), Some(true));
        assert_eq!(
            options["payloadURL"].as_dictionary().unwrap()["relative"].as_string(),
            Some("http://192.0.2.1:7380/")
        );
        assert!(options["platformSpecificOptions"].as_data().is_some());
    }

    #[test]
    fn a_refusal_gives_the_phone_s_reason_and_a_locked_phone_is_locked() {
        let said = r#"device returned an error: Dictionary({"code": Integer(4864), "userInfo": Dictionary({"NSDebugDescription": String("dictionary required here")})})"#;
        assert_eq!(
            refusal(said).to_string(),
            "Safari did not open the page: dictionary required here"
        );
        let locked = r#"Dictionary({"userInfo": Dictionary({"NSLocalizedFailureReason": String("Unable to launch com.apple.mobilesafari because the device was not, or could not be, unlocked."), "BSErrorCodeDescription": String("Locked")})})"#;
        assert!(matches!(refusal(locked), DeviceError::Locked));
        assert_eq!(reason("no words here"), "no words here");
    }
}
