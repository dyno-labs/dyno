//! Verifying a receipt: what someone does who received a build, its result and a lab's receipt.
//!
//! The signature is checked over the receipt's JSON as the file holds it, not over the fields
//! this dyno knows, so a receipt with a field from a newer signer still verifies. Only a key that
//! the caller gave can verify: a receipt names its key by id and never brings the key, because a
//! receipt that brought its own key would prove nothing.

use dyno_schema::canonical::{MAX_SAFE_INTEGER, jcs, sha256_of_jcs};
use dyno_schema::receipt::{
    ED25519, RECEIPT_VERSION, Receipt, VerifyCheck, VerifyCheckId, VerifyKey, VerifyOutcome,
    VerifyResult,
};
use dyno_schema::{SCHEMA_VERSION, TOOL_VERSION};
use serde_json::{Number, Value};

use crate::encoding::{bytes_of_base64url, is_lower_hex};
use crate::keys::{KEY_ID_LENGTH, SIGNATURE_BYTES, TrustedKey};
use crate::times;

/// How many hex characters a SHA-256 has.
const SHA256_HEX_LENGTH: usize = 64;

/// Why a file cannot be verified at all: it is not a version 1 receipt. This is an error of the
/// input, and never a verdict.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    #[error("it is not a receipt: {0}")]
    NotAReceipt(String),
    #[error(
        "it has receipt_version {0}, but this dyno reads receipt_version {RECEIPT_VERSION}: a \
         newer dyno may read it"
    )]
    AnotherVersion(Number),
    #[error(
        "its signature algorithm is {0:?}, but receipt_version {RECEIPT_VERSION} has only {ED25519}"
    )]
    AnotherAlgorithm(String),
}

/// What the receipt is compared with, beyond its signature.
#[derive(Clone, Copy, Debug, Default)]
pub struct Evidence<'a> {
    /// The result file's JSON, to compare its hash, its build id and its `result_hash`.
    pub result: Option<&'a Value>,
    /// The build id of the files the caller holds, worked out by the code `dyno check` uses.
    pub build_id: Option<&'a str>,
}

/// Verifies a receipt file's JSON against the keys that the caller trusts, and compares it with
/// the evidence. It gives a verdict for every receipt of version 1, and an error for anything
/// else.
pub fn verify(
    envelope: &Value,
    keys: &[TrustedKey],
    evidence: Evidence,
) -> Result<VerifyResult, ReceiptError> {
    let signed = Signed::read(envelope)?;
    let receipt = &signed.receipt;
    let mut checks = Vec::new();

    let signer = find_signer(&signed, keys, &mut checks);
    checks.push(not_retired(signer, &signed));
    let signature_holds = signer.is_some();
    compare_with_result(receipt, evidence.result, signature_holds, &mut checks);
    checks.push(compare(
        VerifyCheckId::BuildId,
        Some(&receipt.build_id),
        evidence.build_id.map(|build_id| Some(build_id.to_owned())),
        signature_holds,
        "no build was given",
        |expected, actual| {
            format!(
                "the files have build id {}, and the receipt is for build {expected}",
                actual.unwrap_or("none")
            )
        },
    ));

    let reason = checks
        .iter()
        .find(|check| check.outcome == VerifyOutcome::Mismatch)
        .and_then(|check| check.reason.clone());
    Ok(VerifyResult {
        schema_version: SCHEMA_VERSION,
        tool: TOOL_VERSION.to_owned(),
        verified: signature_holds && reason.is_none(),
        reason,
        checks,
        key: signer.map(|key| VerifyKey {
            key_id: key.key_id().to_owned(),
            created_at: key.created_at().map(str::to_owned),
            retired_at: key.retired_at().map(str::to_owned),
            retired: key.retired_at().is_some(),
        }),
        receipt: signer.map(|_| signed.receipt),
    })
}

