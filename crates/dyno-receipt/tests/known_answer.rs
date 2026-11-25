//! The known-answer receipt in `fixtures/receipts/`: a result, a key and the one receipt they
//! give, byte for byte.
//!
//! Another implementation of the format, such as a signer in another language, tests against
//! the same files and the same values, which `docs/receipts.md` states. So these values do not
//! change: a test that fails here means the bytes of a receipt changed, and every receipt that a
//! lab has signed would stop verifying.

use std::fs;
use std::path::{Path, PathBuf};

use dyno_receipt::keys::{PrivateKeyFile, SigningKey, trusted_keys};
use dyno_receipt::sign::{Issue, receipt_of_result, sign};
use dyno_receipt::verify::{Evidence, verify};
use dyno_schema::canonical::{jcs, sha256_hex, sha256_of_jcs, to_canonical_json};
use dyno_schema::receipt::{PublicKeys, ReceiptEnvelope, VerifyOutcome};
use serde_json::Value;

/// The test key's public key: the first test key of RFC 8032, section 7.1, in base64url.
const PUBLIC_KEY: &str = "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo";

/// The first 16 hex characters of the SHA-256 of the public key's raw 32 bytes.
const KEY_ID: &str = "21fe31dfa154a261";

/// The SHA-256 of the RFC 8785 canonical JSON of `result.json`.
const RESULT_SHA256: &str = "c36e9f14f9d5f7124781553595d8a64f379007a651cdafd42531cd3558310845";

/// The build id of the build the result is for, `crates/dyno-cli/tests/data/tiny-build`.
const BUILD_ID: &str = "54bc36cfde26ce4949a736033d9f3d251b6f2ba547725ab7e19779f77be911c5";

/// The SHA-256 of the bytes that are signed: the canonical JSON of the receipt.
const SIGNED_BYTES_SHA256: &str =
    "7e6d6b3e28a1912cd0e074400131973ab7e9811cf90350c35a44ed3ecb1f2f7b";

/// The Ed25519 signature of those bytes, in base64url.
const SIGNATURE: &str =
    "dZBJ_Y9Q11GLzd5I37aQNKNaArZ0ofkWO2XV50x_oJZroEqZdX9xp8Xbb5oZYM38ygmzkkwoLXOjWqj0GlCuDQ";

/// What the lab said about the run when it signed.
const ISSUE: Issue<'static> = Issue {
    issuer: "lab.example",
    check_id: "chk_0001",
    issued_at: "2026-11-25T11:00:00Z",
    lab_kind: "own_phones",
    lab_backend: Some("fake-phone"),
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/receipts")
        .join(name)
}

fn text(name: &str) -> String {
    let path = fixture(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn json(name: &str) -> Value {
    serde_json::from_str(&text(name)).unwrap()
}

fn test_key() -> SigningKey {
    let file: PrivateKeyFile = serde_json::from_str(&text("test-private-key.json")).unwrap();
    SigningKey::from_file(&file).unwrap()
}

#[test]
fn the_test_key_is_the_rfc_8032_key_with_its_key_id() {
    let key = test_key();
    assert_eq!(key.key_id(), KEY_ID);
    let keys: PublicKeys = serde_json::from_str(&text("keys.json")).unwrap();
    assert_eq!(keys.keys.len(), 1);
    assert_eq!(keys.keys[0].public_key, PUBLIC_KEY);
    assert_eq!(keys.keys[0].key_id, KEY_ID);
    assert_eq!(keys.keys[0], key.public_entry("2026-11-01T00:00:00Z"));
}

#[test]
fn the_result_file_has_its_known_hash() {
    assert_eq!(sha256_of_jcs(&json("result.json")).unwrap(), RESULT_SHA256);
}

#[test]
fn the_signed_bytes_are_the_canonical_json_of_the_receipt() {
    let envelope = json("receipt.json");
    let signed_bytes = text("receipt.canonical.json");
    assert_eq!(jcs(&envelope["receipt"]).unwrap(), signed_bytes);
    assert_eq!(sha256_hex(signed_bytes.as_bytes()), SIGNED_BYTES_SHA256);
    // The typed receipt writes the same bytes, so dyno signs what it later verifies.
    let typed: ReceiptEnvelope = serde_json::from_value(envelope).unwrap();
    assert_eq!(to_canonical_json(&typed.receipt).unwrap(), signed_bytes);
}

#[test]
fn signing_the_result_with_the_test_key_gives_the_receipt_file_byte_for_byte() {
    let key = test_key();
    let receipt = receipt_of_result(&json("result.json"), &ISSUE, &key.key_id()).unwrap();
    assert_eq!(receipt.result_sha256, RESULT_SHA256);
    assert_eq!(receipt.build_id, BUILD_ID);
    let envelope = sign(receipt, &key).unwrap();
    assert_eq!(envelope.signature, SIGNATURE);
    let written = to_canonical_json(&envelope).unwrap() + "\n";
    assert_eq!(written, text("receipt.json"));
}

#[test]
fn the_receipt_file_verifies_against_the_keys_document_the_result_and_the_build_id() {
    let keys: PublicKeys = serde_json::from_str(&text("keys.json")).unwrap();
    let result = json("result.json");
    let evidence = Evidence {
        result: Some(&result),
        build_id: Some(BUILD_ID),
    };
    let verdict = verify(
        &json("receipt.json"),
        &trusted_keys(&keys).unwrap(),
        evidence,
    )
    .unwrap();
    assert!(verdict.verified, "{:?}", verdict.reason);
    assert!(
        verdict
            .checks
            .iter()
            .all(|check| check.outcome == VerifyOutcome::Match)
    );
}
