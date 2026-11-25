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
/// that one key.
///
/// It writes over no file, and removes none: a private key that is written over is lost, and so
/// are the older keys of a keys document. Each file is made with a create that fails when the
/// path is there, so nothing that happens between a check and a write can change that.
pub fn keygen(args: KeygenArgs) -> Result<(), CliError> {
    let shown = args.key.display();
    if let Some(out) = &args.out {
        // Neither file is there yet, so the two paths are compared before any key is made.
        if names_the_same_new_file(&args.key, out) {
            return Err(CliError::usage(format!(
                "--out {} is the private key file of --key: give the keys document a file of \
                 its own",
                out.display()
            )));
        }
        if out.symlink_metadata().is_ok() {
            return Err(already_there(out, "name a new file with --out"));
        }
    }
    let key = SigningKey::generate().map_err(CliError::internal)?;
    let key_json = output::json_text(&key.to_file())?;
    let document = PublicKeys {
        keys: vec![key.public_entry(&now_to_the_second())],
    };
    let document_json = output::json_text(&document)?;

    let mut key_file =
        new_file(&args.key, Access::OwnerOnly).map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => CliError::usage(format!(
                "{shown} is there already, and dyno does not write over a private key: name a \
                 new file"
            )),
            _ => cannot_write(&args.key, &error),
        })?;
    // Both files are made before either is written, so a second file that cannot be made leaves
    // an empty first one, and no key without its keys document.
    let mut document_file = match &args.out {
        Some(out) => Some(
            new_file(out, Access::Shared)
                .map_err(|error| second_file_not_made(&args.key, out, &error))?,
        ),
        None => None,
    };
    key_file
        .write_all(key_json.as_bytes())
        .map_err(|error| cannot_write(&args.key, &error))?;
    let document_place = match (&mut document_file, &args.out) {
        (Some(file), Some(out)) => {
            file.write_all(document_json.as_bytes())
                .map_err(|error| cannot_write(out, &error))?;
            out.display().to_string()
        }
        _ => {
            output::write_text(&document_json, None)?;
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

/// Runs `dyno receipt sign`: writes the signed receipt for a result file, to stdout or to a new
/// file. It writes over no file, so it cannot write over its private key or its result.
pub fn sign(args: SignArgs) -> Result<(), CliError> {
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
    let receipt_json = output::json_text(&envelope)?;
    let receipt_place = match &args.out {
        // The file is made, never opened: an --out that is the private key or the result, by
        // any path or link, is there already, so it is refused and nothing is ever cut short.
        Some(out) => {
            let mut file = new_file(out, Access::Shared).map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => already_there(out, "name a new file with --out"),
                _ => cannot_write(out, &error),
            })?;
            file.write_all(receipt_json.as_bytes())
                .map_err(|error| cannot_write(out, &error))?;
            out.display().to_string()
        }
        None => {
            output::write_text(&receipt_json, None)?;
            "the JSON on stdout. Use --out FILE to write it to a file.".to_owned()
        }
    };
    let receipt = &envelope.receipt;
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

/// Who can read a new file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    /// Only its owner, on unix: a private key.
    OwnerOnly,
    /// Whoever the folder and the umask let: a document for other people.
    Shared,
}

/// Makes a file that is not there yet, and fails when the path names anything: a file, a
/// folder, a pipe, or a link, even one that leads nowhere. Nothing is opened that is there, so
/// nothing is cut short and nothing waits on a pipe.
fn new_file(path: &Path, access: Access) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if access == Access::OwnerOnly {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Off unix, the folder's own access rules apply to both kinds.
    #[cfg(not(unix))]
    let _ = access;
    options.open(path)
}

