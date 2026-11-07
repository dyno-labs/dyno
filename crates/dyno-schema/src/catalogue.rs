//! A catalogue run: `dyno check --games <games.toml> --shard <i/n>`.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

use crate::config::{self, ConfigFormat};
use crate::launch_url::LaunchUrl;
use crate::result::CheckResult;
use crate::status::Status;

/// A games file: the games a catalogue run checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GamesFile {
    /// The file format version. Must equal the schema version this dyno reads.
    pub schema_version: u32,
    /// One `[[game]]` table per game.
    #[serde(rename = "game")]
    pub games: Vec<GameEntry>,
}

/// One game in a games file. It needs a build, a launch URL or both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GameEntry {
    /// Unique in the file. Shards split the games in id order.
    pub id: String,
    /// The build folder or .zip, relative to the games file.
    #[serde(default)]
    pub build: Option<String>,
    /// An http or https URL, checked when the file loads.
    #[serde(default)]
    pub launch_url: Option<String>,
    /// The release version label for the result.
    #[serde(default)]
    pub release: Option<String>,
}

/// Shard I of N, written "I/N" with I from 1 to N. Game k in id order, counting from 0, falls
/// in shard (k mod N) + 1.
///
/// `Shard::new` and deserializing both check that 1 <= I <= N, so every shard is valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[schemars(deny_unknown_fields)]
pub struct Shard {
    /// I, from 1 to N.
    #[schemars(range(min = 1))]
    index: u32,
    /// N, the number of shards.
    #[schemars(range(min = 1))]
    count: u32,
}

/// A shard as JSON holds it, before `Shard::new` checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShard {
    index: u32,
    count: u32,
}

/// The output of a catalogue run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CatalogueResult {
    pub schema_version: u32,
    /// Null when the run covered every game.
    pub shard: Option<Shard>,
    /// The worst status of any game, by the precedence on `Status`. Incomplete when the
    /// selection has no games, because an empty run checked nothing.
    pub status: Status,
    /// Sorted by game id.
    pub games: Vec<CatalogueEntry>,
}

/// One game's result in a catalogue run. Each result keeps its own hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CatalogueEntry {
    pub id: String,
    pub result: CheckResult,
}

/// Why a games file was rejected.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("games file {0}")]
pub struct GamesFileError(String);

impl GamesFile {
    /// Reads a games file. A `.json` file is JSON. Anything else is TOML.
    pub fn load(path: &Path) -> Result<GamesFile, GamesFileError> {
        let games: GamesFile = config::load(path).map_err(GamesFileError)?;
        games
            .validate()
            .map_err(|problem| GamesFileError(format!("{}: {problem}", path.display())))?;
        Ok(games)
    }

    /// Parses a games file's text.
    pub fn parse(text: &str, format: ConfigFormat) -> Result<GamesFile, GamesFileError> {
        let games: GamesFile = config::parse(text, format).map_err(GamesFileError)?;
        games.validate().map_err(GamesFileError)?;
        Ok(games)
    }

    /// The games in this shard, in id order. Every game when `shard` is None.
    pub fn select(&self, shard: Option<Shard>) -> Vec<&GameEntry> {
        let mut games: Vec<&GameEntry> = self.games.iter().collect();
        games.sort_by(|left, right| left.id.cmp(&right.id));
        let Some(shard) = shard else {
            return games;
        };
        games
            .into_iter()
            .enumerate()
            .filter(|(position, _)| shard.contains(*position))
            .map(|(_, game)| game)
            .collect()
    }

    fn validate(&self) -> Result<(), String> {
        config::check_schema_version(self.schema_version)?;
        let mut seen = BTreeSet::new();
        for game in &self.games {
            if game.id.trim().is_empty() {
                return Err("a game has an empty id".to_owned());
            }
            if game.id.trim() != game.id {
                return Err(format!(
                    "game id {:?} starts or ends with white space",
                    game.id
                ));
            }
            if !seen.insert(game.id.as_str()) {
                return Err(format!("game id {:?} appears more than once", game.id));
            }
            if game.build.is_none() && game.launch_url.is_none() {
                return Err(format!(
                    "game {:?} needs a build, a launch_url or both",
                    game.id
                ));
            }
            if let Some(launch_url) = &game.launch_url {
                LaunchUrl::parse(launch_url)
                    .map_err(|error| format!("game {:?}: {error}", game.id))?;
            }
        }
        Ok(())
    }
}

impl Shard {
    /// Shard `index` of `count`. Errors unless 1 <= index <= count.
    pub fn new(index: u32, count: u32) -> Result<Shard, String> {
        if index == 0 || index > count {
            return Err(format!(
                "shard {index}/{count} must be I/N with 1 <= I <= N, for example 1/4"
            ));
        }
        Ok(Shard { index, count })
    }

