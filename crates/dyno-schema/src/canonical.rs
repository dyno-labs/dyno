//! Canonical JSON (RFC 8785) and the SHA-256 hashes built on it.
//!
//! Canonical JSON has sorted keys, one number format and no free whitespace, so the same value
//! gives the same bytes on every machine. A stable hash also needs every list sorted by a
//! complete key and every path relative with '/'. `RelPath` normalizes paths, and
//! `CheckResult::new` and `CompareResult::new` sort the lists.

use std::fmt::{self, Write};

use serde::Serialize;
use serde::ser::Error as _;
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

/// The largest safe integer: 2^53 - 1. Canonical JSON writes every number as a double, and above
/// it a double cannot hold every integer, so two integers can give the same bytes.
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// The value as RFC 8785 canonical JSON. An integer outside the safe integer range is an error, and
/// so is a float that is NaN or infinite.
pub fn to_canonical_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    // This serializes twice on purpose. The `Value` is only for the integer check, because
    // `to_value` turns NaN into null. The canonicalizer reads the original, so it still rejects
    // NaN.
    if let Some(number) = first_unsafe_integer(&serde_json::to_value(value)?) {
        return Err(serde_json::Error::custom(outside_the_safe_range(
            "integer", number,
        )));
    }
    serde_json_canonicalizer::to_string(value)
}

/// A sort key that breaks ties between list items: the item's canonical JSON.
///
/// Two items with the same key are the same in every byte of canonical JSON, so their order
/// cannot change the output or a hash. An item that cannot be written gets an empty key. That
/// never hides a fault, because the item is in the result, so writing the result fails.
pub(crate) fn tie_breaker<T: Serialize>(item: &T) -> String {
    to_canonical_json(item).unwrap_or_default()
}

/// Whether the integer is safe: a double holds it and every integer next to it exactly.
pub(crate) fn is_safe_integer(integer: i64) -> bool {
    (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&integer)
}

/// Lowercase hex SHA-256 of the value's canonical JSON.
pub fn sha256_of_canonical_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    Ok(sha256_hex(to_canonical_json(value)?.as_bytes()))
}

/// Lowercase hex SHA-256 of some bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hex, "{byte:02x}").expect("writing to a String never fails");
    }
    hex
}

/// The message for an integer outside the safe integer range.
pub(crate) fn outside_the_safe_range(what: &str, integer: impl fmt::Display) -> String {
    format!(
        "{what} {integer} is outside the safe integer range, -{MAX_SAFE_INTEGER} to \
         {MAX_SAFE_INTEGER}"
    )
}

/// The first integer outside the safe range, anywhere in the JSON.
fn first_unsafe_integer(value: &Value) -> Option<&Number> {
    match value {
        Value::Number(number) if is_safe_number(number) => None,
        Value::Number(number) => Some(number),
        Value::Array(items) => items.iter().find_map(first_unsafe_integer),
        Value::Object(fields) => fields.values().find_map(first_unsafe_integer),
        Value::Null | Value::Bool(_) | Value::String(_) => None,
    }
}

/// Whether the number is a float, or an integer in the safe range. A float already is a double,
/// so only a big integer can change when canonical JSON writes it as one.
fn is_safe_number(number: &Number) -> bool {
    if let Some(integer) = number.as_i64() {
        is_safe_integer(integer)
    } else {
        number.is_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_empty_input_matches_the_standard_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// Fields declared out of key order, as serde writes them.
    #[derive(Serialize)]
    struct OutOfOrder {
        b: f64,
        a: Vec<&'static str>,
    }

    #[test]
    fn canonical_json_sorts_keys_and_drops_whitespace() {
        let value = OutOfOrder {
            b: 2.5,
            a: vec!["x", "y"],
        };
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"b":2.5,"a":["x","y"]}"#
        );
        assert_eq!(
            to_canonical_json(&value).unwrap(),
            r#"{"a":["x","y"],"b":2.5}"#
        );
    }

    #[test]
    fn canonical_json_writes_whole_floats_as_integers() {
        assert_eq!(to_canonical_json(&1.0_f64).unwrap(), "1");
    }

    #[test]
    fn canonical_json_keeps_the_largest_safe_integers() {
        assert_eq!(
            to_canonical_json(&MAX_SAFE_INTEGER).unwrap(),
            "9007199254740991"
        );
        assert_eq!(
            to_canonical_json(&-MAX_SAFE_INTEGER).unwrap(),
            "-9007199254740991"
        );
    }

    #[test]
    fn canonical_json_rejects_integers_outside_the_safe_range() {
        let first_unsafe = MAX_SAFE_INTEGER + 1;
        for unsafe_value in [
            serde_json::json!(first_unsafe),
            serde_json::json!(first_unsafe + 1),
            serde_json::json!(u64::MAX),
            serde_json::json!(-first_unsafe),
            serde_json::json!({ "nested": [1, first_unsafe] }),
        ] {
            let error = to_canonical_json(&unsafe_value).unwrap_err();
            assert!(error.to_string().contains("outside the safe"), "{error}");
        }
    }

    #[test]
    fn canonical_json_rejects_floats_that_are_not_finite() {
        for float in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(to_canonical_json(&float).is_err(), "{float}");
        }
    }
}
