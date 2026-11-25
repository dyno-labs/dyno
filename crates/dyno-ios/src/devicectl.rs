//! Apple's `xcrun devicectl`, run as a subprocess on a Mac: it opens the page in the phone's
//! Safari when the phone's app service did not, and it gives the phone's address in the Mac's own
//! tunnel to it ([`crate::tunnel`]).
//!
//! `DYNO_XCRUN` names another program to run instead of `xcrun`, such as a fake in the tests.
//!
//! dyno waits for the program itself, and stops it at a deadline of dyno's own. devicectl has a
//! timer, but a program that stalls before devicectl starts never reads it, and a device run
//! waits for this program while it records the phone.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::net::IpAddr;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::recorder::DeviceError;

/// The variable that names another program to run instead of `xcrun`.
pub const XCRUN_VARIABLE: &str = "DYNO_XCRUN";

/// Safari's bundle identifier.
const SAFARI: &str = "com.apple.mobilesafari";

/// How long devicectl may take to list the devices by its own timer, in seconds.
const LIST_TIMEOUT_S: &str = "10";

/// How long dyno waits for the program that lists the devices before it stops it: devicectl's
/// own time, and a little more to start and to write the list.
const LIST_DEADLINE: Duration = Duration::from_secs(15);

/// How long dyno waits for the program that launches Safari before it stops it.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);

/// How often dyno looks whether the program has ended.
const POLL: Duration = Duration::from_millis(20);

/// The program that runs `xcrun devicectl` for a recording: the one `DYNO_XCRUN` names, or
/// `xcrun` on a Mac. A fake phone has only the one that `DYNO_XCRUN` names. None on a machine
/// with no devicectl for the phone.
pub fn program(fake_phone: bool) -> Option<OsString> {
    std::env::var_os(XCRUN_VARIABLE)
        .or_else(|| (cfg!(target_os = "macos") && !fake_phone).then(|| OsString::from("xcrun")))
}

/// The address of the phone with this UDID inside the Mac's own tunnel to it, as
/// `xcrun devicectl list devices` gives it. An error says why there is none: devicectl does not
/// run or does not end, does not list the phone, or has no tunnel to it now.
///
/// devicectl writes its JSON to a file, and prints a table for people on stdout.
pub fn tunnel_address(xcrun: &OsStr, udid: &str) -> Result<IpAddr, String> {
    tunnel_address_within(xcrun, udid, LIST_DEADLINE)
}

/// [`tunnel_address`], with this long for the program to end.
fn tunnel_address_within(xcrun: &OsStr, udid: &str, deadline: Duration) -> Result<IpAddr, String> {
    let folder = tempfile::Builder::new()
        .prefix("dyno-devicectl-")
        .tempdir()
        .map_err(|error| format!("cannot make a folder for its device list: {error}"))?;
    let listing = folder.path().join("devices.json");
    let mut command = Command::new(xcrun);
    command
        .args(["devicectl", "list", "devices"])
        .args(["--timeout", LIST_TIMEOUT_S, "--json-output"])
        .arg(&listing);
    run_within(command, deadline)?;
    let listing = std::fs::read_to_string(&listing)
        .map_err(|error| format!("cannot read its device list: {error}"))?;
    tunnel_address_in(&listing, udid)
}

/// Runs the program to its end, and gives an error with what it said when it failed. A program
/// that has not ended at the deadline is killed and waited for, so none is left behind, and that
/// is an error too.
fn run_within(mut command: Command, deadline: Duration) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run devicectl: {error}"))?;
    // What the program says is read as it comes, so a program that says a lot does not wait for
    // a reader.
    let (said_to, said) = mpsc::channel();
    if let Some(mut stderr) = child.stderr.take() {
        thread::spawn(move || {
            let mut words = Vec::new();
            let _ = stderr.read_to_end(&mut words);
            let _ = said_to.send(words);
        });
    }
    let until = Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => thread::sleep(POLL),
            ended => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match ended {
                    Err(error) => format!("cannot wait for devicectl: {error}"),
                    Ok(_) => format!(
                        "devicectl did not end in {} s, so dyno stopped it",
                        deadline.as_secs_f64()
                    ),
                });
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    // The program has ended, so its words are all there, unless something it started still
    // holds its output open.
    let words = said
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    // devicectl's errors take several lines, and the reason is printed on one, so the lines
    // are joined.
    let words = String::from_utf8_lossy(&words);
    let lines: Vec<&str> = words
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    Err(format!(
        "devicectl: {}",
        if lines.is_empty() {
            status.to_string()
        } else {
            lines.join(" ")
        }
    ))
}

