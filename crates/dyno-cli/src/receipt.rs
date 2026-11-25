//! `dyno receipt`: what a lab needs to sign receipts. `keygen` makes its key pair, and `sign`
//! makes the receipt for a result. Anyone can run a lab, so both are part of the public tool.
//!
//! A lab's private key is its own: dyno writes it to a file that only its owner can read, never
//! writes over one, and sends it nowhere.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::SystemTime;

use dyno_receipt::keys::{PrivateKeyFile, SigningKey};
use dyno_receipt::sign::{Issue, SignError, receipt_of_result};
use dyno_schema::receipt::PublicKeys;

use crate::cli::{KeygenArgs, SignArgs};
use crate::exit::CliError;
use crate::output;
use crate::verify::read_json;

/// Runs `dyno receipt keygen`: writes a new private key file, and the public keys document with
/// that one key. It writes over no file: a private key that is written over is lost, and so are
/// the older keys of a keys document.
pub fn keygen(args: KeygenArgs) -> Result<(), CliError> {
    if let Some(out) = args.out.as_deref().filter(|out| is_there(out)) {
        return Err(CliError::usage(format!(
            "{} is there already, and dyno receipt keygen writes over no file: name a new file \
             with --out",
            out.display()
        )));
    }
    let key = SigningKey::generate().map_err(CliError::internal)?;
    let json = output::json_text(&key.to_file())?;
    let mut key_file = new_private_key_file(&args.key)?;
    // The key file is there now. So is --out, when it names the same file by another spelling or
    // through a link, and writing the keys document there would write over the new key.
    if let Some(out) = args.out.as_deref().filter(|out| is_there(out)) {
        drop(key_file);
        let _ = fs::remove_file(&args.key);
        return Err(CliError::usage(format!(
            "--out {} is the private key file of --key: give the keys document a file of its own",
            out.display()
        )));
    }
    let shown = args.key.display();
    key_file
        .write_all(json.as_bytes())
        .map_err(|error| CliError::internal(format!("cannot write {shown}: {error}")))?;
    let document = PublicKeys {
        keys: vec![key.public_entry(&now_to_the_second())],
    };
    let document_place = match &args.out {
        Some(out) => {
            write_new_file(out, &output::json_text(&document)?)?;
            out.display().to_string()
        }
        None => {
            output::write_json(&document, None)?;
            "the JSON on stdout".to_owned()
        }
    };
    output::print_summary(&[
        format!("dyno receipt keygen: made key {}", key.key_id()),
        format!("  private key: {shown}. Keep it secret: whoever has it can sign as the lab."),
        format!(
            "  public keys document: {document_place}. Give it to the people who verify, such as \
             at an https URL."
        ),
    ]);
    Ok(())
}

/// Runs `dyno receipt sign`: writes the signed receipt for a result file.
pub fn sign(args: SignArgs) -> Result<(), CliError> {
    // The receipt must not land on one of its own inputs: a private key that is written over is
    // lost.
    if let Some(out) = &args.out {
        for (input, what) in [
            (&args.key, "private key file"),
            (&args.result, "result file"),
        ] {
            if same_file::is_same_file(out, input).unwrap_or(false) {
                return Err(CliError::usage(format!(
                    "--out {} is the {what}: give the receipt a file of its own",
                    out.display()
                )));
            }
        }
    }
    let key = read_private_key(&args.key)?;
    let result = read_json(&args.result, "result file")?;
    let issued_at = args.issued_at.unwrap_or_else(now_to_the_second);
    let issue = Issue {
        issuer: &args.issuer,
        check_id: &args.check_id,
        issued_at: &issued_at,
        lab_kind: &args.lab_kind,
        lab_backend: args.lab_backend.as_deref(),
    };
    let receipt = receipt_of_result(&result, &issue, &key.key_id()).map_err(|error| {
        match error {
            // These are about a flag, not about the result.
            SignError::Empty(_) | SignError::NotAReceiptTime(_) => CliError::usage(error),
            SignError::Json(_) => CliError::internal(error),
            _ => CliError::usage(format!(
                "{} gets no receipt: {error}",
                args.result.display()
            )),
        }
    })?;
    let envelope = dyno_receipt::sign::sign(receipt, &key).map_err(CliError::internal)?;
    output::write_json(&envelope, args.out.as_deref())?;
    let receipt = &envelope.receipt;
    let receipt_place = match &args.out {
        Some(out) => out.display().to_string(),
        None => "the JSON on stdout. Use --out FILE to write it to a file.".to_owned(),
    };
    output::print_summary(&[
        format!(
            "dyno receipt sign: {} signed check {} with key {}",
            receipt.issuer, receipt.check_id, receipt.key_id
        ),
        format!("  build {}", receipt.build_id),
        format!(
            "  device {}, {}",
            receipt.device.key,
            output::count(receipt.measures.len(), "measure", "measures")
        ),
        format!("  receipt: {receipt_place}"),
    ]);
    Ok(())
}

/// The time now as a receipt states it: in UTC, to the second.
fn now_to_the_second() -> String {
    dyno_receipt::receipt_time(SystemTime::now())
}

/// Reads a private key file. A file that cannot be read or is not a key is a usage error. The
/// error never repeats the file's content.
fn read_private_key(path: &Path) -> Result<SigningKey, CliError> {
    let shown = path.display();
    let text = fs::read_to_string(path).map_err(|error| {
        CliError::usage(format!("cannot read private key file {shown}: {error}"))
    })?;
    // serde's error can quote the text it failed on, which here is a secret.
    let file: PrivateKeyFile = serde_json::from_str(&text).map_err(|_| {
        CliError::usage(format!(
            "{shown} is not a private key file that dyno receipt keygen wrote"
        ))
    })?;
    SigningKey::from_file(&file).map_err(|error| CliError::usage(format!("{shown}: {error}")))
}

/// Makes a new, empty private key file that only its owner can read. A file that is there
/// already is a usage error: writing over a lab's key would lose it.
fn new_private_key_file(path: &Path) -> Result<fs::File, CliError> {
    let shown = path.display();
    new_private_file(path).map_err(|error| match error.kind() {
        io::ErrorKind::AlreadyExists => CliError::usage(format!(
            "{shown} is there already, and dyno does not write over a private key: name a new file"
        )),
        _ => CliError::internal(format!("cannot write {shown}: {error}")),
    })
}

/// Writes text to a file that is not there yet, and never to one that is.
fn write_new_file(path: &Path, text: &str) -> Result<(), CliError> {
    let cannot_write =
        |error: io::Error| CliError::internal(format!("cannot write {}: {error}", path.display()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(cannot_write)?;
    file.write_all(text.as_bytes()).map_err(cannot_write)
}

/// Whether the path names anything: a file, a folder, or a link, even one that leads nowhere.
fn is_there(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Makes a file that is not there yet, readable and writable by its owner only.
#[cfg(unix)]
fn new_private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Makes a file that is not there yet. Off unix, the folder's own access rules apply.
#[cfg(not(unix))]
fn new_private_file(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_now_is_a_time_as_a_receipt_states_it() {
        let now = now_to_the_second();
        assert!(dyno_receipt::is_receipt_time(&now), "{now}");
    }
}
