//! `dyno link`: finds or makes the build's game on the dyno site, then writes the build's entry in
//! `dyno.toml`, so that every later check of the build names the game (`dyno_schema::link`).
//!
//! The file keeps its comments and its other entries: dyno edits it with `toml_edit`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use dyno_schema::SCHEMA_VERSION;
use dyno_schema::link::{LINK_FILE_NAME, LinkFile, game_id, nearest_link_file};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value, value};

use crate::cli::LinkArgs;
use crate::exit::CliError;
use crate::site::{NOT_FOUND, SiteError};
use crate::upload::{Uploader, sign_in_again};

/// Where an entry goes: the link file's folder, the build, and the build's path from the folder.
struct Place {
    folder: PathBuf,
    build: PathBuf,
    /// Relative, with `/` between its parts.
    build_path: String,
}

/// Runs `dyno link`. The link file and the build are read before the site is asked, so a bad
/// file or build makes no game.
pub fn run(args: LinkArgs) -> Result<(), CliError> {
    let config = args
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from(LINK_FILE_NAME));
    if config.file_name() != Some(LINK_FILE_NAME.as_ref()) {
        return Err(CliError::usage(format!(
            "{} must be named {LINK_FILE_NAME}, because dyno check looks only for that name",
            config.display()
        )));
    }
    let place = place(&config, &args.build)?;
    let text = read_link_file(&config)?;
    let document = link_document(text.as_deref())
        .map_err(|problem| CliError::usage(format!("{}: {problem}", config.display())))?;
    let uploader = Uploader::signed_in()?;
    let (site, token) = uploader.site_and_token();
    let game = match (&args.name, &args.game_id) {
        (Some(name), _) => {
            let name = name.trim();
            if name.is_empty() {
                return Err(CliError::usage("--name is empty"));
            }
            site.create_game(token, name)
        }
        (None, Some(id)) => site.game(token, id),
        (None, None) => return Err(CliError::usage("give --name or --game-id")),
    }
    .map_err(game_error)?;
    let id = game_id(&game.id).map_err(|problem| {
        CliError::internal(format!("the site gave a game dyno cannot read: {problem}"))
    })?;
    let updated = with_entry(document, &place, &id, &game.name);
    fs::write(&config, updated).map_err(|error| {
        CliError::internal(format!("cannot write {}: {error}", config.display()))
    })?;
    eprintln!(
        "dyno link: {} is {} ({id}), in {}.",
        args.build.display(),
        game.name,
        config.display()
    );
    Ok(())
}

/// Where the build's entry goes in this link file. The build must be below the file's folder, and
/// no other link file may be nearer the build, because `dyno check` reads only the nearest one in
/// the build's parent folders.
fn place(config: &Path, build: &Path) -> Result<Place, CliError> {
    let cannot_read = |error: std::io::Error| {
        CliError::usage(format!("cannot read build {}: {error}", build.display()))
    };
    let resolved = fs::canonicalize(build).map_err(cannot_read)?;
    // A build is what dyno check scans: a folder, or a file whose path as given ends in .zip.
    if resolved.is_dir() {
        fs::read_dir(&resolved).map_err(cannot_read)?;
    } else if build
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        fs::File::open(&resolved).map_err(cannot_read)?;
    } else {
        return Err(CliError::usage(format!(
            "build {} is not a folder or a .zip",
            build.display()
        )));
    }
    let build = resolved;
    let folder = match config.parent() {
        Some(folder) if !folder.as_os_str().is_empty() => folder,
        _ => Path::new("."),
    };
    let folder = fs::canonicalize(folder).map_err(|error| {
        CliError::usage(format!(
            "cannot read the folder of {}: {error}",
            config.display()
        ))
    })?;
    let below = || {
        CliError::usage(format!(
            "{} must be in a folder above the build, because dyno check looks for it in the \
             build's parent folders",
            config.display()
        ))
    };
    let relative = build.strip_prefix(&folder).map_err(|_| below())?;
    let parts = relative
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<Vec<&str>>>()
        .ok_or_else(|| {
            CliError::usage(format!(
                "the path of {} is not UTF-8, so dyno.toml cannot hold it",
                build.display()
            ))
        })?;
    if parts.is_empty() {
        return Err(below());
    }
    if let Some(nearer) = nearest_link_file(&build).filter(|nearest| {
        nearest
            .parent()
            .is_some_and(|nearest| nearest != folder && nearest.starts_with(&folder))
    }) {
        return Err(CliError::usage(format!(
            "{} is nearer the build, so dyno check reads it and not {}: link the build there \
             with --config",
            nearer.display(),
            config.display()
        )));
    }
    Ok(Place {
        build_path: parts.join("/"),
        folder,
        build,
    })
}

