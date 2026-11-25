//! `dyno verify` and `dyno receipt`, through the `dyno` binary, with the known-answer receipt in
//! `fixtures/receipts/`.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;

use common::{Outcome, dyno, fixture_build, json};
use dyno_schema::receipt::{
    PublicKeys, ReceiptEnvelope, VerifyCheckId, VerifyOutcome, VerifyResult,
};
use serde_json::Value;

const VERIFIED: i32 = 0;
const NOT_VERIFIED: i32 = 1;
const USAGE: i32 = 64;
const INTERNAL: i32 = 70;

/// The known-answer files, relative to `tests/data`, where the tests run dyno.
const RECEIPT: &str = "../../../../fixtures/receipts/receipt.json";
const RESULT: &str = "../../../../fixtures/receipts/result.json";
const KEYS: &str = "../../../../fixtures/receipts/keys.json";
const PRIVATE_KEY: &str = "../../../../fixtures/receipts/test-private-key.json";

/// The build that the known-answer result is for.
const BUILD: &str = "tiny-build";

/// The test key's public key and its key id.
const PUBLIC_KEY: &str = "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo";
const KEY_ID: &str = "21fe31dfa154a261";

/// Another key: 32 zero bytes, which signed nothing.
const OTHER_PUBLIC_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn assert_exit(outcome: &Outcome, code: i32) {
    assert_eq!(
        outcome.code, code,
        "args: {:?}\nstdout: {}\nstderr: {}",
        outcome.args, outcome.stdout, outcome.stderr
    );
}

fn assert_stderr_has(outcome: &Outcome, text: &str) {
    assert!(outcome.stderr.contains(text), "{}", outcome.stderr);
}

fn verdict(outcome: &Outcome) -> VerifyResult {
    serde_json::from_str(&outcome.stdout).expect("stdout is a verdict")
}

fn outcome_of(verdict: &VerifyResult, id: VerifyCheckId) -> VerifyOutcome {
    let check = verdict.checks.iter().find(|check| check.id == id);
    check.unwrap().outcome
}

