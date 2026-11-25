//! Inventory: every supplied file with its size, SHA-256 and kind, and the build id.
//!
//! A folder and a .zip of it give the same inventory, so the other families never know which one
//! they read. The build id covers every file's path and SHA-256, so a rename changes it (design
//! section 3, decision R3). It does not cover external URLs, so `bundle` lists them.
//!
//! The inventory is the build: what it leaves out is not part of the build, and the build id does
//! not cover it. A server that gives a build to a browser must give the same files and no others,
//! so the rule is here for it too: [`is_skipped_name`] for the names, and [`listed_file`] for one
//! path of a build folder.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use dyno_schema::canonical::sha256_hex;
use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::{BuildManifest, ManifestFile};
use dyno_schema::measure::{Evidence, Measure, MeasureStatus, MeasureValue, Method, Uncertainty};
use dyno_schema::path::RelPath;
use walkdir::WalkDir;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::ScanError;
use crate::model::{FamilyScan, Inventory, saturating_i64};
use crate::sniff;

/// The measures this family owns.
pub const MEASURES: &[MeasureId] = &[MeasureId::StaticInventoryFiles];

/// File and folder names the scan skips anywhere in a build: operating system litter and version
/// control.
const SKIPPED_NAMES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini", "__MACOSX", ".git"];

/// What a .zip may hold once unpacked, so a small zip cannot fill memory or run for hours (a zip
/// bomb). The inventory keeps every file in memory, and real builds are tens of MB.
const ZIP_LIMITS: ZipLimits = ZipLimits {
    bytes: 1 << 30,
    entries: 100_000,
};

/// The most a .zip may unpack to, and the most entries it may have.
#[derive(Clone, Copy, Debug)]
struct ZipLimits {
    bytes: u64,
    entries: usize,
}

/// What the file count leaves out, for the measure's scope.
const NOT_LISTED: &str =
    "symbolic links, and .DS_Store, Thumbs.db, desktop.ini, __MACOSX and .git anywhere";

/// Lists every file in a build folder or .zip, with paths relative to the build root, and keeps
/// their contents for the other families.
///
/// Symbolic links are skipped, never followed. A zip entry name that is absolute or has a `..`
/// part is an error, even when it would land inside the build.
pub fn scan(build: &Path) -> Result<FamilyScan<Inventory>, ScanError> {
    let files = if build.is_dir() {
        read_folder(build)?
    } else if is_zip_file(build) {
        read_zip(build)?
    } else {
        return Err(ScanError::NotABuild(build.to_path_buf()));
    };
    let inventory = inventory_of(files);
    Ok(FamilyScan {
        measures: vec![file_count(&inventory)],
        summary: inventory,
    })
}

/// The inventory of these files: each one's size, SHA-256 and kind, and the build id.
pub fn inventory_of(files: BTreeMap<RelPath, Vec<u8>>) -> Inventory {
    let manifest_files: Vec<ManifestFile> = files
        .iter()
        .map(|(path, contents)| ManifestFile {
            path: path.clone(),
            bytes: contents.len() as u64,
            sha256: sha256_hex(contents),
            kind: sniff::file_kind(path, contents),
        })
        .collect();
    let manifest = BuildManifest {
        build_id: Some(build_id(&manifest_files)),
        files: manifest_files,
    };
    Inventory::new(manifest, files)
}

/// The build id of files sorted by path: the SHA-256 of one line per file, each the path, a NUL
/// byte, the file's SHA-256 in lowercase hex and a newline.
pub fn build_id(files: &[ManifestFile]) -> String {
    let mut lines = Vec::new();
    for file in files {
        lines.extend_from_slice(file.path.as_str().as_bytes());
        lines.push(b'\0');
        lines.extend_from_slice(file.sha256.as_bytes());
        lines.push(b'\n');
    }
    sha256_hex(&lines)
}

fn is_zip_file(path: &Path) -> bool {
    let has_zip_extension = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    path.is_file() && has_zip_extension
}

fn read_folder(build: &Path) -> Result<BTreeMap<RelPath, Vec<u8>>, ScanError> {
    let unreadable = |path: &Path, source| ScanError::Unreadable {
        path: path.to_path_buf(),
        source,
    };
    let walk = WalkDir::new(build)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_skipped_name(entry.file_name()));
    let mut files = BTreeMap::new();
    for entry in walk {
        let entry = entry.map_err(|error| {
            let path = error.path().unwrap_or(build).to_path_buf();
            unreadable(&path, error.into())
        })?;
        // With links not followed, a symbolic link is neither a file nor a folder.
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(build)
            .expect("the walk stays inside the build");
        let path = RelPath::from_path(relative)?;
        let contents = fs::read(entry.path()).map_err(|error| unreadable(entry.path(), error))?;
        files.insert(path, contents);
    }
    Ok(files)
}

