//! The result hash: SHA-256 over the canonical JSON of the deterministic part only.

use dyno_schema::budget::{BudgetLimit, BudgetVerdict};
use dyno_schema::canonical::{
    MAX_SAFE_INTEGER, sha256_hex, sha256_of_canonical_json, to_canonical_json,
};
use dyno_schema::environment::{
    BrowserEngine, DeviceKind, Environment, EnvironmentBrowser, EnvironmentDevice, EnvironmentOs,
    OsName,
};
use dyno_schema::ids::{BudgetId, MeasureId, RuleId};
use dyno_schema::manifest::{BuildManifest, FileKind, ManifestFile};
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, NOT_BUILT_YET,
};
use dyno_schema::path::RelPath;
use dyno_schema::profile::{HostProfile, Scenario};
use dyno_schema::result::{
    BuildLink, CheckResult, Deterministic, DeviceClass, Inputs, LoadedResult, Machine, Release,
    ResultError, RunInfo, Target, TierReport, ToolBuild, Versions, raw_hash_matches,
};
use dyno_schema::rule::{Finding, Fix, FixAction};
use dyno_schema::status::{RunCoverage, Status, Verdict};
use serde_json::Value;

/// The files of one build, as a directory walk might list them on some platform.
const WALK: &[(&str, u64)] = &[
    ("index.html", 512),
    ("js/main.js", 2_048),
    ("img/hero.png", 4_096),
    ("img/ui/button.png", 256),
];

/// The bytes by type items of the build, out of key order.
const ITEMS: &[(&str, i64)] = &[("html", 512), ("js", 2_048), ("image", 4_352)];

/// The measures of the build, out of id order.
const MEASURE_ORDER: &[MeasureId] = &[
    MeasureId::StaticBytesByType,
    MeasureId::StaticAudioDecoded48k,
    MeasureId::StaticInventoryFiles,
];

/// The tool version the test results record.
const TOOL_VERSION: &str = "0.0.0-test";

/// When the reference result started.
const REFERENCE_TIME: &str = "2026-11-06T10:00:00.000Z";

/// The smallest integer above the safe range: 2^53.
const FIRST_UNSAFE: i64 = MAX_SAFE_INTEGER + 1;

/// A manifest entry. The file's hash stands in for its content, so it comes from the clean path.
fn manifest_file(path: &str, bytes: u64) -> ManifestFile {
    let path = RelPath::new(path).unwrap();
    let sha256 = sha256_hex(path.as_str().as_bytes());
    ManifestFile {
        path,
        bytes,
        sha256,
        kind: FileKind::Other,
    }
}

fn bytes_by_type(items: &[(&str, i64)]) -> Measure {
    let mut measure = Measure::blank(MeasureId::StaticBytesByType, MeasureStatus::Estimated);
    measure.value = Some(MeasureValue::Integer(
        items.iter().map(|(_, bytes)| bytes).sum(),
    ));
    measure.items = items
        .iter()
        .map(|(kind, bytes)| MeasureItem {
            key: (*kind).to_owned(),
            value: MeasureValue::Integer(*bytes),
        })
        .collect();
    measure
}

/// A deterministic part with the files, items and measures in the order given. The verdicts and
/// findings are always out of order, so the sort has work to do.
fn deterministic(
    walk: &[(&str, u64)],
    items: &[(&str, i64)],
    measure_order: &[MeasureId],
) -> Deterministic {
    let measures: Vec<Measure> = measure_order
        .iter()
        .map(|&id| {
            if id == MeasureId::StaticBytesByType {
                bytes_by_type(items)
            } else {
                Measure::missing(id, NOT_BUILT_YET)
            }
        })
        .collect();
    Deterministic {
        versions: Versions {
            schema: 0,
            tool: TOOL_VERSION.to_owned(),
            scoring: 0,
            model: None,
        },
        inputs: Inputs {
            build: true,
            launch_url: false,
            host_profile: HostProfile::reels().reference(),
            budget: None,
            scenario: Scenario::Alone,
            spins: None,
            device: false,
            device_runs: None,
        },
        build: Some(BuildManifest {
            build_id: Some(sha256_hex(b"build")),
            files: walk
                .iter()
                .map(|(path, bytes)| manifest_file(path, *bytes))
                .collect(),
        }),
        status: Status::Incomplete,
        run_coverage: RunCoverage::NotRequested,
        measures,
        verdicts: vec![
            BudgetVerdict {
                budget: BudgetId::TexturesDecodedMb,
                measure: MeasureId::StaticTexturesDecoded,
                verdict: Verdict::Incomplete,
                actual: None,
                limit: BudgetLimit::Bytes {
                    warn_above: None,
                    fail_above: Some(1),
                },
            },
            BudgetVerdict {
                budget: BudgetId::AudioDecodedMb,
                measure: MeasureId::StaticAudioDecoded48k,
                verdict: Verdict::Incomplete,
                actual: None,
                limit: BudgetLimit::Bytes {
                    warn_above: None,
                    fail_above: Some(1),
                },
            },
        ],
        findings: vec![
            Finding::incomplete(RuleId::FilesDuplicate, NOT_BUILT_YET),
            Finding::incomplete(RuleId::AudioStereo, NOT_BUILT_YET),
        ],
    }
}

