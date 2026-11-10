//! The devices dyno knows: every iPhone from 2018 on. `packages/schema/json/devices.json` is
//! generated from this list, and a site reads it.
//!
//! A device's key is the slug of its model name: "iPhone 14 Pro Max" is `iphone-14-pro-max`. A
//! device run reads the phone's model identifier, such as `iPhone15,3`, and keys its device by the
//! model this list gives for it. A run on a model that this list does not have keys it by the
//! identifier. The list holds no memory sizes. Apple does not publish them, and each run reads its
//! own.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::environment::{DeviceKind, EnvironmentDevice, OsName};

/// One device in the catalogue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DeviceEntry {
    /// The slug of the name, such as "iphone-14-pro-max".
    pub key: String,
    /// The model name, such as "iPhone 14 Pro Max".
    pub name: String,
    pub vendor: String,
    pub kind: DeviceKind,
    pub os_family: OsName,
    /// The year the model came out.
    pub year: u16,
    /// Apple's model identifiers for it, such as "iPhone15,3": what the phone gives as its
    /// `ProductType`.
    pub identifiers: Vec<String>,
}

/// The vendor of every device a device run can record: it records Apple's devices only.
pub const APPLE: &str = "Apple";

/// Every iPhone from 2018 on, in the order they came out: (key, name, year, identifiers). The
/// identifiers are the ones Xcode's simulator profiles give, and the iPhone XS Max's second one.
const IPHONES: &[(&str, &str, u16, &[&str])] = &[
    ("iphone-xs", "iPhone XS", 2018, &["iPhone11,2"]),
    (
        "iphone-xs-max",
        "iPhone XS Max",
        2018,
        &["iPhone11,4", "iPhone11,6"],
    ),
    ("iphone-xr", "iPhone XR", 2018, &["iPhone11,8"]),
    ("iphone-11", "iPhone 11", 2019, &["iPhone12,1"]),
    ("iphone-11-pro", "iPhone 11 Pro", 2019, &["iPhone12,3"]),
    (
        "iphone-11-pro-max",
        "iPhone 11 Pro Max",
        2019,
        &["iPhone12,5"],
    ),
    (
        "iphone-se-2nd-generation",
        "iPhone SE (2nd generation)",
        2020,
        &["iPhone12,8"],
    ),
    ("iphone-12-mini", "iPhone 12 mini", 2020, &["iPhone13,1"]),
    ("iphone-12", "iPhone 12", 2020, &["iPhone13,2"]),
    ("iphone-12-pro", "iPhone 12 Pro", 2020, &["iPhone13,3"]),
    (
        "iphone-12-pro-max",
        "iPhone 12 Pro Max",
        2020,
        &["iPhone13,4"],
    ),
    ("iphone-13-mini", "iPhone 13 mini", 2021, &["iPhone14,4"]),
    ("iphone-13", "iPhone 13", 2021, &["iPhone14,5"]),
    ("iphone-13-pro", "iPhone 13 Pro", 2021, &["iPhone14,2"]),
    (
        "iphone-13-pro-max",
        "iPhone 13 Pro Max",
        2021,
        &["iPhone14,3"],
    ),
    (
        "iphone-se-3rd-generation",
        "iPhone SE (3rd generation)",
        2022,
        &["iPhone14,6"],
    ),
    ("iphone-14", "iPhone 14", 2022, &["iPhone14,7"]),
    ("iphone-14-plus", "iPhone 14 Plus", 2022, &["iPhone14,8"]),
    ("iphone-14-pro", "iPhone 14 Pro", 2022, &["iPhone15,2"]),
    (
        "iphone-14-pro-max",
        "iPhone 14 Pro Max",
        2022,
        &["iPhone15,3"],
    ),
    ("iphone-15", "iPhone 15", 2023, &["iPhone15,4"]),
    ("iphone-15-plus", "iPhone 15 Plus", 2023, &["iPhone15,5"]),
    ("iphone-15-pro", "iPhone 15 Pro", 2023, &["iPhone16,1"]),
    (
        "iphone-15-pro-max",
        "iPhone 15 Pro Max",
        2023,
        &["iPhone16,2"],
    ),
    ("iphone-16", "iPhone 16", 2024, &["iPhone17,3"]),
    ("iphone-16-plus", "iPhone 16 Plus", 2024, &["iPhone17,4"]),
    ("iphone-16-pro", "iPhone 16 Pro", 2024, &["iPhone17,1"]),
    (
        "iphone-16-pro-max",
        "iPhone 16 Pro Max",
        2024,
        &["iPhone17,2"],
    ),
    ("iphone-16e", "iPhone 16e", 2025, &["iPhone17,5"]),
    ("iphone-17", "iPhone 17", 2025, &["iPhone18,3"]),
    ("iphone-air", "iPhone Air", 2025, &["iPhone18,4"]),
    ("iphone-17-pro", "iPhone 17 Pro", 2025, &["iPhone18,1"]),
    (
        "iphone-17-pro-max",
        "iPhone 17 Pro Max",
        2025,
        &["iPhone18,2"],
    ),
    ("iphone-17e", "iPhone 17e", 2026, &["iPhone18,5"]),
    ("iphone-18-pro", "iPhone 18 Pro", 2026, &["iPhone19,2"]),
    (
        "iphone-18-pro-max",
        "iPhone 18 Pro Max",
        2026,
        &["iPhone19,3"],
    ),
];

