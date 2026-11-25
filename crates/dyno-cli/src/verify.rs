//! `dyno verify`: whether a lab's signed receipt holds, for the result and the files you have.
//!
//! The command reads the files and the keys, and `dyno_receipt::verify` gives the verdict. It
//! opens a connection only when `--keys` is a URL.

use std::fs;
use std::path::Path;
use std::time::Duration;

use dyno_receipt::keys::{TrustedKey, trusted_keys};
use dyno_receipt::verify::Evidence;
use dyno_schema::TOOL_VERSION;
use dyno_schema::receipt::PublicKeys;
use serde_json::Value;
use ureq::Agent;
use url::{Host, Url};

use crate::cli::VerifyArgs;
use crate::exit::CliError;
use crate::output;

/// How long the connection to a keys document may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the whole request for a keys document may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The most bytes a keys document may have. A key's entry is about 200 bytes, so this is room
/// for thousands of keys, and a bad URL cannot fill memory.
const KEYS_DOCUMENT_LIMIT: u64 = 1 << 20;

/// Why verify does nothing with no key.
const NO_KEY: &str = "give the lab's public key with --key, or its keys document with --keys. A \
    receipt names its key and never brings it, because a receipt that brought its own key would \
    prove nothing";

/// Runs `dyno verify` and returns whether the receipt is verified, for the exit code.
pub fn run(args: VerifyArgs) -> Result<bool, CliError> {
    if args.key.is_empty() && args.keys.is_none() {
        return Err(CliError::usage(NO_KEY));
    }
    let receipt = read_json(&args.receipt, "receipt file")?;
    let mut keys = Vec::new();
    for text in &args.key {
        let key = TrustedKey::from_base64url(text)
            .map_err(|error| CliError::usage(format!("--key: {error}")))?;
        keys.push(key);
    }
    if let Some(source) = &args.keys {
        keys.extend(keys_document(source)?);
    }
    let result = match &args.result {
        Some(path) => Some(read_json(path, "result file")?),
        None => None,
    };
    let build_id = match &args.build {
        Some(build) => Some(build_id_of(build)?),
        None => None,
    };
    let evidence = Evidence {
        result: result.as_ref(),
        build_id: build_id.as_deref(),
    };
    let verdict = dyno_receipt::verify::verify(&receipt, &keys, evidence)
        .map_err(|error| CliError::usage(format!("{}: {error}", args.receipt.display())))?;
    output::write_json(&verdict, args.out.as_deref())?;
    output::print_summary(&output::verify_summary(&verdict));
    Ok(verdict.verified)
}

/// Reads a JSON file. A file that cannot be read or is not JSON is a usage error.
pub fn read_json(path: &Path, what: &str) -> Result<Value, CliError> {
    let shown = path.display();
    let text = fs::read_to_string(path)
        .map_err(|error| CliError::usage(format!("cannot read {what} {shown}: {error}")))?;
    serde_json::from_str(&text)
        .map_err(|error| CliError::usage(format!("{what} {shown} is not JSON: {error}")))
}

/// The build id of a build folder or .zip, from the inventory that `dyno check` makes, so the two
/// commands can never give one build two ids.
fn build_id_of(build: &Path) -> Result<String, CliError> {
    let inventory = dyno_static::inventory::scan(build).map_err(CliError::usage)?;
    inventory
        .summary
        .into_manifest()
        .and_then(|manifest| manifest.build_id)
        .ok_or_else(|| CliError::internal("the inventory gave no build id"))
}

/// The keys of a lab's keys document, from a file or a URL.
fn keys_document(source: &str) -> Result<Vec<TrustedKey>, CliError> {
    let text = match keys_url(source)? {
        Some(url) => fetch(&url)?,
        None => fs::read_to_string(source).map_err(|error| {
            CliError::usage(format!("cannot read keys document {source}: {error}"))
        })?,
    };
    let document: PublicKeys = serde_json::from_str(&text)
        .map_err(|error| CliError::usage(format!("{source} is not a keys document: {error}")))?;
    trusted_keys(&document).map_err(|error| CliError::usage(format!("{source}: {error}")))
}

/// The URL that `--keys` names, or None when it names a file.
///
/// The keys decide which receipts verify, so they come over https, where nobody on the network
/// can swap them. Plain http is taken only from this machine itself, such as a lab under test.
fn keys_url(source: &str) -> Result<Option<Url>, CliError> {
    let lower = source.to_ascii_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return Ok(None);
    }
    let url = Url::parse(source)
        .map_err(|error| CliError::usage(format!("--keys {source:?} is not a URL: {error}")))?;
    let this_machine = match url.host() {
        Some(Host::Domain(name)) => name == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    if url.scheme() != "https" && !this_machine {
        return Err(CliError::usage(format!(
            "--keys {source:?} must be a file or an https URL: over plain http, anyone on the \
             network can swap the keys"
        )));
    }
    Ok(Some(url))
}

/// Gets a keys document from its URL. A URL that cannot be reached, or that answers with an
/// error, is an I/O error.
fn fetch(url: &Url) -> Result<String, CliError> {
    let cannot_get = |problem: String| {
        CliError::internal(format!("cannot get the keys document {url}: {problem}"))
    };
    let agent: Agent = Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(REQUEST_TIMEOUT))
        // A redirect must not take an https URL to plain http.
        .https_only(url.scheme() == "https")
        .http_status_as_error(false)
        .user_agent(format!("dyno/{TOOL_VERSION}"))
        .build()
        .into();
    let mut response = agent
        .get(url.as_str())
        .call()
        .map_err(|error| cannot_get(error.to_string()))?;
    if !response.status().is_success() {
        return Err(cannot_get(format!(
            "the answer was HTTP {}",
            response.status().as_u16()
        )));
    }
    response
        .body_mut()
        .with_config()
        .limit(KEYS_DOCUMENT_LIMIT)
        .read_to_string()
        .map_err(|error| cannot_get(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_a_file_and_an_https_url_is_a_url() {
        for file in [
            "keys.json",
            "./lab/keys.json",
            "/etc/lab/keys.json",
            "https",
        ] {
            assert_eq!(keys_url(file).unwrap(), None, "{file}");
        }
        let url = keys_url("HTTPS://lab.example/keys.json").unwrap().unwrap();
        assert_eq!(url.as_str(), "https://lab.example/keys.json");
    }

    #[test]
    fn plain_http_is_taken_only_from_this_machine() {
        for url in [
            "http://localhost:8000/keys.json",
            "http://127.0.0.1:8000/keys.json",
            "http://[::1]:8000/keys.json",
        ] {
            assert!(keys_url(url).unwrap().is_some(), "{url}");
        }
        for url in [
            "http://lab.example/keys.json",
            "http://192.168.1.20/keys.json",
            "http://localhost.lab.example/keys.json",
            "http://[2001:db8::1]/keys.json",
        ] {
            let error = keys_url(url).unwrap_err().to_string();
            assert!(error.contains("must be a file or an https URL"), "{error}");
        }
    }

    #[test]
    fn text_that_starts_like_a_url_and_is_not_one_is_a_usage_error() {
        let error = keys_url("https://").unwrap_err();
        assert_eq!(error.exit_code(), crate::exit::USAGE);
        assert!(error.to_string().contains("is not a URL"), "{error}");
    }
}