fn measure_mut(deterministic: &mut Deterministic, id: MeasureId) -> &mut Measure {
    deterministic
        .measures
        .iter_mut()
        .find(|measure| measure.id == id)
        .unwrap()
}

fn run_info(started_at: &str, os: &str) -> RunInfo {
    RunInfo {
        run_id: Some("0b1f8f8e-5d3a-4c7e-9a52-1f2d3c4b5a69".to_owned()),
        build_link: Some(BuildLink::Scanned),
        environment: None,
        started_at: started_at.to_owned(),
        duration_ms: 12,
        machine: Machine {
            os: os.to_owned(),
            arch: "aarch64".to_owned(),
            cpus: 8,
        },
        tool_build: ToolBuild {
            version: TOOL_VERSION.to_owned(),
            git_sha: None,
            profile: "debug".to_owned(),
        },
        target: Target {
            build: Some("dist".to_owned()),
            launch_url: None,
        },
        release: Release::default(),
        driver: None,
        measures: Vec::new(),
        findings: Vec::new(),
        tier: TierReport {
            status: MeasureStatus::Unsupported,
            tier: None,
            device_class: DeviceClass::ReferenceIphone,
            phase: 1,
            reason: None,
        },
        timeline: None,
    }
}

fn reference_result() -> CheckResult {
    CheckResult::new(
        deterministic(WALK, ITEMS, MEASURE_ORDER),
        run_info(REFERENCE_TIME, "macos"),
    )
    .unwrap()
}

/// The reference parts with something in every list, findings that share a rule in each part,
/// and a finding whose measures are out of order. So only a complete sort key gives one order.
fn parts_with_every_list_filled() -> (Deterministic, RunInfo) {
    let mut deterministic = deterministic(WALK, ITEMS, MEASURE_ORDER);
    let bytes = measure_mut(&mut deterministic, MeasureId::StaticBytesByType);
    bytes.evidence = vec![
        Evidence {
            method: Method::FileSize,
            detail: "every file".to_owned(),
        },
        Evidence {
            method: Method::FileHash,
            detail: "every file".to_owned(),
        },
    ];
    bytes.scope.not_covered = vec![
        "z.bin: unreadable".to_owned(),
        "a.bin: unreadable".to_owned(),
    ];
    let mut first_copies = Finding::incomplete(RuleId::FilesDuplicate, "first");
    first_copies.fixes = vec![remove_copy("b.png", 2), remove_copy("a.png", 1)];
    let mut second_copies = Finding::incomplete(RuleId::FilesDuplicate, "second");
    second_copies.fixes = vec![remove_copy("d.png", 4), remove_copy("c.png", 3)];
    // The rule lists the 48 kHz measure before the 24 kHz one, out of key order.
    let pcm = Finding::incomplete(RuleId::AudioPcmOverBudget, "no audio summary");
    deterministic
        .findings
        .extend([first_copies, second_copies, pcm]);

    let mut run = run_info(REFERENCE_TIME, "macos");
    run.measures = vec![
        Measure::missing(MeasureId::LabTimeReady, NOT_BUILT_YET),
        Measure::missing(MeasureId::LabJsHeap, NOT_BUILT_YET),
    ];
    run.findings = vec![
        Finding::incomplete(RuleId::ContractReadyInTime, "no ready message"),
        Finding::incomplete(RuleId::ContractReadyInTime, "the run stopped"),
    ];
    (deterministic, run)
}

/// A fix that removes a copy of a file.
fn remove_copy(path: &str, saves_bytes: u64) -> Fix {
    Fix {
        action: FixAction::RemoveDuplicate,
        path: Some(RelPath::new(path).unwrap()),
        saves_bytes,
        saves_decoded_bytes: None,
    }
}