/// Every device in the catalogue, in the order they came out.
pub fn devices() -> Vec<DeviceEntry> {
    IPHONES
        .iter()
        .map(|&(key, name, year, identifiers)| DeviceEntry {
            key: key.to_owned(),
            name: name.to_owned(),
            vendor: APPLE.to_owned(),
            kind: DeviceKind::Phone,
            os_family: OsName::Ios,
            year,
            identifiers: identifiers.iter().map(|&id| id.to_owned()).collect(),
        })
        .collect()
}

/// The device with this key. None when the catalogue does not list it.
pub fn find_device(key: &str) -> Option<DeviceEntry> {
    devices().into_iter().find(|device| device.key == key)
}

/// The model name of an Apple model identifier, such as "iPhone 14 Pro Max" for `iPhone15,3`.
/// A model that the catalogue does not list is named by its identifier.
pub fn model_name(identifier: &str) -> String {
    let identifier = identifier.trim();
    IPHONES
        .iter()
        .find(|(_, _, _, identifiers)| identifiers.contains(&identifier))
        .map_or_else(|| identifier.to_owned(), |&(_, name, _, _)| name.to_owned())
}

/// A device's key: its model name in lower case, with one `-` for each run of characters that are
/// not ASCII letters or digits, and none at either end. "iPhone SE (2nd generation)" is
/// `iphone-se-2nd-generation`.
pub fn device_key(model: &str) -> String {
    let mut key = String::with_capacity(model.len());
    for character in model.chars() {
        if character.is_ascii_alphanumeric() {
            key.push(character.to_ascii_lowercase());
        } else if !key.is_empty() && !key.ends_with('-') {
            key.push('-');
        }
    }
    while key.ends_with('-') {
        key.pop();
    }
    key
}

/// The OS of an Apple model by its name: iPadOS on an iPad, macOS on a Mac, and iOS otherwise.
pub fn apple_os(model: &str) -> OsName {
    match apple_kind(model) {
        DeviceKind::Tablet => OsName::Ipados,
        DeviceKind::Desktop => OsName::Macos,
        DeviceKind::Phone => OsName::Ios,
    }
}

/// The kind of an Apple model by its name. A device run records a phone, so a name that is not an
/// iPad's or a Mac's is a phone's.
fn apple_kind(model: &str) -> DeviceKind {
    if model.starts_with("iPad") {
        DeviceKind::Tablet
    } else if model.starts_with("Mac") || model.starts_with("iMac") {
        DeviceKind::Desktop
    } else {
        DeviceKind::Phone
    }
}

