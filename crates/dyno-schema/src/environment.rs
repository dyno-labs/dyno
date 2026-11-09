//! What actually ran a check: the browser, the OS and the device. It is `run.environment`,
//! outside the hash.
//!
//! Each driver fills in what it can see. The CDP driver knows its Chromium and this machine. The
//! served run reads the user agent that the harness page sends. A device run reads the model, the
//! iOS version and the memory from its trace, and they win over the user agent. The exec driver
//! gives only the browser that its `hello` names.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What ran a check. A field is null when the driver does not say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Environment {
    pub browser: Option<EnvironmentBrowser>,
    pub os: Option<EnvironmentOs>,
    /// Null for a desktop browser.
    pub device: Option<EnvironmentDevice>,
}

/// The browser that loaded the game.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnvironmentBrowser {
    /// Such as "Chromium" or "Safari".
    pub name: String,
    /// Such as "152.0.7977.82". Null when the driver does not say.
    pub version: Option<String>,
    /// Null when dyno does not know the browser's engine.
    pub engine: Option<BrowserEngine>,
    /// Whether the browser ran with no window. Null when the driver does not say.
    pub headless: Option<bool>,
}

/// A browser engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEngine {
    /// Chromium and the browsers built on it.
    Blink,
    /// Safari, and every browser on iOS.
    Webkit,
    /// Firefox.
    Gecko,
}

/// The operating system the browser ran on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnvironmentOs {
    pub name: OsName,
    /// Such as "26.6.2". Null when the source does not give it. A user agent's OS version is
    /// frozen, as Safari on iOS 26 says iOS 18.6, so dyno never reads it from there.
    pub version: Option<String>,
}

/// An operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum OsName {
    Linux,
    Macos,
    Windows,
    Ios,
    Ipados,
    Android,
}

/// The device the browser ran on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnvironmentDevice {
    /// The device catalogue's key, such as "iphone-14-pro-max": the model name in lower case,
    /// with one `-` for each run of other characters. A model the catalogue does not list keeps
    /// its own key. Null when the model is not known, as from a user agent, which names only
    /// "iPhone".
    pub key: Option<String>,
    /// The model as the trace names it, such as "iPhone 14 Pro Max", or the kind of device a user
    /// agent names, such as "iPhone". Never the device's own name, which is often its owner's.
    pub name: String,
    /// Such as "Apple".
    pub vendor: String,
    pub kind: DeviceKind,
    /// The device's memory, from a device run's trace. Null otherwise.
    pub memory_bytes: Option<u64>,
}

/// A kind of device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Phone,
    Tablet,
    Desktop,
}

impl OsName {
    /// The OS this dyno runs on. None for an OS the result has no name for.
    pub fn of_this_machine() -> Option<OsName> {
        match std::env::consts::OS {
            "linux" => Some(OsName::Linux),
            "macos" => Some(OsName::Macos),
            "windows" => Some(OsName::Windows),
            _ => None,
        }
    }
}

impl BrowserEngine {
    /// The engine of a browser by its name, such as "Chromium" or "Safari". None for a name dyno
    /// does not know.
    pub fn of_browser(name: &str) -> Option<BrowserEngine> {
        let lower = name.to_ascii_lowercase();
        if lower.contains("chrom") || lower.contains("edge") {
            Some(BrowserEngine::Blink)
        } else if lower.contains("safari") || lower.contains("webkit") {
            Some(BrowserEngine::Webkit)
        } else if lower.contains("firefox") || lower.contains("gecko") {
            Some(BrowserEngine::Gecko)
        } else {
            None
        }
    }
}

impl EnvironmentBrowser {
    /// A browser from a label such as "Chromium 140.0.7339.0": the name, then the version when
    /// the last word starts with a digit. It does not say whether the browser was headless. None
    /// for an empty label.
    pub fn from_label(label: &str) -> Option<EnvironmentBrowser> {
        let label = label.trim();
        let (name, version) = match label.rsplit_once(char::is_whitespace) {
            Some((name, last)) if last.starts_with(|c: char| c.is_ascii_digit()) => {
                (name.trim_end(), Some(last.to_owned()))
            }
            _ => (label, None),
        };
        if name.is_empty() {
            return None;
        }
        Some(EnvironmentBrowser {
            name: name.to_owned(),
            version,
            engine: BrowserEngine::of_browser(name),
            headless: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_gives_the_name_the_version_and_the_engine() {
        assert_eq!(
            EnvironmentBrowser::from_label("Chromium 140.0.7339.0"),
            Some(EnvironmentBrowser {
                name: "Chromium".to_owned(),
                version: Some("140.0.7339.0".to_owned()),
                engine: Some(BrowserEngine::Blink),
                headless: None,
            })
        );
        let webkit = EnvironmentBrowser::from_label("Playwright WebKit 26.0").unwrap();
        assert_eq!(webkit.name, "Playwright WebKit");
        assert_eq!(webkit.engine, Some(BrowserEngine::Webkit));
    }

    #[test]
    fn a_label_with_no_version_is_all_name() {
        let browser = EnvironmentBrowser::from_label("Ladybird nightly").unwrap();
        assert_eq!(browser.name, "Ladybird nightly");
        assert_eq!(browser.version, None);
        assert_eq!(browser.engine, None);
        assert_eq!(EnvironmentBrowser::from_label("  "), None);
    }

    #[test]
    fn each_engine_is_known_by_its_browsers() {
        let cases = [
            ("HeadlessChrome", Some(BrowserEngine::Blink)),
            ("Microsoft Edge", Some(BrowserEngine::Blink)),
            ("Safari", Some(BrowserEngine::Webkit)),
            ("Firefox", Some(BrowserEngine::Gecko)),
            ("curl", None),
        ];
        for (name, engine) in cases {
            assert_eq!(BrowserEngine::of_browser(name), engine, "{name}");
        }
    }

    #[test]
    fn this_machine_has_an_os_name() {
        // CI runs on Linux and macOS, which both have one.
        assert!(OsName::of_this_machine().is_some());
    }
}
