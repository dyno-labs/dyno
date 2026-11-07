//! Fixture loading for the static scan tests (fixtures/README.md).
//!
//! Each test file reads only its own top-level table of `expected.toml`.

// Each test file uses only some of these helpers.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use dyno_schema::manifest::{BuildManifest, FileKind, ManifestFile};
use dyno_schema::measure::MeasureValue;
use dyno_schema::path::RelPath;
use dyno_schema::profile::{HostProfile, Viewport};
use dyno_static::model::Inventory;
use toml::{Table, Value};
use walkdir::WalkDir;

/// One fixture: a build, and the values the formulas must give for it.
pub struct Fixture {
    /// The folder name under `fixtures/`.
    pub name: String,
    /// The `build` folder or the `build.zip` file.
    pub build: PathBuf,
    expected: Table,
}

/// File and folder names the inventory skips anywhere in a build (design section 3), so the
/// loader skips them too.
const SKIPPED_NAMES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini", "__MACOSX", ".git"];

/// Milli-units in one: a DPR of 3 is 3,000.
const MILLI: u64 = 1_000;

/// Every fixture, sorted by name. A folder under `fixtures/` is a fixture when it has an
/// `expected.toml`.
pub fn fixtures() -> Vec<Fixture> {
    let mut names: Vec<String> = fs::read_dir(fixtures_dir())
        .expect("fixtures/ is readable")
        .map(|entry| entry.expect("a fixtures/ entry is readable").path())
        .filter(|path| path.join("expected.toml").is_file())
        .map(|path| file_name(&path))
        .collect();
    assert!(!names.is_empty(), "no fixtures found");
    names.sort();
    names.iter().map(|name| fixture(name)).collect()
}

/// The fixture with this folder name.
pub fn fixture(name: &str) -> Fixture {
    let folder = fixtures_dir().join(name);
    let text = fs::read_to_string(folder.join("expected.toml"))
        .unwrap_or_else(|error| panic!("{name}/expected.toml: {error}"));
    let expected: Table = text
        .parse()
        .unwrap_or_else(|error| panic!("{name}/expected.toml: {error}"));
    let input = expected
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{name}/expected.toml has no input"));
    Fixture {
        name: name.to_owned(),
        build: folder.join(input),
        expected,
    }
}

impl Fixture {
    /// One top-level table of `expected.toml`, such as "audio". None when the fixture has no
    /// values of that kind.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.expected.get(name).and_then(Value::as_table)
    }

    /// One top-level table of `expected.toml`. Panics with the fixture's name when it is absent.
    pub fn required_table(&self, name: &str) -> &Table {
        self.table(name)
            .unwrap_or_else(|| panic!("{}/expected.toml has no [{name}]", self.name))
    }

    /// Whether the build is a .zip file, not a folder.
    pub fn is_zip(&self) -> bool {
        self.build
            .extension()
            .is_some_and(|extension| extension == "zip")
    }

    /// Every file in the build, by its path with '/' between parts.
    ///
    /// A zip's paths keep its top folder: stripping it is the inventory's job.
    pub fn files(&self) -> BTreeMap<String, Vec<u8>> {
        if self.is_zip() {
            zip_files(&self.build)
        } else {
            folder_files(&self.build)
        }
    }

    /// The fixture's `[profile]` as the host profile the formulas take: the Reels profile with the
    /// table's values in place of its own.
    pub fn profile(&self) -> HostProfile {
        let table = self.required_table("profile");
        let dpr_milli = integer(table, "dpr_milli");
        assert_eq!(dpr_milli % MILLI, 0, "{}: the DPR must be whole", self.name);
        let frame = table
            .get("frame_css")
            .and_then(Value::as_array)
            .map(Vec::as_slice);
        let Some([width, height]) = frame else {
            panic!("{}: frame_css is not [width, height]", self.name);
        };
        let pixels = |side: &Value| {
            let pixels = side.as_integer().expect("frame_css holds integers");
            u32::try_from(pixels).expect("frame_css fits in u32")
        };
        HostProfile {
            viewport: Viewport {
                width: pixels(width),
                height: pixels(height),
            },
            device_scale_factor: u32::try_from(dpr_milli / MILLI).expect("the DPR fits in u32"),
            webgpu: table
                .get("webgpu")
                .and_then(Value::as_bool)
                .expect("webgpu is a boolean"),
            ..HostProfile::reels()
        }
    }

    /// The manifest that `[inventory]` lists: each file's path, size, kind and SHA-256, and the
    /// build id.
    pub fn manifest(&self) -> BuildManifest {
        let table = self.required_table("inventory");
        let files = rows(table, "file")
            .into_iter()
            .map(|row| ManifestFile {
                path: path(text(row, "path")),
                bytes: integer(row, "bytes"),
                sha256: text(row, "sha256").to_owned(),
                kind: file_kind(text(row, "bucket")),
            })
            .collect();
        BuildManifest {
            build_id: Some(text(table, "build_id").to_owned()),
            files,
        }
    }

    /// The inventory the scan gives for the build, a folder or a zip.
    pub fn scanned_inventory(&self) -> Inventory {
        dyno_static::inventory::scan(&self.build)
            .unwrap_or_else(|error| panic!("{}: {error}", self.name))
            .summary
    }

    /// The inventory that `[inventory]` lists, with the build's bytes.
    ///
    /// Bundle, textures and audio can test with it without running the inventory. Folder
    /// fixtures only: a zip's paths still hold its top folder.
    pub fn inventory(&self) -> Inventory {
        assert!(
            !self.is_zip(),
            "{}: inventory() reads folders only",
            self.name
        );
        let manifest = self.manifest();
        let mut unlisted = self.files();
        let mut contents = BTreeMap::new();
        for file in &manifest.files {
            let bytes = unlisted
                .remove(file.path.as_str())
                .unwrap_or_else(|| panic!("{}: {} is not in the build", self.name, file.path));
            contents.insert(file.path.clone(), bytes);
        }
        let unlisted: Vec<&String> = unlisted.keys().collect();
        assert!(
            unlisted.is_empty(),
            "{}: [inventory] does not list {unlisted:?}",
            self.name
        );
        Inventory::new(manifest, contents)
    }
}

