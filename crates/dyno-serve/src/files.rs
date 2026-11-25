//! The build's files, as the game server serves them: a URL path to a file of the build, its
//! content type, and each HTML page with the probe's script tag added, or as it is.
//!
//! The server gives the bytes that the static scan read, from memory, and nothing else. So a
//! page under test loads exactly what the build id covers: no file that the scan skips, such as
//! operating system litter, a version control folder or anything behind a symbolic link, no file
//! that came to the folder after the scan, and no bytes that changed after it. There is one rule
//! for what a build's files are, the scan's walk, and one list, the scan's inventory
//! (`dyno_static::model::BuildFiles`). A build that is opened with no scan is scanned here.
//!
//! The server reads nothing from the folder while it serves. It asks the folder one thing, once,
//! when the build is opened: whether its volume gives a file under a name in another case, as
//! the default volume of a Mac does. On such a volume a game can ask for `index.html` and get
//! the build's `Index.html`, so the server gives that file for a name that differs only in case.

use std::path::{Path, PathBuf};

use dyno_schema::path::RelPath;
use dyno_static::model::BuildFiles;
use percent_encoding::percent_decode_str;

use crate::ServeError;
use crate::http::{RangeAsk, Request, Response, range_ask};

/// The probe's path on the game server of a served run. It is the one path there that is dyno's
/// and not the build's.
pub const PROBE_PATH: &str = "/__dyno/probe.js";

/// The tag added to each HTML page, before any of the page's own scripts. It loads the probe from
/// the game's own origin, so a policy that allows only `'self'` scripts still runs it, and asks it
/// to relay its events to the harness page.
const PROBE_TAG: &[u8] = br#"<script src="/__dyno/probe.js" data-dyno-relay></script>"#;

/// The page a folder serves.
const INDEX: &str = "index.html";

/// What the game server does to each HTML page of the build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pages {
    /// Adds the probe's script tag, for a browser with no driver to add the probe.
    WithProbe,
    /// Sends each page as it is, for a browser whose driver adds the probe itself, so the page
    /// loads only the game's own files.
    AsTheyAre,
}

/// A build to serve: the files of one scan of a build folder, with their bytes.
#[derive(Clone, Debug)]
pub struct Build {
    files: BuildFiles,
    /// Whether the folder's volume gives a file under a name in another case.
    folds_case: bool,
}

/// One file of a build: its path as the scan listed it, and the bytes the scan read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildFile<'b> {
    pub path: &'b RelPath,
    pub bytes: &'b [u8],
}

/// An error unless `path` is a folder: a served run serves a build folder, not a .zip.
pub fn check_folder(path: &Path) -> Result<(), ServeError> {
    if path.is_dir() {
        return Ok(());
    }
    Err(ServeError::Build(format!(
        "{} is not a folder: the served run serves a build folder, so unzip a .zip first",
        path.display()
    )))
}

/// An error when the build has a file at the probe's path, which a served run's game server
/// gives the probe at: the page would get the probe where it asked for the build's file.
pub fn check_probe_path(build: &Build) -> Result<(), ServeError> {
    match build.file(PROBE_PATH) {
        Some(file) => Err(ServeError::Build(format!(
            "the build has a file at {}, which is where a served run gives the page dyno's own \
             probe: give the file another name",
            file.path.as_str()
        ))),
        None => Ok(()),
    }
}

impl Build {
    /// The build in this folder, scanned now: the files that the static scan's inventory lists,
    /// with the bytes it read. A .zip or a missing folder is an error, and so is a folder that
    /// the scan cannot read.
    pub fn open(folder: &Path) -> Result<Build, ServeError> {
        check_folder(folder)?;
        let scan = dyno_static::inventory::scan(folder)
            .map_err(|error| ServeError::Build(error.to_string()))?;
        let (_, files) = scan.summary.into_manifest_and_files();
        Ok(Build::of_scan(folder, files))
    }

    /// The build in this folder as a scan of it read it: `files` are the bytes of that scan, so
    /// the server gives what the scan's build id covers.
    pub fn of_scan(folder: &Path, files: BuildFiles) -> Build {
        Build {
            folds_case: folds_case(folder, &files),
            files,
        }
    }

