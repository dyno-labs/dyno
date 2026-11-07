//! Paths relative to the build root, always with '/'.

use std::fmt;
use std::path::{Component, Path};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

/// A path relative to the build root, with '/' between parts on every platform.
///
/// Construction normalizes the separators, so the same file gives the same bytes in a result on
/// macOS, Linux and Windows.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, TS)]
#[schemars(transparent)]
#[ts(type = "string")]
pub struct RelPath(String);

/// Why a string is not a relative build path.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RelPathError {
    #[error("path is empty")]
    Empty,
    #[error("path {0:?} is absolute; paths in a result are relative to the build root")]
    Absolute(String),
    #[error("path {0:?} has a '..' part")]
    ParentPart(String),
    #[error("path {0:?} is not valid UTF-8")]
    NotUtf8(String),
    #[error("path {0:?} has a '\\' in a file name")]
    BackslashInName(String),
}

impl RelPath {
    /// Normalizes `raw`: '\' becomes '/', and "." parts and repeated separators go.
    pub fn new(raw: &str) -> Result<RelPath, RelPathError> {
        let slashed = raw.replace('\\', "/");
        if slashed.starts_with('/') || has_drive_letter(&slashed) {
            return Err(RelPathError::Absolute(raw.to_owned()));
        }
        let parts: Vec<&str> = slashed
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .collect();
        if parts.contains(&"..") {
            return Err(RelPathError::ParentPart(raw.to_owned()));
        }
        if parts.is_empty() {
            return Err(RelPathError::Empty);
        }
        Ok(RelPath(parts.join("/")))
    }

    /// Builds the path from a filesystem path that is already relative to the build root.
    ///
    /// A file name must be UTF-8 and must not hold a '\'. On macOS and Linux '\' is a legal
    /// file name character, and the result would read it as a separator. The components are
    /// already checked, so a top folder named "c:" is a folder here, not a drive.
    pub fn from_path(path: &Path) -> Result<RelPath, RelPathError> {
        let shown = || path.display().to_string();
        let mut parts = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => {
                    let part = part
                        .to_str()
                        .ok_or_else(|| RelPathError::NotUtf8(shown()))?;
                    if part.contains('\\') {
                        return Err(RelPathError::BackslashInName(shown()));
                    }
                    parts.push(part);
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(RelPathError::ParentPart(shown()));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(RelPathError::Absolute(shown()));
                }
            }
        }
        if parts.is_empty() {
            return Err(RelPathError::Empty);
        }
        Ok(RelPath(parts.join("/")))
    }

    /// The normalized path, as it appears in JSON.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for RelPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<RelPath, D::Error> {
        let raw = String::deserialize(deserializer)?;
        RelPath::new(&raw).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A Windows drive prefix: "C:" alone, or "C:/" at the start. "a:b.png" is a file name.
fn has_drive_letter(path: &str) -> bool {
    match path.as_bytes() {
        [letter, b':'] => letter.is_ascii_alphabetic(),
        [letter, b':', b'/', ..] => letter.is_ascii_alphabetic(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backslashes_become_slashes() {
        assert_eq!(
            RelPath::new(r"images\hero.png").unwrap().as_str(),
            "images/hero.png"
        );
    }

    #[test]
    fn dot_parts_and_repeated_slashes_go() {
        assert_eq!(
            RelPath::new("./images//./hero.png").unwrap().as_str(),
            "images/hero.png"
        );
    }

    #[test]
    fn root_paths_are_rejected() {
        assert!(matches!(
            RelPath::new("/etc/passwd"),
            Err(RelPathError::Absolute(_))
        ));
    }

    #[test]
    fn drive_letter_paths_are_rejected() {
        assert!(matches!(
            RelPath::new(r"C:\build\a.png"),
            Err(RelPathError::Absolute(_))
        ));
        assert!(matches!(RelPath::new("C:"), Err(RelPathError::Absolute(_))));
    }

    #[test]
    fn a_colon_in_a_file_name_is_not_a_drive() {
        assert_eq!(RelPath::new("a:b.png").unwrap().as_str(), "a:b.png");
    }

    #[test]
    fn parent_parts_are_rejected() {
        assert!(matches!(
            RelPath::new("images/../../secret"),
            Err(RelPathError::ParentPart(_))
        ));
    }

    #[test]
    fn empty_paths_are_rejected() {
        assert_eq!(RelPath::new("./"), Err(RelPathError::Empty));
    }

    #[test]
    fn filesystem_paths_join_with_slashes() {
        let path = Path::new("images").join("ui").join("hero.png");
        assert_eq!(
            RelPath::from_path(&path).unwrap().as_str(),
            "images/ui/hero.png"
        );
    }

    #[test]
    fn filesystem_parent_parts_are_rejected() {
        let path = Path::new("images").join("..").join("hero.png");
        assert!(matches!(
            RelPath::from_path(&path),
            Err(RelPathError::ParentPart(_))
        ));
    }

    #[test]
    fn filesystem_root_paths_are_rejected() {
        let path = Path::new("/").join("hero.png");
        assert!(matches!(
            RelPath::from_path(&path),
            Err(RelPathError::Absolute(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_top_folder_named_like_a_drive_is_a_folder() {
        let path = Path::new("c:").join("x.png");
        assert_eq!(RelPath::from_path(&path).unwrap().as_str(), "c:/x.png");
    }

    #[test]
    fn an_empty_filesystem_path_is_rejected() {
        assert_eq!(RelPath::from_path(Path::new(".")), Err(RelPathError::Empty));
    }

    #[cfg(unix)]
    #[test]
    fn a_backslash_in_a_file_name_is_rejected() {
        let path = Path::new(r"images\hero.png");
        assert!(matches!(
            RelPath::from_path(path),
            Err(RelPathError::BackslashInName(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_name_that_is_not_utf8_is_rejected() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"hero\xff.png"));
        assert!(matches!(
            RelPath::from_path(path),
            Err(RelPathError::NotUtf8(_))
        ));
    }

    #[test]
    fn deserializing_normalizes() {
        let path: RelPath = serde_json::from_str(r#""audio\\music.ogg""#).unwrap();
        assert_eq!(path.as_str(), "audio/music.ogg");
    }

    #[test]
    fn deserializing_rejects_a_path_that_is_not_relative() {
        let error = serde_json::from_str::<RelPath>(r#""../secret""#).unwrap_err();
        assert!(error.to_string().contains("has a '..' part"), "{error}");
    }
}