/// A non-negative integer value of a table, such as a byte count.
pub fn integer(table: &Table, key: &str) -> u64 {
    let value = table
        .get(key)
        .and_then(Value::as_integer)
        .unwrap_or_else(|| panic!("{key} is an integer"));
    u64::try_from(value).unwrap_or_else(|_| panic!("{key} is not negative"))
}

/// An expected count or byte total as a measure value.
pub fn expected_value(expected: u64) -> MeasureValue {
    MeasureValue::Integer(i64::try_from(expected).expect("an expected value fits in i64"))
}

/// A measure value read back as a count or byte total.
pub fn non_negative(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_else(|_| panic!("{value} is not negative"))
}

/// A string value of a table, such as a path.
pub fn text<'a>(table: &'a Table, key: &str) -> &'a str {
    table
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} is a string"))
}

/// A table inside a table, such as `[audio.total]`.
pub fn sub_table<'a>(table: &'a Table, key: &str) -> &'a Table {
    table
        .get(key)
        .and_then(Value::as_table)
        .unwrap_or_else(|| panic!("{key} is a table"))
}

/// The rows of an array of tables, such as `[[inventory.file]]`. Empty when it has none.
pub fn rows<'a>(table: &'a Table, key: &str) -> Vec<&'a Table> {
    let Some(rows) = table.get(key) else {
        return Vec::new();
    };
    rows.as_array()
        .unwrap_or_else(|| panic!("{key} is an array"))
        .iter()
        .map(|row| {
            row.as_table()
                .unwrap_or_else(|| panic!("a {key} row is a table"))
        })
        .collect()
}

/// A list of paths, such as `not_covered`.
pub fn paths(table: &Table, key: &str) -> Vec<RelPath> {
    table
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{key} is an array"))
        .iter()
        .map(|value| path(value.as_str().expect("a path is a string")))
        .collect()
}

/// A build path from `expected.toml`.
pub fn path(raw: &str) -> RelPath {
    RelPath::new(raw).unwrap_or_else(|error| panic!("{raw}: {error}"))
}

/// The manifest's file kind for an `expected.toml` bucket, such as "image". It goes through serde
/// so that the bucket names are `FileKind`'s own JSON names.
pub fn file_kind(bucket: &str) -> FileKind {
    Value::String(bucket.to_owned())
        .try_into()
        .unwrap_or_else(|error| panic!("bucket {bucket:?} is not a file kind: {error}"))
}

/// The bucket name of a file kind, such as "image".
pub fn bucket(kind: FileKind) -> String {
    match Value::try_from(kind) {
        Ok(Value::String(bucket)) => bucket,
        other => panic!("{kind:?} is not a bucket name: {other:?}"),
    }
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .expect("a fixture name is UTF-8")
        .to_owned()
}

fn zip_files(build: &Path) -> BTreeMap<String, Vec<u8>> {
    let file = File::open(build).expect("the zip opens");
    let mut archive = zip::ZipArchive::new(file).expect("the zip is readable");
    let mut files = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("a zip entry is readable");
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().expect("a zip entry name is UTF-8");
        let path = RelPath::new(&name).expect("a zip entry path is relative");
        if is_skipped(&path) {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .expect("a zip entry decompresses");
        files.insert(path.as_str().to_owned(), bytes);
    }
    files
}

fn folder_files(build: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in WalkDir::new(build).follow_links(false) {
        let entry = entry.expect("a build entry is readable");
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(build).expect("inside the build");
        let path = RelPath::from_path(relative).expect("a build path is relative and UTF-8");
        if is_skipped(&path) {
            continue;
        }
        let bytes = fs::read(entry.path()).expect("a build file is readable");
        files.insert(path.as_str().to_owned(), bytes);
    }
    files
}

fn is_skipped(path: &RelPath) -> bool {
    path.as_str()
        .split('/')
        .any(|part| SKIPPED_NAMES.contains(&part))
}