/// The known-answer file as JSON.
fn fixture_json(path: &str) -> Value {
    let text = fs::read_to_string(common::data_dir().join(path)).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Writes JSON to `name` in `dir` and returns the path.
fn write_json(dir: &Path, name: &str, json: &Value) -> String {
    let path = dir.join(name);
    fs::write(&path, json.to_string()).unwrap();
    path.display().to_string()
}

/// The known-answer keys document with its key retired at this time.
fn keys_retired_at(dir: &Path, retired_at: &str) -> String {
    let mut keys = fixture_json(KEYS);
    keys["keys"][0]["retired_at"] = retired_at.into();
    write_json(dir, "keys.json", &keys)
}

/// A server on this machine that answers every request with this status and body, and its URL.
fn serve(status: u16, body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/keys.json", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                line.clear();
            }
            let _ = write!(
                &stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

#[test]
fn a_receipt_with_its_keys_its_result_and_its_build_is_verified() {
    let outcome = dyno(&[
        "verify", RECEIPT, "--keys", KEYS, "--result", RESULT, "--build", BUILD,
    ]);
    assert_exit(&outcome, VERIFIED);
    let verdict = verdict(&outcome);
    assert!(verdict.verified);
    assert!(
        verdict
            .checks
            .iter()
            .all(|check| check.outcome == VerifyOutcome::Match)
    );
    let mut settings = insta::Settings::clone_current();
    // The verdict names the dyno that verified, which each release changes.
    settings.add_redaction(".tool", "[tool]");
    settings.bind(|| insta::assert_json_snapshot!("verify", json(&outcome)));
    insta::assert_snapshot!("verify_summary", outcome.stderr);
}

#[test]
fn with_a_key_alone_the_receipt_is_verified_and_the_summary_says_the_build_was_not_checked() {
    let outcome = dyno(&["verify", RECEIPT, "--key", PUBLIC_KEY]);
    assert_exit(&outcome, VERIFIED);
    let verdict = verdict(&outcome);
    assert_eq!(
        outcome_of(&verdict, VerifyCheckId::Signature),
        VerifyOutcome::Match
    );
    assert_eq!(
        outcome_of(&verdict, VerifyCheckId::BuildId),
        VerifyOutcome::NotChecked
    );
    assert_eq!(verdict.receipt.unwrap().issuer, "lab.example");
    assert_stderr_has(
        &outcome,
        "dyno verify: verified: lab.example signed this receipt with key 21fe31dfa154a261",
    );
    assert_stderr_has(&outcome, "the build was not checked: give --build");
    assert_stderr_has(&outcome, "  device.phys_footprint: 200.0 MB");
}

#[test]
fn with_no_key_verify_refuses_and_says_why() {
    let outcome = dyno(&["verify", RECEIPT, "--result", RESULT, "--build", BUILD]);
    assert_exit(&outcome, USAGE);
    assert!(outcome.stdout.is_empty());
    assert_stderr_has(
        &outcome,
        "a receipt that brought its own key would prove nothing",
    );
}

#[test]
fn a_key_that_did_not_sign_the_receipt_does_not_verify_it() {
    let outcome = dyno(&["verify", RECEIPT, "--key", OTHER_PUBLIC_KEY]);
    assert_exit(&outcome, NOT_VERIFIED);
    let verdict = verdict(&outcome);
    assert!(!verdict.verified);
    assert_eq!(
        outcome_of(&verdict, VerifyCheckId::Key),
        VerifyOutcome::Mismatch
    );
    assert_eq!(verdict.receipt, None);
    assert_stderr_has(
        &outcome,
        "dyno verify: not verified: none of the 1 key given has the receipt's key id",
    );
}

#[test]
fn the_lab_s_key_among_others_verifies_whichever_flag_gives_it() {
    let outcome = dyno(&[
        "verify",
        RECEIPT,
        "--key",
        OTHER_PUBLIC_KEY,
        "--key",
        PUBLIC_KEY,
        "--keys",
        KEYS,
    ]);
    assert_exit(&outcome, VERIFIED);
}

#[test]
fn a_receipt_that_was_changed_does_not_verify() {
    let folder = tempfile::tempdir().unwrap();
    let mut changed = fixture_json(RECEIPT);
    changed["receipt"]["measures"][2]["value"] = 104_857_600.into();
    let path = write_json(folder.path(), "receipt.json", &changed);
    let outcome = dyno(&["verify", &path, "--key", PUBLIC_KEY]);
    assert_exit(&outcome, NOT_VERIFIED);
    assert_stderr_has(&outcome, "signature: MISMATCH");
    // The changed reading is nobody's claim, so neither output repeats it.
    assert!(!outcome.stdout.contains("104857600"));
    assert!(!outcome.stderr.contains("100.0 MB"), "{}", outcome.stderr);
}

#[test]
fn files_that_are_not_the_build_the_lab_ran_do_not_verify() {
    let other_build = fixture_build("duplicates");
    let outcome = dyno(&[
        "verify",
        RECEIPT,
        "--key",
        PUBLIC_KEY,
        "--build",
        &other_build,
    ]);
    assert_exit(&outcome, NOT_VERIFIED);
    let verdict = verdict(&outcome);
    assert_eq!(
        outcome_of(&verdict, VerifyCheckId::Signature),
        VerifyOutcome::Match
    );
    assert_eq!(
        outcome_of(&verdict, VerifyCheckId::BuildId),
        VerifyOutcome::Mismatch
    );
    assert_stderr_has(
        &outcome,
        "dyno verify: not verified: the files have build id ",
    );
}

#[test]
fn the_build_id_is_the_one_dyno_check_gives_for_a_folder_and_for_a_zip_of_it() {
    let folder = fixture_build("duplicates");
    let zip = "../../../../fixtures/zip-input/build.zip";
    let checked = json(&dyno(&["check", &folder]));
    let build_id = checked["deterministic"]["build"]["build_id"].clone();
    for build in [folder.as_str(), zip] {
        let outcome = dyno(&["verify", RECEIPT, "--key", PUBLIC_KEY, "--build", build]);
        let verdict = json(&outcome);
        let found = verdict["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == "build_id")
            .unwrap();
        assert_eq!(found["actual"], build_id, "{build}");
    }
}

#[test]
fn a_result_that_is_not_the_one_the_lab_signed_does_not_verify() {
    let folder = tempfile::tempdir().unwrap();
    let mut other = fixture_json(RESULT);
    other["run"]["duration_ms"] = 1.into();
    let path = write_json(folder.path(), "result.json", &other);
    let outcome = dyno(&["verify", RECEIPT, "--key", PUBLIC_KEY, "--result", &path]);
    assert_exit(&outcome, NOT_VERIFIED);
    let verdict = verdict(&outcome);
    assert_eq!(
        outcome_of(&verdict, VerifyCheckId::ResultSha256),
        VerifyOutcome::Mismatch
    );
    assert_eq!(
        outcome_of(&verdict, VerifyCheckId::ResultHash),
        VerifyOutcome::Match
    );
}

#[test]
fn a_result_file_with_other_spacing_and_key_order_is_still_the_one_signed() {
    let folder = tempfile::tempdir().unwrap();
    let pretty = serde_json::to_string_pretty(&fixture_json(RESULT)).unwrap();
    let path = folder.path().join("result.json");
    fs::write(&path, pretty).unwrap();
    let path = path.display().to_string();
    let outcome = dyno(&["verify", RECEIPT, "--key", PUBLIC_KEY, "--result", &path]);
    assert_exit(&outcome, VERIFIED);
}

#[test]
fn a_retired_key_verifies_a_receipt_issued_before_and_the_output_says_it_is_retired() {
    let folder = tempfile::tempdir().unwrap();
    // The receipt was issued at 2026-11-25T11:00:00Z.
    let keys = keys_retired_at(folder.path(), "2026-11-25T11:00:01Z");
    let outcome = dyno(&["verify", RECEIPT, "--keys", &keys]);
    assert_exit(&outcome, VERIFIED);
    let key = verdict(&outcome).key.unwrap();
    assert!(key.retired);
    assert_eq!(key.retired_at.as_deref(), Some("2026-11-25T11:00:01Z"));
    assert_stderr_has(
        &outcome,
        "the lab retired the key at 2026-11-25T11:00:01Z: it verifies only a receipt issued \
         before then",
    );
}

#[test]
fn a_retired_key_does_not_verify_a_receipt_issued_after_it_was_retired() {
    let folder = tempfile::tempdir().unwrap();
    let keys = keys_retired_at(folder.path(), "2026-11-25T11:00:00Z");
    let outcome = dyno(&["verify", RECEIPT, "--keys", &keys]);
    assert_exit(&outcome, NOT_VERIFIED);
    assert_stderr_has(
        &outcome,
        "not verified: the lab retired key 21fe31dfa154a261 at 2026-11-25T11:00:00Z",
    );
}

#[test]
fn out_writes_the_verdict_to_a_file_and_nothing_to_stdout() {
    let folder = tempfile::tempdir().unwrap();
    let out = folder.path().join("verdict.json");
    let outcome = dyno(&[
        "verify",
        RECEIPT,
        "--key",
        PUBLIC_KEY,
        "--out",
        &out.display().to_string(),
    ]);
    assert_exit(&outcome, VERIFIED);
    assert!(outcome.stdout.is_empty());
    let written: VerifyResult = serde_json::from_str(&fs::read_to_string(out).unwrap()).unwrap();
    assert!(written.verified);
}

#[test]
fn a_keys_document_at_a_url_on_this_machine_verifies() {
    let keys = fs::read_to_string(common::data_dir().join(KEYS)).unwrap();
    let url = serve(200, keys);
    let outcome = dyno(&["verify", RECEIPT, "--keys", &url]);
    assert_exit(&outcome, VERIFIED);
    assert_eq!(verdict(&outcome).key.unwrap().key_id, KEY_ID);
}

#[test]
fn a_keys_url_that_answers_with_an_error_or_does_not_answer_is_an_io_error() {
    let missing = serve(404, "no such page".to_owned());
    let outcome = dyno(&["verify", RECEIPT, "--keys", &missing]);
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, "cannot get the keys document");
    assert_stderr_has(&outcome, "the answer was HTTP 404");

    // A port that nothing listens on: it is bound, then closed.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/keys.json", listener.local_addr().unwrap())
    };
    let outcome = dyno(&["verify", RECEIPT, "--keys", &closed]);
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, "cannot get the keys document");
}

