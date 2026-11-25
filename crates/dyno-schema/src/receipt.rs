//! A lab's signed receipt for a check result, a lab's public keys, and the verdict of
//! `dyno verify`.
//!
//! A lab owns real phones and runs dyno for people who do not. It signs a small receipt for each
//! result, so that someone who receives the build can trust the lab's phone readings for exactly
//! the files they hold, without running the check again. A result's own `result_hash` leaves out
//! the run part, which holds the phone readings, so the hash alone cannot do this.
//!
//! The format is open, and anyone can run a lab with a key of their own. `docs/receipts.md` gives
//! the format, and the `dyno-receipt` crate signs and verifies it. The signature is over the
//! RFC 8785 canonical JSON of the whole `receipt` object as the lab wrote it, so a field that
//! this dyno does not know is still signed. These types read only the fields that version 1
//! names, and ignore the rest.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use ts_rs::TS;

use crate::profile::{ProfileRef, Scenario};

/// The receipt format that this dyno writes and reads. A verifier refuses any other.
pub const RECEIPT_VERSION: u32 = 1;

/// The one signature algorithm of receipt version 1, as `algorithm` names it.
pub const ED25519: &str = "ed25519";

/// A receipt with its signature: the file that a lab gives with a result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReceiptEnvelope {
    pub receipt: Receipt,
    /// The signature algorithm: `ed25519`.
    pub algorithm: String,
    /// The Ed25519 signature over the UTF-8 bytes of the RFC 8785 canonical JSON of `receipt`,
    /// in base64url with no padding.
    pub signature: String,
}

/// What a lab signs: which result it stands behind, for which build, on which device, and the
/// device's readings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Receipt {
    /// The format version: 1.
    pub receipt_version: u32,
    /// The lab's name, such as a domain.
    pub issuer: String,
    /// Which of the lab's keys signed: the first 16 hex characters of the SHA-256 of the raw
    /// 32-byte public key.
    pub key_id: String,
    /// When the lab signed: RFC 3339, in UTC, to the second, ending in `Z`.
    pub issued_at: String,
    /// The lab's id for the run.
    pub check_id: String,
    /// The build id of the files the lab ran: 64 lower-case hex characters.
    pub build_id: String,
    /// Lower-case hex SHA-256 of the RFC 8785 canonical JSON of the whole result file.
    pub result_sha256: String,
    /// The result's own `result_hash`, as the result states it. Null when it states none.
    pub result_hash: Option<String>,
    /// The dyno version that made the result.
    pub dyno_version: String,
    pub lab: ReceiptLab,
    pub device: ReceiptDevice,
    /// How the lab ran the check: the host profile's name and version, the scenario, the spins
    /// and the device runs. [`ReceiptProtocol`] is the object that dyno writes. A receipt from
    /// another signer can name its fields in another way, so a reader gets the object as it is.
    #[ts(type = "{ [key in string]?: unknown }")]
    pub protocol: Map<String, Value>,
    /// The device's readings.
    pub measures: Vec<ReceiptMeasure>,
}

/// The lab that ran the check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReceiptLab {
    /// What kind of lab it is, in the lab's own words.
    pub kind: String,
    /// What the lab's devices run on, such as the name of a device cloud. Null when the lab does
    /// not say.
    pub backend: Option<String>,
}

/// The device that ran the check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReceiptDevice {
    /// The device catalogue's key, such as "iphone-14-pro-max".
    pub key: String,
    /// The model's name, such as "iPhone 14 Pro Max". Null when the result does not name it.
    pub model: Option<String>,
    /// The OS version, such as "26.6.2 (23G90)". Null when the result does not give it.
    pub os_version: Option<String>,
}

/// The `protocol` object that `dyno receipt sign` writes, from the result's inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReceiptProtocol {
    pub host_profile: ProfileRef,
    pub scenario: Scenario,
    /// The demo spins the check asked for. Null when none.
    pub spins: Option<u32>,
    /// The runs of the game, and of the empty build, that the check asked for. Null when the
    /// check ran once.
    pub device_runs: Option<u32>,
}