/// The tunnel address of the phone with this UDID in devicectl's device list: the
/// `connectionProperties.tunnelIPAddress` of the device whose `hardwareProperties.udid` it is. A
/// device has the address only while the Mac's tunnel to it is up.
fn tunnel_address_in(listing: &str, udid: &str) -> Result<IpAddr, String> {
    let listing: Value = serde_json::from_str(listing)
        .map_err(|error| format!("its device list is not JSON: {error}"))?;
    let devices = listing["result"]["devices"]
        .as_array()
        .ok_or("its device list has no devices")?;
    let is_the_phone = |device: &&Value| {
        device["hardwareProperties"]["udid"]
            .as_str()
            .is_some_and(|listed| listed.eq_ignore_ascii_case(udid))
    };
    let phone = devices
        .iter()
        .find(is_the_phone)
        .ok_or("it does not list the phone")?;
    let connection = &phone["connectionProperties"];
    let Some(address) = connection["tunnelIPAddress"].as_str() else {
        let state = connection["tunnelState"].as_str().unwrap_or("not given");
        return Err(format!(
            "this Mac has no tunnel to the phone now (tunnel state: {state})"
        ));
    };
    address
        .parse()
        .map_err(|_| format!("its tunnel address for the phone is not an address: {address:?}"))
}