#[test]
fn a_keys_document_larger_than_the_limit_is_not_read() {
    let large = format!(r#"{{"keys":[],"padding":"{}"}}"#, "x".repeat(1 << 20));
    let outcome = dyno(&["verify", RECEIPT, "--keys", &serve(200, large)]);
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, "cannot get the keys document");
}

#[test]
fn a_keys_url_over_plain_http_on_another_machine_is_refused_before_any_connection() {
    let outcome = dyno(&["verify", RECEIPT, "--keys", "http://lab.invalid/keys.json"]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "must be a file or an https URL");
}

#[test]
fn an_input_that_cannot_be_read_or_is_not_what_it_should_be_is_a_usage_error() {
    let folder = tempfile::tempdir().unwrap();
    let not_json = folder.path().join("not.json");
    fs::write(&not_json, "not json").unwrap();
    let not_json = not_json.display().to_string();
    let some_json = write_json(folder.path(), "some.json", &serde_json::json!({ "a": 1 }));
    let mut later = fixture_json(RECEIPT);
    later["receipt"]["receipt_version"] = 2.into();
    let later = write_json(folder.path(), "later.json", &later);
    let mut wrong_id = fixture_json(KEYS);
    wrong_id["keys"][0]["key_id"] = "0000000000000000".into();
    let wrong_id = write_json(folder.path(), "wrong-id.json", &wrong_id);

    let key = ["--key", PUBLIC_KEY];
    let cases: [(&[&str], &str); 12] = [
        (&["missing.json"], "cannot read receipt file missing.json"),
        (&[&not_json], "is not JSON"),
        (
            &[&some_json],
            "it is not a receipt: it has no `receipt` object",
        ),
        (
            &[&later],
            "it has receipt_version 2, but this dyno reads receipt_version 1",
        ),
        (
            &[RECEIPT, "--result", "missing.json"],
            "cannot read result file missing.json",
        ),
        (&[RECEIPT, "--result", &not_json], "is not JSON"),
        (
            &[RECEIPT, "--build", "no-such-build"],
            "is not a folder or a .zip file",
        ),
        (
            &[RECEIPT, "--key", "abc"],
            "--key: \"abc\" is not a public key",
        ),
        (
            &[RECEIPT, "--keys", "missing.json"],
            "cannot read keys document missing.json",
        ),
        (&[RECEIPT, "--keys", &some_json], "is not a keys document"),
        (
            &[RECEIPT, "--keys", &wrong_id],
            "a public key whose key id is 21fe31dfa154a261",
        ),
        (&[RECEIPT, "--keys", "https://"], "is not a URL"),
    ];
    for (args, problem) in cases {
        let mut all_args = vec!["verify"];
        all_args.extend_from_slice(args);
        // The cases that are not about the key still give one, so each fails for its own reason.
        if !args.contains(&"--keys") && !args.contains(&"--key") {
            all_args.extend_from_slice(&key);
        }
        let outcome = dyno(&all_args);
        assert_exit(&outcome, USAGE);
        assert!(outcome.stdout.is_empty(), "{:?}", outcome.args);
        assert_stderr_has(&outcome, problem);
    }
}

