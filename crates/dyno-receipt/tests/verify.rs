//! Verifying a receipt against a lab's keys, a result file and a build id.

mod common;

use common::{
    ISSUE, LAB_KEY_ID, build_id, device_result, keys_document, lab_key, other_key, receipt_file,
    receipt_file_issued, signed_by, trusted_lab_key,
};
use dyno_receipt::keys::{TrustedKey, trusted_keys};
use dyno_receipt::verify::{Evidence, ReceiptError, verify};
use dyno_schema::canonical::to_canonical_json;
use dyno_schema::receipt::{VerifyCheck, VerifyCheckId, VerifyOutcome, VerifyResult};
use serde_json::{Value, json};

use VerifyCheckId::{
    BuildId, Key, KeyNotRetired, ResultBuildId, ResultHash, ResultSha256, Signature,
};
use VerifyOutcome::{Match, Mismatch, NotChecked};

/// A change to a receipt file's JSON.
type Change = fn(&mut Value);

/// Verifies with the lab's key, given alone, and no other evidence.
fn verify_signature(receipt_file: &Value) -> VerifyResult {
    verify(receipt_file, &[trusted_lab_key()], Evidence::default()).unwrap()
}

/// Verifies against the result and the build id too.
fn verify_all(receipt_file: &Value, result: &Value, build_id: &str) -> VerifyResult {
    let evidence = Evidence {
        result: Some(result),
        build_id: Some(build_id),
    };
    verify(receipt_file, &[trusted_lab_key()], evidence).unwrap()
}

fn check(verdict: &VerifyResult, id: VerifyCheckId) -> &VerifyCheck {
    verdict.checks.iter().find(|check| check.id == id).unwrap()
}

fn outcomes(verdict: &VerifyResult) -> Vec<(VerifyCheckId, VerifyOutcome)> {
    verdict
        .checks
        .iter()
        .map(|check| (check.id, check.outcome))
        .collect()
}

fn refused(receipt_file: &Value) -> ReceiptError {
    verify(receipt_file, &[trusted_lab_key()], Evidence::default()).unwrap_err()
}

/// The receipt object of the device result, as JSON, for a test to change before it signs.
fn receipt_json() -> Value {
    receipt_file(&device_result())["receipt"].clone()
}

#[test]
fn a_receipt_with_its_result_and_its_build_is_verified_and_every_check_matches() {
    let result = device_result();
    let verdict = verify_all(&receipt_file(&result), &result, &build_id());
    assert!(verdict.verified);
    assert_eq!(verdict.reason, None);
    assert_eq!(
        outcomes(&verdict),
        [
            (Key, Match),
            (Signature, Match),
            (KeyNotRetired, Match),
            (ResultSha256, Match),
            (ResultBuildId, Match),
            (ResultHash, Match),
            (BuildId, Match),
        ]
    );
    let build = check(&verdict, BuildId);
    assert_eq!(build.expected, Some(build_id()));
    assert_eq!(build.actual, Some(build_id()));
    assert_eq!(build.reason, None);
}

#[test]
fn the_verdict_gives_the_issuer_the_key_the_time_the_device_and_the_measures() {
    let verdict = verify_signature(&receipt_file(&device_result()));
    let receipt = verdict.receipt.as_ref().unwrap();
    assert_eq!(receipt.issuer, "lab.example");
    assert_eq!(receipt.issued_at, "2026-11-25T10:00:00Z");
    assert_eq!(receipt.device.key, "iphone-14-pro-max");
    assert_eq!(receipt.measures.len(), 4);
    assert_eq!(receipt.measures[2].id, "device.phys_footprint");
    let key = verdict.key.as_ref().unwrap();
    assert_eq!(key.key_id, LAB_KEY_ID);
    assert!(!key.retired);
    assert_eq!(verdict.schema_version, 0);
    assert_eq!(verdict.tool, dyno_schema::TOOL_VERSION);
    // dyno writes it as it writes every result.
    assert!(to_canonical_json(&verdict).is_ok());
}

