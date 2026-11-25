//! `dyno verify` and `dyno receipt`, through the `dyno` binary, with the known-answer receipt in
//! `fixtures/receipts/`.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use common::{Outcome, data_dir, dyno, dyno_in, fixture_build, json};
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
    Server::start(status, "", body).url
}

/// A server on this machine, and how many requests it got.
struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
}

impl Server {
    /// Answers every request with this status, these header lines and this body.
    fn start(status: u16, headers: &str, body: String) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/keys.json", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let headers = headers.to_owned();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                    line.clear();
                }
                seen.fetch_add(1, Ordering::SeqCst);
                let _ = write!(
                    &stream,
                    "HTTP/1.1 {status} X\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Server { url, requests }
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// The known-answer keys document, as its text.
fn keys_text() -> String {
    fs::read_to_string(common::data_dir().join(KEYS)).unwrap()
}

/// The known-answer file's text with a second member put at the start of one of its objects.
fn with_a_name_twice(path: &str, object_start: &str, second_member: &str) -> String {
    let text = fs::read_to_string(common::data_dir().join(path)).unwrap();
    assert!(text.contains(object_start), "{path}");
    text.replacen(object_start, &format!("{object_start}{second_member},"), 1)
}

/// Writes text to `name` in `dir` and returns the path.
fn write_text(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path.display().to_string()
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
    let url = serve(200, keys_text());
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

#[test]
fn a_receipt_with_one_name_twice_is_refused_though_its_last_values_are_signed() {
    let folder = tempfile::tempdir().unwrap();
    // A reader that keeps the last value sees the signed receipt. One that keeps the first sees
    // no measures, and another issuer deep inside.
    let cases = [
        ("\"receipt\":{", "\"measures\":[]", "measures"),
        ("\"lab\":{", "\"kind\":\"another lab\"", "kind"),
        ("{", "\"signature\":\"AAAA\"", "signature"),
        (
            "\"protocol\":{",
            "\"added_later\":1,\"added_later\":2",
            "added_later",
        ),
    ];
    for (object_start, second_member, name) in cases {
        let text = with_a_name_twice(RECEIPT, object_start, second_member);
        // A plain parser reads it, and keeps the last value of each name.
        assert!(serde_json::from_str::<Value>(&text).is_ok());
        let path = write_text(folder.path(), "receipt.json", &text);
        let outcome = dyno(&["verify", &path, "--key", PUBLIC_KEY]);
        assert_exit(&outcome, USAGE);
        assert!(outcome.stdout.is_empty());
        assert_stderr_has(
            &outcome,
            &format!("the name \"{name}\" is twice in one object"),
        );
    }
}

#[test]
fn a_result_or_a_keys_document_with_one_name_twice_is_refused() {
    let folder = tempfile::tempdir().unwrap();
    let result = with_a_name_twice(RESULT, "\"run\":{", "\"duration_ms\":1");
    let result = write_text(folder.path(), "result.json", &result);
    let outcome = dyno(&["verify", RECEIPT, "--key", PUBLIC_KEY, "--result", &result]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "the name \"duration_ms\" is twice in one object");

    // A lab does not sign such a result either.
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
    assert_stderr_has(&outcome, "is twice in one object");

    let keys = with_a_name_twice(KEYS, "{", "\"keys\":[]");
    let from_file = write_text(folder.path(), "keys.json", &keys);
    let from_url = serve(200, keys);
    for source in [from_file, from_url] {
        let outcome = dyno(&["verify", RECEIPT, "--keys", &source]);
        assert_exit(&outcome, USAGE);
        assert_stderr_has(
            &outcome,
            "is not a keys document: the name \"keys\" is twice",
        );
    }
}

#[test]
fn a_key_given_with_both_flags_is_retired_as_the_keys_document_says() {
    let folder = tempfile::tempdir().unwrap();
    // The lab retired the key before it issued the receipt. --key alone knows no retirement.
    let keys = keys_retired_at(folder.path(), "2026-11-25T10:00:00Z");
    assert_exit(&dyno(&["verify", RECEIPT, "--key", PUBLIC_KEY]), VERIFIED);
    let orders: [&[&str]; 2] = [
        &["verify", RECEIPT, "--key", PUBLIC_KEY, "--keys", &keys],
        &["verify", RECEIPT, "--keys", &keys, "--key", PUBLIC_KEY],
    ];
    for args in orders {
        let outcome = dyno(args);
        assert_exit(&outcome, NOT_VERIFIED);
        assert_stderr_has(
            &outcome,
            "the lab retired key 21fe31dfa154a261 at 2026-11-25T10:00:00Z",
        );
        assert!(verdict(&outcome).key.unwrap().retired);
    }
}

#[test]
fn a_keys_document_with_a_key_id_twice_is_refused() {
    let folder = tempfile::tempdir().unwrap();
    // The key is retired in its first entry and in use in its second.
    let mut keys = fixture_json(KEYS);
    let in_use = keys["keys"][0].clone();
    keys["keys"][0]["retired_at"] = "2026-11-25T10:00:00Z".into();
    keys["keys"].as_array_mut().unwrap().push(in_use);
    let keys = write_json(folder.path(), "keys.json", &keys);
    let outcome = dyno(&["verify", RECEIPT, "--keys", &keys]);
    assert_exit(&outcome, USAGE);
    assert!(outcome.stdout.is_empty());
    assert_stderr_has(
        &outcome,
        "the keys document has key id 21fe31dfa154a261 twice",
    );
}

/// A copy of the zip fixture with one entry renamed to the name of another, in its local header
/// and in the central directory.
fn zip_with_a_name_twice(dir: &Path) -> String {
    let mut bytes = fs::read(data_dir().join("../../../../fixtures/zip-input/build.zip")).unwrap();
    let (from, to) = (b"game/empty-b.txt", b"game/empty-a.txt");
    let mut renamed = 0;
    for at in 0..bytes.len() - from.len() {
        if &bytes[at..at + from.len()] == from {
            bytes[at..at + to.len()].copy_from_slice(to);
            renamed += 1;
        }
    }
    assert_eq!(renamed, 2);
    let path = dir.join("build.zip");
    fs::write(&path, bytes).unwrap();
    path.display().to_string()
}

#[test]
fn a_zip_with_two_entries_of_one_name_is_refused_by_verify_and_by_check() {
    let folder = tempfile::tempdir().unwrap();
    let zip = zip_with_a_name_twice(folder.path());
    let verified = dyno(&["verify", RECEIPT, "--key", PUBLIC_KEY, "--build", &zip]);
    let checked = dyno(&["check", &zip]);
    for outcome in [verified, checked] {
        assert_exit(&outcome, USAGE);
        assert!(outcome.stdout.is_empty());
        assert_stderr_has(&outcome, "has two entries with the same name");
    }
}

#[test]
fn a_plain_http_keys_url_follows_no_redirect() {
    // The first server sends the request on to a second one, which has the keys.
    let keys = Server::start(200, "", keys_text());
    let redirect = Server::start(302, &format!("Location: {}\r\n", keys.url), String::new());
    let outcome = dyno(&["verify", RECEIPT, "--keys", &redirect.url]);
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, "the answer was HTTP 302");
    assert_eq!(redirect.requests(), 1);
    assert_eq!(keys.requests(), 0);
}

#[test]
fn a_plain_http_keys_url_uses_no_proxy_from_the_environment() {
    // A proxy that would answer for any URL with the keys. The URL itself has nothing behind it.
    let proxy = Server::start(200, "", keys_text());
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/keys.json", listener.local_addr().unwrap())
    };
    let proxy_url = proxy.url.trim_end_matches("/keys.json");
    let env: Vec<(&str, &str)> = ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
        .into_iter()
        .map(|name| (name, proxy_url))
        .chain([("NO_PROXY", ""), ("no_proxy", "")])
        .collect();
    let outcome = dyno_in(&data_dir(), &env, &["verify", RECEIPT, "--keys", &closed]);
    assert_exit(&outcome, INTERNAL);
    assert_stderr_has(&outcome, "cannot get the keys document");
    assert_eq!(proxy.requests(), 0);
}