#[test]
fn signing_the_known_answer_result_gives_the_known_answer_receipt_byte_for_byte() {
    let outcome = dyno(&[
        "receipt",
        "sign",
        "--key",
        PRIVATE_KEY,
        "--result",
        RESULT,
        "--issuer",
        "lab.example",
        "--check-id",
        "chk_0001",
        "--lab-backend",
        "fake-phone",
        "--issued-at",
        "2026-11-25T11:00:00Z",
    ]);
    assert_exit(&outcome, 0);
    let expected = fs::read_to_string(common::data_dir().join(RECEIPT)).unwrap();
    assert_eq!(outcome.stdout, expected);
    assert_stderr_has(
        &outcome,
        "dyno receipt sign: lab.example signed check chk_0001 with key 21fe31dfa154a261",
    );
    assert_stderr_has(&outcome, "  device iphone-14-pro-max, 4 measures");
}

#[test]
fn a_new_key_pair_signs_a_receipt_that_its_keys_document_verifies() {
    let folder = tempfile::tempdir().unwrap();
    let path = |name: &str| folder.path().join(name).display().to_string();
    let (key, keys, receipt) = (
        path("lab-key.json"),
        path("keys.json"),
        path("receipt.json"),
    );

    let made = dyno(&["receipt", "keygen", "--key", &key, "--out", &keys]);
    assert_exit(&made, 0);
    assert!(made.stdout.is_empty());
    assert_stderr_has(&made, "Keep it secret");
    let document: PublicKeys = serde_json::from_str(&fs::read_to_string(&keys).unwrap()).unwrap();
    assert_eq!(document.keys.len(), 1);
    assert_eq!(document.keys[0].algorithm, "ed25519");
    assert_eq!(document.keys[0].retired_at, None);
    assert_stderr_has(&made, &format!("made key {}", document.keys[0].key_id));

    // With no --issued-at, the receipt is issued now, after the key was made.
    let signed = dyno(&[
        "receipt",
        "sign",
        "--key",
        &key,
        "--result",
        RESULT,
        "--issuer",
        "my-lab.example",
        "--check-id",
        "run 7",
        "--lab-kind",
        "device_cloud",
        "--out",
        &receipt,
    ]);
    assert_exit(&signed, 0);
    assert!(signed.stdout.is_empty());
    let envelope: ReceiptEnvelope =
        serde_json::from_str(&fs::read_to_string(&receipt).unwrap()).unwrap();
    assert_eq!(envelope.receipt.key_id, document.keys[0].key_id);
    assert_eq!(envelope.receipt.lab.kind, "device_cloud");
    assert_eq!(envelope.receipt.lab.backend, None);
    assert!(envelope.receipt.issued_at >= document.keys[0].created_at);

    let verified = dyno(&[
        "verify", &receipt, "--keys", &keys, "--result", RESULT, "--build", BUILD,
    ]);
    assert_exit(&verified, VERIFIED);
    // The known-answer key did not sign it.
    assert_exit(&dyno(&["verify", &receipt, "--keys", KEYS]), NOT_VERIFIED);
}