#[test]
fn with_only_a_key_the_result_and_the_build_are_not_checked_and_the_verdict_says_so() {
    let verdict = verify_signature(&receipt_file(&device_result()));
    assert!(verdict.verified);
    assert_eq!(
        outcomes(&verdict),
        [
            (Key, Match),
            (Signature, Match),
            (KeyNotRetired, Match),
            (ResultSha256, NotChecked),
            (ResultBuildId, NotChecked),
            (ResultHash, NotChecked),
            (BuildId, NotChecked),
        ]
    );
    assert_eq!(
        check(&verdict, ResultSha256).reason.as_deref(),
        Some("no result file was given")
    );
    let build = check(&verdict, BuildId);
    assert_eq!(build.reason.as_deref(), Some("no build was given"));
    // What was not checked still says what the receipt states.
    assert_eq!(build.expected, Some(build_id()));
}

#[test]
fn a_key_with_another_key_id_is_not_tried_and_the_receipt_is_not_verified() {
    let keys = [TrustedKey::from_base64url(&common::base64url(&other_key().public_key())).unwrap()];
    let verdict = verify(&receipt_file(&device_result()), &keys, Evidence::default()).unwrap();
    assert!(!verdict.verified);
    assert_eq!(
        verdict.reason.as_deref(),
        Some("none of the 1 key given has the receipt's key id 21fe31dfa154a261")
    );
    assert_eq!(check(&verdict, Key).outcome, Mismatch);
    assert_eq!(check(&verdict, Signature).outcome, NotChecked);
    assert_eq!(check(&verdict, KeyNotRetired).outcome, NotChecked);
    assert_eq!(verdict.key, None);
}

#[test]
fn with_no_key_at_all_nothing_is_verified() {
    let verdict = verify(&receipt_file(&device_result()), &[], Evidence::default()).unwrap();
    assert!(!verdict.verified);
    assert!(
        verdict
            .reason
            .as_deref()
            .unwrap()
            .starts_with("none of the 0 keys given")
    );
}

#[test]
fn a_receipt_that_no_key_signed_is_not_repeated_in_the_verdict() {
    let result = device_result();
    let mut file = receipt_file(&result);
    file["receipt"]["measures"][2]["value"] = 1.into();
    let verdict = verify_all(&file, &result, &build_id());
    assert!(!verdict.verified);
    assert_eq!(verdict.receipt, None);
    assert_eq!(verdict.key, None);
}

#[test]
fn a_receipt_changed_after_it_was_signed_does_not_verify() {
    let changes: [Change; 5] = [
        |file| file["receipt"]["build_id"] = build_id().replace('a', "b").into(),
        |file| file["receipt"]["measures"][2]["value"] = 104_857_600.into(),
        |file| file["receipt"]["issuer"] = "another-lab.example".into(),
        |file| file["receipt"]["added_later"] = true.into(),
        |file| file["receipt"]["device"]["os_version"] = Value::Null,
    ];
    for change in changes {
        let mut file = receipt_file(&device_result());
        change(&mut file);
        let verdict = verify_signature(&file);
        assert!(!verdict.verified, "{file}");
        assert_eq!(check(&verdict, Key).outcome, Match);
        assert_eq!(check(&verdict, Signature).outcome, Mismatch);
        assert!(
            verdict
                .reason
                .as_deref()
                .unwrap()
                .starts_with("the signature is not key 21fe31dfa154a261's signature"),
        );
    }
}

#[test]
fn another_key_s_signature_under_the_lab_s_key_id_does_not_verify() {
    // The receipt names the lab's key, and another key signed it.
    let forged = signed_by(&other_key(), receipt_json());
    let verdict = verify_signature(&forged);
    assert!(!verdict.verified);
    assert_eq!(check(&verdict, Signature).outcome, Mismatch);
}

#[test]
fn the_other_checks_are_not_made_when_no_key_signed_the_receipt() {
    let result = device_result();
    let forged = signed_by(&other_key(), receipt_json());
    let verdict = verify_all(&forged, &result, &build_id());
    for id in [
        KeyNotRetired,
        ResultSha256,
        ResultBuildId,
        ResultHash,
        BuildId,
    ] {
        let check = check(&verdict, id);
        assert_eq!(check.outcome, NotChecked, "{id}");
        assert_eq!(
            check.reason.as_deref(),
            Some("no given key signed the receipt"),
            "{id}"
        );
    }
}