/// Whether two paths of files that are not there yet name the same file: the same name in the
/// same folder, once each folder's own path is resolved. A folder that cannot be resolved has
/// no file to compare, and making the file there fails later.
fn names_the_same_new_file(one: &Path, other: &Path) -> bool {
    let resolved = |path: &Path| {
        let folder = match path.parent() {
            Some(folder) if !folder.as_os_str().is_empty() => folder,
            _ => Path::new("."),
        };
        Some((fs::canonicalize(folder).ok()?, path.file_name()?.to_owned()))
    };
    match (resolved(one), resolved(other)) {
        (Some(one), Some(other)) => one == other,
        _ => false,
    }
}

/// The error when keygen made the private key file and cannot make the keys document. It names
/// the file that was made, because dyno removes no file by its path: the person does.
///
/// A path that is there already is the usual cause when the two names differ only in their
/// case, on a volume that takes such names as one file.
fn second_file_not_made(key: &Path, out: &Path, error: &io::Error) -> CliError {
    let made = format!(
        "{} was made and is empty: remove it before you try again",
        key.display()
    );
    match error.kind() {
        io::ErrorKind::AlreadyExists => already_there(out, &made),
        _ => CliError::internal(format!("cannot write {}: {error}. {made}", out.display())),
    }
}

/// The usage error for an output path that is there already.
fn already_there(path: &Path, what_to_do: &str) -> CliError {
    CliError::usage(format!(
        "{} is there already, and dyno receipt writes over no file: {what_to_do}",
        path.display()
    ))
}

fn cannot_write(path: &Path, error: &io::Error) -> CliError {
    CliError::internal(format!("cannot write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_now_is_a_time_as_a_receipt_states_it() {
        let now = now_to_the_second();
        assert!(dyno_receipt::is_receipt_time(&now), "{now}");
    }

    #[test]
    fn two_spellings_of_one_new_file_are_the_same_file() {
        let folder = tempfile::tempdir().unwrap();
        let inside = folder.path().join("inside");
        fs::create_dir(&inside).unwrap();
        let key = folder.path().join("lab-key.json");
        let same = [
            folder.path().join(".").join("lab-key.json"),
            inside.join("..").join("lab-key.json"),
        ];
        for other in same {
            assert!(names_the_same_new_file(&key, &other), "{}", other.display());
        }
        let different = [
            folder.path().join("keys.json"),
            inside.join("lab-key.json"),
            folder.path().join("no-such-folder").join("lab-key.json"),
            folder.path().join(".."),
        ];
        for other in different {
            assert!(
                !names_the_same_new_file(&key, &other),
                "{}",
                other.display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_reached_through_a_link_holds_the_same_new_file() {
        let folder = tempfile::tempdir().unwrap();
        let real = folder.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = folder.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(names_the_same_new_file(
            &real.join("lab-key.json"),
            &link.join("lab-key.json")
        ));
    }

    #[test]
    fn the_error_for_a_second_file_that_cannot_be_made_names_the_first() {
        let (key, out) = (Path::new("Lab.json"), Path::new("lab.json"));
        // What a volume that takes the two names as one file answers.
        let there = io::Error::from(io::ErrorKind::AlreadyExists);
        let error = second_file_not_made(key, out, &there);
        assert_eq!(error.exit_code(), crate::exit::USAGE);
        assert_eq!(
            error.to_string(),
            "lab.json is there already, and dyno receipt writes over no file: Lab.json was made \
             and is empty: remove it before you try again"
        );
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let error = second_file_not_made(key, out, &denied);
        assert_eq!(error.exit_code(), crate::exit::INTERNAL);
        let said = error.to_string();
        assert!(said.starts_with("cannot write lab.json: "), "{said}");
        assert!(said.ends_with("Lab.json was made and is empty: remove it before you try again"));
    }

    #[test]
    fn a_new_file_is_never_made_where_something_is() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("file.json");
        new_file(&path, Access::Shared).unwrap();
        for access in [Access::Shared, Access::OwnerOnly] {
            let error = new_file(&path, access).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        }
        let error = new_file(folder.path(), Access::Shared).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }
}