/// Restarts Safari on the phone with `url` open, as `xcrun devicectl device process launch`
/// does.
pub fn open_in_safari(xcrun: &OsStr, udid: &str, url: &str) -> Result<(), DeviceError> {
    let mut command = Command::new(xcrun);
    command
        .args(["devicectl", "device", "process", "launch", "--device", udid])
        .args(["--terminate-existing", "--payload-url", url, SAFARI]);
    run_within(command, LAUNCH_DEADLINE).map_err(DeviceError::Open)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = "00008120-000000000000001E";

    /// A stub xcrun of this crate's tests (`tests/data`).
    #[cfg(unix)]
    fn stub(name: &str) -> OsString {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name)
            .into_os_string()
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_never_ends_is_stopped_at_dyno_s_own_deadline() {
        let started = Instant::now();
        let deadline = Duration::from_millis(300);
        let error = tunnel_address_within(&stub("stalled-xcrun.sh"), PHONE, deadline).unwrap_err();
        assert_eq!(error, "devicectl did not end in 0.3 s, so dyno stopped it");
        // The program sleeps for minutes. dyno did not wait for it.
        assert!(started.elapsed() >= deadline);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_is_stopped_is_waited_for_so_none_is_left_behind() {
        let mut command = Command::new(stub("stalled-xcrun.sh"));
        command.arg("devicectl");
        // The call returns only once the program is gone: it is killed, then waited for.
        let error = run_within(command, Duration::from_millis(100)).unwrap_err();
        assert!(
            error.starts_with("devicectl did not end in 0.1 s"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_lists_the_phone_gives_its_tunnel_address() {
        assert_eq!(
            tunnel_address(&stub("stub-xcrun.sh"), PHONE),
            Ok("fd7b:e5b:6f53::1".parse().unwrap())
        );
        assert_eq!(
            tunnel_address(&stub("stub-xcrun.sh"), "00008110-000000000000003E").unwrap_err(),
            "it does not list the phone"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_fails_gives_its_words_on_one_line() {
        assert_eq!(
            tunnel_address(&stub("failing-xcrun.sh"), PHONE).unwrap_err(),
            "devicectl: ERROR: devicectl failed for the test. The phone was not found."
        );
        let error = open_in_safari(&stub("failing-xcrun.sh"), PHONE, "http://192.0.2.1/");
        assert_eq!(
            error.unwrap_err().to_string(),
            "Safari did not open the page: devicectl: ERROR: devicectl failed for the test. The \
             phone was not found."
        );
    }

    #[test]
    fn a_program_that_is_not_there_cannot_run() {
        let error = tunnel_address(OsStr::new("no-such-xcrun-in-tests"), PHONE).unwrap_err();
        assert!(error.starts_with("cannot run devicectl: "), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_launch_runs_the_program_with_the_url() {
        let folder = tempfile::tempdir().unwrap();
        // The stub makes a file at the path its URL gives.
        let launched = folder.path().join("launched");
        open_in_safari(&stub("stub-xcrun.sh"), PHONE, launched.to_str().unwrap()).unwrap();
        assert!(launched.exists());
    }

    /// A device list as devicectl writes it, cut to the fields dyno reads, with one other device.
    fn listing(connection: &str) -> String {
        format!(
            r#"{{
              "info": {{ "outcome": "success", "jsonVersion": 2 }},
              "result": {{ "devices": [
                {{
                  "connectionProperties": {{ "tunnelState": "connected", "tunnelIPAddress": "fd00:1::1" }},
                  "hardwareProperties": {{ "udid": "00008030-000000000000002E" }}
                }},
                {{
                  "connectionProperties": {connection},
                  "hardwareProperties": {{ "udid": "{PHONE}" }}
                }}
              ] }}
            }}"#
        )
    }

    #[test]
    fn the_tunnel_address_is_the_one_of_the_phone_with_the_udid() {
        let connected = listing(
            r#"{ "tunnelState": "connected", "tunnelIPAddress": "fd7b:e5b:6f53::1", "transportType": "wired" }"#,
        );
        assert_eq!(
            tunnel_address_in(&connected, PHONE),
            Ok("fd7b:e5b:6f53::1".parse().unwrap())
        );
        // usbmuxd and devicectl do not always write a UDID's letters in the same case.
        assert_eq!(
            tunnel_address_in(&connected, &PHONE.to_lowercase()),
            Ok("fd7b:e5b:6f53::1".parse().unwrap())
        );
    }

    #[test]
    fn a_phone_with_no_tunnel_now_has_no_address_and_the_reason_gives_the_state() {
        let disconnected = listing(r#"{ "tunnelState": "disconnected" }"#);
        assert_eq!(
            tunnel_address_in(&disconnected, PHONE).unwrap_err(),
            "this Mac has no tunnel to the phone now (tunnel state: disconnected)"
        );
        let no_state = listing("{}");
        assert_eq!(
            tunnel_address_in(&no_state, PHONE).unwrap_err(),
            "this Mac has no tunnel to the phone now (tunnel state: not given)"
        );
    }

    #[test]
    fn a_phone_that_is_not_listed_has_no_address() {
        let connected = listing(r#"{ "tunnelIPAddress": "fd7b:e5b:6f53::1" }"#);
        assert_eq!(
            tunnel_address_in(&connected, "00008110-000000000000003E").unwrap_err(),
            "it does not list the phone"
        );
    }

    #[test]
    fn a_list_that_is_not_devicectl_s_gives_no_address() {
        assert!(
            tunnel_address_in("not json", PHONE)
                .unwrap_err()
                .starts_with("its device list is not JSON: ")
        );
        assert_eq!(
            tunnel_address_in("{}", PHONE).unwrap_err(),
            "its device list has no devices"
        );
        let not_an_address = listing(r#"{ "tunnelIPAddress": "phone.local" }"#);
        assert_eq!(
            tunnel_address_in(&not_an_address, PHONE).unwrap_err(),
            r#"its tunnel address for the phone is not an address: "phone.local""#
        );
    }
}