/// Reverses every list in both parts, and every list inside their items.
fn reverse_every_list(deterministic: &mut Deterministic, run: &mut RunInfo) {
    deterministic.build.as_mut().unwrap().files.reverse();
    for measures in [&mut deterministic.measures, &mut run.measures] {
        measures.reverse();
        for measure in measures {
            measure.items.reverse();
            measure.evidence.reverse();
            measure.scope.not_covered.reverse();
        }
    }
    deterministic.verdicts.reverse();
    for findings in [&mut deterministic.findings, &mut run.findings] {
        findings.reverse();
        for finding in findings {
            finding.measures.reverse();
            finding.fixes.reverse();
        }
    }
}

/// A reference result whose decoded audio was estimated at `bytes`.
fn result_with_audio(bytes: i64) -> CheckResult {
    let mut deterministic = deterministic(WALK, ITEMS, MEASURE_ORDER);
    let audio = measure_mut(&mut deterministic, MeasureId::StaticAudioDecoded48k);
    audio.status = MeasureStatus::Estimated;
    audio.reason = None;
    audio.value = Some(MeasureValue::Integer(bytes));
    CheckResult::new(deterministic, run_info(REFERENCE_TIME, "macos")).unwrap()
}

fn reference_json() -> Value {
    serde_json::to_value(reference_result()).unwrap()
}

/// Sets `result_hash` to the hash of the `deterministic` JSON, as the dyno that wrote it would.
fn rehash(json: &mut Value) {
    json["result_hash"] = sha256_of_canonical_json(&json["deterministic"])
        .unwrap()
        .into();
}

#[test]
fn the_hash_is_sha256_of_the_canonical_deterministic_part() {
    let result = reference_result();
    let canonical = to_canonical_json(&result.deterministic).unwrap();
    assert_eq!(result.result_hash, sha256_hex(canonical.as_bytes()));
    assert_eq!(result.result_hash.len(), 64);
}

#[test]
fn reversing_every_list_gives_the_same_result_and_hash() {
    let (mut deterministic, mut run) = parts_with_every_list_filled();
    let forward = CheckResult::new(deterministic.clone(), run.clone()).unwrap();
    reverse_every_list(&mut deterministic, &mut run);
    let backward = CheckResult::new(deterministic, run).unwrap();
    assert_eq!(backward.result_hash, forward.result_hash);
    assert_eq!(backward, forward);
}

#[test]
fn a_measure_twice_in_one_part_is_rejected() {
    let mut deterministic = deterministic(WALK, ITEMS, MEASURE_ORDER);
    deterministic
        .measures
        .push(Measure::missing(MeasureId::StaticInventoryFiles, "again"));
    let error = CheckResult::new(deterministic, run_info(REFERENCE_TIME, "macos")).unwrap_err();
    assert!(matches!(
        error,
        ResultError::DuplicateMeasure(MeasureId::StaticInventoryFiles)
    ));
}

#[test]
fn a_measure_in_both_parts_is_rejected() {
    let mut run = run_info(REFERENCE_TIME, "macos");
    run.measures
        .push(Measure::missing(MeasureId::StaticInventoryFiles, "again"));
    let error = CheckResult::new(deterministic(WALK, ITEMS, MEASURE_ORDER), run).unwrap_err();
    assert!(matches!(
        error,
        ResultError::DuplicateMeasure(MeasureId::StaticInventoryFiles)
    ));
}

#[test]
fn a_budget_with_two_verdicts_is_rejected() {
    let mut deterministic = deterministic(WALK, ITEMS, MEASURE_ORDER);
    let again = deterministic.verdicts[0].clone();
    let budget = again.budget;
    deterministic.verdicts.push(again);
    let error = CheckResult::new(deterministic, run_info(REFERENCE_TIME, "macos")).unwrap_err();
    assert!(matches!(error, ResultError::DuplicateVerdict(id) if id == budget));
}

#[test]
fn a_build_file_twice_is_rejected_even_with_another_separator() {
    let mut walk = WALK.to_vec();
    walk.push(("img\\hero.png", 1));
    let deterministic = deterministic(&walk, ITEMS, MEASURE_ORDER);
    let error = CheckResult::new(deterministic, run_info(REFERENCE_TIME, "macos")).unwrap_err();
    assert!(
        matches!(&error, ResultError::DuplicateFile(path) if path.as_str() == "img/hero.png"),
        "{error}"
    );
}

