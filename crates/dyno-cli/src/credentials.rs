//! The saved token of `dyno login`, and `DYNO_TOKEN`, which wins over it.
//!
//! The token is in `credentials.json` in dyno's config folder: `DYNO_CONFIG_DIR`, or else `dyno`
//! in `XDG_CONFIG_HOME`, or else `~/.config/dyno`. On unix only its owner can read it.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::exit::CliError;

/// The environment variable with a token, which wins over the saved one.
pub const TOKEN_VAR: &str = "DYNO_TOKEN";

/// The name of the file in the config folder.
const FILE_NAME: &str = "credentials.json";

/// The content of the credentials file.
#[derive(Deserialize, Serialize)]
struct Credentials {
    token: String,
}

/// Where the token is saved. With no config folder to name, it is a usage error.
pub fn path() -> Result<PathBuf, CliError> {
    let set = |name: &str| env::var_os(name).filter(|value| !value.is_empty());
    let folder = if let Some(folder) = set("DYNO_CONFIG_DIR") {
        PathBuf::from(folder)
    } else if let Some(config) = set("XDG_CONFIG_HOME") {
        Path::new(&config).join("dyno")
    } else if let Some(home) = set("HOME") {
        Path::new(&home).join(".config").join("dyno")
    } else {
        return Err(CliError::usage(
            "no folder for the token: set DYNO_CONFIG_DIR or HOME",
        ));
    };
    Ok(folder.join(FILE_NAME))
}

/// The token to use: `DYNO_TOKEN`, or else the saved one. None when there is neither.
pub fn token() -> Result<Option<String>, CliError> {
    if let Some(token) = env_token() {
        return Ok(Some(token));
    }
    load(&path()?)
}

/// The token in `DYNO_TOKEN`, when it is set and not empty.
pub fn env_token() -> Option<String> {
    env::var(TOKEN_VAR)
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
}

/// The token saved at `path`. None when there is no file. A file that cannot be read is a usage
/// error.
fn load(path: &Path) -> Result<Option<String>, CliError> {
    let shown = path.display();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(CliError::usage(format!("cannot read {shown}: {error}"))),
    };
    let credentials: Credentials = serde_json::from_str(&text).map_err(|error| {
        CliError::usage(format!(
            "{shown} is not a dyno credentials file: {error}. Run dyno login again"
        ))
    })?;
    Ok(Some(credentials.token))
}

/// Saves the token at `path`, readable by its owner only, and makes the folder if it is not
/// there. The file is written beside its final place and then renamed, so a reader never sees
/// half of it.
pub fn save(path: &Path, token: &str) -> Result<(), CliError> {
    let cannot_save = |error: io::Error| {
        CliError::internal(format!(
            "cannot save the token to {}: {error}",
            path.display()
        ))
    };
    let folder = path.parent().unwrap_or(Path::new("."));
    make_private_folder(folder).map_err(cannot_save)?;
    let mut json = serde_json::to_string(&Credentials {
        token: token.to_owned(),
    })
    .map_err(CliError::internal)?;
    json.push('\n');
    // A temporary file is readable by its owner only on unix.
    let mut file = tempfile::NamedTempFile::new_in(folder).map_err(cannot_save)?;
    file.write_all(json.as_bytes()).map_err(cannot_save)?;
    file.persist(path)
        .map_err(|error| cannot_save(error.error))?;
    Ok(())
}

/// Removes the saved token at `path`. False when there was none.
pub fn remove(path: &Path) -> Result<bool, CliError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(CliError::internal(format!(
            "cannot remove {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(unix)]
fn make_private_folder(folder: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
}

#[cfg(not(unix))]
fn make_private_folder(folder: &Path) -> io::Result<()> {
    fs::create_dir_all(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_token_loads_back() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("dyno").join(FILE_NAME);
        assert_eq!(load(&path).unwrap(), None);
        save(&path, "dyno_one").unwrap();
        save(&path, "dyno_two").unwrap();
        assert_eq!(load(&path).unwrap().as_deref(), Some("dyno_two"));
        assert!(remove(&path).unwrap());
        assert!(!remove(&path).unwrap());
    }

    #[test]
    fn a_file_that_is_not_credentials_is_a_usage_error() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(FILE_NAME);
        fs::write(&path, "token = 1").unwrap();
        assert!(matches!(load(&path), Err(CliError::Usage(_))));
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_an_io_error() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("a-file");
        fs::write(&file, "").unwrap();
        let path = file.join(FILE_NAME);
        assert!(matches!(save(&path, "dyno_x"), Err(CliError::Internal(_))));
        assert!(matches!(load(&path), Err(CliError::Usage(_))));
        assert!(matches!(remove(&path), Err(CliError::Internal(_))));
    }
}