/// A receipt of version 1 as its file holds it: the fields this dyno knows, the bytes the
/// signature is over, and the signature.
struct Signed {
    receipt: Receipt,
    /// The UTF-8 bytes of the RFC 8785 canonical JSON of the whole `receipt` object.
    canonical: String,
    signature: [u8; SIGNATURE_BYTES],
}

impl Signed {
    /// Reads a receipt file's JSON. The version is read first, so a receipt of a later version
    /// is refused as such, whatever else it holds.
    fn read(envelope: &Value) -> Result<Signed, ReceiptError> {
        let not_a_receipt = |problem: &str| ReceiptError::NotAReceipt(problem.to_owned());
        let json = envelope
            .get("receipt")
            .filter(|receipt| receipt.is_object())
            .ok_or_else(|| not_a_receipt("it has no `receipt` object"))?;
        match json.get("receipt_version") {
            Some(Value::Number(version))
                if version.as_u64() == Some(u64::from(RECEIPT_VERSION)) => {}
            Some(Value::Number(version)) => {
                return Err(ReceiptError::AnotherVersion(version.clone()));
            }
            _ => return Err(not_a_receipt("its `receipt_version` is not a number")),
        }
        match envelope.get("algorithm") {
            Some(Value::String(algorithm)) if algorithm == ED25519 => {}
            Some(Value::String(algorithm)) => {
                return Err(ReceiptError::AnotherAlgorithm(algorithm.clone()));
            }
            _ => return Err(not_a_receipt("its `algorithm` is not text")),
        }
        let signature = envelope
            .get("signature")
            .and_then(Value::as_str)
            .and_then(bytes_of_base64url)
            .ok_or_else(|| {
                not_a_receipt("its `signature` is not 64 bytes in base64url with no padding")
            })?;
        let mut receipt: Receipt = serde_json::from_value(json.clone())
            .map_err(|error| ReceiptError::NotAReceipt(error.to_string()))?;
        let shapes = [
            (
                is_lower_hex(&receipt.key_id, KEY_ID_LENGTH),
                "its `key_id` is not 16 lower-case hex characters",
            ),
            (
                is_lower_hex(&receipt.build_id, SHA256_HEX_LENGTH),
                "its `build_id` is not 64 lower-case hex characters",
            ),
            (
                is_lower_hex(&receipt.result_sha256, SHA256_HEX_LENGTH),
                "its `result_sha256` is not 64 lower-case hex characters",
            ),
            (
                times::is_receipt_time(&receipt.issued_at),
                "its `issued_at` is not a UTC time to the second, such as 2026-11-25T10:00:00Z",
            ),
        ];
        if let Some((_, problem)) = shapes.iter().find(|(holds, _)| !holds) {
            return Err(not_a_receipt(problem));
        }
        let canonical = jcs(json).map_err(|error| ReceiptError::NotAReceipt(error.to_string()))?;
        // The verdict repeats the receipt, and dyno writes no integer that a double cannot hold.
        receipt.protocol.values_mut().for_each(write_as_doubles);
        for measure in &mut receipt.measures {
            if let Some(value) = &mut measure.value {
                *value = as_double(value);
            }
        }
        Ok(Signed {
            receipt,
            canonical,
            signature,
        })
    }
}

