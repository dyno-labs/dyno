//! Budget files: parsing, validation and the hash a result records.

use std::path::Path;

use dyno_schema::budget::{Budget, BudgetLimit, Severity};
use dyno_schema::config::ConfigFormat;
use dyno_schema::ids::BudgetId;

const AUDIO_ONLY_TOML: &str = r#"
    schema_version = 0
    name = "audio only"

    [audio]
    decoded_mb = { warn = 64, fail = 128 }
"#;

const AUDIO_ONLY_JSON: &str = r#"{
    "schema_version": 0,
    "name": "audio only",
    "audio": { "decoded_mb": { "warn": 64, "fail": 128 } }
}"#;

fn parse_error(text: &str) -> String {
    Budget::parse(text, ConfigFormat::Toml)
        .unwrap_err()
        .to_string()
}

#[test]
fn the_example_budget_file_loads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/budget.toml");
    let budget = Budget::load(&path).unwrap();
    for id in BudgetId::ALL {
        assert!(
            budget.limit(*id).is_some(),
            "the example sets no {id} budget"
        );
    }
}

#[test]
fn toml_and_json_with_the_same_budgets_give_the_same_hash() {
    let from_toml = Budget::parse(AUDIO_ONLY_TOML, ConfigFormat::Toml).unwrap();
    let from_json = Budget::parse(AUDIO_ONLY_JSON, ConfigFormat::Json).unwrap();
    assert_eq!(
        from_toml.reference().unwrap(),
        from_json.reference().unwrap()
    );
}

#[test]
fn a_different_limit_gives_a_different_hash() {
    let original = Budget::parse(AUDIO_ONLY_TOML, ConfigFormat::Toml).unwrap();
    let changed =
        Budget::parse(&AUDIO_ONLY_TOML.replace("128", "129"), ConfigFormat::Toml).unwrap();
    assert_ne!(
        original.reference().unwrap().sha256,
        changed.reference().unwrap().sha256
    );
}

#[test]
fn size_limits_convert_from_mb_to_bytes() {
    let budget = Budget::parse(AUDIO_ONLY_TOML, ConfigFormat::Toml).unwrap();
    let expected = BudgetLimit::Bytes {
        warn_above: Some(67_108_864),
        fail_above: Some(134_217_728),
    };
    assert_eq!(budget.limit(BudgetId::AudioDecodedMb), Some(expected));
    assert_eq!(budget.limit(BudgetId::TexturesDecodedMb), None);
}

#[test]
fn flag_budgets_keep_their_severity() {
    let budget = Budget::parse(
        "schema_version = 0\n[render]\nantialias = \"fail\"\n",
        ConfigFormat::Toml,
    )
    .unwrap();
    assert_eq!(
        budget.limit(BudgetId::RenderAntialias),
        Some(BudgetLimit::Flag {
            when_on: Severity::Fail
        })
    );
}

#[test]
fn share_limits_stay_in_basis_points() {
    let budget = Budget::parse(
        "schema_version = 0\n[audio]\nsilence_share_bp = { warn = 1000 }\n",
        ConfigFormat::Toml,
    )
    .unwrap();
    let expected = BudgetLimit::BasisPoints {
        warn_above: Some(1000),
        fail_above: None,
    };
    assert_eq!(budget.limit(BudgetId::AudioSilenceShareBp), Some(expected));
}

#[test]
fn a_share_limit_must_be_below_100_percent() {
    let error = parse_error("schema_version = 0\n[audio]\nsilence_share_bp = { warn = 10000 }\n");
    assert!(
        error.contains("audio.silence_share_bp: limits must be below 10000 bp (100%)"),
        "{error}"
    );
}

#[test]
fn a_share_limit_just_below_100_percent_is_accepted() {
    let budget = Budget::parse(
        "schema_version = 0\n[audio]\nsilence_share_bp = { warn = 9999 }\n",
        ConfigFormat::Toml,
    );
    assert!(budget.is_ok(), "{budget:?}");
}

#[test]
fn a_rule_threshold_in_mb_is_checked_like_a_budget() {
    let error = parse_error("schema_version = 0\n[audio]\nstereo_saving_mb = { warn = 0 }\n");
    assert!(
        error.contains("audio.stereo_saving_mb: limits must be above 0 MB"),
        "{error}"
    );
}

#[test]
fn the_duplicate_rule_takes_a_severity() {
    let budget = Budget::parse(
        "schema_version = 0\n[files]\nduplicate = \"fail\"\n",
        ConfigFormat::Toml,
    )
    .unwrap();
    let expected = BudgetLimit::Flag {
        when_on: Severity::Fail,
    };
    assert_eq!(budget.limit(BudgetId::FilesDuplicate), Some(expected));
}

#[test]
fn unknown_keys_are_rejected_with_their_name() {
    let error = parse_error("schema_version = 0\n[audio]\ndecoded_mbb = { fail = 1 }\n");
    assert!(error.contains("unknown field `decoded_mbb`"), "{error}");
    assert!(
        error.contains("expected one of `decoded_mb`"),
        "the error should name the expected key: {error}"
    );
}

#[test]
fn unknown_sections_are_rejected() {
    let error = parse_error("schema_version = 0\n[video]\ndecoded_mb = { fail = 1 }\n");
    assert!(error.contains("unknown field `video`"), "{error}");
}

#[test]
fn unknown_json_keys_are_rejected() {
    let error = Budget::parse(
        r#"{ "schema_version": 0, "audoi": {} }"#,
        ConfigFormat::Json,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("unknown field `audoi`"), "{error}");
}

#[test]
fn warn_must_be_below_fail() {
    let error =
        parse_error("schema_version = 0\n[textures]\ndecoded_mb = { warn = 300, fail = 200 }\n");
    assert!(
        error.contains("textures.decoded_mb: warn (300 MB) must be below fail (200 MB)"),
        "{error}"
    );
}

#[test]
fn warn_equal_to_fail_is_rejected() {
    let error =
        parse_error("schema_version = 0\n[textures]\ndecoded_mb = { warn = 200, fail = 200 }\n");
    assert!(
        error.contains("warn (200 MB) must be below fail (200 MB)"),
        "{error}"
    );
}

#[test]
fn a_size_limit_needs_warn_or_fail() {
    let error = parse_error("schema_version = 0\n[audio]\ndecoded_mb = {}\n");
    assert!(
        error.contains("audio.decoded_mb: set warn, fail or both"),
        "{error}"
    );
}

#[test]
fn zero_limits_are_rejected() {
    for limit in ["{ fail = 0 }", "{ warn = 0 }", "{ warn = 0, fail = 10 }"] {
        let error = parse_error(&format!(
            "schema_version = 0\n[audio]\ndecoded_mb = {limit}\n"
        ));
        assert!(error.contains("above 0 MB"), "{limit}: {error}");
    }
}

#[test]
fn another_schema_version_is_rejected() {
    let error = parse_error("schema_version = 1\n");
    assert!(
        error.contains("has schema_version 1, but this dyno reads schema_version 0"),
        "{error}"
    );
}

#[test]
fn schema_version_is_required() {
    let error = parse_error("name = \"x\"\n");
    assert!(error.contains("missing field `schema_version`"), "{error}");
}

#[test]
fn a_bad_severity_names_the_allowed_values() {
    let error = parse_error("schema_version = 0\n[render]\nantialias = \"never\"\n");
    assert!(
        error.contains("allow") && error.contains("warn") && error.contains("fail"),
        "{error}"
    );
}
