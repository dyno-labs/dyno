//! A result of a check with a run on a phone, as a lab's signer gets it, and a lab's key.

#![allow(dead_code)]

use dyno_receipt::keys::{SigningKey, TrustedKey};
use dyno_receipt::sign::{Issue, receipt_of_result, sign};
use dyno_schema::canonical::{jcs, sha256_hex};
use dyno_schema::environment::{DeviceKind, Environment, EnvironmentDevice, EnvironmentOs, OsName};
use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::{BuildManifest, FileKind, ManifestFile};
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue};
use dyno_schema::path::RelPath;
use dyno_schema::profile::{HostProfile, Scenario};
use dyno_schema::receipt::{PublicKey, PublicKeys};
use dyno_schema::result::{
    BuildLink, CheckResult, Deterministic, DeviceClass, Inputs, Machine, Release, RunInfo, Target,
    TierReport, ToolBuild, Versions,
};
use dyno_schema::status::{RunCoverage, Status};
use serde_json::Value;

/// The dyno version the test results record.
pub const TOOL_VERSION: &str = "0.0.0-test";

/// The build id of the test results: a SHA-256, as a build id is.
pub fn build_id() -> String {
    sha256_hex(b"the files of a build")
}

/// The lab's key: the first test key of RFC 8032, section 7.1.
pub fn lab_key() -> SigningKey {
    SigningKey::from_seed([
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ])
}

/// The key id of [`lab_key`].
pub const LAB_KEY_ID: &str = "21fe31dfa154a261";

/// The public key of [`lab_key`], as `--key` takes it.
pub const LAB_PUBLIC_KEY: &str = "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo";

/// Another key, which signed nothing here.
pub fn other_key() -> SigningKey {
    SigningKey::from_seed([7; 32])
}

/// What the lab says about the run.
pub const ISSUE: Issue<'static> = Issue {
    issuer: "lab.example",
    check_id: "chk_0001",
    issued_at: "2026-11-25T10:00:00Z",
    lab_kind: "own_phones",
    lab_backend: None,
};

fn device_measure(id: MeasureId, value: i64) -> Measure {
    Measure {
        required: true,
        value: Some(MeasureValue::Integer(value)),
        ..Measure::blank(id, MeasureStatus::Measured)
    }
}

/// The two parts of a check of a build with three runs on an iPhone 14 Pro Max.
pub fn device_parts() -> (Deterministic, RunInfo) {
    let path = RelPath::new("index.html").unwrap();
    let deterministic = Deterministic {
        versions: Versions {
            schema: 0,
            tool: TOOL_VERSION.to_owned(),
            scoring: 0,
            model: None,
        },
        inputs: Inputs {
            build: true,
            launch_url: true,
            host_profile: HostProfile::reels().reference(),
            budget: None,
            scenario: Scenario::Alone,
            spins: None,
            device: true,
            device_runs: Some(3),
        },
        build: Some(BuildManifest {
            build_id: Some(build_id()),
            files: vec![ManifestFile {
                sha256: sha256_hex(b"<!doctype html>"),
                path,
                bytes: 15,
                kind: FileKind::Html,
            }],
        }),
        status: Status::Pass,
        run_coverage: RunCoverage::Complete,
        measures: vec![Measure {
            value: Some(MeasureValue::Integer(1)),
            ..Measure::blank(MeasureId::StaticInventoryFiles, MeasureStatus::Estimated)
        }],
        verdicts: Vec::new(),
        findings: Vec::new(),
    };
    let run = RunInfo {
        run_id: Some("0b1f8f8e-5d3a-4c7e-9a52-1f2d3c4b5a69".to_owned()),
        build_link: Some(BuildLink::Served),
        environment: Some(Environment {
            browser: None,
            os: Some(EnvironmentOs {
                name: OsName::Ios,
                version: Some("26.6.2 (23G90)".to_owned()),
            }),
            device: Some(EnvironmentDevice {
                key: Some("iphone-14-pro-max".to_owned()),
                name: "iPhone 14 Pro Max".to_owned(),
                vendor: "Apple".to_owned(),
                kind: DeviceKind::Phone,
                memory_bytes: Some(5_905_580_032),
            }),
        }),
        serve_path: None,
        lab: None,
        started_at: "2026-11-25T09:58:00.000Z".to_owned(),
        duration_ms: 90_000,
        machine: Machine {
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            cpus: 8,
        },
        tool_build: ToolBuild {
            version: TOOL_VERSION.to_owned(),
            git_sha: None,
            profile: "release".to_owned(),
        },
        target: Target {
            build: Some("dist".to_owned()),
            launch_url: None,
        },
        release: Release::default(),
        driver: None,
        measures: vec![
            device_measure(MeasureId::DeviceKills, 0),
            device_measure(MeasureId::DevicePhysFootprint, 209_715_200),
            device_measure(MeasureId::DevicePhysFootprintPeak, 220_200_960),
            device_measure(MeasureId::DeviceFootprintOverBaseline, 146_800_640),
            // A browser measure, which no receipt lists.
            Measure::missing(MeasureId::LabJsHeap, "Safari gives no JS heap"),
        ],
        findings: Vec::new(),
        tier: TierReport {
            status: MeasureStatus::Unsupported,
            tier: None,
            device_class: DeviceClass::ReferenceIphone,
            phase: 1,
            reason: None,
        },
        timeline: None,
    };
    (deterministic, run)
}

