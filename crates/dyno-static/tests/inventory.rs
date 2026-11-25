//! The inventory: every file in a build folder or .zip, with its size, kind and SHA-256, and the
//! build id (design section 3).

mod common;

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use common::{expected_value, fixture, fixtures, integer};
use dyno_schema::canonical::sha256_hex;
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{MeasureStatus, UncertaintyKind};
use dyno_schema::path::RelPath;
use dyno_static::ScanError;
use dyno_static::inventory;
use dyno_static::model::Inventory;
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

/// Operating system litter and version control, which a build never needs.
const LITTER: [&str; 5] = [".DS_Store", "Thumbs.db", "desktop.ini", "__MACOSX", ".git"];

#[test]
fn every_fixture_gives_the_files_and_build_id_its_inventory_lists() {
    for fixture in fixtures() {
        let inventory = scan(&fixture.build);
        assert_eq!(
            inventory.into_manifest(),
            Some(fixture.manifest()),
            "{}",
            fixture.name
        );
    }
}

#[test]
fn every_fixture_counts_its_files_as_an_exact_estimate() {
    for fixture in fixtures() {
        let expected = fixture.required_table("inventory");
        let scan = inventory::scan(&fixture.build).unwrap();
        let [file_count] = scan.measures.as_slice() else {
            panic!("{}: the inventory gives one measure", fixture.name);
        };
        assert_eq!(
            file_count.id,
            MeasureId::StaticInventoryFiles,
            "{}",
            fixture.name
        );
        assert_eq!(
            file_count.status,
            MeasureStatus::Estimated,
            "{}",
            fixture.name
        );
        assert_eq!(
            file_count.uncertainty.kind,
            UncertaintyKind::Exact,
            "{}",
            fixture.name
        );
        assert_eq!(
            file_count.value,
            Some(expected_value(integer(expected, "file_count"))),
            "{}",
            fixture.name
        );
    }
}

#[test]
fn the_file_count_names_the_litter_it_leaves_out() {
    let build = folder_with(&[("index.html", b"<html>")]);
    let scan = inventory::scan(build.path()).unwrap();
    let [not_covered] = scan.measures[0].scope.not_covered.as_slice() else {
        panic!("one scope note: {:?}", scan.measures[0].scope);
    };
    for name in LITTER {
        assert!(not_covered.contains(name), "{name} in {not_covered:?}");
    }
}