impl EnvironmentDevice {
    /// The device of one of Apple's models, by its name, with the memory a device run read. The
    /// catalogue gives the kind of a model it lists. None for an empty name.
    pub fn of_apple_model(model: &str, memory_bytes: Option<u64>) -> Option<EnvironmentDevice> {
        let key = device_key(model);
        if key.is_empty() {
            return None;
        }
        let kind = find_device(&key).map_or_else(|| apple_kind(model), |device| device.kind);
        Some(EnvironmentDevice {
            key: Some(key),
            name: model.trim().to_owned(),
            vendor: APPLE.to_owned(),
            kind,
            memory_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_name_slugs_to_its_key() {
        for device in devices() {
            assert_eq!(device_key(&device.name), device.key, "{}", device.name);
        }
    }

    #[test]
    fn keys_are_unique() {
        let mut seen = HashSet::new();
        for device in devices() {
            assert!(seen.insert(device.key.clone()), "{} repeats", device.key);
        }
    }

    #[test]
    fn the_catalogue_runs_from_the_iphone_xs_to_the_iphone_18_pro() {
        let devices = devices();
        assert_eq!(devices.first().unwrap().key, "iphone-xs");
        assert_eq!(devices.last().unwrap().key, "iphone-18-pro-max");
        assert!(devices.iter().map(|device| device.year).is_sorted());
        assert!(devices.iter().all(|device| device.year >= 2018));
    }

    #[test]
    fn every_device_has_an_identifier_and_no_identifier_names_two() {
        let mut seen = HashSet::new();
        for device in devices() {
            assert!(!device.identifiers.is_empty(), "{}", device.key);
            for identifier in &device.identifiers {
                assert!(seen.insert(identifier.clone()), "{identifier} repeats");
            }
        }
    }

    #[test]
    fn an_identifier_gives_its_model_name_or_itself() {
        assert_eq!(model_name("iPhone15,3"), "iPhone 14 Pro Max");
        assert_eq!(model_name(" iPhone11,6 "), "iPhone XS Max");
        assert_eq!(model_name("iPhone99,1"), "iPhone99,1");
        assert_eq!(device_key(&model_name("iPhone99,1")), "iphone99-1");
    }

    #[test]
    fn a_key_is_the_name_in_lower_case_with_dashes() {
        assert_eq!(device_key("iPhone 14 Pro Max"), "iphone-14-pro-max");
        assert_eq!(
            device_key(" iPhone SE (3rd generation) "),
            "iphone-se-3rd-generation"
        );
        assert_eq!(device_key("iPhone 18 Fold"), "iphone-18-fold");
        assert_eq!(device_key("()"), "");
    }

    #[test]
    fn a_listed_model_takes_its_kind_from_the_catalogue() {
        let device =
            EnvironmentDevice::of_apple_model("iPhone 14 Pro Max", Some(6_442_450_944)).unwrap();
        assert_eq!(device.key.as_deref(), Some("iphone-14-pro-max"));
        assert_eq!(device.name, "iPhone 14 Pro Max");
        assert_eq!(device.vendor, "Apple");
        assert_eq!(device.kind, DeviceKind::Phone);
        assert_eq!(device.memory_bytes, Some(6_442_450_944));
    }

    #[test]
    fn a_model_the_catalogue_lacks_keeps_its_key_and_name() {
        let ipad = EnvironmentDevice::of_apple_model("iPad Pro 13-inch (M4)", None).unwrap();
        assert_eq!(ipad.key.as_deref(), Some("ipad-pro-13-inch-m4"));
        assert_eq!(ipad.kind, DeviceKind::Tablet);
        assert_eq!(apple_os("iPad Pro 13-inch (M4)"), OsName::Ipados);
        let mac = EnvironmentDevice::of_apple_model("Mac mini", None).unwrap();
        assert_eq!(mac.kind, DeviceKind::Desktop);
        assert_eq!(apple_os("Mac mini"), OsName::Macos);
        assert_eq!(apple_os("iPhone 18 Fold"), OsName::Ios);
        assert_eq!(EnvironmentDevice::of_apple_model("", None), None);
    }
}