#[test]
fn keygen_prints_the_keys_document_and_never_writes_over_a_private_key() {
    let folder = tempfile::tempdir().unwrap();
    let key = folder.path().join("lab-key.json");
    let key_path = key.display().to_string();
    let made = dyno(&["receipt", "keygen", "--key", &key_path]);
    assert_exit(&made, 0);
    let document: PublicKeys = serde_json::from_str(&made.stdout).unwrap();
    let private_key = fs::read_to_string(&key).unwrap();
    let file: Value = serde_json::from_str(&private_key).unwrap();
    assert_eq!(file["kty"], "OKP");
    assert_eq!(file["crv"], "Ed25519");
    assert_eq!(file["kid"], document.keys[0].key_id.as_str());
    assert_eq!(file["x"], document.keys[0].public_key.as_str());
    // The private key is in its file only.
    let secret = file["d"].as_str().unwrap();
    assert!(!made.stdout.contains(secret) && !made.stderr.contains(secret));

    let again = dyno(&["receipt", "keygen", "--key", &key_path]);
    assert_exit(&again, USAGE);
    assert!(again.stdout.is_empty());
    assert_stderr_has(&again, "dyno does not write over a private key");
    assert_eq!(fs::read_to_string(&key).unwrap(), private_key);
}

#[cfg(unix)]
#[test]
fn only_the_owner_can_read_the_private_key_file() {
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir().unwrap();
    let key = folder.path().join("lab-key.json");
    assert_exit(
        &dyno(&["receipt", "keygen", "--key", &key.display().to_string()]),
        0,
    );
    let mode = fs::metadata(&key).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_private_key_file_in_a_folder_that_is_not_there_is_an_io_error() {
    let folder = tempfile::tempdir().unwrap();
    let key = folder.path().join("no-folder").join("lab-key.json");
    let outcome = dyno(&["receipt", "keygen", "--key", &key.display().to_string()]);
    assert_exit(&outcome, INTERNAL);
    assert!(outcome.stdout.is_empty());
}

#[test]
fn a_result_with_no_run_on_a_device_gets_no_receipt() {
    let folder = tempfile::tempdir().unwrap();
    let result = folder.path().join("static.json").display().to_string();
    dyno(&["check", BUILD, "--out", &result]);
    let outcome = dyno(&[
        "receipt",
        "sign",
        "--key",
        PRIVATE_KEY,
        "--result",
        &result,
        "--issuer",
        "lab.example",
        "--check-id",
        "chk_1",
    ]);
    assert_exit(&outcome, USAGE);
    assert!(outcome.stdout.is_empty());
    assert_stderr_has(&outcome, "static.json gets no receipt: it names no device");
}

#[test]
fn a_result_that_was_edited_gets_no_receipt() {
    let folder = tempfile::tempdir().unwrap();
    let mut edited = fixture_json(RESULT);
    edited["deterministic"]["status"] = "pass".into();
    let result = write_json(folder.path(), "edited.json", &edited);
    let outcome = dyno(&[
        "receipt",
        "sign",
        "--key",
        PRIVATE_KEY,
        "--result",
        &result,
        "--issuer",
        "lab.example",
        "--check-id",
        "chk_1",
    ]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "it was edited after dyno wrote it");
}

#[test]
fn a_bad_flag_or_file_of_sign_is_a_usage_error() {
    let folder = tempfile::tempdir().unwrap();
    // A private key file whose public key is another key's.
    let mut mixed = fixture_json(PRIVATE_KEY);
    mixed["x"] = OTHER_PUBLIC_KEY.into();
    let mixed = write_json(folder.path(), "mixed.json", &mixed);
    let not_a_key = write_json(
        folder.path(),
        "not-a-key.json",
        &serde_json::json!({ "d": "a secret that must not be printed" }),
    );
    let sign = |key: &str, result: &str, issuer: &str, issued_at: &str| {
        dyno(&[
            "receipt",
            "sign",
            "--key",
            key,
            "--result",
            result,
            "--issuer",
            issuer,
            "--check-id",
            "chk_1",
            "--issued-at",
            issued_at,
        ])
    };
    let now = "2026-11-25T11:00:00Z";
    let cases = [
        (
            sign("missing.json", RESULT, "lab", now),
            "cannot read private key file missing.json",
        ),
        (
            sign(&mixed, RESULT, "lab", now),
            "its x is not the public key of its d",
        ),
        (
            sign(&not_a_key, RESULT, "lab", now),
            "is not a private key file",
        ),
        (
            sign(PRIVATE_KEY, "missing.json", "lab", now),
            "cannot read result file missing.json",
        ),
        (
            sign(PRIVATE_KEY, RESULT, " ", now),
            "the issuer must not be empty",
        ),
        (
            sign(PRIVATE_KEY, RESULT, "lab", "11:00"),
            "it must be in UTC, to the second",
        ),
    ];
    for (outcome, problem) in cases {
        assert_exit(&outcome, USAGE);
        assert!(outcome.stdout.is_empty(), "{:?}", outcome.args);
        assert_stderr_has(&outcome, problem);
        assert!(!outcome.stderr.contains("a secret that must not be printed"));
    }
}