#[test]
fn a_zip_gives_the_same_inventory_as_the_folder_it_holds() {
    let zipped = scan(&fixture("zip-input").build);
    let folder = scan(&fixture("duplicates").build);
    assert_eq!(zipped, folder);
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_skipped_not_followed() {
    let outside = folder_with(&[("secret.js", b"secret"), ("assets/hidden.png", b"png")]);
    let build = folder_with(&[("index.html", b"<html>")]);
    std::os::unix::fs::symlink(
        outside.path().join("secret.js"),
        build.path().join("linked.js"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("assets"),
        build.path().join("linked-assets"),
    )
    .unwrap();
    assert_eq!(paths(&scan(build.path())), ["index.html"]);
}

#[test]
fn litter_is_skipped_in_any_folder() {
    let build = folder_with(&[("index.html", b"<html>")]);
    for name in LITTER {
        write_file(build.path(), &format!("assets/{name}"), b"litter");
        write_file(build.path(), &format!("{name}/inside.js"), b"litter");
    }
    assert_eq!(paths(&scan(build.path())), ["index.html"]);
}

/// Every path under `folder` that gives a file when links are followed, relative to it: what a
/// server would give if it followed any path it was asked for.
fn every_reachable_file(folder: &Path) -> BTreeSet<String> {
    walkdir::WalkDir::new(folder)
        .follow_links(true)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let relative = entry.path().strip_prefix(folder).unwrap();
            RelPath::from_path(relative).unwrap().as_str().to_owned()
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn a_path_is_a_listed_file_exactly_when_the_inventory_lists_it() {
    use std::os::unix::fs::symlink;

    let outside = folder_with(&[("secret.js", b"secret"), ("assets/hidden.png", b"png")]);
    let build = folder_with(&[
        ("index.html", b"<html>"),
        ("js/main.js", b"1"),
        ("assets/a b.png", b"png"),
    ]);
    // The folder with its own links resolved, as a server holds it.
    let root = build.path().canonicalize().unwrap();
    for name in LITTER {
        write_file(&root, &format!("assets/{name}"), b"litter");
        write_file(&root, &format!("{name}/inside.js"), b"litter");
    }
    // Links out of the build, and links to a file and a folder that the inventory lists.
    symlink(outside.path().join("secret.js"), root.join("linked.js")).unwrap();
    symlink(outside.path().join("assets"), root.join("linked-assets")).unwrap();
    symlink(root.join("js/main.js"), root.join("same-main.js")).unwrap();
    symlink(root.join("js"), root.join("same-js")).unwrap();

    let inventory = scan(&root);
    let listed: BTreeSet<String> = paths(&inventory).into_iter().map(str::to_owned).collect();
    assert_eq!(
        listed,
        BTreeSet::from(["assets/a b.png", "index.html", "js/main.js"].map(str::to_owned))
    );
    let reachable = every_reachable_file(&root);
    assert!(reachable.is_superset(&listed));
    // 5 litter files, 5 files in litter folders, 2 files through links out of the build, and 2
    // through links inside it.
    assert_eq!(reachable.len(), listed.len() + 14, "{reachable:?}");
    for path in &reachable {
        let file = inventory::listed_file(&root, &RelPath::new(path).unwrap());
        if listed.contains(path) {
            assert_eq!(file, Some(root.join(path)), "{path}");
        } else {
            assert_eq!(file, None, "{path}");
        }
    }
}

#[test]
fn a_path_that_names_no_file_is_not_a_listed_file() {
    let build = folder_with(&[("index.html", b"<html>"), ("js/main.js", b"1")]);
    let root = build.path().canonicalize().unwrap();
    for path in ["missing.js", "js", "js/missing.js", "index.html/inside"] {
        let path = RelPath::new(path).unwrap();
        assert_eq!(inventory::listed_file(&root, &path), None, "{path:?}");
    }
}

#[test]
fn every_file_of_every_fixture_folder_is_a_listed_file() {
    let mut folders = 0;
    for fixture in fixtures().into_iter().filter(|f| f.build.is_dir()) {
        folders += 1;
        let root = fixture.build.canonicalize().unwrap();
        for file in scan(&root).files() {
            assert_eq!(
                inventory::listed_file(&root, &file.path),
                Some(root.join(file.path.as_str())),
                "{}: {:?}",
                fixture.name,
                file.path
            );
        }
    }
    assert!(folders > 0);
}

#[test]
fn a_zip_drops_the_one_folder_its_build_sits_in() {
    let zip = zip_with(&[("game/index.html", b"<html>"), ("game/js/main.js", b"1")]);
    assert_eq!(paths(&scan(&zip.path)), ["index.html", "js/main.js"]);
}

#[test]
fn a_zip_with_a_file_at_its_root_keeps_its_paths() {
    let zip = zip_with(&[("game/main.js", b"1"), ("index.html", b"<html>")]);
    assert_eq!(paths(&scan(&zip.path)), ["game/main.js", "index.html"]);
}

#[test]
fn a_zip_skips_folders_symbolic_links_and_litter() {
    let zip = TempZip::new();
    let mut writer = ZipWriter::new(File::create(&zip.path).unwrap());
    let options = SimpleFileOptions::default();
    writer.add_directory("game/assets/", options).unwrap();
    writer
        .add_symlink("game/linked.js", "../../secret.js", options)
        .unwrap();
    for (name, contents) in [
        ("game/index.html", b"<html>".as_slice()),
        ("game/.DS_Store", b"litter"),
        ("__MACOSX/game/._index.html", b"litter"),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(contents).unwrap();
    }
    writer.finish().unwrap();
    assert_eq!(paths(&scan(&zip.path)), ["index.html"]);
}

#[test]
fn a_zip_entry_outside_the_build_is_rejected() {
    for name in ["../escape.js", "game/../../escape.js", "/etc/escape.js"] {
        let zip = zip_with(&[(name, b"1")]);
        let result = inventory::scan(&zip.path);
        assert!(matches!(result, Err(ScanError::BadPath(_))), "{name}");
    }
}

#[test]
fn a_zip_with_two_entries_for_one_path_is_rejected() {
    let zip = zip_with(&[("main.js", b"1"), ("./main.js", b"2")]);
    let result = inventory::scan(&zip.path);
    assert!(matches!(result, Err(ScanError::DuplicateZipEntry(_))));
}

#[test]
fn a_zip_entry_name_with_a_nul_byte_is_rejected() {
    let zip = zip_with(&[("a.js\0", b"1")]);
    let result = inventory::scan(&zip.path);
    assert!(matches!(result, Err(ScanError::NulInZipName(_))));
}

#[test]
fn a_damaged_zip_is_rejected() {
    let zip = TempZip::new();
    fs::write(&zip.path, b"not a zip").unwrap();
    let result = inventory::scan(&zip.path);
    assert!(matches!(result, Err(ScanError::BadZip { .. })));
}

#[test]
fn the_build_id_hashes_each_path_and_sha256_in_path_order() {
    let build = folder_with(&[("b.js", b"b"), ("a.js", b"a")]);
    let lines = format!("a.js\0{}\nb.js\0{}\n", sha256_hex(b"a"), sha256_hex(b"b"));
    let manifest = scan(build.path()).into_manifest().unwrap();
    assert_eq!(manifest.build_id, Some(sha256_hex(lines.as_bytes())));
}

#[test]
fn renaming_a_file_changes_the_build_id() {
    let before = folder_with(&[("a.js", b"same")]);
    let after = folder_with(&[("b.js", b"same")]);
    let build_id = |build: &TempDir| scan(build.path()).into_manifest().unwrap().build_id;
    assert_ne!(build_id(&before), build_id(&after));
}

/// A .zip in a temporary folder, which goes when it drops.
struct TempZip {
    _folder: TempDir,
    path: std::path::PathBuf,
}

impl TempZip {
    fn new() -> TempZip {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("build.zip");
        TempZip {
            _folder: folder,
            path,
        }
    }
}

fn scan(build: &Path) -> Inventory {
    inventory::scan(build)
        .unwrap_or_else(|error| panic!("{build:?}: {error}"))
        .summary
}

fn paths(inventory: &Inventory) -> Vec<&str> {
    inventory
        .files()
        .iter()
        .map(|file| file.path.as_str())
        .collect()
}

fn folder_with(files: &[(&str, &[u8])]) -> TempDir {
    let folder = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        write_file(folder.path(), path, contents);
    }
    folder
}

fn write_file(folder: &Path, path: &str, contents: &[u8]) {
    let path = folder.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn zip_with(entries: &[(&str, &[u8])]) -> TempZip {
    let zip = TempZip::new();
    let mut writer = ZipWriter::new(File::create(&zip.path).unwrap());
    for (name, contents) in entries {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(contents).unwrap();
    }
    writer.finish().unwrap();
    zip
}