/// The link file's text, checked, or None when there is no file yet. A file that cannot be read
/// or does not parse is a usage error.
fn read_link_file(config: &Path) -> Result<Option<String>, CliError> {
    let text = match fs::read_to_string(config) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::usage(format!(
                "cannot read {}: {error}",
                config.display()
            )));
        }
    };
    LinkFile::parse(&text)
        .map_err(|error| CliError::usage(format!("{}: {error}", config.display())))?;
    Ok(Some(text))
}

/// The link file to edit, or a new one, with its games as `[[game]]` tables that an entry can
/// join. An error when they are not tables, so a file that cannot take the entry makes no game.
fn link_document(text: Option<&str>) -> Result<DocumentMut, String> {
    let mut document = match text {
        Some(text) => text
            .parse::<DocumentMut>()
            .map_err(|error| error.to_string())?,
        None => {
            let mut document = DocumentMut::new();
            document["schema_version"] = value(i64::from(SCHEMA_VERSION));
            document
        }
    };
    let games = document
        .entry("game")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    if !games.is_array_of_tables() {
        return Err("game must be [[game]] tables".to_owned());
    }
    Ok(document)
}

/// The link file with the build's entry set to this game: the entry whose build is the same
/// folder or .zip gets the id and the name, and a build with no entry gets a new one at the end.
/// Comments, other entries and the order of the file stay as they were.
fn with_entry(mut document: DocumentMut, place: &Place, id: &str, name: &str) -> String {
    let games = document["game"]
        .as_array_of_tables_mut()
        .expect("link_document makes game an array of tables");
    let entry = games.iter_mut().find(|game| {
        game.get("build")
            .and_then(Item::as_str)
            .is_some_and(|build| same_file(&place.folder.join(build), &place.build))
    });
    match entry {
        Some(game) => {
            set(game, "id", id);
            set(game, "name", name);
        }
        None => {
            let mut game = Table::new();
            game["id"] = value(id);
            game["name"] = value(name);
            game["build"] = value(&place.build_path);
            games.push(game);
        }
    }
    document.to_string()
}

/// Sets a key to a string, and keeps a comment that was after its old value.
fn set(table: &mut Table, key: &str, text: &str) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            let decor = old.decor().clone();
            *old = Value::from(text);
            *old.decor_mut() = decor;
        }
        None => {
            table.insert(key, value(text));
        }
    }
}

/// Whether the path names the same file or folder as `build`, which is resolved.
fn same_file(path: &Path, build: &Path) -> bool {
    fs::canonicalize(path).is_ok_and(|path| path == build)
}

