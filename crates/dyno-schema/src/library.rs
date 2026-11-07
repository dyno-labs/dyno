//! The libraries the static scan detects in a build, and the bucket each belongs to by what it
//! does.
//!
//! `static.bundle.engines` keys each library it finds `<bucket>.<library>`, such as
//! `rendering.pixi`, so a result says each library's bucket without a lookup. The scan detects a
//! library only by a signature it knows, so a library it does not know is never detected and
//! never gets a bucket.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A library the static scan can detect.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Library {
    Pixi,
    Howler,
    /// StakeEngine's web-sdk. It bundles its own Pixi, which the scan detects as Pixi.
    WebSdk,
}

/// What a library does. Only the rendering bucket says how the game draws.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum LibraryBucket {
    /// Draws the game, such as Pixi.
    Rendering,
    /// Plays sound, such as Howler.
    Audio,
    /// Anything else, such as an SDK or a utility.
    Other,
}

impl Library {
    /// Every library the scan detects, in the order the result lists them.
    pub const ALL: &[Library] = &[Library::Howler, Library::Pixi, Library::WebSdk];

    /// The bucket of each library. A library added later must be given one here: there is no
    /// default.
    pub fn bucket(self) -> LibraryBucket {
        match self {
            Library::Pixi => LibraryBucket::Rendering,
            Library::Howler => LibraryBucket::Audio,
            Library::WebSdk => LibraryBucket::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Library::Pixi => "pixi",
            Library::Howler => "howler",
            Library::WebSdk => "web_sdk",
        }
    }

    /// The key of the library's item in `static.bundle.engines`: `<bucket>.<library>`.
    pub fn item_key(self) -> String {
        format!("{}.{}", self.bucket().as_str(), self.as_str())
    }
}

impl LibraryBucket {
    pub fn as_str(self) -> &'static str {
        match self {
            LibraryBucket::Rendering => "rendering",
            LibraryBucket::Audio => "audio",
            LibraryBucket::Other => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_library_has_the_bucket_of_what_it_does() {
        assert_eq!(Library::Pixi.bucket(), LibraryBucket::Rendering);
        assert_eq!(Library::Howler.bucket(), LibraryBucket::Audio);
        assert_eq!(Library::WebSdk.bucket(), LibraryBucket::Other);
    }

    #[test]
    fn an_item_key_is_the_bucket_then_the_library() {
        assert_eq!(Library::Pixi.item_key(), "rendering.pixi");
        assert_eq!(Library::Howler.item_key(), "audio.howler");
        assert_eq!(Library::WebSdk.item_key(), "other.web_sdk");
    }

    #[test]
    fn the_names_match_their_json() {
        for &library in Library::ALL {
            let json = serde_json::to_value(library).unwrap();
            assert_eq!(json, library.as_str());
            let bucket = serde_json::to_value(library.bucket()).unwrap();
            assert_eq!(bucket, library.bucket().as_str());
        }
    }
}