    /// I, from 1 to N.
    pub fn index(self) -> u32 {
        self.index
    }

    /// N, the number of shards.
    pub fn count(self) -> u32 {
        self.count
    }

    /// Whether the game at this position in id order, counting from 0, falls in this shard.
    pub fn contains(self, position: usize) -> bool {
        let shard_count = self.count as usize;
        let shard_from_zero = position % shard_count;
        let shard_from_one = shard_from_zero + 1;
        shard_from_one == self.index as usize
    }
}

impl<'de> Deserialize<'de> for Shard {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Shard, D::Error> {
        let raw = RawShard::deserialize(deserializer)?;
        Shard::new(raw.index, raw.count).map_err(serde::de::Error::custom)
    }
}

impl FromStr for Shard {
    type Err = String;

    fn from_str(text: &str) -> Result<Shard, String> {
        let invalid = || format!("shard {text:?} must be I/N with 1 <= I <= N, for example 1/4");
        let (index, count) = text.split_once('/').ok_or_else(invalid)?;
        let index: u32 = index.trim().parse().map_err(|_| invalid())?;
        let count: u32 = count.trim().parse().map_err(|_| invalid())?;
        Shard::new(index, count).map_err(|_| invalid())
    }
}

impl fmt::Display for Shard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.index, self.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREE_GAMES: &str = r#"
        schema_version = 0

        [[game]]
        id = "c"
        build = "builds/c"

        [[game]]
        id = "a"
        launch_url = "https://example.com/a"

        [[game]]
        id = "b"
        build = "builds/b"
        release = "v2"
    "#;

    fn ids(games: &[&GameEntry]) -> Vec<String> {
        games.iter().map(|game| game.id.clone()).collect()
    }

    #[test]
    fn shards_split_games_in_id_order() {
        let games = GamesFile::parse(THREE_GAMES, ConfigFormat::Toml).unwrap();
        let first: Shard = "1/2".parse().unwrap();
        let second: Shard = "2/2".parse().unwrap();
        assert_eq!(ids(&games.select(Some(first))), ["a", "c"]);
        assert_eq!(ids(&games.select(Some(second))), ["b"]);
    }

    #[test]
    fn no_shard_selects_every_game_in_id_order() {
        let games = GamesFile::parse(THREE_GAMES, ConfigFormat::Toml).unwrap();
        assert_eq!(ids(&games.select(None)), ["a", "b", "c"]);
    }

    #[test]
    fn bad_shards_are_rejected() {
        for text in ["0/2", "3/2", "1", "a/b", "1/0"] {
            assert!(text.parse::<Shard>().is_err(), "{text}");
        }
    }

    #[test]
    fn deserializing_a_bad_shard_is_rejected() {
        let error = serde_json::from_str::<Shard>(r#"{"index": 0, "count": 0}"#).unwrap_err();
        assert!(error.to_string().contains("1 <= I <= N"), "{error}");
        let shard: Shard = serde_json::from_str(r#"{"index": 2, "count": 3}"#).unwrap();
        assert_eq!(shard, Shard::new(2, 3).unwrap());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let text = r#"
            schema_version = 0

            [[game]]
            id = "a"
            build = "x"

            [[game]]
            id = "a"
            build = "y"
        "#;
        let error = GamesFile::parse(text, ConfigFormat::Toml).unwrap_err();
        assert!(error.to_string().contains("more than once"), "{error}");
    }

    #[test]
    fn ids_with_white_space_around_them_are_rejected() {
        let text = "schema_version = 0\n[[game]]\nid = \" a\"\nbuild = \"x\"\n";
        let error = GamesFile::parse(text, ConfigFormat::Toml).unwrap_err();
        assert!(error.to_string().contains("white space"), "{error}");
    }

    #[test]
    fn a_game_needs_a_target() {
        let text = "schema_version = 0\n[[game]]\nid = \"a\"\n";
        let error = GamesFile::parse(text, ConfigFormat::Toml).unwrap_err();
        assert!(error.to_string().contains("needs a build"), "{error}");
    }

    #[test]
    fn a_bad_launch_url_is_rejected_without_repeating_it() {
        let text =
            "schema_version = 0\n[[game]]\nid = \"a\"\nlaunch_url = \"https://u:secret/x@h/\"\n";
        let error = GamesFile::parse(text, ConfigFormat::Toml).unwrap_err();
        assert!(error.to_string().contains("not a valid URL"), "{error}");
        assert!(!error.to_string().contains("secret"), "{error}");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let text = "schema_version = 0\n[[game]]\nid = \"a\"\nbuild = \"x\"\nbiuld = \"y\"\n";
        let error = GamesFile::parse(text, ConfigFormat::Toml).unwrap_err();
        assert!(
            error.to_string().contains("unknown field `biuld`"),
            "{error}"
        );
    }
}