/// A site error as the command's error. A game or token the site does not know is a usage error.
fn game_error(error: SiteError) -> CliError {
    match sign_in_again(error) {
        SiteError::Refused {
            status: NOT_FOUND,
            message,
        } => CliError::usage(message),
        error => error.into_cli_error(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOON_LANDER: &str = "6f1c2a8e-0000-4000-8000-000000000000";
    const TILE_POP: &str = "1b4e28ba-2fa1-41d2-883f-0016d3cca427";

    /// A repository with two build folders, and the place of the first in its dyno.toml.
    fn repository() -> (tempfile::TempDir, Place) {
        let root = tempfile::tempdir().unwrap();
        for build in ["games/moon_lander/dist", "games/tile_pop/dist"] {
            fs::create_dir_all(root.path().join(build)).unwrap();
        }
        let config = root.path().join(LINK_FILE_NAME);
        let place = place(&config, &root.path().join("games/moon_lander/dist")).unwrap();
        (root, place)
    }

    #[test]
    fn a_new_file_gets_the_schema_version_and_the_entry() {
        let (_root, place) = repository();
        let text = with_entry(
            link_document(None).unwrap(),
            &place,
            MOON_LANDER,
            "Moon Lander",
        );
        assert_eq!(
            text,
            format!(
                "schema_version = 0\n\n[[game]]\nid = \"{MOON_LANDER}\"\nname = \"Moon Lander\"\nbuild = \"games/moon_lander/dist\"\n"
            )
        );
        let links = LinkFile::parse(&text).unwrap();
        assert_eq!(
            links.entry_for(&place.folder, &place.build).unwrap().id,
            MOON_LANDER
        );
    }

    #[test]
    fn a_new_entry_goes_at_the_end_and_keeps_the_comments() {
        let (_root, place) = repository();
        let before = format!(
            "# The games of this repository.\nschema_version = 0\n\n[[game]]\n# The puzzle.\nid = \"{TILE_POP}\" # on the site\nbuild = \"games/tile_pop/dist\"\n"
        );
        let text = with_entry(
            link_document(Some(&before)).unwrap(),
            &place,
            MOON_LANDER,
            "Moon Lander",
        );
        assert!(text.starts_with(&before), "{text}");
        assert!(
            text.ends_with("build = \"games/moon_lander/dist\"\n"),
            "{text}"
        );
        assert_eq!(LinkFile::parse(&text).unwrap().games.len(), 2);
    }

    #[test]
    fn the_entry_for_the_build_is_updated_in_place() {
        let (_root, place) = repository();
        let before = format!(
            "schema_version = 0\n\n[[game]]\nid = \"{TILE_POP}\" # the old game\nname = \"Tile Pop\"\nbuild = \"./games/moon_lander/dist/\"\n\n[[game]]\nid = \"{TILE_POP}\"\nbuild = \"games/tile_pop/dist\"\n"
        );
        let text = with_entry(
            link_document(Some(&before)).unwrap(),
            &place,
            MOON_LANDER,
            "Moon Lander",
        );
        assert_eq!(
            text,
            format!(
                "schema_version = 0\n\n[[game]]\nid = \"{MOON_LANDER}\" # the old game\nname = \"Moon Lander\"\nbuild = \"./games/moon_lander/dist/\"\n\n[[game]]\nid = \"{TILE_POP}\"\nbuild = \"games/tile_pop/dist\"\n"
            )
        );
    }

    #[test]
    fn an_entry_with_no_name_gets_one() {
        let (_root, place) = repository();
        let before = format!(
            "schema_version = 0\n[[game]]\nid = \"{TILE_POP}\"\nbuild = \"games/moon_lander/dist\"\n"
        );
        let text = with_entry(
            link_document(Some(&before)).unwrap(),
            &place,
            MOON_LANDER,
            "Moon Lander",
        );
        let links = LinkFile::parse(&text).unwrap();
        assert_eq!(links.games[0].name.as_deref(), Some("Moon Lander"));
        assert_eq!(links.games[0].id, MOON_LANDER);
    }

    #[test]
    fn games_that_are_not_tables_cannot_take_an_entry() {
        let error = link_document(Some("schema_version = 0\ngame = []\n")).unwrap_err();
        assert!(error.contains("[[game]] tables"), "{error}");
    }

    #[test]
    fn a_build_is_a_folder_or_a_zip() {
        let (root, _) = repository();
        let config = root.path().join(LINK_FILE_NAME);
        let readme = root.path().join("games/README.md");
        fs::write(&readme, "# Games\n").unwrap();
        assert!(matches!(place(&config, &readme), Err(CliError::Usage(_))));
        let zip = root.path().join("games/moon_lander.ZIP");
        fs::write(&zip, "PK").unwrap();
        assert_eq!(
            place(&config, &zip).unwrap().build_path,
            "games/moon_lander.ZIP"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_zip_is_known_by_the_path_as_given_as_dyno_check_knows_it() {
        let (root, _) = repository();
        let config = root.path().join(LINK_FILE_NAME);
        let archive = root.path().join("games/archive");
        fs::write(&archive, "PK").unwrap();
        let zip = root.path().join("games/build.zip");
        std::os::unix::fs::symlink(&archive, &zip).unwrap();
        assert_eq!(place(&config, &zip).unwrap().build_path, "games/archive");
    }

    #[test]
    fn the_build_must_be_below_the_file_s_folder() {
        let (root, _) = repository();
        let inside = root.path().join("games/moon_lander/dist/dyno.toml");
        let error = place(&inside, &root.path().join("games/moon_lander/dist"));
        assert!(matches!(error, Err(CliError::Usage(_))));
        let elsewhere = root.path().join("games/tile_pop/dyno.toml");
        let error = place(&elsewhere, &root.path().join("games/moon_lander/dist"));
        assert!(matches!(error, Err(CliError::Usage(_))));
        let missing = place(
            &root.path().join(LINK_FILE_NAME),
            &root.path().join("games/missing"),
        );
        assert!(matches!(missing, Err(CliError::Usage(_))));
    }

    #[test]
    fn a_nearer_link_file_must_take_the_entry() {
        let (root, _) = repository();
        fs::write(root.path().join("games/dyno.toml"), "schema_version = 0\n").unwrap();
        let build = root.path().join("games/moon_lander/dist");
        let error = place(&root.path().join(LINK_FILE_NAME), &build);
        let Err(CliError::Usage(message)) = error else {
            panic!("{:?}", error.map(|place| place.build_path));
        };
        assert!(message.contains("is nearer the build"), "{message}");
        // The nearer file itself, and a file below it, are fine.
        assert!(place(&root.path().join("games").join(LINK_FILE_NAME), &build).is_ok());
        let below = root.path().join("games/moon_lander").join(LINK_FILE_NAME);
        assert!(place(&below, &build).is_ok());
    }

    #[test]
    fn a_file_in_this_folder_takes_a_relative_build_path() {
        let (root, _) = repository();
        let place = place(
            &root.path().join("games").join(LINK_FILE_NAME),
            &root.path().join("games/tile_pop/dist"),
        )
        .unwrap();
        assert_eq!(place.build_path, "tile_pop/dist");
    }
}