#[test]
fn a_field_this_dyno_does_not_know_is_ignored_and_is_still_signed() {
    let mut receipt = receipt_json();
    receipt["added_later"] = json!({ "by": "a newer signer" });
    receipt["lab"]["city"] = "somewhere".into();
    let file = signed_by(&lab_key(), receipt);
    assert!(verify_signature(&file).verified);

    // Taking the field out again changes what was signed.
    let mut stripped = file.clone();
    stripped["receipt"]
        .as_object_mut()
        .unwrap()
        .remove("added_later");
    assert_eq!(
        check(&verify_signature(&stripped), Signature).outcome,
        Mismatch
    );
}

#[test]
fn a_field_outside_the_receipt_is_ignored_and_is_not_signed() {
    let mut file = receipt_file(&device_result());
    file["note"] = "not signed".into();
    assert!(verify_signature(&file).verified);
}

#[test]
fn the_spelling_of_the_receipt_file_does_not_matter() {
    // Other key order, other whitespace and other spellings of the same numbers.
    let file = receipt_file(&device_result());
    let pretty = serde_json::to_string_pretty(&file)
        .unwrap()
        .replace("\"receipt_version\": 1", "\"receipt_version\": 1 ")
        .replace(": 209715200", ": 2.097152e8");
    assert!(pretty.contains("2.097152e8"));
    let read: Value = serde_json::from_str(&pretty).unwrap();
    assert!(verify_signature(&read).verified);
}

#[test]
fn a_receipt_of_a_later_version_is_refused_even_with_a_good_signature() {
    let mut receipt = receipt_json();
    receipt["receipt_version"] = 2.into();
    let error = refused(&signed_by(&lab_key(), receipt));
    assert_eq!(
        error.to_string(),
        "it has receipt_version 2, but this dyno reads receipt_version 1: a newer dyno may read it"
    );
}

#[test]
fn a_receipt_with_another_algorithm_is_refused() {
    let mut file = receipt_file(&device_result());
    file["algorithm"] = "ecdsa-p256".into();
    assert_eq!(
        refused(&file),
        ReceiptError::AnotherAlgorithm("ecdsa-p256".to_owned())
    );
}

#[test]
fn json_that_is_not_a_receipt_is_refused_and_the_error_says_why() {
    let cases: [(Change, &str); 11] = [
        (|file| *file = json!([]), "no `receipt` object"),
        (
            |file| file["receipt"] = "text".into(),
            "no `receipt` object",
        ),
        (
            |file| file["receipt"]["receipt_version"] = "1".into(),
            "`receipt_version` is not a number",
        ),
        (
            |file| file["algorithm"] = Value::Null,
            "`algorithm` is not text",
        ),
        (
            |file| file["signature"] = "AAAA".into(),
            "`signature` is not 64 bytes",
        ),
        (
            |file| {
                let padded = format!("{}==", file["signature"].as_str().unwrap());
                file["signature"] = padded.into();
            },
            "`signature` is not 64 bytes",
        ),
        (
            |file| {
                file["receipt"].as_object_mut().unwrap().remove("issuer");
            },
            "missing field `issuer`",
        ),
        (
            |file| file["receipt"]["key_id"] = "21FE31DFA154A261".into(),
            "`key_id` is not 16 lower-case hex",
        ),
        (
            |file| file["receipt"]["build_id"] = "abc".into(),
            "`build_id` is not 64 lower-case hex",
        ),
        (
            |file| file["receipt"]["result_sha256"] = "".into(),
            "`result_sha256` is not 64 lower-case hex",
        ),
        (
            |file| file["receipt"]["issued_at"] = "2026-11-25T10:00:00.000Z".into(),
            "`issued_at` is not a UTC time to the second",
        ),
    ];
    for (change, why) in cases {
        let mut file = receipt_file(&device_result());
        change(&mut file);
        let error = refused(&file).to_string();
        assert!(error.starts_with("it is not a receipt: "), "{error}");
        assert!(error.contains(why), "{error}");
    }
}