#[test]
fn a_keys_url_of_localhost_reaches_a_server_on_this_machine() {
    let server = Server::start(200, "", keys_text());
    let url = server.url.replace("127.0.0.1", "localhost");
    let outcome = dyno(&["verify", RECEIPT, "--keys", &url]);
    assert_exit(&outcome, VERIFIED);
    assert_eq!(server.requests(), 1);
}

#[test]
fn keygen_writes_no_keys_document_over_its_new_private_key() {
    let folder = tempfile::tempdir().unwrap();
    let key = folder.path().join("lab-key.json");
    let key_path = key.display().to_string();
    // The same file, spelled in two ways.
    let other_spelling = folder.path().join(".").join("lab-key.json");
    for out in [key_path.clone(), other_spelling.display().to_string()] {
        let outcome = dyno(&["receipt", "keygen", "--key", &key_path, "--out", &out]);
        assert_exit(&outcome, USAGE);
        assert!(outcome.stdout.is_empty());
        assert_stderr_has(&outcome, "is the private key file of --key");
        // Nothing is left: no key that has no keys document, and no document in the key's place.
        assert!(!key.exists());
    }
}

#[cfg(unix)]
#[test]
fn keygen_writes_no_keys_document_through_a_link_to_its_new_private_key() {
    let folder = tempfile::tempdir().unwrap();
    let key = folder.path().join("lab-key.json");
    let link = folder.path().join("keys.json");
    std::os::unix::fs::symlink(&key, &link).unwrap();
    let outcome = dyno(&[
        "receipt",
        "keygen",
        "--key",
        &key.display().to_string(),
        "--out",
        &link.display().to_string(),
    ]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "is there already");
    assert!(!key.exists());
}

