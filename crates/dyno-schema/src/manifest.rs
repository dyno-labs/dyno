//! The build manifest: every supplied file with its size, hash and kind, and the build id.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::path::RelPath;

/// Every supplied file of one build. `compare` diffs two manifests to find suspect assets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct BuildManifest {
    /// Lowercase hex SHA-256 over every file's path and SHA-256, sorted by path, so a rename
    /// changes it. It is the join key for lab and field rows, and does not cover external URLs.
    /// Null until the inventory is implemented. docs/design/static-scan.md gives the exact bytes.
    pub build_id: Option<String>,
    /// Sorted by path.
    pub files: Vec<ManifestFile>,
}

/// One supplied file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ManifestFile {
    pub path: RelPath,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the file's bytes.
    pub sha256: String,
    pub kind: FileKind,
}

/// The bucket a file counts in, for bytes by type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Js,
    Css,
    Html,
    Image,
    /// GPU texture containers such as KTX2.
    Texture,
    Audio,
    Video,
    Font,
    /// JSON, atlases, sprite maps and other data files.
    Data,
    Other,
}

impl BuildManifest {
    /// Sorts the files by path.
    pub fn sort_lists(&mut self) {
        self.files.sort_by(|left, right| left.path.cmp(&right.path));
    }
}