#[test]
fn a_retired_key_verifies_a_receipt_issued_before_it_was_retired_and_the_verdict_says_so() {
    let keys = trusted_keys(&keys_document(Some("2026-12-01T00:00:00Z"))).unwrap();
    let file = receipt_file_issued(&device_result(), "2026-11-30T23:59:59Z");
    let verdict = verify(&file, &keys, Evidence::default()).unwrap();
    assert!(verdict.verified);
    let key = verdict.key.as_ref().unwrap();
    assert!(key.retired);
    assert_eq!(key.retired_at.as_deref(), Some("2026-12-01T00:00:00Z"));
    assert_eq!(key.created_at.as_deref(), Some("2026-11-01T00:00:00Z"));
    let not_retired = check(&verdict, KeyNotRetired);
    assert_eq!(not_retired.outcome, Match);
    assert_eq!(
        not_retired.expected.as_deref(),
        Some("2026-11-30T23:59:59Z")
    );
    assert_eq!(not_retired.actual.as_deref(), Some("2026-12-01T00:00:00Z"));
}

#[test]
fn a_retired_key_does_not_verify_a_receipt_issued_when_it_was_retired_or_later() {
    // The second time is the same instant as the first, with an offset and a fraction.
    for retired_at in ["2026-12-01T00:00:00Z", "2026-12-01T02:00:00.000+02:00"] {
        let keys = trusted_keys(&keys_document(Some(retired_at))).unwrap();
        for issued_at in ["2026-12-01T00:00:00Z", "2027-01-01T00:00:00Z"] {
            let file = receipt_file_issued(&device_result(), issued_at);
            let verdict = verify(&file, &keys, Evidence::default()).unwrap();
            assert!(!verdict.verified, "{issued_at}");
            assert_eq!(check(&verdict, Signature).outcome, Match);
            assert_eq!(check(&verdict, KeyNotRetired).outcome, Mismatch);
            assert_eq!(
                verdict.reason,
                Some(format!(
                    "the lab retired key 21fe31dfa154a261 at {retired_at}, and the receipt says \
                     it was issued at {issued_at}"
                ))
            );
            // The lab did sign it, so the verdict shows what it signed, and with which key.
            assert!(verdict.receipt.is_some());
            assert!(verdict.key.as_ref().unwrap().retired);
        }
    }
}

#[test]
fn a_key_in_use_from_the_keys_document_is_not_retired() {
    let keys = trusted_keys(&keys_document(None)).unwrap();
    let verdict = verify(&receipt_file(&device_result()), &keys, Evidence::default()).unwrap();
    assert!(verdict.verified);
    let key = verdict.key.as_ref().unwrap();
    assert!(!key.retired);
    assert_eq!(key.retired_at, None);
    assert_eq!(check(&verdict, KeyNotRetired).actual, None);
}

#[test]
fn the_lab_s_key_among_other_keys_and_given_twice_still_verifies() {
    let other = TrustedKey::from_base64url(&common::base64url(&other_key().public_key())).unwrap();
    let keys = [other, trusted_lab_key(), trusted_lab_key()];
    let verdict = verify(&receipt_file(&device_result()), &keys, Evidence::default()).unwrap();
    assert!(verdict.verified);
}

#[test]
fn a_result_file_that_is_not_the_one_signed_does_not_match() {
    let result = device_result();
    let file = receipt_file(&result);
    // The run part is outside result_hash, so only result_sha256 sees this change.
    let mut other_run = result.clone();
    other_run["run"]["measures"][2]["value"] = 104_857_600.into();
    let verdict = verify_all(&file, &other_run, &build_id());
    assert!(!verdict.verified);
    assert_eq!(
        outcomes(&verdict)[3..6],
        [
            (ResultSha256, Mismatch),
            (ResultBuildId, Match),
            (ResultHash, Match),
        ]
    );
    assert_eq!(
        verdict.reason.as_deref(),
        Some("the result file is not the one the lab signed: its content differs")
    );
    let sha256 = check(&verdict, ResultSha256);
    assert_ne!(sha256.actual, sha256.expected);
    assert_eq!(sha256.actual.as_ref().unwrap().len(), 64);
}

#[test]
fn a_result_of_another_build_or_with_another_hash_does_not_match() {
    let result = device_result();
    let file = receipt_file(&result);
    let mut other = result.clone();
    other["deterministic"]["build"]["build_id"] = "f".repeat(64).into();
    other["result_hash"] = "e".repeat(64).into();
    let verdict = verify_all(&file, &other, &build_id());
    assert!(!verdict.verified);
    assert_eq!(
        check(&verdict, ResultBuildId).reason,
        Some(format!(
            "the result is for build {}, and the receipt is for build {}",
            "f".repeat(64),
            build_id()
        ))
    );
    assert_eq!(
        check(&verdict, ResultHash).reason,
        Some(format!(
            "the result states the hash {}, and the receipt states {}",
            "e".repeat(64),
            result["result_hash"].as_str().unwrap()
        ))
    );
}

