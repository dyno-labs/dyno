//! Reading the input files: budget files and games files. Both are TOML, and JSON also works.

use std::path::Path;

use serde::de::DeserializeOwned;

use crate::SCHEMA_VERSION;

/// The format of an input file, picked by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Json,
}

impl ConfigFormat {
    /// JSON for a `.json` file. TOML for anything else.
    pub fn from_extension(path: &Path) -> ConfigFormat {
        let is_json = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        if is_json {
            ConfigFormat::Json
        } else {
            ConfigFormat::Toml
        }
    }
}

/// Reads and parses an input file. The error message names the file.
pub fn load<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let name_the_file = |error: String| format!("{}: {error}", path.display());
    let text = std::fs::read_to_string(path).map_err(|error| name_the_file(error.to_string()))?;
    parse(&text, ConfigFormat::from_extension(path)).map_err(name_the_file)
}

/// Parses an input file's text. Unknown keys fail on types that deny them.
pub fn parse<T: DeserializeOwned>(text: &str, format: ConfigFormat) -> Result<T, String> {
    match format {
        ConfigFormat::Toml => {
            toml::from_str(text).map_err(|error| error.to_string().trim_end().to_owned())
        }
        ConfigFormat::Json => serde_json::from_str(text).map_err(|error| error.to_string()),
    }
}

/// An error unless an input file's `schema_version` is the one this dyno reads.
pub fn check_schema_version(schema_version: u32) -> Result<(), String> {
    if schema_version == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(format!(
            "has schema_version {schema_version}, but this dyno reads schema_version {SCHEMA_VERSION}"
        ))
    }
}
