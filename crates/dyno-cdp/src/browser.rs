//! Finds and launches Chromium for one run, with a fresh profile that goes when the run ends.
//!
//! Site isolation stays on, so the game frame is its own target. The device pixel ratio comes
//! from `--force-device-scale-factor`: a cross-site frame takes its ratio from the browser's
//! screen, never from the page's emulation (CDP feasibility test, check 4).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use dyno_schema::profile::HostProfile;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::CdpError;

/// The environment variable that names the Chromium binary to run.
pub const CHROMIUM_ENV: &str = "DYNO_CHROMIUM";

/// Where Chromium is found when `DYNO_CHROMIUM` is not set: names on the PATH, then the macOS
/// apps.
const CANDIDATES: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
];

/// The line Chromium writes on stderr when its DevTools endpoint is ready.
const LISTENING: &str = "DevTools listening on ";

/// How long Chromium has to write its DevTools endpoint.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// The flags of every run, from the CDP feasibility test. A fresh profile, no first-run pages and
/// no background traffic, so the only requests are the run's own.
const FLAGS: &[&str] = &[
    "--headless=new",
    "--remote-debugging-port=0",
    "--site-per-process",
    "--no-first-run",
    "--no-default-browser-check",
    "--use-mock-keychain",
    "--password-store=basic",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
    "--disable-default-apps",
    "--mute-audio",
];

/// A running Chromium. Dropping it kills the process and deletes its profile; `close` also waits
/// for the process to go, so the profile can be deleted for sure.
pub struct Browser {
    process: Child,
    web_socket_url: String,
    profile: TempDir,
}

/// How long Chromium has to exit after `Browser.close` before it is killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often, and how far apart, deleting the profile is tried: a helper process can still hold
/// a file for a moment after the browser exits.
const PROFILE_DELETE_TRIES: u32 = 10;
const PROFILE_DELETE_PAUSE: Duration = Duration::from_millis(100);

impl Browser {
    /// Launches Chromium with the profile's screen and waits for its DevTools endpoint.
    pub async fn launch(profile: &HostProfile) -> Result<Browser, CdpError> {
        let binary = find_chromium()?;
        let profile_folder = tempfile::Builder::new()
            .prefix("dyno-chromium-")
            .tempdir()
            .map_err(|error| CdpError::Launch(format!("no profile folder: {error}")))?;
        let mut process = Command::new(&binary)
            .args(FLAGS)
            .arg(format!(
                "--user-data-dir={}",
                profile_folder.path().display()
            ))
            .arg(format!(
                "--force-device-scale-factor={}",
                profile.device_scale_factor
            ))
            .arg(format!(
                "--window-size={},{}",
                profile.viewport.width, profile.viewport.height
            ))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| CdpError::Launch(format!("{}: {error}", binary.display())))?;
        let stderr = process
            .stderr
            .take()
            .ok_or_else(|| CdpError::Launch("no stderr".to_owned()))?;
        let web_socket_url = tokio::time::timeout(START_TIMEOUT, devtools_url(stderr))
            .await
            .map_err(|_| CdpError::Launch("Chromium did not start in time".to_owned()))??;
        Ok(Browser {
            process,
            web_socket_url,
            profile: profile_folder,
        })
    }

    pub fn web_socket_url(&self) -> &str {
        &self.web_socket_url
    }

    /// Waits for the process to exit after `Browser.close`, kills it when it does not, and
    /// deletes the profile folder, trying again while a file is still held.
    pub async fn close(mut self) {
        if tokio::time::timeout(EXIT_TIMEOUT, self.process.wait())
            .await
            .is_err()
        {
            // The process may have exited between the wait and the kill.
            let _ = self.process.kill().await;
        }
        let folder = self.profile.path().to_path_buf();
        for _ in 0..PROFILE_DELETE_TRIES {
            match std::fs::remove_dir_all(&folder) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(PROFILE_DELETE_PAUSE).await;
                }
                _ => break,
            }
        }
    }
}

/// The DevTools WebSocket URL from Chromium's stderr. The rest of stderr is drained in the
/// background, so Chromium never blocks on a full pipe.
async fn devtools_url(stderr: tokio::process::ChildStderr) -> Result<String, CdpError> {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(url) = line.split_once(LISTENING).map(|(_, url)| url.trim()) {
            let url = url.to_owned();
            tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
            return Ok(url);
        }
    }
    Err(CdpError::Launch(
        "Chromium exited before its DevTools endpoint was ready".to_owned(),
    ))
}

/// The Chromium to run: `DYNO_CHROMIUM`, else the first candidate found.
pub fn find_chromium() -> Result<PathBuf, CdpError> {
    if let Some(named) = env::var_os(CHROMIUM_ENV) {
        let named = PathBuf::from(named);
        return if named.is_file() {
            Ok(named)
        } else {
            Err(CdpError::NoChromium(format!(
                "{CHROMIUM_ENV} names {}, which is not a file",
                named.display()
            )))
        };
    }
    CANDIDATES
        .iter()
        .find_map(|candidate| on_path(Path::new(candidate)))
        .ok_or_else(|| {
            CdpError::NoChromium(format!(
                "no Chromium found; install it or set {CHROMIUM_ENV}"
            ))
        })
}

/// The candidate itself when it is a path to a file, else the first file of that name on the
/// PATH.
fn on_path(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_absolute() {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|folder| folder.join(candidate))
        .find(|binary| binary.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DevTools URL that a process gives on stderr, when it writes `script` there.
    #[cfg(unix)]
    async fn devtools_url_of(script: &str) -> Result<String, CdpError> {
        let mut child = Command::new("sh")
            .args(["-c", script])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let url = devtools_url(child.stderr.take().unwrap()).await;
        let _ = child.wait().await;
        url
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_devtools_url_is_read_from_its_line_on_stderr() {
        let url = devtools_url_of(
            "echo 'a warning' >&2; echo 'DevTools listening on ws://127.0.0.1:9/devtools/browser/x ' >&2",
        )
        .await;
        assert_eq!(url.unwrap(), "ws://127.0.0.1:9/devtools/browser/x");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_browser_that_exits_before_it_listens_is_a_launch_error() {
        let error = devtools_url_of("echo 'cannot open display' >&2")
            .await
            .unwrap_err();
        assert!(matches!(error, CdpError::Launch(_)), "{error}");
    }

    #[test]
    fn an_absolute_candidate_is_used_when_it_is_a_file() {
        let file = env::current_exe().unwrap();
        assert_eq!(on_path(&file), Some(file));
    }

    #[test]
    fn an_absolute_candidate_that_is_not_a_file_is_not_found() {
        let folder = env::temp_dir();
        assert_eq!(on_path(&folder), None);
        assert_eq!(on_path(&folder.join("no-such-chromium")), None);
    }
}