#[test]
fn json_that_is_not_a_result_has_no_build_id_and_no_hash_to_match() {
    let file = receipt_file(&device_result());
    let verdict = verify_all(&file, &json!({ "some": "json" }), &build_id());
    assert!(!verdict.verified);
    let build = check(&verdict, ResultBuildId);
    assert_eq!(build.actual, None);
    assert!(
        build
            .reason
            .as_ref()
            .unwrap()
            .starts_with("the result has no build id")
    );
    let hash = check(&verdict, ResultHash);
    assert!(
        hash.reason
            .as_ref()
            .unwrap()
            .starts_with("the result states the hash none, and the receipt states ")
    );
}

#[test]
fn a_receipt_that_states_no_result_hash_matches_a_result_with_none() {
    let mut receipt = receipt_json();
    receipt["result_hash"] = Value::Null;
    let file = signed_by(&lab_key(), receipt);
    let with_none = json!({ "some": "json" });
    let evidence = Evidence {
        result: Some(&with_none),
        build_id: None,
    };
    let verdict = verify(&file, &[trusted_lab_key()], evidence).unwrap();
    let hash = check(&verdict, ResultHash);
    assert_eq!(hash.outcome, Match);
    assert_eq!((&hash.expected, &hash.actual), (&None, &None));

    // And it does not match a result that states one.
    let result = device_result();
    let verdict = verify_all(&file, &result, &build_id());
    assert_eq!(
        check(&verdict, ResultHash).reason,
        Some(format!(
            "the result states the hash {}, and the receipt states none",
            result["result_hash"].as_str().unwrap()
        ))
    );
}

#[test]
fn files_with_another_build_id_do_not_match() {
    let result = device_result();
    let other_build = "0".repeat(64);
    let verdict = verify_all(&receipt_file(&result), &result, &other_build);
    assert!(!verdict.verified);
    assert_eq!(
        verdict.reason,
        Some(format!(
            "the files have build id {other_build}, and the receipt is for build {}",
            build_id()
        ))
    );
    assert_eq!(check(&verdict, BuildId).outcome, Mismatch);
    // The lab's receipt is real. It is for other files.
    assert!(verdict.receipt.is_some());
}

#[test]
fn a_signer_in_javascript_rounds_a_large_integer_and_the_receipt_still_verifies() {
    // JSON.stringify writes 2^53 + 1 as 9007199254740992, so those are the signed bytes. The
    // verdict states the number that was signed.
    let mut receipt = receipt_json();
    receipt["measures"][2]["value"] = 9_007_199_254_740_993_u64.into();
    receipt["protocol"]["large"] = json!([9_007_199_254_740_993_u64]);
    let file = signed_by(&lab_key(), receipt);
    let verdict = verify_signature(&file);
    assert!(verdict.verified);
    let signed = verdict.receipt.as_ref().unwrap();
    assert_eq!(
        signed.measures[2].value.as_ref().unwrap().as_f64(),
        Some(9_007_199_254_740_992.0)
    );
    let written = to_canonical_json(&verdict).unwrap();
    assert!(
        written.contains(r#""large":[9007199254740992]"#),
        "{written}"
    );
    assert!(written.contains(r#""value":9007199254740992"#), "{written}");
}

#[test]
fn a_receipt_from_a_signer_that_names_the_protocol_another_way_still_verifies() {
    let mut receipt = receipt_json();
    receipt["protocol"] = json!({ "profile": "reels@2", "runs": 3 });
    receipt["measures"] = json!([{ "id": "device.new_reading", "value": 1.5, "unit": null }]);
    let verdict = verify_signature(&signed_by(&lab_key(), receipt));
    assert!(verdict.verified);
    let signed = verdict.receipt.unwrap();
    assert_eq!(signed.protocol["profile"], "reels@2");
    assert_eq!(
        signed.measures[0].value.as_ref().unwrap().as_f64(),
        Some(1.5)
    );
    assert_eq!(signed.check_id, ISSUE.check_id);
}