#[test]
fn the_hash_does_not_depend_on_path_separators() {
    let windows_walk: Vec<(String, u64)> = WALK
        .iter()
        .map(|(path, bytes)| (path.replace('/', "\\"), *bytes))
        .collect();
    let windows_walk: Vec<(&str, u64)> = windows_walk
        .iter()
        .map(|(path, bytes)| (path.as_str(), *bytes))
        .collect();
    let from_windows = deterministic(&windows_walk, ITEMS, MEASURE_ORDER);
    let result = CheckResult::new(from_windows, run_info(REFERENCE_TIME, "macos")).unwrap();
    assert_eq!(result.result_hash, reference_result().result_hash);
}

#[test]
fn the_hash_excludes_the_run_part() {
    let later_on_linux = CheckResult::new(
        deterministic(WALK, ITEMS, MEASURE_ORDER),
        run_info("2026-11-07T23:59:59.999Z", "linux"),
    )
    .unwrap();
    assert_eq!(later_on_linux.result_hash, reference_result().result_hash);
}

#[test]
fn the_hash_excludes_the_run_id_build_link_environment_and_game_id() {
    let mut run = run_info(REFERENCE_TIME, "macos");
    run.run_id = Some("5c0a1d2e-3f40-4a51-8b62-7c83d94ea5f6".to_owned());
    run.build_link = Some(BuildLink::Served);
    run.release.game_id = Some("6f1c2a8e-0000-4000-8000-000000000000".to_owned());
    run.environment = Some(Environment {
        browser: Some(EnvironmentBrowser {
            name: "Safari".to_owned(),
            version: Some("26.2".to_owned()),
            engine: Some(BrowserEngine::Webkit),
            headless: Some(false),
        }),
        os: Some(EnvironmentOs {
            name: OsName::Ios,
            version: Some("26.6.2".to_owned()),
        }),
        device: Some(EnvironmentDevice {
            key: Some("iphone-14-pro-max".to_owned()),
            name: "iPhone 14 Pro Max".to_owned(),
            vendor: "Apple".to_owned(),
            kind: DeviceKind::Phone,
            memory_bytes: Some(6_442_450_944),
        }),
    });
    let result = CheckResult::new(deterministic(WALK, ITEMS, MEASURE_ORDER), run).unwrap();
    assert_eq!(result.result_hash, reference_result().result_hash);
}

#[test]
fn a_result_from_before_the_run_identity_fields_still_reads_and_matches() {
    let mut json = reference_json();
    let run = json["run"].as_object_mut().unwrap();
    for field in ["run_id", "build_link", "environment"] {
        run.remove(field).unwrap();
    }
    run["release"]
        .as_object_mut()
        .unwrap()
        .remove("game_id")
        .unwrap();
    let loaded = LoadedResult::from_json(json).unwrap();
    assert!(loaded.hash_matches);
    assert_eq!(loaded.result.run.run_id, None);
    assert_eq!(loaded.result.run.build_link, None);
    assert_eq!(loaded.result.run.environment, None);
    assert_eq!(loaded.result.run.release.game_id, None);
}

#[test]
fn device_runs_are_left_out_of_the_json_and_the_hash_when_not_given() {
    let reference = reference_json();
    assert!(
        reference["deterministic"]["inputs"]
            .get("device_runs")
            .is_none()
    );
    let mut with_runs = deterministic(WALK, ITEMS, MEASURE_ORDER);
    with_runs.inputs.device_runs = Some(3);
    let result = CheckResult::new(with_runs, run_info(REFERENCE_TIME, "macos")).unwrap();
    assert_ne!(result.result_hash, reference_result().result_hash);
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["deterministic"]["inputs"]["device_runs"], 3);
}

#[test]
fn the_hash_changes_when_a_deterministic_value_changes() {
    let mut bigger_files = WALK.to_vec();
    bigger_files[0].1 += 1;
    let changed = CheckResult::new(
        deterministic(&bigger_files, ITEMS, MEASURE_ORDER),
        run_info(REFERENCE_TIME, "macos"),
    )
    .unwrap();
    assert_ne!(changed.result_hash, reference_result().result_hash);
}

#[test]
fn integers_from_2_to_the_53_hash_apart_by_the_reason_that_names_them() {
    let two_to_the_53 = result_with_audio(FIRST_UNSAFE);
    let one_more = result_with_audio(FIRST_UNSAFE + 1);
    assert_ne!(two_to_the_53.result_hash, one_more.result_hash);
}