    /// The file of the build that a URL path names. A folder gives its `index.html`.
    ///
    /// None for anything that the scan did not list, and for a path that leaves the build, such
    /// as through `..`. On a volume that gives a file under a name in another case, a path that
    /// differs from one listed file's only in case names that file.
    pub fn file(&self, url_path: &str) -> Option<BuildFile<'_>> {
        let mut asked = PathBuf::new();
        for segment in url_path.split('/') {
            let segment = percent_decode_str(segment).decode_utf8().ok()?;
            match segment.as_ref() {
                "" | "." => {}
                ".." => return None,
                name if name.contains(['/', '\\', '\0']) => return None,
                name => asked.push(name),
            }
        }
        // The paths as the inventory writes them, by the same rule. The root is no file.
        let file = RelPath::from_path(&asked).ok();
        let index = RelPath::from_path(&asked.join(INDEX)).ok()?;
        let listed = |path: &RelPath| self.files.entry(path);
        let found = file
            .as_ref()
            .and_then(listed)
            .or_else(|| listed(&index))
            .or_else(|| file.as_ref().and_then(|path| self.in_another_case(path)))
            .or_else(|| self.in_another_case(&index));
        found.map(|(path, bytes)| BuildFile { path, bytes })
    }

    /// The one listed file whose path differs from `path` only in the case of its letters, on a
    /// volume that would give that file for `path`. None on any other volume.
    fn in_another_case(&self, path: &RelPath) -> Option<(&RelPath, &[u8])> {
        if !self.folds_case {
            return None;
        }
        let asked = path.as_str().to_lowercase();
        let mut same = self
            .files
            .paths()
            .filter(|listed| listed.as_str().to_lowercase() == asked);
        // Such a volume cannot hold two names that differ only in case, so there is one at most.
        let only = same.next()?;
        same.next().is_none().then(|| self.files.entry(only))?
    }
}

/// Whether the volume that holds `folder` gives a file under a name in another case. It asks for
/// one listed file by its path with every ASCII letter in the other case: the volume folds case
/// when that is the same file. A build with no letter in any path gives no answer, so false.
fn folds_case(folder: &Path, files: &BuildFiles) -> bool {
    let probe = files.paths().find_map(|path| {
        let swapped: String = path
            .as_str()
            .chars()
            .map(|c| match c {
                c if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
                c => c.to_ascii_lowercase(),
            })
            .collect();
        // A path with no letter cannot ask, and neither can one whose other spelling is a file
        // of the build too, which only a volume that keeps case apart can hold.
        let other = RelPath::new(&swapped).ok()?;
        (other != *path && files.get(&other).is_none())
            .then(|| (folder.join(path.as_str()), folder.join(swapped)))
    });
    probe.is_some_and(|(listed, swapped)| is_same_file(&listed, &swapped))
}

