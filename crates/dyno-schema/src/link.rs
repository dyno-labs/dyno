//! The link file, `dyno.toml`: which game on the dyno site each build folder belongs to.
//!
//! dyno looks for it in the build's parent folders, nearest first, and reads the first one it
//! finds. Its entry for the build gives the game's id, and the game's name as the `--game` label
//! when none is given. A file with no entry for the build gives no game. One file can name many
//! build folders, as a games file does, so one repository can hold many games.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::{self, ConfigFormat};

/// The link file's name.
pub const LINK_FILE_NAME: &str = "dyno.toml";

/// A link file: one `[[game]]` table per build folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkFile {
    /// The file format version. Must equal the schema version this dyno reads.
    pub schema_version: u32,
    /// One `[[game]]` table per build folder.
    #[serde(rename = "game", default)]
    pub games: Vec<LinkedGame>,
}

/// One build folder and the game it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkedGame {
    /// The game's id on the dyno site, a UUID.
    pub id: String,
    /// The game's name: the `--game` label when none is given.
    #[serde(default)]
    pub name: Option<String>,
    /// The build folder or .zip, relative to this file. Unique in the file.
    pub build: String,
}

/// The entry for a build, and the file it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundLink {
    pub file: PathBuf,
    pub game: LinkedGame,
}

/// Why a link file was rejected.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("link file {0}")]
pub struct LinkFileError(String);

impl LinkFile {
    /// Reads a link file. It is TOML whatever its name.
    pub fn load(path: &Path) -> Result<LinkFile, LinkFileError> {
        let name_the_file =
            |problem: String| LinkFileError(format!("{}: {problem}", path.display()));
        let text =
            std::fs::read_to_string(path).map_err(|error| name_the_file(error.to_string()))?;
        LinkFile::parse_toml(&text).map_err(name_the_file)
    }

    /// Parses a link file's text.
    pub fn parse(text: &str) -> Result<LinkFile, LinkFileError> {
        LinkFile::parse_toml(text).map_err(LinkFileError)
    }

    fn parse_toml(text: &str) -> Result<LinkFile, String> {
        let mut file: LinkFile = config::parse(text, ConfigFormat::Toml)?;
        file.validate()?;
        Ok(file)
    }

    /// Checks the file, and writes each game id in its canonical form.
    fn validate(&mut self) -> Result<(), String> {
        config::check_schema_version(self.schema_version)?;
        let mut builds = BTreeSet::new();
        for game in &mut self.games {
            if game.build.trim().is_empty() {
                return Err("a game has an empty build".to_owned());
            }
            if !builds.insert(game.build.clone()) {
                return Err(format!("build {:?} appears more than once", game.build));
            }
            game.id = game_id(&game.id)
                .map_err(|problem| format!("build {:?}: {problem}", game.build))?;
            if game
                .name
                .as_deref()
                .is_some_and(|name| name.trim().is_empty())
            {
                return Err(format!("build {:?} has an empty name", game.build));
            }
        }
        Ok(())
    }

    /// The entry whose build, relative to `folder`, is the same file or folder as `build`. Both
    /// paths are resolved, so a `..` or a link in either one still matches. An entry whose build
    /// does not exist matches nothing.
    pub fn entry_for(&self, folder: &Path, build: &Path) -> Option<&LinkedGame> {
        let build = std::fs::canonicalize(build).ok()?;
        self.games.iter().find(|game| {
            std::fs::canonicalize(folder.join(&game.build)).is_ok_and(|path| path == build)
        })
    }
}

/// A game id in its canonical form: a UUID in lower case with dashes. An error for text that is
/// not a UUID.
pub fn game_id(text: &str) -> Result<String, String> {
    uuid::Uuid::parse_str(text.trim())
        .map(|id| id.hyphenated().to_string())
        .map_err(|_| format!("game id {text:?} is not a UUID"))
}

/// The entry for `build` in the nearest link file above it. A file that does not parse is an
/// error. None when there is no file, the nearest one has no entry for the build, or the build
/// does not exist.
pub fn find_link(build: &Path) -> Result<Option<FoundLink>, LinkFileError> {
    let Ok(build) = std::fs::canonicalize(build) else {
        return Ok(None);
    };
    let Some(file) = nearest_link_file(&build) else {
        return Ok(None);
    };
    let links = LinkFile::load(&file)?;
    let folder = file.parent().unwrap_or(Path::new(""));
    Ok(links.entry_for(folder, &build).map(|game| FoundLink {
        game: game.clone(),
        file: file.clone(),
    }))
}