/// One reading of the device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReceiptMeasure {
    /// The measure's id, such as "device.phys_footprint".
    pub id: String,
    /// The reading, in the measure's unit. Null when the device gave none.
    #[ts(type = "number | null")]
    pub value: Option<Number>,
    /// The unit, such as "bytes". Null when the signer does not give it.
    pub unit: Option<String>,
}

/// A lab's public keys, as the JSON document the lab publishes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PublicKeys {
    pub keys: Vec<PublicKey>,
}

/// One of a lab's public keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PublicKey {
    /// The first 16 hex characters of the SHA-256 of the raw 32-byte public key.
    pub key_id: String,
    /// The key's algorithm: `ed25519`. A verifier leaves out a key of an algorithm that it does
    /// not know.
    pub algorithm: String,
    /// The raw 32 bytes of the public key, in base64url with no padding.
    pub public_key: String,
    /// When the lab made the key: RFC 3339.
    pub created_at: String,
    /// When the lab stopped signing with the key: RFC 3339. Null while the lab still signs with
    /// it. A retired key still verifies a receipt that was issued before this time.
    pub retired_at: Option<String>,
}

/// The output of `dyno verify`: whether a receipt holds, and each check that says so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct VerifyResult {
    pub schema_version: u32,
    /// The dyno version that verified.
    pub tool: String,
    /// True when a given key signed the receipt, and nothing that was checked differs from it.
    /// The exit code comes from it alone.
    pub verified: bool,
    /// Why the receipt is not verified: the first check that did not match. Null when it is.
    pub reason: Option<String>,
    /// Every check, in the order of [`VerifyCheckId`]. A check that a flag did not ask for is
    /// there too, as `not_checked`.
    pub checks: Vec<VerifyCheck>,
    /// The key that signed the receipt. Null when no given key did.
    pub key: Option<VerifyKey>,
    /// What the lab signed: the issuer, the time, the device and its readings. Null unless the
    /// signature matched, because dyno does not repeat a claim that nobody signed.
    pub receipt: Option<Receipt>,
}

/// One check of a receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct VerifyCheck {
    pub id: VerifyCheckId,
    pub outcome: VerifyOutcome,
    /// What the receipt states. Null for a check with no value to state.
    pub expected: Option<String>,
    /// What dyno found in the result, or worked out from the result or the build. Null when it
    /// found none, or did not look.
    pub actual: Option<String>,
    /// Why the check did not match, or was not made. Null when it matched.
    pub reason: Option<String>,
}

/// The things `dyno verify` checks, in the order it checks them.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum VerifyCheckId {
    /// One of the given keys has the receipt's `key_id`.
    Key,
    /// The signature is that key's, over the receipt.
    Signature,
    /// The lab had not retired the key when it issued the receipt.
    KeyNotRetired,
    /// `--result`: the SHA-256 of the result file's canonical JSON is the receipt's
    /// `result_sha256`.
    ResultSha256,
    /// `--result`: the result's build id is the receipt's `build_id`.
    ResultBuildId,
    /// `--result`: the result's `result_hash` is the receipt's.
    ResultHash,
    /// `--build`: the build id of the files is the receipt's `build_id`.
    BuildId,
}

/// The outcome of one check of a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum VerifyOutcome {
    Match,
    Mismatch,
    /// No flag asked for the check, or a check before it did not match.
    NotChecked,
}

/// The key that signed a receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct VerifyKey {
    pub key_id: String,
    /// When the lab made the key. Null for a key given with `--key`, which has no times.
    pub created_at: Option<String>,
    /// When the lab retired the key. Null when it has not, and for a key given with `--key`.
    pub retired_at: Option<String>,
    /// Whether the lab has retired the key. A retired key still verifies a receipt that was
    /// issued before `retired_at`.
    pub retired: bool,
}

