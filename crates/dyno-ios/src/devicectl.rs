//! Apple's `xcrun devicectl`, run as a subprocess: whether a phone is locked, and opening a page
//! in its Safari.
//!
//! `DYNO_XCRUN` replaces `xcrun` here too.

use std::process::Stdio;

use crate::xctrace::{Device, DeviceError, xcrun};

/// Safari's bundle identifier.
const SAFARI: &str = "com.apple.mobilesafari";

/// The key of `devicectl device info lockState` that says whether the phone is locked.
const PASSCODE_REQUIRED: &str = "passcodeRequired: ";

/// Whether the device is locked, as `xcrun devicectl device info lockState` says. None when
/// devicectl cannot tell, such as for the Mac itself or with an older Xcode.
pub fn is_locked(device: &Device) -> Option<bool> {
    let output = xcrun()
        .args(["devicectl", "device", "info", "lockState", "--device"])
        .arg(&device.udid)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_lock_state(&String::from_utf8_lossy(&output.stdout))
}

/// The `passcodeRequired` value of a lockState listing: true when the phone is locked. The
/// listing puts a bullet before each key, so the key is found inside the line.
pub fn parse_lock_state(listing: &str) -> Option<bool> {
    listing.lines().find_map(|line| {
        let (_, value) = line.split_once(PASSCODE_REQUIRED)?;
        value.trim().parse().ok()
    })
}

/// Restarts Safari on the phone with `url` open, as `xcrun devicectl device process launch`
/// does.
pub fn open_in_safari(device: &Device, url: &str) -> Result<(), DeviceError> {
    let output = xcrun()
        .args(["devicectl", "device", "process", "launch", "--device"])
        .arg(&device.udid)
        .args(["--terminate-existing", "--payload-url", url, SAFARI])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| DeviceError::Open(error.to_string()))?;
    if !output.status.success() {
        // devicectl's errors take several lines, and the reason is printed on one.
        let said = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<&str> = said
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        return Err(DeviceError::Open(if lines.is_empty() {
            output.status.to_string()
        } else {
            lines.join(" ")
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_that_needs_its_passcode_is_locked() {
        let listing = "• passcodeRequired: true\n• unlockedSinceBoot: true\n";
        assert_eq!(parse_lock_state(listing), Some(true));
    }

    #[test]
    fn a_phone_that_needs_no_passcode_is_unlocked() {
        let listing = "Acquired tunnel connection to device.\n• passcodeRequired: false\n";
        assert_eq!(parse_lock_state(listing), Some(false));
    }

    #[test]
    fn a_listing_with_no_lock_state_tells_nothing() {
        assert_eq!(
            parse_lock_state("ERROR: The device is not available.\n"),
            None
        );
        assert_eq!(parse_lock_state("• passcodeRequired: maybe\n"), None);
    }
}
