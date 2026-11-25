//! Making a receipt for a result, and signing it.

mod common;

use common::{
    ISSUE, LAB_KEY_ID, LAB_PUBLIC_KEY, TOOL_VERSION, build_id, device_parts, device_result,
    lab_key, result_of,
};
use dyno_receipt::keys::TrustedKey;
use dyno_receipt::sign::{Issue, SignError, receipt_of_result, sign};
use dyno_schema::canonical::{jcs, sha256_hex, to_canonical_json};
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue};
use dyno_schema::result::{Deterministic, RunInfo};
use serde_json::{Value, json};

/// The result of [`device_parts`] after a change to them.
fn result_with(change: impl FnOnce(&mut Deterministic, &mut RunInfo)) -> Value {
    let (mut deterministic, mut run) = device_parts();
    change(&mut deterministic, &mut run);
    result_of(deterministic, run)
}

fn error_for(result: &Value) -> SignError {
    receipt_of_result(result, &ISSUE, LAB_KEY_ID).unwrap_err()
}

fn run_measure(run: &mut RunInfo, id: MeasureId) -> &mut Measure {
    run.measures
        .iter_mut()
        .find(|measure| measure.id == id)
        .unwrap()
}

#[test]
fn a_receipt_takes_each_value_from_the_result_s_own_fields() {
    let result = device_result();
    let receipt = receipt_of_result(&result, &ISSUE, LAB_KEY_ID).unwrap();
    assert_eq!(
        serde_json::to_value(&receipt).unwrap(),
        json!({
            "receipt_version": 1,
            "issuer": "lab.example",
            "key_id": LAB_KEY_ID,
            "issued_at": "2026-11-25T10:00:00Z",
            "check_id": "chk_0001",
            "build_id": build_id(),
            "result_sha256": sha256_hex(jcs(&result).unwrap().as_bytes()),
            "result_hash": result["result_hash"],
            "dyno_version": TOOL_VERSION,
            "lab": { "kind": "own_phones", "backend": null },
            "device": {
                "key": "iphone-14-pro-max",
                "model": "iPhone 14 Pro Max",
                "os_version": "26.6.2 (23G90)"
            },
            "protocol": {
                "host_profile": { "name": "reels", "version": 2 },
                "scenario": "alone",
                "spins": null,
                "device_runs": 3
            },
            "measures": [
                { "id": "device.footprint_over_baseline", "value": 146_800_640, "unit": "bytes_change" },
                { "id": "device.kills", "value": 0, "unit": "count" },
                { "id": "device.phys_footprint", "value": 209_715_200, "unit": "bytes" },
                { "id": "device.phys_footprint_peak", "value": 220_200_960, "unit": "bytes" }
            ]
        })
    );
}

#[test]
fn the_lab_s_backend_is_in_the_receipt_when_the_lab_names_it() {
    let issue = Issue {
        lab_kind: "device_cloud",
        lab_backend: Some("a device cloud"),
        ..ISSUE
    };
    let receipt = receipt_of_result(&device_result(), &issue, LAB_KEY_ID).unwrap();
    assert_eq!(receipt.lab.kind, "device_cloud");
    assert_eq!(receipt.lab.backend.as_deref(), Some("a device cloud"));
}

#[test]
fn result_sha256_covers_the_whole_file_and_result_hash_only_the_deterministic_part() {
    let result = device_result();
    let receipt = receipt_of_result(&result, &ISSUE, LAB_KEY_ID).unwrap();
    // A later run of the same build: the run part differs, and so does a field that this dyno
    // does not know.
    let mut later = result.clone();
    later["run"]["started_at"] = "2026-11-26T09:58:00.000Z".into();
    later["run"]["added_later"] = "by a newer dyno".into();
    let later_receipt = receipt_of_result(&later, &ISSUE, LAB_KEY_ID).unwrap();
    assert_eq!(later_receipt.result_hash, receipt.result_hash);
    assert_ne!(later_receipt.result_sha256, receipt.result_sha256);
    assert_eq!(
        later_receipt.result_sha256,
        sha256_hex(jcs(&later).unwrap().as_bytes())
    );
}

#[test]
fn a_run_on_one_phone_with_no_runs_flag_and_no_os_version_gives_nulls() {
    let result = result_with(|deterministic, run| {
        deterministic.inputs.device_runs = None;
        deterministic.inputs.spins = Some(20);
        run.environment.as_mut().unwrap().os = None;
    });
    let receipt = receipt_of_result(&result, &ISSUE, LAB_KEY_ID).unwrap();
    assert_eq!(receipt.device.os_version, None);
    assert_eq!(receipt.protocol["device_runs"], Value::Null);
    assert_eq!(receipt.protocol["spins"], 20);
}