/// The nearest link file above a build, which must be resolved. The search starts in the build's
/// parent folder, so a file inside a build folder never counts.
pub fn nearest_link_file(build: &Path) -> Option<PathBuf> {
    build
        .ancestors()
        .skip(1)
        .map(|folder| folder.join(LINK_FILE_NAME))
        .find(|file| file.is_file())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const MOON_LANDER: &str = "6f1c2a8e-0000-4000-8000-000000000000";
    const TILE_POP: &str = "1b4e28ba-2fa1-41d2-883f-0016d3cca427";

    /// A repository with two games' builds, and a link file at its root that names them.
    fn repository(link_file: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for build in ["games/moon_lander/dist", "games/tile_pop/dist"] {
            fs::create_dir_all(root.path().join(build)).unwrap();
        }
        fs::write(root.path().join(LINK_FILE_NAME), link_file).unwrap();
        root
    }

    fn two_games() -> String {
        format!(
            r#"
            schema_version = 0

            [[game]]
            id = "{MOON_LANDER}"
            name = "Moon Lander"
            build = "games/moon_lander/dist" # relative to this file

            [[game]]
            id = "{}"
            build = "games/tile_pop/dist/"
            "#,
            TILE_POP.to_ascii_uppercase()
        )
    }

    #[test]
    fn the_nearest_file_with_an_entry_names_the_game() {
        let root = repository(&two_games());
        let build = root.path().join("games/moon_lander/dist");
        let found = find_link(&build).unwrap().unwrap();
        assert_eq!(found.game.id, MOON_LANDER);
        assert_eq!(found.game.name.as_deref(), Some("Moon Lander"));
        assert_eq!(
            found.file,
            root.path().join(LINK_FILE_NAME).canonicalize().unwrap()
        );
    }

    #[test]
    fn a_game_id_is_written_in_its_canonical_form() {
        let root = repository(&two_games());
        let build = root.path().join("games/tile_pop/dist");
        let found = find_link(&build).unwrap().unwrap();
        assert_eq!(found.game.id, TILE_POP);
        assert_eq!(found.game.name, None);
    }

    #[test]
    fn a_build_given_by_another_path_still_matches() {
        let root = repository(&two_games());
        let build = root.path().join("games/tile_pop/../moon_lander/dist/");
        assert_eq!(find_link(&build).unwrap().unwrap().game.id, MOON_LANDER);
    }

    #[test]
    fn a_nearer_file_with_no_entry_for_the_build_gives_no_game() {
        let root = repository(&two_games());
        fs::write(
            root.path().join("games/dyno.toml"),
            format!("schema_version = 0\n[[game]]\nid = \"{TILE_POP}\"\nbuild = \"elsewhere\"\n"),
        )
        .unwrap();
        let build = root.path().join("games/moon_lander/dist");
        assert_eq!(find_link(&build).unwrap(), None);
    }

    #[test]
    fn a_file_above_the_nearest_one_is_not_read() {
        let root = repository("not toml at all");
        fs::write(
            root.path().join("games/dyno.toml"),
            format!(
                "schema_version = 0\n[[game]]\nid = \"{TILE_POP}\"\nbuild = \"moon_lander/dist\"\n"
            ),
        )
        .unwrap();
        let build = root.path().join("games/moon_lander/dist");
        assert_eq!(find_link(&build).unwrap().unwrap().game.id, TILE_POP);
    }

    #[test]
    fn a_nearer_file_with_an_entry_wins() {
        let root = repository(&two_games());
        fs::write(
            root.path().join("games/moon_lander/dyno.toml"),
            format!("schema_version = 0\n[[game]]\nid = \"{TILE_POP}\"\nbuild = \"dist\"\n"),
        )
        .unwrap();
        let build = root.path().join("games/moon_lander/dist");
        assert_eq!(find_link(&build).unwrap().unwrap().game.id, TILE_POP);
    }

    #[test]
    fn a_file_inside_the_build_does_not_count() {
        let root = repository("schema_version = 0\n");
        let build = root.path().join("games/moon_lander/dist");
        fs::write(
            build.join(LINK_FILE_NAME),
            format!("schema_version = 0\n[[game]]\nid = \"{TILE_POP}\"\nbuild = \".\"\n"),
        )
        .unwrap();
        assert_eq!(find_link(&build).unwrap(), None);
    }

    #[test]
    fn no_file_or_no_build_gives_no_link() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("dist")).unwrap();
        assert_eq!(find_link(&root.path().join("dist")).unwrap(), None);
        assert_eq!(find_link(&root.path().join("missing")).unwrap(), None);
    }

    #[test]
    fn a_file_that_does_not_parse_is_an_error_that_names_it() {
        let root = repository(
            "schema_version = 0\n[[game]]\nid = \"not-a-uuid\"\nbuild = \"games/moon_lander/dist\"\n",
        );
        let error = find_link(&root.path().join("games/moon_lander/dist")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("dyno.toml"), "{message}");
        assert!(message.contains("is not a UUID"), "{message}");
    }

    #[test]
    fn bad_files_are_rejected() {
        let cases = [
            ("schema_version = 1\n", "schema_version 1"),
            (
                "schema_version = 0\n[[game]]\nid = \"6f1c2a8e-0000-4000-8000-000000000000\"\nbuild = \" \"\n",
                "empty build",
            ),
            (
                "schema_version = 0\n[[game]]\nid = \"6f1c2a8e-0000-4000-8000-000000000000\"\nbuild = \"a\"\n[[game]]\nid = \"6f1c2a8e-0000-4000-8000-000000000000\"\nbuild = \"a\"\n",
                "more than once",
            ),
            (
                "schema_version = 0\n[[game]]\nid = \"6f1c2a8e-0000-4000-8000-000000000000\"\nbuild = \"a\"\nname = \"\"\n",
                "empty name",
            ),
            (
                "schema_version = 0\n[[game]]\nid = \"6f1c2a8e-0000-4000-8000-000000000000\"\nbuild = \"a\"\nbiuld = \"b\"\n",
                "unknown field `biuld`",
            ),
        ];
        for (text, problem) in cases {
            let error = LinkFile::parse(text).unwrap_err().to_string();
            assert!(error.contains(problem), "{text}: {error}");
        }
    }

    #[test]
    fn a_game_id_must_be_a_uuid() {
        assert_eq!(
            game_id(" 6F1C2A8E-0000-4000-8000-000000000000 ").unwrap(),
            MOON_LANDER
        );
        assert!(game_id("moon_lander").is_err());
    }
}