#[test]
fn an_integer_beyond_2_to_the_53_makes_the_measure_invalid_and_is_dropped() {
    let one_more = result_with_audio(FIRST_UNSAFE + 1);
    let audio = one_more
        .deterministic
        .measures
        .iter()
        .find(|measure| measure.id == MeasureId::StaticAudioDecoded48k)
        .unwrap();
    assert_eq!(audio.status, MeasureStatus::Invalid);
    assert_eq!(audio.value, None);
    let reason = audio.reason.as_deref().unwrap();
    assert!(reason.contains(&(FIRST_UNSAFE + 1).to_string()), "{reason}");
}

#[test]
fn lists_come_out_sorted_by_their_sort_keys() {
    let result = reference_result();
    let deterministic = &result.deterministic;
    let paths: Vec<&str> = deterministic
        .build
        .as_ref()
        .unwrap()
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "img/hero.png",
            "img/ui/button.png",
            "index.html",
            "js/main.js"
        ]
    );
    let ids: Vec<&str> = deterministic
        .measures
        .iter()
        .map(|measure| measure.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "static.audio.decoded_48k",
            "static.bytes.by_type",
            "static.inventory.files"
        ]
    );
    let budgets: Vec<&str> = deterministic
        .verdicts
        .iter()
        .map(|verdict| verdict.budget.as_str())
        .collect();
    assert_eq!(budgets, ["audio.decoded_mb", "textures.decoded_mb"]);
    let rules: Vec<&str> = deterministic
        .findings
        .iter()
        .map(|finding| finding.rule.as_str())
        .collect();
    assert_eq!(rules, ["audio.stereo", "files.duplicate"]);
}

#[test]
fn a_result_survives_a_json_round_trip() {
    let result = reference_result();
    let text = to_canonical_json(&result).unwrap();
    let loaded = LoadedResult::from_json(serde_json::from_str(&text).unwrap()).unwrap();
    assert!(loaded.hash_matches);
    assert_eq!(loaded.result, result);
}

#[test]
fn an_edited_result_no_longer_matches_its_hash() {
    let mut json = reference_json();
    assert!(raw_hash_matches(&json));
    json["deterministic"]["status"] = "pass".into();
    assert!(!raw_hash_matches(&json));
}

#[test]
fn a_result_with_reordered_lists_does_not_match() {
    let mut json = reference_json();
    let findings = json["deterministic"]["findings"].as_array_mut().unwrap();
    findings.reverse();
    assert!(!raw_hash_matches(&json));
}

#[test]
fn a_result_with_a_field_this_dyno_does_not_know_still_matches() {
    let mut json = reference_json();
    json["deterministic"]["added_later"] = "by a newer dyno".into();
    json["deterministic"]["measures"][0]["added_later"] = 1.into();
    rehash(&mut json);
    let loaded = LoadedResult::from_json(json).unwrap();
    assert!(loaded.hash_matches);
    // The typed result drops the new fields, so its hash differs. That is why the check reads
    // the JSON.
    let typed_hash = sha256_of_canonical_json(&loaded.result.deterministic).unwrap();
    assert_ne!(typed_hash, loaded.result.result_hash);
}

#[test]
fn an_edit_from_2_to_the_53_to_one_more_does_not_match() {
    let plain_hash = |json: &Value| {
        let text = serde_json_canonicalizer::to_string(&json["deterministic"]).unwrap();
        sha256_hex(text.as_bytes())
    };
    let two_to_the_53 = FIRST_UNSAFE;
    let mut json = reference_json();
    json["deterministic"]["build"]["files"][0]["bytes"] = two_to_the_53.into();
    let rounded_hash = plain_hash(&json);
    json["deterministic"]["build"]["files"][0]["bytes"] = (two_to_the_53 + 1).into();
    // A plain canonicalizer writes 2^53 and 2^53 + 1 the same, so it gives both one hash.
    assert_eq!(plain_hash(&json), rounded_hash);
    json["result_hash"] = rounded_hash.into();
    assert!(!raw_hash_matches(&json));
}

#[test]
fn json_with_no_hash_or_deterministic_part_never_matches() {
    let mut no_hash = reference_json();
    no_hash.as_object_mut().unwrap().remove("result_hash");
    let mut no_deterministic = reference_json();
    no_deterministic
        .as_object_mut()
        .unwrap()
        .remove("deterministic");
    assert!(!raw_hash_matches(&no_hash));
    assert!(!raw_hash_matches(&no_deterministic));
}

#[test]
fn a_hash_that_is_not_a_string_never_matches() {
    let mut json = reference_json();
    let hash = json["result_hash"].take();
    json["result_hash"] = Value::Array(vec![hash]);
    assert!(!raw_hash_matches(&json));
}