#[test]
fn a_device_measure_with_no_reading_has_a_null_value() {
    let result = result_with(|_, run| {
        *run_measure(run, MeasureId::DeviceFootprintOverBaseline) = Measure {
            required: true,
            ..Measure::missing(MeasureId::DeviceFootprintOverBaseline, "needs --runs")
        };
        // An invalid measure keeps the value that failed its check. A receipt has no status, so
        // it must not state that value as a reading.
        let kills = run_measure(run, MeasureId::DeviceKills);
        kills.status = MeasureStatus::Invalid;
        kills.value = Some(MeasureValue::Integer(-1));
    });
    let receipt = receipt_of_result(&result, &ISSUE, LAB_KEY_ID).unwrap();
    let values: Vec<(&str, Option<String>)> = receipt
        .measures
        .iter()
        .map(|measure| {
            (
                measure.id.as_str(),
                measure.value.as_ref().map(ToString::to_string),
            )
        })
        .collect();
    assert_eq!(
        values,
        [
            ("device.footprint_over_baseline", None),
            ("device.kills", None),
            ("device.phys_footprint", Some("209715200".to_owned())),
            ("device.phys_footprint_peak", Some("220200960".to_owned())),
        ]
    );
}

#[test]
fn a_result_that_was_edited_gets_no_receipt() {
    let mut edited = device_result();
    edited["deterministic"]["status"] = "warn".into();
    assert!(matches!(error_for(&edited), SignError::Edited));
}

#[test]
fn a_result_with_no_build_gets_no_receipt() {
    let no_build = result_with(|deterministic, _| deterministic.build = None);
    let no_build_id = result_with(|deterministic, _| {
        deterministic.build.as_mut().unwrap().build_id = None;
    });
    for result in [no_build, no_build_id] {
        let error = error_for(&result);
        assert!(matches!(error, SignError::NoBuild), "{error}");
        assert!(error.to_string().contains("deterministic.build.build_id"));
    }
}

#[test]
fn a_result_with_no_device_gets_no_receipt() {
    let no_environment = result_with(|_, run| run.environment = None);
    let no_device = result_with(|_, run| run.environment.as_mut().unwrap().device = None);
    // A browser's user agent names only "iPhone", so the device has no key.
    let no_key = result_with(|_, run| {
        let environment = run.environment.as_mut().unwrap();
        environment.device.as_mut().unwrap().key = None;
    });
    for result in [no_environment, no_device, no_key] {
        let error = error_for(&result);
        assert!(matches!(error, SignError::NoDevice), "{error}");
        assert!(error.to_string().contains("run.environment.device.key"));
    }
}

#[test]
fn a_result_of_another_schema_version_gets_no_receipt_and_the_error_names_it() {
    let mut newer = device_result();
    newer["schema_version"] = 7.into();
    let error = error_for(&newer);
    assert!(matches!(error, SignError::AnotherSchema(7)), "{error}");
    assert!(error.to_string().contains("schema_version 7"));
}

#[test]
fn json_that_is_not_a_result_gets_no_receipt() {
    for json in [json!({}), json!([1, 2]), json!({ "schema_version": 0 })] {
        let error = error_for(&json);
        assert!(matches!(error, SignError::NotAResult(_)), "{error}");
    }
}

#[test]
fn an_empty_issuer_check_id_or_lab_kind_is_refused() {
    let result = device_result();
    let cases = [
        (
            Issue {
                issuer: " ",
                ..ISSUE
            },
            "issuer",
        ),
        (
            Issue {
                check_id: "",
                ..ISSUE
            },
            "check id",
        ),
        (
            Issue {
                lab_kind: "",
                ..ISSUE
            },
            "lab kind",
        ),
    ];
    for (issue, name) in cases {
        let error = receipt_of_result(&result, &issue, LAB_KEY_ID).unwrap_err();
        assert_eq!(error.to_string(), format!("the {name} must not be empty"));
    }
}

#[test]
fn a_time_that_is_not_utc_to_the_second_is_refused() {
    let result = device_result();
    for issued_at in [
        "2026-11-25T10:00:00.000Z",
        "2026-11-25T11:00:00+01:00",
        "now",
    ] {
        let issue = Issue { issued_at, ..ISSUE };
        let error = receipt_of_result(&result, &issue, LAB_KEY_ID).unwrap_err();
        assert!(matches!(error, SignError::NotAReceiptTime(_)), "{error}");
    }
}

#[test]
fn the_signature_is_the_key_s_over_the_receipt_s_canonical_json() {
    let receipt = receipt_of_result(&device_result(), &ISSUE, LAB_KEY_ID).unwrap();
    let canonical = to_canonical_json(&receipt).unwrap();
    let envelope = sign(receipt.clone(), &lab_key()).unwrap();
    assert_eq!(envelope.algorithm, "ed25519");
    assert_eq!(envelope.receipt, receipt);
    assert_eq!(
        envelope.signature,
        common::base64url(&lab_key().sign(canonical.as_bytes()))
    );
    // 64 bytes are 86 base64url characters, with no padding.
    assert_eq!(envelope.signature.len(), 86);
    assert!(!envelope.signature.contains('='));
    let signature = lab_key().sign(canonical.as_bytes());
    let key = TrustedKey::from_base64url(LAB_PUBLIC_KEY).unwrap();
    assert!(key.signed(canonical.as_bytes(), &signature));
}

#[test]
fn the_same_receipt_and_key_always_give_the_same_signature() {
    let receipt = receipt_of_result(&device_result(), &ISSUE, LAB_KEY_ID).unwrap();
    let once = sign(receipt.clone(), &lab_key()).unwrap();
    let again = sign(receipt, &lab_key()).unwrap();
    assert_eq!(once, again);
}