fn read_zip(build: &Path) -> Result<BTreeMap<RelPath, Vec<u8>>, ScanError> {
    read_zip_within(build, ZIP_LIMITS)
}

/// Reads a .zip, refusing one that has more entries than `limits` allows, or that states more
/// unpacked bytes. The central directory's sizes are checked before any entry is read. The zip
/// crate refuses an entry that unpacks past the size it states, so the sizes hold.
fn read_zip_within(
    build: &Path,
    limits: ZipLimits,
) -> Result<BTreeMap<RelPath, Vec<u8>>, ScanError> {
    let bad_zip = |source: ZipError| ScanError::BadZip {
        path: build.to_path_buf(),
        source,
    };
    let too_large = |reason: String| ScanError::ZipTooLarge {
        path: build.to_path_buf(),
        reason,
    };
    let file = File::open(build).map_err(|source| ScanError::Unreadable {
        path: build.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(bad_zip)?;
    // The archive keeps one entry for each name in the central directory, so the loop below
    // never sees a second one. Programs that unpack differ in which of the two they keep, and
    // then the build id would be for files that nobody ran. The archive says only that a name
    // is there twice, not which, and a folder named twice counts too.
    if archive.has_duplicate_names() {
        return Err(ScanError::SameNameTwiceInZip(build.to_path_buf()));
    }
    if archive.len() > limits.entries {
        return Err(too_large(format!(
            "it has {} entries, more than {}",
            archive.len(),
            limits.entries
        )));
    }
    let mut stated_bytes: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(bad_zip)?;
        stated_bytes = stated_bytes.saturating_add(entry.size());
    }
    if stated_bytes > limits.bytes {
        return Err(too_large(format!(
            "it states {stated_bytes} bytes unpacked, more than {}",
            limits.bytes
        )));
    }
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(bad_zip)?;
        // A symbolic link in a zip is not a file, so it is skipped like a folder.
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().map_err(bad_zip)?;
        // The build id puts a NUL after each path, so a NUL inside one could forge another file.
        if name.contains('\0') {
            return Err(ScanError::NulInZipName(name.into_owned()));
        }
        let path = RelPath::new(&name)?;
        if path.as_str().split('/').any(is_skipped_name) {
            continue;
        }
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .map_err(|error| bad_zip(error.into()))?;
        entries.push((path, contents));
    }
    let mut files = BTreeMap::new();
    for (path, contents) in without_top_folder(entries)? {
        if files.contains_key(&path) {
            return Err(ScanError::DuplicateZipEntry(path));
        }
        files.insert(path, contents);
    }
    Ok(files)
}

/// Whether a file or folder of this name is left out of a build, anywhere in it: operating
/// system litter and version control. A folder of such a name is left out with all it holds.
pub fn is_skipped_name(name: impl AsRef<OsStr>) -> bool {
    let name = name.as_ref();
    SKIPPED_NAMES.iter().any(|skipped| name == *skipped)
}

/// The file that `path` names in the build folder `build`, when the inventory of that folder
/// lists it. `build` must have its own links resolved, as `Path::canonicalize` gives it.
///
/// None for a path the inventory leaves out: one with a skipped name in it, one that reaches
/// its file through a symbolic link, and one that names no file. The walk does not follow links,
/// so a listed file is reached by its own path, and resolving that path's links changes nothing.
pub fn listed_file(build: &Path, path: &RelPath) -> Option<PathBuf> {
    if path.as_str().split('/').any(is_skipped_name) {
        return None;
    }
    let file: PathBuf = std::iter::once(build.as_os_str())
        .chain(path.as_str().split('/').map(OsStr::new))
        .collect();
    let resolved = file.canonicalize().ok()?;
    (resolved == file && resolved.is_file()).then_some(file)
}