/// Finds the given key that signed the receipt, and adds the `key` and `signature` checks. A
/// key with another key id is never tried.
fn find_signer<'a>(
    signed: &Signed,
    keys: &'a [TrustedKey],
    checks: &mut Vec<VerifyCheck>,
) -> Option<&'a TrustedKey> {
    let key_id = &signed.receipt.key_id;
    let mut named = keys.iter().filter(|key| key.key_id() == key_id).peekable();
    if named.peek().is_none() {
        checks.push(VerifyCheck {
            id: VerifyCheckId::Key,
            outcome: VerifyOutcome::Mismatch,
            expected: Some(key_id.clone()),
            actual: None,
            reason: Some(format!(
                "none of the {} given has the receipt's key id {key_id}",
                count_of_keys(keys.len())
            )),
        });
        checks.push(not_checked(
            VerifyCheckId::Signature,
            None,
            "no given key has the receipt's key id",
        ));
        return None;
    }
    checks.push(VerifyCheck {
        id: VerifyCheckId::Key,
        outcome: VerifyOutcome::Match,
        expected: Some(key_id.clone()),
        actual: Some(key_id.clone()),
        reason: None,
    });
    // Two keys share a key id only when one was given twice, so the first that signed is it.
    let signer = named.find(|key| key.signed(signed.canonical.as_bytes(), &signed.signature));
    checks.push(match signer {
        Some(_) => VerifyCheck {
            id: VerifyCheckId::Signature,
            outcome: VerifyOutcome::Match,
            expected: None,
            actual: None,
            reason: None,
        },
        None => VerifyCheck {
            id: VerifyCheckId::Signature,
            outcome: VerifyOutcome::Mismatch,
            expected: None,
            actual: None,
            reason: Some(format!(
                "the signature is not key {key_id}'s signature of this receipt: the receipt was \
                 changed after it was signed, or another key signed it"
            )),
        },
    });
    signer
}

/// The check that the lab had not retired the key when it issued the receipt. A retired key
/// still verifies a receipt that was issued before its `retired_at`.
///
/// The receipt's own `issued_at` is what is compared, and the key's holder wrote it. So retiring
/// a key stops honest use of it. It is no defence against a stolen key: a lab takes such a key
/// out of its keys document.
fn not_retired(signer: Option<&TrustedKey>, signed: &Signed) -> VerifyCheck {
    let id = VerifyCheckId::KeyNotRetired;
    let Some(key) = signer else {
        return not_checked(id, None, SIGNATURE_DID_NOT_MATCH);
    };
    let issued_at = &signed.receipt.issued_at;
    let issued = times::parse(issued_at).expect("a receipt's issued_at was checked when read");
    if key.was_retired_at(issued) {
        let retired_at = key.retired_at().unwrap_or_default();
        return VerifyCheck {
            id,
            outcome: VerifyOutcome::Mismatch,
            expected: Some(issued_at.clone()),
            actual: Some(retired_at.to_owned()),
            reason: Some(format!(
                "the lab retired key {} at {retired_at}, and the receipt says it was issued at \
                 {issued_at}",
                key.key_id()
            )),
        };
    }
    VerifyCheck {
        id,
        outcome: VerifyOutcome::Match,
        expected: Some(issued_at.clone()),
        actual: key.retired_at().map(str::to_owned),
        reason: None,
    }
}

/// Why a check that reads the receipt's values was not made.
const SIGNATURE_DID_NOT_MATCH: &str = "no given key signed the receipt";