/// Whether two paths name one file, and not one a link to the other. Only Unix says.
#[cfg(unix)]
fn is_same_file(one: &Path, other: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (one.symlink_metadata(), other.symlink_metadata()) {
        (Ok(one), Ok(other)) => one.dev() == other.dev() && one.ino() == other.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn is_same_file(_one: &Path, _other: &Path) -> bool {
    false
}

/// The response to a request for a build file: the file, part of it, or an error. Only GET and
/// HEAD read a file.
pub(crate) fn build_response(build: &Build, request: &Request, pages: Pages) -> Response {
    if request.method != "GET" && request.method != "HEAD" {
        return Response::error("405 Method Not Allowed");
    }
    match build.file(&request.path) {
        Some(file) => file_response(
            Path::new(file.path.as_str()),
            file.bytes,
            request.range.as_deref(),
            pages,
        ),
        None => Response::error("404 Not Found"),
    }
}

/// The response for a file: the whole file, or the part a `Range` header asks for. With the
/// probe, an HTML page gets the probe's tag and is always sent whole. The response has its own
/// copy of the bytes it sends, and no more.
pub fn file_response(path: &Path, contents: &[u8], range: Option<&str>, pages: Pages) -> Response {
    let content_type = content_type(path);
    if pages == Pages::WithProbe && is_html(path) {
        return Response::new("200 OK", content_type, with_probe(contents));
    }
    let length = contents.len() as u64;
    let mut response = match range_ask(range, length) {
        RangeAsk::Whole => Response::new("200 OK", content_type, contents.to_vec()),
        RangeAsk::Part(start, end) => {
            // Both ends are inside the file, so they fit in usize.
            let part = contents[start as usize..=end as usize].to_vec();
            let mut response = Response::new("206 Partial Content", content_type, part);
            response
                .headers
                .push(("Content-Range", format!("bytes {start}-{end}/{length}")));
            response
        }
        RangeAsk::Unsatisfiable => {
            let mut response = Response::error("416 Range Not Satisfiable");
            response
                .headers
                .push(("Content-Range", format!("bytes */{length}")));
            response
        }
    };
    response.headers.push(("Accept-Ranges", "bytes".to_owned()));
    response
}

/// The page with the probe's tag first in its `<head>`, or after `<html>` or the doctype when it
/// has no head. A page with none of them gets the tag first.
pub fn with_probe(html: &[u8]) -> Vec<u8> {
    let lower = html.to_ascii_lowercase();
    let at = end_of_tag(&lower, b"<head")
        .or_else(|| end_of_tag(&lower, b"<html"))
        .or_else(|| end_of_tag(&lower, b"<!doctype"))
        .unwrap_or(0);
    let mut page = Vec::with_capacity(html.len() + PROBE_TAG.len());
    page.extend_from_slice(&html[..at]);
    page.extend_from_slice(PROBE_TAG);
    page.extend_from_slice(&html[at..]);
    page
}

/// Where the first tag with this exact name ends: `<head>` and `<head lang="en">`, not
/// `<header>`.
fn end_of_tag(lower: &[u8], open: &[u8]) -> Option<usize> {
    memchr::memmem::find_iter(lower, open).find_map(|start| {
        let after = start + open.len();
        let name_ends = lower
            .get(after)
            .is_some_and(|&byte| byte == b'>' || byte.is_ascii_whitespace());
        if !name_ends {
            return None;
        }
        memchr::memchr(b'>', &lower[after..]).map(|close| after + close + 1)
    })
}

fn is_html(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("html" | "htm"))
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

/// The content type of a build file, by its extension. A browser needs the right one for
/// scripts, styles, WebAssembly and media.
pub fn content_type(path: &Path) -> &'static str {
    match extension(path).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("txt" | "atlas" | "fnt") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("ktx2") => "image/ktx2",
        Some("mp3") => "audio/mpeg",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("m4a" | "aac") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("weba") => "audio/webm",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("wasm") => "application/wasm",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

    /// The listed path of the file that a URL path names in the build.
    fn named<'b>(build: &'b Build, url_path: &str) -> Option<&'b str> {
        build.file(url_path).map(|file| file.path.as_str())
    }

    fn write_file(folder: &Path, path: &str, contents: &str) {
        let path = folder.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// A build of these files, with no folder behind it, on a volume that folds case or not.
    fn build_of(files: &[(&str, &str)], folds_case: bool) -> Build {
        let files: std::collections::BTreeMap<RelPath, Vec<u8>> = files
            .iter()
            .map(|(path, contents)| (RelPath::new(path).unwrap(), contents.as_bytes().to_vec()))
            .collect();
        Build {
            files: files.into(),
            folds_case,
        }
    }

    #[test]
    fn the_probe_goes_first_in_the_head() {
        let page = with_probe(b"<!doctype html><html><HEAD lang=en><script src=a.js></script>");
        assert_eq!(
            text(&page),
            r#"<!doctype html><html><HEAD lang=en><script src="/__dyno/probe.js" data-dyno-relay></script><script src=a.js></script>"#
        );
    }

    #[test]
    fn a_header_tag_is_not_the_head() {
        let page = with_probe(b"<html><header></header><head></head>");
        assert!(text(&page).starts_with(r#"<html><header></header><head><script src="/__dyno"#));
    }

    #[test]
    fn a_page_with_no_head_gets_the_probe_after_html_or_the_doctype() {
        assert!(
            text(&with_probe(b"<!DOCTYPE html><html><script>"))
                .starts_with(r#"<!DOCTYPE html><html><script src="/__dyno"#)
        );
        assert!(
            text(&with_probe(b"<!DOCTYPE html><script>"))
                .starts_with(r#"<!DOCTYPE html><script src="/__dyno"#)
        );
        assert!(text(&with_probe(b"<script>")).starts_with(r#"<script src="/__dyno"#));
    }

    #[test]
    fn a_path_names_a_file_of_the_build_and_a_folder_names_its_index() {
        let folder = tempfile::tempdir().unwrap();
        write_file(folder.path(), "index.html", "page");
        write_file(folder.path(), "assets/a b.png", "png");
        write_file(folder.path(), "help/index.html", "help");
        let build = Build::open(folder.path()).unwrap();
        assert_eq!(named(&build, "/"), Some("index.html"));
        assert_eq!(named(&build, "/assets/a%20b.png"), Some("assets/a b.png"));
        assert_eq!(named(&build, "/help"), Some("help/index.html"));
        assert_eq!(named(&build, "/help/"), Some("help/index.html"));
        assert_eq!(named(&build, "/assets/missing.png"), None);
        // A folder with no page of its own names nothing.
        assert_eq!(named(&build, "/assets/"), None);
        assert_eq!(build.file("/assets/a%20b.png").unwrap().bytes, b"png");
    }

    #[test]
    fn a_path_that_leaves_the_build_names_nothing() {
        let outer = tempfile::tempdir().unwrap();
        write_file(outer.path(), "secret.txt", "secret");
        write_file(outer.path(), "build/index.html", "page");
        let build = Build::open(&outer.path().join("build")).unwrap();
        for path in ["/../secret.txt", "/%2e%2e/secret.txt", "/..%2fsecret.txt"] {
            assert_eq!(named(&build, path), None, "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_names_nothing_whether_it_leaves_the_build_or_not() {
        let outer = tempfile::tempdir().unwrap();
        write_file(outer.path(), "secret.txt", "secret");
        let root = outer.path().join("build");
        write_file(&root, "js/main.js", "1");
        std::os::unix::fs::symlink(outer.path().join("secret.txt"), root.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("js/main.js"), root.join("same.js")).unwrap();
        std::os::unix::fs::symlink(root.join("js"), root.join("same-js")).unwrap();
        let build = Build::open(&root).unwrap();
        // The scan lists the file once, by its own path, so that is its only URL.
        assert_eq!(named(&build, "/js/main.js"), Some("js/main.js"));
        for path in ["/link.txt", "/same.js", "/same-js/main.js"] {
            assert_eq!(named(&build, path), None, "{path}");
        }
    }

    /// Operating system litter and version control, which the scan's inventory leaves out.
    const SKIPPED: [&str; 5] = [".DS_Store", "Thumbs.db", "desktop.ini", "__MACOSX", ".git"];

    #[test]
    fn a_file_or_folder_that_the_scan_skips_names_nothing() {
        let folder = tempfile::tempdir().unwrap();
        write_file(folder.path(), "assets/a.png", "png");
        for name in SKIPPED {
            write_file(folder.path(), &format!("assets/{name}"), "litter");
            write_file(folder.path(), &format!("{name}/inside.js"), "litter");
            write_file(folder.path(), &format!("{name}/index.html"), "litter");
        }
        let build = Build::open(folder.path()).unwrap();
        for name in SKIPPED {
            for path in [
                format!("/assets/{name}"),
                format!("/{name}/inside.js"),
                // A folder's own page is not served from a skipped folder either.
                format!("/{name}/"),
                format!("/{name}"),
                // Nor is a skipped entry under a name in another case, on any volume.
                format!("/{}/inside.js", name.to_uppercase()),
                format!("/{}/inside.js", name.to_lowercase()),
            ] {
                assert_eq!(named(&build, &path), None, "{path}");
            }
        }
        // A file beside them is still served.
        assert_eq!(named(&build, "/assets/a.png"), Some("assets/a.png"));
    }

    #[test]
    fn the_bytes_served_are_the_scan_s_whatever_the_folder_holds_later() {
        let folder = tempfile::tempdir().unwrap();
        write_file(folder.path(), "index.html", "scanned page");
        write_file(folder.path(), "js/main.js", "scanned script");
        let build = Build::open(folder.path()).unwrap();
        // After the scan: a file is edited, one is swapped for a link, one is added, one goes.
        write_file(folder.path(), "index.html", "edited page");
        write_file(folder.path(), "js/late.js", "late");
        fs::remove_file(folder.path().join("js/main.js")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hostname", folder.path().join("js/main.js")).unwrap();
        assert_eq!(build.file("/").unwrap().bytes, b"scanned page");
        assert_eq!(build.file("/js/main.js").unwrap().bytes, b"scanned script");
        assert_eq!(named(&build, "/js/late.js"), None);
    }

    #[test]
    fn a_build_of_a_scan_serves_the_bytes_of_that_scan() {
        let folder = tempfile::tempdir().unwrap();
        write_file(folder.path(), "index.html", "scanned page");
        let scan = dyno_static::inventory::scan(folder.path()).unwrap();
        let (_, files) = scan.summary.into_manifest_and_files();
        write_file(folder.path(), "index.html", "edited page");
        let build = Build::of_scan(folder.path(), files);
        assert_eq!(build.file("/").unwrap().bytes, b"scanned page");
    }

    #[test]
    fn on_a_volume_that_keeps_case_apart_only_the_exact_name_is_the_file() {
        let build = build_of(&[("Index.html", "page"), ("JS/Main.js", "1")], false);
        assert_eq!(named(&build, "/Index.html"), Some("Index.html"));
        for path in ["/", "/index.html", "/js/main.js", "/JS/main.js"] {
            assert_eq!(named(&build, path), None, "{path}");
        }
    }

    #[test]
    fn on_a_volume_that_folds_case_a_name_in_another_case_is_the_listed_file() {
        let build = build_of(
            &[
                ("Index.html", "page"),
                ("JS/Main.js", "1"),
                ("Help/INDEX.HTML", "help"),
            ],
            true,
        );
        // The root finds the build's page, as the volume would give it.
        assert_eq!(named(&build, "/"), Some("Index.html"));
        assert_eq!(named(&build, "/index.html"), Some("Index.html"));
        assert_eq!(named(&build, "/js/main.js"), Some("JS/Main.js"));
        assert_eq!(named(&build, "/help/"), Some("Help/INDEX.HTML"));
        assert_eq!(build.file("/js/MAIN.JS").unwrap().bytes, b"1");
        // What the scan did not list is still not served.
        assert_eq!(named(&build, "/js/other.js"), None);
        assert_eq!(named(&build, "/.git/config"), None);
    }

    #[test]
    fn an_exact_name_wins_over_a_name_in_another_case() {
        let build = build_of(&[("a.js", "lower"), ("A.js", "upper")], true);
        assert_eq!(build.file("/a.js").unwrap().bytes, b"lower");
        assert_eq!(build.file("/A.js").unwrap().bytes, b"upper");
        // Two files differ from this name only in case, so it names neither.
        assert_eq!(named(&build, "/A.JS"), None);
    }

    #[test]
    fn the_build_serves_a_name_in_another_case_exactly_when_its_volume_does() {
        let folder = tempfile::tempdir().unwrap();
        write_file(folder.path(), "Index.html", "page");
        // What this volume does: a Mac's default volume gives the file, Linux's does not.
        let volume_folds_case = folder.path().join("index.html").exists();
        let build = Build::open(folder.path()).unwrap();
        assert_eq!(build.folds_case, volume_folds_case);
        assert_eq!(named(&build, "/Index.html"), Some("Index.html"));
        assert_eq!(
            named(&build, "/"),
            volume_folds_case.then_some("Index.html")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_under_the_other_spelling_does_not_make_a_volume_fold_case() {
        let folder = tempfile::tempdir().unwrap();
        write_file(folder.path(), "a.js", "1");
        if folder.path().join("A.JS").exists() {
            // This volume folds case, so it cannot hold the link.
            return;
        }
        std::os::unix::fs::symlink(folder.path().join("a.js"), folder.path().join("A.JS")).unwrap();
        let build = Build::open(folder.path()).unwrap();
        assert!(!build.folds_case);
        assert_eq!(named(&build, "/A.JS"), None);
    }

    #[test]
    fn a_build_with_no_letter_in_a_path_cannot_ask_its_volume() {
        let folder = tempfile::tempdir().unwrap();
        write_file(folder.path(), "1/2", "data");
        assert!(!Build::open(folder.path()).unwrap().folds_case);
    }

    #[test]
    fn a_zip_is_not_a_build_folder() {
        let folder = tempfile::tempdir().unwrap();
        let zip = folder.path().join("build.zip");
        fs::write(&zip, "PK").unwrap();
        assert!(matches!(Build::open(&zip), Err(ServeError::Build(_))));
        assert!(matches!(check_folder(&zip), Err(ServeError::Build(_))));
        assert!(check_folder(folder.path()).is_ok());
    }

    #[test]
    fn a_build_file_at_the_probe_s_path_is_an_error_and_one_beside_it_is_not() {
        let beside = build_of(
            &[("__dyno/data.json", "{}"), ("__dyno/probe.json", "{}")],
            false,
        );
        assert!(check_probe_path(&beside).is_ok());
        let at_it = build_of(&[("__dyno/probe.js", "the build's")], false);
        let error = check_probe_path(&at_it).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the build has a file at __dyno/probe.js, which is where a served run gives the page \
             dyno's own probe: give the file another name"
        );
        // On a volume that folds case, the page would get the same file under another case.
        let in_another_case = build_of(&[("__DYNO/Probe.js", "the build's")], true);
        assert!(check_probe_path(&in_another_case).is_err());
    }

    #[test]
    fn a_range_request_gets_part_of_a_media_file() {
        let response = file_response(
            Path::new("a.mp3"),
            b"0123456789",
            Some("bytes=2-4"),
            Pages::WithProbe,
        );
        assert_eq!(response.status, "206 Partial Content");
        assert_eq!(response.body, b"234");
        assert!(
            response
                .headers
                .contains(&("Content-Range", "bytes 2-4/10".to_owned()))
        );
    }

    #[test]
    fn a_range_past_the_end_of_a_file_is_refused_with_its_length() {
        let response = file_response(
            Path::new("a.mp3"),
            b"0123",
            Some("bytes=10-"),
            Pages::WithProbe,
        );
        assert_eq!(response.status, "416 Range Not Satisfiable");
        assert!(
            response
                .headers
                .contains(&("Content-Range", "bytes */4".to_owned()))
        );
    }

    #[test]
    fn an_html_page_is_sent_whole_with_the_probe() {
        let response = file_response(
            Path::new("index.html"),
            b"<head>",
            Some("bytes=0-1"),
            Pages::WithProbe,
        );
        assert_eq!(response.status, "200 OK");
        assert!(text(&response.body).contains("data-dyno-relay"));
    }

    #[test]
    fn an_html_page_as_it_is_has_no_probe_and_is_a_file_like_any_other() {
        let whole = file_response(
            Path::new("index.html"),
            b"<head></head>",
            None,
            Pages::AsTheyAre,
        );
        assert_eq!(whole.body, b"<head></head>");
        assert_eq!(whole.content_type, "text/html; charset=utf-8");
        let part = file_response(
            Path::new("index.html"),
            b"<head></head>",
            Some("bytes=0-5"),
            Pages::AsTheyAre,
        );
        assert_eq!(part.status, "206 Partial Content");
        assert_eq!(part.body, b"<head>");
    }

    #[test]
    fn wasm_and_audio_get_their_content_types() {
        assert_eq!(content_type(Path::new("a.WASM")), "application/wasm");
        assert_eq!(content_type(Path::new("music.m4a")), "audio/mp4");
        assert_eq!(
            content_type(Path::new("data.bin")),
            "application/octet-stream"
        );
    }
}
