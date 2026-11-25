//! Apple's `xcrun devicectl`, run as a subprocess on a Mac: it opens the page in the phone's
//! Safari when the phone's app service did not, and it gives the phone's address in the Mac's own
//! tunnel to it ([`crate::tunnel`]).
//!
//! `DYNO_XCRUN` names another program to run instead of `xcrun`, such as a fake in the tests.

use std::ffi::OsString;
use std::net::IpAddr;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::recorder::DeviceError;

/// The variable that names another program to run instead of `xcrun`.
pub const XCRUN_VARIABLE: &str = "DYNO_XCRUN";

/// Safari's bundle identifier.
const SAFARI: &str = "com.apple.mobilesafari";

/// How long devicectl may take to list the devices, in seconds. It has its own timer.
const LIST_TIMEOUT_S: &str = "10";

/// The address of the phone with this UDID inside the Mac's own tunnel to it, as
/// `xcrun devicectl list devices` gives it. An error says why there is none: devicectl does not
/// run, does not list the phone, or has no tunnel to it now.
///
/// devicectl writes its JSON to a file, and prints a table for people on stdout.
pub fn tunnel_address(udid: &str) -> Result<IpAddr, String> {
    let folder = tempfile::Builder::new()
        .prefix("dyno-devicectl-")
        .tempdir()
        .map_err(|error| format!("cannot make a folder for its device list: {error}"))?;
    let listing = folder.path().join("devices.json");
    let program = std::env::var_os(XCRUN_VARIABLE).unwrap_or_else(|| OsString::from("xcrun"));
    let output = Command::new(program)
        .args(["devicectl", "list", "devices"])
        .args(["--timeout", LIST_TIMEOUT_S, "--json-output"])
        .arg(&listing)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run devicectl: {error}"))?;
    if !output.status.success() {
        return Err(format!("devicectl: {}", said(&output)));
    }
    let listing = std::fs::read_to_string(&listing)
        .map_err(|error| format!("cannot read its device list: {error}"))?;
    tunnel_address_in(&listing, udid)
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
pub fn open_in_safari(udid: &str, url: &str) -> Result<(), DeviceError> {
    let program = std::env::var_os(XCRUN_VARIABLE).unwrap_or_else(|| OsString::from("xcrun"));
    let output = Command::new(program)
        .args(["devicectl", "device", "process", "launch", "--device", udid])
        .args(["--terminate-existing", "--payload-url", url, SAFARI])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| DeviceError::Open(format!("cannot run devicectl: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(DeviceError::Open(format!("devicectl: {}", said(&output))))
}

/// What a devicectl that failed said. Its errors take several lines, and the reason is printed on
/// one, so the lines are joined. With no words, its exit status.
fn said(output: &std::process::Output) -> String {
    let said = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = said
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        output.status.to_string()
    } else {
        lines.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = "00008120-000000000000001E";

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