/// The JSON of a result file with these two parts, as dyno writes it.
pub fn result_of(deterministic: Deterministic, run: RunInfo) -> Value {
    serde_json::to_value(CheckResult::new(deterministic, run).unwrap()).unwrap()
}

/// The JSON of the result file of [`device_parts`].
pub fn device_result() -> Value {
    let (deterministic, run) = device_parts();
    result_of(deterministic, run)
}

/// The receipt file's JSON for a result, signed by [`lab_key`].
pub fn receipt_file(result: &Value) -> Value {
    receipt_file_issued(result, ISSUE.issued_at)
}

/// The same, issued at another time.
pub fn receipt_file_issued(result: &Value, issued_at: &str) -> Value {
    let issue = Issue { issued_at, ..ISSUE };
    let receipt = receipt_of_result(result, &issue, LAB_KEY_ID).unwrap();
    serde_json::to_value(sign(receipt, &lab_key()).unwrap()).unwrap()
}

/// A receipt file's JSON for any `receipt` object, signed by `key` over its canonical JSON, as
/// a signer in another language would make it.
pub fn signed_by(key: &SigningKey, receipt: Value) -> Value {
    let signature = key.sign(jcs(&receipt).unwrap().as_bytes());
    serde_json::json!({
        "receipt": receipt,
        "algorithm": "ed25519",
        "signature": base64url(&signature),
    })
}

/// Base64url with no padding, written here so that the tests do not lean on the code they test.
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let mut group = [0_u8; 3];
        group[..chunk.len()].copy_from_slice(chunk);
        let bits = u32::from(group[0]) << 16 | u32::from(group[1]) << 8 | u32::from(group[2]);
        for index in 0..=chunk.len() {
            let sextet = (bits >> (18 - 6 * index)) & 0x3f;
            text.push(char::from(ALPHABET[sextet as usize]));
        }
    }
    text
}

/// The lab's key as the caller trusts it, given alone.
pub fn trusted_lab_key() -> TrustedKey {
    TrustedKey::from_base64url(LAB_PUBLIC_KEY).unwrap()
}

/// The lab's keys document, with [`lab_key`] retired at `retired_at`, or not retired.
pub fn keys_document(retired_at: Option<&str>) -> PublicKeys {
    PublicKeys {
        keys: vec![PublicKey {
            retired_at: retired_at.map(str::to_owned),
            ..lab_key().public_entry("2026-11-01T00:00:00Z")
        }],
    }
}
