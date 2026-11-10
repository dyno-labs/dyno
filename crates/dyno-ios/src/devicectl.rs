//! Apple's `xcrun devicectl`, run as a subprocess on a Mac: it opens the page in the phone's
//! Safari when the phone's app service did not.
//!
//! `DYNO_XCRUN` names another program to run instead of `xcrun`, such as a fake in the tests.

use std::ffi::OsString;
use std::process::{Command, Stdio};

use crate::recorder::DeviceError;

/// The variable that names another program to run instead of `xcrun`.
pub const XCRUN_VARIABLE: &str = "DYNO_XCRUN";

/// Safari's bundle identifier.
const SAFARI: &str = "com.apple.mobilesafari";

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
    // devicectl's errors take several lines, and the reason is printed on one.
    let said = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = said
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    Err(DeviceError::Open(format!(
        "devicectl: {}",
        if lines.is_empty() {
            output.status.to_string()
        } else {
            lines.join(" ")
        }
    )))
}