/// Strips the one folder every entry sits under, as zipping a build folder makes it. With an
/// entry at the root, such as `index.html`, the paths stay as they are.
fn without_top_folder(
    entries: Vec<(RelPath, Vec<u8>)>,
) -> Result<Vec<(RelPath, Vec<u8>)>, ScanError> {
    let Some(top_folder) = shared_top_folder(&entries) else {
        return Ok(entries);
    };
    let inside_top_folder = top_folder.len() + 1;
    entries
        .into_iter()
        .map(|(path, contents)| Ok((RelPath::new(&path.as_str()[inside_top_folder..])?, contents)))
        .collect()
}

/// The first part of every entry's path, when they all share one and none sits at the root.
fn shared_top_folder(entries: &[(RelPath, Vec<u8>)]) -> Option<String> {
    let mut top_folders = entries
        .iter()
        .map(|(path, _)| path.as_str().split_once('/').map(|(top, _)| top));
    let Some(Some(first)) = top_folders.next() else {
        return None;
    };
    top_folders
        .all(|top| top == Some(first))
        .then(|| first.to_owned())
}

fn file_count(inventory: &Inventory) -> Measure {
    let mut measure = Measure {
        value: Some(MeasureValue::Integer(saturating_i64(
            inventory.files().len(),
        ))),
        evidence: vec![Evidence {
            method: Method::FileHash,
            detail: "read and hashed every file".to_owned(),
        }],
        uncertainty: Uncertainty::exact(),
        ..Measure::blank(MeasureId::StaticInventoryFiles, MeasureStatus::Estimated)
    };
    measure.scope.not_covered = vec![NOT_LISTED.to_owned()];
    measure
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    #[test]
    fn the_words_for_what_is_not_listed_name_every_skipped_name() {
        for name in SKIPPED_NAMES {
            assert!(NOT_LISTED.contains(name), "{name}");
            assert!(is_skipped_name(name), "{name}");
        }
        assert!(NOT_LISTED.contains("symbolic links"));
    }

    #[test]
    fn a_name_is_skipped_only_when_it_is_the_whole_name() {
        for name in [
            ".gitignore",
            "git",
            "my.DS_Store",
            "__MACOSX__",
            "index.html",
        ] {
            assert!(!is_skipped_name(name), "{name}");
        }
    }

    fn zip_with(files: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let zip = tempfile::Builder::new().suffix(".zip").tempfile().unwrap();
        let mut writer = ZipWriter::new(zip.reopen().unwrap());
        for (name, contents) in files {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
        zip
    }

    fn limits(bytes: u64, entries: usize) -> ZipLimits {
        ZipLimits { bytes, entries }
    }

    #[test]
    fn a_zip_within_the_limits_is_read() {
        let zip = zip_with(&[("a.js", b"1234"), ("b.js", b"5678")]);
        let files = read_zip_within(zip.path(), limits(8, 2)).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn a_zip_with_too_many_entries_is_refused_before_it_is_read() {
        let zip = zip_with(&[("a.js", b"1"), ("b.js", b"2"), ("c.js", b"3")]);
        let result = read_zip_within(zip.path(), limits(100, 2));
        let Err(ScanError::ZipTooLarge { reason, .. }) = result else {
            panic!("{result:?}");
        };
        assert_eq!(reason, "it has 3 entries, more than 2");
    }

    #[test]
    fn a_zip_that_states_too_many_bytes_is_refused_before_it_is_read() {
        let zip = zip_with(&[("a.js", b"1234"), ("b.js", b"56789")]);
        let result = read_zip_within(zip.path(), limits(8, 10));
        let Err(ScanError::ZipTooLarge { reason, .. }) = result else {
            panic!("{result:?}");
        };
        assert_eq!(reason, "it states 9 bytes unpacked, more than 8");
    }

    /// Rewrites the unpacked size of the one entry in `zip`, in its local header and its central
    /// directory header, as a crafted zip can.
    fn state_unpacked_size(zip: &Path, size: u32) {
        let mut bytes = fs::read(zip).unwrap();
        let local = find(&bytes, &[0x50, 0x4b, 0x03, 0x04]);
        bytes[local + 22..local + 26].copy_from_slice(&size.to_le_bytes());
        let central = find(&bytes, &[0x50, 0x4b, 0x01, 0x02]);
        bytes[central + 24..central + 28].copy_from_slice(&size.to_le_bytes());
        fs::write(zip, bytes).unwrap();
    }

    fn find(bytes: &[u8], signature: &[u8]) -> usize {
        bytes
            .windows(signature.len())
            .position(|window| window == signature)
            .unwrap()
    }

    #[test]
    fn an_entry_that_unpacks_past_what_it_states_is_refused() {
        let zeros = vec![0_u8; 1 << 20];
        let zip = zip_with(&[("zeros.bin", &zeros)]);
        state_unpacked_size(zip.path(), 10);
        let result = read_zip_within(zip.path(), limits(1_000, 10));
        assert!(
            matches!(result, Err(ScanError::BadZip { .. })),
            "{result:?}"
        );
    }

    /// Where an entry's name is written in a zip.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum NamedIn {
        /// The entry's local header only, before its bytes.
        LocalHeader,
        /// The local header and the central directory.
        BothPlaces,
    }

    /// Renames the entry called `from` to `to`, a name of the same length, as a crafted zip can.
    fn rename_entry(zip: &Path, from: &[u8], to: &[u8], place: NamedIn) {
        assert_eq!(from.len(), to.len());
        let mut bytes = fs::read(zip).unwrap();
        let found: Vec<usize> = (0..bytes.len() - from.len())
            .filter(|&at| &bytes[at..at + from.len()] == from)
            .collect();
        // Once in the entry's local header, and once in the central directory after it.
        assert_eq!(found.len(), 2);
        let renamed = match place {
            NamedIn::LocalHeader => &found[..1],
            NamedIn::BothPlaces => &found[..],
        };
        for &at in renamed {
            bytes[at..at + to.len()].copy_from_slice(to);
        }
        fs::write(zip, bytes).unwrap();
    }

    #[test]
    fn a_zip_with_two_entries_of_one_name_is_refused() {
        // One unpacker gives the first game.js and another the second, so the zip is two builds.
        let zip = zip_with(&[
            ("index.html", b"<script src=game.js></script>"),
            ("game.js", b"console.log('one')"),
            ("gamf.js", b"console.log('two')"),
        ]);
        assert_eq!(scan(zip.path()).unwrap().summary.files().len(), 3);
        rename_entry(zip.path(), b"gamf.js", b"game.js", NamedIn::BothPlaces);
        let result = scan(zip.path());
        let Err(ScanError::SameNameTwiceInZip(path)) = &result else {
            panic!("{:?}", result.map(|scan| scan.measures));
        };
        assert_eq!(path, zip.path());
        let said = result.unwrap_err().to_string();
        assert!(said.contains("names a file or a folder twice"), "{said}");
    }

    #[test]
    fn a_zip_that_names_one_folder_twice_is_refused_too() {
        // The zip reader says only that the central directory has a name twice, not whether it
        // is a file or a folder. A folder named twice is rare, so it is refused with the rest.
        let zip = tempfile::Builder::new().suffix(".zip").tempfile().unwrap();
        let mut writer = ZipWriter::new(zip.reopen().unwrap());
        let options = SimpleFileOptions::default();
        writer.add_directory("assets-a/", options).unwrap();
        writer.add_directory("assets-b/", options).unwrap();
        writer.start_file("assets-a/game.js", options).unwrap();
        writer.write_all(b"console.log('one')").unwrap();
        writer.finish().unwrap();
        assert!(scan(zip.path()).is_ok());
        rename_entry(zip.path(), b"assets-b/", b"assets-a/", NamedIn::BothPlaces);

        let said = scan(zip.path()).unwrap_err().to_string();
        assert!(
            said.contains("names a file or a folder twice in its central directory"),
            "{said}"
        );
    }

    #[test]
    fn a_zip_holds_what_its_central_directory_names_whatever_a_local_header_says() {
        // dyno follows the central directory and does not compare it with the local headers.
        let zip = zip_with(&[
            ("index.html", b"<script src=game.js></script>"),
            ("game.js", b"console.log('one')"),
        ]);
        rename_entry(zip.path(), b"game.js", b"evil.js", NamedIn::LocalHeader);
        let inventory = scan(zip.path()).unwrap().summary;
        let paths: Vec<&str> = inventory
            .files()
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, ["game.js", "index.html"]);
    }

    #[test]
    fn a_highly_compressed_zip_is_refused_by_what_it_states() {
        let zeros = vec![0_u8; 1 << 20];
        let zip = zip_with(&[("zeros.bin", &zeros)]);
        assert!(fs::metadata(zip.path()).unwrap().len() < 10_000);
        let result = read_zip_within(zip.path(), limits(1 << 19, 10));
        assert!(matches!(result, Err(ScanError::ZipTooLarge { .. })));
    }
}
