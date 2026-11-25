//! Making a receipt: what a lab does once dyno has written a result.
//!
//! [`receipt_of_result`] takes every value it can from the result file, so the lab adds only who
//! it is and what it calls the run. `docs/receipts.md` lists the result field behind each receipt
//! field. [`sign`] signs the receipt.

use dyno_schema::SCHEMA_VERSION;
use dyno_schema::canonical::{sha256_of_jcs, to_canonical_json};
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue, Source};
use dyno_schema::receipt::{
    ED25519, RECEIPT_VERSION, Receipt, ReceiptDevice, ReceiptEnvelope, ReceiptLab, ReceiptMeasure,
    ReceiptProtocol,
};
use dyno_schema::result::LoadedResult;
use serde::Serialize;
use serde_json::{Map, Number, Value};

use crate::encoding::base64url;
use crate::keys::SigningKey;
use crate::times::is_receipt_time;

/// What the lab says about a run, which no result holds.
#[derive(Clone, Copy, Debug)]
pub struct Issue<'a> {
    /// The lab's name, such as a domain.
    pub issuer: &'a str,
    /// The lab's id for the run.
    pub check_id: &'a str,
    /// When the lab signs: RFC 3339, in UTC, to the second, ending in `Z`.
    pub issued_at: &'a str,
    /// What kind of lab it is, in the lab's own words.
    pub lab_kind: &'a str,
    /// What the lab's devices run on. None when the lab does not say.
    pub lab_backend: Option<&'a str>,
}

/// Why a result gets no receipt.
#[derive(Debug, thiserror::Error)]
pub enum SignError {
    #[error("it is not a dyno check result: {0}")]
    NotAResult(serde_json::Error),
    #[error("it has schema_version {0}, but this dyno reads schema_version {SCHEMA_VERSION}")]
    AnotherSchema(u64),
    #[error("its result_hash does not match its content: it was edited after dyno wrote it")]
    Edited,
    #[error(
        "it has no build id (deterministic.build.build_id): a receipt is for the files of a build"
    )]
    NoBuild,
    #[error(
        "it names no device (run.environment.device.key): a receipt is for a run on a device, \
         such as a check with --device"
    )]
    NoDevice,
    #[error("the time {0:?} must be in UTC, to the second, such as 2026-11-25T10:00:00Z")]
    NotAReceiptTime(String),
    #[error("the {0} must not be empty")]
    Empty(&'static str),
    #[error("the receipt does not serialize: {0}")]
    Json(#[from] serde_json::Error),
}

/// The receipt for a result file, given as the JSON it holds. `key_id` names the key that will
/// sign it.
///
/// The result must be one that dyno wrote and nobody edited, of a build, with a run on a device.
/// `result_sha256` is over the whole file as it is, so a field that this dyno does not know is
/// covered too.
pub fn receipt_of_result(
    result_json: &Value,
    issue: &Issue,
    key_id: &str,
) -> Result<Receipt, SignError> {
    for (name, text) in [
        ("issuer", issue.issuer),
        ("check id", issue.check_id),
        ("lab kind", issue.lab_kind),
    ] {
        if text.trim().is_empty() {
            return Err(SignError::Empty(name));
        }
    }
    if !is_receipt_time(issue.issued_at) {
        return Err(SignError::NotAReceiptTime(issue.issued_at.to_owned()));
    }
    // The version is read before the rest, so a result from another dyno gets a clear error.
    let version = result_json.get("schema_version").and_then(Value::as_u64);
    if let Some(version) = version.filter(|version| *version != u64::from(SCHEMA_VERSION)) {
        return Err(SignError::AnotherSchema(version));
    }
    let loaded = LoadedResult::from_json(result_json.clone()).map_err(SignError::NotAResult)?;
    let result = loaded.result;
    if !loaded.hash_matches {
        return Err(SignError::Edited);
    }
    let inputs = &result.deterministic.inputs;
    let build_id = result
        .deterministic
        .build
        .as_ref()
        .and_then(|build| build.build_id.clone())
        .ok_or(SignError::NoBuild)?;
    let environment = result.run.environment.as_ref();
    let device = environment.and_then(|environment| environment.device.as_ref());
    let Some(device_key) = device.and_then(|device| device.key.clone()) else {
        return Err(SignError::NoDevice);
    };
    let protocol = ReceiptProtocol {
        host_profile: inputs.host_profile.clone(),
        scenario: inputs.scenario,
        spins: inputs.spins,
        device_runs: inputs.device_runs,
    };
    Ok(Receipt {
        receipt_version: RECEIPT_VERSION,
        issuer: issue.issuer.to_owned(),
        key_id: key_id.to_owned(),
        issued_at: issue.issued_at.to_owned(),
        check_id: issue.check_id.to_owned(),
        build_id,
        result_sha256: sha256_of_jcs(result_json)?,
        result_hash: Some(result.result_hash),
        dyno_version: result.deterministic.versions.tool,
        lab: ReceiptLab {
            kind: issue.lab_kind.to_owned(),
            backend: issue.lab_backend.map(str::to_owned),
        },
        device: ReceiptDevice {
            key: device_key,
            model: device.map(|device| device.name.clone()),
            os_version: environment
                .and_then(|environment| environment.os.as_ref())
                .and_then(|os| os.version.clone()),
        },
        protocol: object_of(&protocol)?,
        measures: result
            .run
            .measures
            .iter()
            .filter(|measure| measure.source == Source::Device)
            .map(receipt_measure)
            .collect::<Result<_, _>>()?,
    })
}

/// Signs a receipt with the key its `key_id` names: an Ed25519 signature over the UTF-8 bytes of
/// the receipt's RFC 8785 canonical JSON.
pub fn sign(receipt: Receipt, key: &SigningKey) -> Result<ReceiptEnvelope, SignError> {
    let canonical = to_canonical_json(&receipt)?;
    let signature = key.sign(canonical.as_bytes());
    Ok(ReceiptEnvelope {
        receipt,
        algorithm: ED25519.to_owned(),
        signature: base64url(&signature),
    })
}

/// A device measure as a receipt states it. Only a reading has a value: a measure that is
/// missing or invalid gives null, because a receipt has no status to say that a value failed its
/// check.
fn receipt_measure(measure: &Measure) -> Result<ReceiptMeasure, SignError> {
    let value = match (&measure.status, &measure.value) {
        (MeasureStatus::Measured, Some(MeasureValue::Integer(integer))) => {
            Some(Number::from(*integer))
        }
        _ => None,
    };
    let unit = serde_json::to_value(measure.unit)?;
    Ok(ReceiptMeasure {
        id: measure.id.as_str().to_owned(),
        value,
        unit: unit.as_str().map(str::to_owned),
    })
}

/// The JSON object that a struct serializes to.
fn object_of<T: Serialize>(value: &T) -> Result<Map<String, Value>, serde_json::Error> {
    serde_json::from_value(serde_json::to_value(value)?)
}