impl VerifyCheckId {
    /// Every check, in the order `dyno verify` makes them.
    pub const ALL: &[VerifyCheckId] = &[
        VerifyCheckId::Key,
        VerifyCheckId::Signature,
        VerifyCheckId::KeyNotRetired,
        VerifyCheckId::ResultSha256,
        VerifyCheckId::ResultBuildId,
        VerifyCheckId::ResultHash,
        VerifyCheckId::BuildId,
    ];

    /// The id as it appears in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            VerifyCheckId::Key => "key",
            VerifyCheckId::Signature => "signature",
            VerifyCheckId::KeyNotRetired => "key_not_retired",
            VerifyCheckId::ResultSha256 => "result_sha256",
            VerifyCheckId::ResultBuildId => "result_build_id",
            VerifyCheckId::ResultHash => "result_hash",
            VerifyCheckId::BuildId => "build_id",
        }
    }
}

impl std::fmt::Display for VerifyCheckId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A receipt with every field of version 1, and fields that version 1 does not have.
    const WITH_UNKNOWN_FIELDS: &str = r#"{
        "receipt": {
            "receipt_version": 1,
            "issuer": "lab.example",
            "key_id": "0123456789abcdef",
            "issued_at": "2026-11-25T10:00:00Z",
            "check_id": "chk_1",
            "build_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "result_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "result_hash": null,
            "dyno_version": "0.1.3",
            "lab": { "kind": "own_phones", "backend": null, "city": "added later" },
            "device": { "key": "iphone-14-pro-max", "model": null, "os_version": null },
            "protocol": { "anything": ["the", "signer", "writes"] },
            "measures": [
                { "id": "device.phys_footprint", "value": 209715200, "unit": "bytes" },
                { "id": "device.from_a_newer_dyno", "value": 1.5, "unit": null, "note": "new" },
                { "id": "device.kills", "value": null, "unit": "count" }
            ],
            "added_later": true
        },
        "algorithm": "ed25519",
        "signature": "AA",
        "added_later": 1
    }"#;

    #[test]
    fn a_receipt_with_fields_this_dyno_does_not_know_still_reads() {
        let envelope: ReceiptEnvelope = serde_json::from_str(WITH_UNKNOWN_FIELDS).unwrap();
        let receipt = envelope.receipt;
        assert_eq!(receipt.receipt_version, RECEIPT_VERSION);
        assert_eq!(envelope.algorithm, ED25519);
        assert_eq!(receipt.result_hash, None);
        assert_eq!(receipt.lab.kind, "own_phones");
        assert_eq!(receipt.protocol["anything"][1], "signer");
        // A measure id from a newer dyno is kept as text, and a value keeps its own spelling.
        let values: Vec<Option<String>> = receipt
            .measures
            .iter()
            .map(|measure| measure.value.as_ref().map(ToString::to_string))
            .collect();
        assert_eq!(
            values,
            [Some("209715200".to_owned()), Some("1.5".to_owned()), None]
        );
    }

    #[test]
    fn the_protocol_that_dyno_writes_has_the_inputs_field_names() {
        let protocol = ReceiptProtocol {
            host_profile: ProfileRef {
                name: "reels".to_owned(),
                version: 2,
            },
            scenario: Scenario::Alone,
            spins: None,
            device_runs: Some(3),
        };
        assert_eq!(
            serde_json::to_value(protocol).unwrap(),
            serde_json::json!({
                "host_profile": { "name": "reels", "version": 2 },
                "scenario": "alone",
                "spins": null,
                "device_runs": 3
            })
        );
    }

    #[test]
    fn each_check_id_is_written_as_its_name_in_the_order_it_is_checked() {
        let names: Vec<String> = VerifyCheckId::ALL
            .iter()
            .map(|id| {
                serde_json::to_value(id)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let shown: Vec<String> = VerifyCheckId::ALL.iter().map(ToString::to_string).collect();
        assert_eq!(names, shown);
        assert!(VerifyCheckId::ALL.is_sorted());
        assert_eq!(names[0], "key");
        assert_eq!(names[6], "build_id");
    }
}