/// Adds the three checks of the result file: its hash, its build id and its `result_hash`.
fn compare_with_result(
    receipt: &Receipt,
    result: Option<&Value>,
    signature_holds: bool,
    checks: &mut Vec<VerifyCheck>,
) {
    const NO_RESULT: &str = "no result file was given";
    let text_at = |pointer: &str| {
        result.map(|result| {
            result
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    };
    checks.push(compare(
        VerifyCheckId::ResultSha256,
        Some(&receipt.result_sha256),
        result.map(|result| sha256_of_jcs(result).ok()),
        signature_holds,
        NO_RESULT,
        |_, _| "the result file is not the one the lab signed: its content differs".to_owned(),
    ));
    checks.push(compare(
        VerifyCheckId::ResultBuildId,
        Some(&receipt.build_id),
        text_at("/deterministic/build/build_id"),
        signature_holds,
        NO_RESULT,
        |expected, actual| match actual {
            Some(actual) => {
                format!("the result is for build {actual}, and the receipt is for build {expected}")
            }
            None => format!("the result has no build id, and the receipt is for build {expected}"),
        },
    ));
    checks.push(compare(
        VerifyCheckId::ResultHash,
        receipt.result_hash.as_deref(),
        text_at("/result_hash"),
        signature_holds,
        NO_RESULT,
        |expected, actual| {
            format!(
                "the result states the hash {}, and the receipt states {}",
                actual.unwrap_or("none"),
                if expected.is_empty() {
                    "none"
                } else {
                    expected
                }
            )
        },
    ));
}

/// One check that compares what the receipt states with what dyno found.
///
/// `found` is None when the caller gave nothing to look in, which leaves the check not made, with
/// `nothing_given` as the reason. A check is not made either when no key signed the receipt,
/// because its values are then nobody's claim.
fn compare(
    id: VerifyCheckId,
    expected: Option<&str>,
    found: Option<Option<String>>,
    signature_holds: bool,
    nothing_given: &str,
    differs: impl Fn(&str, Option<&str>) -> String,
) -> VerifyCheck {
    let Some(actual) = found else {
        return not_checked(id, expected, nothing_given);
    };
    if !signature_holds {
        return not_checked(id, expected, SIGNATURE_DID_NOT_MATCH);
    }
    let matches = expected == actual.as_deref();
    VerifyCheck {
        id,
        outcome: if matches {
            VerifyOutcome::Match
        } else {
            VerifyOutcome::Mismatch
        },
        expected: expected.map(str::to_owned),
        reason: (!matches).then(|| differs(expected.unwrap_or_default(), actual.as_deref())),
        actual,
    }
}

fn not_checked(id: VerifyCheckId, expected: Option<&str>, reason: &str) -> VerifyCheck {
    VerifyCheck {
        id,
        outcome: VerifyOutcome::NotChecked,
        expected: expected.map(str::to_owned),
        actual: None,
        reason: Some(reason.to_owned()),
    }
}

/// "1 key" or "3 keys".
fn count_of_keys(count: usize) -> String {
    if count == 1 {
        "1 key".to_owned()
    } else {
        format!("{count} keys")
    }
}

/// Rewrites every integer in the JSON that a double cannot hold exactly as the double it rounds
/// to, which is what the signed bytes say.
fn write_as_doubles(json: &mut Value) {
    match json {
        Value::Number(number) => *number = as_double(number),
        Value::Array(items) => items.iter_mut().for_each(write_as_doubles),
        Value::Object(fields) => fields.values_mut().for_each(write_as_doubles),
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

/// The number as it is, or as the double it rounds to when it is an integer outside the safe
/// integer range.
fn as_double(number: &Number) -> Number {
    let safe = number
        .as_i64()
        .is_some_and(|integer| integer.unsigned_abs() <= MAX_SAFE_INTEGER.unsigned_abs());
    if safe || number.is_f64() {
        return number.clone();
    }
    number
        .as_f64()
        .and_then(Number::from_f64)
        .unwrap_or_else(|| number.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_integer_a_double_cannot_hold_is_written_as_the_double_it_rounds_to() {
        let mut json = serde_json::json!({
            "safe": 9_007_199_254_740_991_u64,
            "list": [9_007_199_254_740_993_u64, -9_007_199_254_740_993_i64, 1.5, "text", null],
            "unsigned": u64::MAX
        });
        write_as_doubles(&mut json);
        assert_eq!(
            json,
            serde_json::json!({
                "safe": 9_007_199_254_740_991_u64,
                "list": [9_007_199_254_740_992.0, -9_007_199_254_740_992.0, 1.5, "text", null],
                "unsigned": 18_446_744_073_709_551_615.0
            })
        );
        // Each number is now one that dyno's own canonical JSON takes.
        assert!(dyno_schema::canonical::to_canonical_json(&json).is_ok());
    }

    #[test]
    fn the_key_count_is_singular_for_one_key() {
        assert_eq!(count_of_keys(1), "1 key");
        assert_eq!(count_of_keys(0), "0 keys");
        assert_eq!(count_of_keys(2), "2 keys");
    }
}