#[test]
fn keygen_writes_over_no_keys_document_that_is_there() {
    let folder = tempfile::tempdir().unwrap();
    let key = folder.path().join("lab-key.json");
    let keys = write_text(folder.path(), "keys.json", "the lab's older keys");
    let outcome = dyno(&[
        "receipt",
        "keygen",
        "--key",
        &key.display().to_string(),
        "--out",
        &keys,
    ]);
    assert_exit(&outcome, USAGE);
    assert_stderr_has(&outcome, "dyno receipt keygen writes over no file");
    assert_eq!(fs::read_to_string(&keys).unwrap(), "the lab's older keys");
    // No key was made for a document that could not be written.
    assert!(!key.exists());
}

#[test]
fn sign_writes_no_receipt_over_its_private_key_or_its_result() {
    let folder = tempfile::tempdir().unwrap();
    let key = folder.path().join("lab-key.json");
    let result = folder.path().join("result.json");
    fs::copy(data_dir().join(PRIVATE_KEY), &key).unwrap();
    fs::copy(data_dir().join(RESULT), &result).unwrap();
    let (key_text, result_text) = (fs::read(&key).unwrap(), fs::read(&result).unwrap());
    let key_path = key.display().to_string();
    let result_path = result.display().to_string();
    let other_spelling = folder.path().join(".").join("lab-key.json");
    let cases = [
        (key_path.clone(), "is the private key file"),
        (
            other_spelling.display().to_string(),
            "is the private key file",
        ),
        (result_path.clone(), "is the result file"),
    ];
    for (out, problem) in cases {
        let outcome = dyno(&[
            "receipt",
            "sign",
            "--key",
            &key_path,
            "--result",
            &result_path,
            "--issuer",
            "lab.example",
            "--check-id",
            "chk_1",
            "--out",
            &out,
        ]);
        assert_exit(&outcome, USAGE);
        assert_stderr_has(&outcome, problem);
        assert_eq!(fs::read(&key).unwrap(), key_text);
        assert_eq!(fs::read(&result).unwrap(), result_text);
    }
}

#[cfg(unix)]
#[test]
fn sign_writes_no_receipt_through_a_link_to_its_private_key() {
    let folder = tempfile::tempdir().unwrap();
    let key = folder.path().join("lab-key.json");
    fs::copy(data_dir().join(PRIVATE_KEY), &key).unwrap();
    let key_text = fs::read(&key).unwrap();
    let soft = folder.path().join("soft.json");
    let hard = folder.path().join("hard.json");
    std::os::unix::fs::symlink(&key, &soft).unwrap();
    fs::hard_link(&key, &hard).unwrap();
    for out in [soft, hard] {
        let outcome = dyno(&[
            "receipt",
            "sign",
            "--key",
            &key.display().to_string(),
            "--result",
            RESULT,
            "--issuer",
            "lab.example",
            "--check-id",
            "chk_1",
            "--out",
            &out.display().to_string(),
        ]);
        assert_exit(&outcome, USAGE);
        assert_stderr_has(&outcome, "is the private key file");
        assert_eq!(fs::read(&key).unwrap(), key_text);
    }
}
