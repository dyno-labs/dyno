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
//! A request names a file by its exact path, byte for byte, as a web server on a volume that
//! keeps case apart does. The server reads nothing from the folder while it serves, so it gives
//! the same answers on every machine, whatever volume the build sits on. A Mac's default volume
//! gives `Index.html` for `index.html`. The server does not: on most hosts the game would get a
//! 404 for that, so it gets one here.
//!
//! The bytes are held once. A response points at them, and has no copy of the file.

use std::path::{Path, PathBuf};

use dyno_schema::path::RelPath;
use dyno_static::model::BuildFiles;
use percent_encoding::percent_decode_str;

use crate::ServeError;
use crate::http::{Body, RangeAsk, Request, Response, range_ask};

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
/// gives the probe at: the page would get the probe where it asked for the build's file. Only a
/// file at that very path counts. A folder of that name does not, because its files have paths
/// of their own.
pub fn check_probe_path(build: &Build) -> Result<(), ServeError> {
    let probe_path = PROBE_PATH.trim_start_matches('/');
    match RelPath::new(probe_path) {
        Ok(path) if build.files.get(&path).is_some() => Err(ServeError::Build(format!(
            "the build has a file at {probe_path}, which is where a served run gives the page \
             dyno's own probe: give the file another name"
        ))),
        _ => Ok(()),
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
        Ok(Build::of_scan(files))
    }

    /// The build as a scan read it: `files` are the bytes of that scan, so the server gives what
    /// the scan's build id covers.
    pub fn of_scan(files: BuildFiles) -> Build {
        Build { files }
    }

    /// The file of the build that a URL path names: the file whose path in the scan is exactly
    /// that, in the same case. A path that names no file names the `index.html` under it, when
    /// the scan lists one, as a folder's URL does.
    ///
    /// None for anything that the scan did not list, and for a path that leaves the build, such
    /// as through `..`.
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
        let (path, bytes) = file
            .as_ref()
            .and_then(|path| self.files.entry(path))
            .or_else(|| self.files.entry(&index))?;
        Some(BuildFile { path, bytes })
    }
}

/// The response to a request for a build file: the file, part of it, or an error. Only GET and
/// HEAD read a file.
pub(crate) fn build_response<'b>(
    build: &'b Build,
    request: &Request,
    pages: Pages,
) -> Response<'b> {
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
/// probe, an HTML page gets the probe's tag and is always sent whole. The response points at the
/// file's bytes and copies none of them.
pub fn file_response<'b>(
    path: &Path,
    contents: &'b [u8],
    range: Option<&str>,
    pages: Pages,
) -> Response<'b> {
    let content_type = content_type(path);
    if pages == Pages::WithProbe && is_html(path) {
        return Response {
            status: "200 OK",
            content_type,
            headers: Vec::new(),
            body: Body::Tagged {
                page: contents,
                tag: PROBE_TAG,
                at: probe_at(contents),
            },
        };
    }
    let length = contents.len() as u64;
    let mut response = match range_ask(range, length) {
        RangeAsk::Whole => Response::shared("200 OK", content_type, contents),
        RangeAsk::Part(start, end) => {
            // Both ends are inside the file, so they fit in usize.
            let part = &contents[start as usize..=end as usize];
            let mut response = Response::shared("206 Partial Content", content_type, part);
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

/// Where the probe's tag goes in a page: first in its `<head>`, or after `<html>` or the doctype
/// when it has no head. A page with none of them gets the tag first. It reads the page where it
/// is, and copies none of it.
fn probe_at(html: &[u8]) -> usize {
    end_of_tag(html, b"<head")
        .or_else(|| end_of_tag(html, b"<html"))
        .or_else(|| end_of_tag(html, b"<!doctype"))
        .unwrap_or(0)
}

/// Where the first tag with this exact name ends, in any case: `<head>`, `<HEAD>` and
/// `<head lang="en">`, not `<header>`. `open` is the tag's start in lower case.
fn end_of_tag(html: &[u8], open: &[u8]) -> Option<usize> {
    memchr::memchr_iter(b'<', html).find_map(|start| {
        let after = start + open.len();
        let named = html
            .get(start..after)
            .is_some_and(|name| name.eq_ignore_ascii_case(open));
        let name_ends = html
            .get(after)
            .is_some_and(|&byte| byte == b'>' || byte.is_ascii_whitespace());
        if !(named && name_ends) {
            return None;
        }
        memchr::memchr(b'>', &html[after..]).map(|close| after + close + 1)
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

    /// A build of these files, with no folder behind it.
    fn build_of(files: &[(&str, &str)]) -> Build {
        let files: std::collections::BTreeMap<RelPath, Vec<u8>> = files
            .iter()
            .map(|(path, contents)| (RelPath::new(path).unwrap(), contents.as_bytes().to_vec()))
            .collect();
        Build::of_scan(files.into())
    }

    /// A page with the probe's tag where the server puts it.
    fn with_probe(html: &[u8]) -> Vec<u8> {
        file_response(Path::new("index.html"), html, None, Pages::WithProbe)
            .body
            .to_vec()
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
    fn the_tag_goes_after_the_head_in_any_case_and_after_its_attributes() {
        let pages: [(&[u8], usize); 8] = [
            (b"<head>", 6),
            (b"<HEAD>", 6),
            (b"<HeAd lang=en>", 14),
            (b"<html>\n<head\tdata-x>", 20),
            (b"<header></header><Head></Head>", 23),
            (b"<!DocType html><p>", 15),
            (b"<head", 0),
            (b"no tags here", 0),
        ];
        for (page, at) in pages {
            assert_eq!(probe_at(page), at, "{}", text(page));
        }
    }

    #[test]
    fn a_page_s_tagged_length_is_its_own_and_the_tag_s() {
        let page = b"<!doctype html><HTML><HEAD><title>a</title>";
        let response = file_response(Path::new("index.html"), page, None, Pages::WithProbe);
        assert_eq!(response.body.len(), page.len() + PROBE_TAG.len());
        assert_eq!(response.body.to_vec().len(), response.body.len());
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
                // Nor is a skipped entry under a name in another case, on any volume: the
                // server asks the scan, which has no such entry, and never the folder.
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
        let build = Build::of_scan(files);
        assert_eq!(build.file("/").unwrap().bytes, b"scanned page");
    }

    #[test]
    fn only_the_exact_name_is_the_file_in_the_same_case() {
        let build = build_of(&[("Index.html", "page"), ("JS/Main.js", "1")]);
        assert_eq!(named(&build, "/Index.html"), Some("Index.html"));
        assert_eq!(named(&build, "/JS/Main.js"), Some("JS/Main.js"));
        // The root asks for `index.html`, which this build does not have.
        for path in [
            "/",
            "/index.html",
            "/js/main.js",
            "/JS/main.js",
            "/js/Main.js",
        ] {
            assert_eq!(named(&build, path), None, "{path}");
        }
    }

    #[test]
    fn two_files_that_differ_only_in_case_are_two_files() {
        let build = build_of(&[("a.js", "lower"), ("A.js", "upper")]);
        assert_eq!(build.file("/a.js").unwrap().bytes, b"lower");
        assert_eq!(build.file("/A.js").unwrap().bytes, b"upper");
        assert_eq!(named(&build, "/A.JS"), None);
    }

    #[test]
    fn a_build_answers_exact_names_only_whatever_volume_its_folder_is_on() {
        let folder = tempfile::tempdir().unwrap();
        write_file(folder.path(), "Index.html", "page");
        let build = Build::open(folder.path()).unwrap();
        // A Mac's default volume gives the file for `index.html`. The server asks the scan, not
        // the folder, so it gives the same answers here as on any machine.
        assert_eq!(named(&build, "/Index.html"), Some("Index.html"));
        assert_eq!(named(&build, "/index.html"), None);
        assert_eq!(named(&build, "/"), None);
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
        let beside = build_of(&[("__dyno/data.json", "{}"), ("__dyno/probe.json", "{}")]);
        assert!(check_probe_path(&beside).is_ok());
        let at_it = build_of(&[("__dyno/probe.js", "the build's")]);
        let error = check_probe_path(&at_it).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the build has a file at __dyno/probe.js, which is where a served run gives the page \
             dyno's own probe: give the file another name"
        );
        // A name in another case is another file, at another URL.
        let in_another_case = build_of(&[("__DYNO/Probe.js", "the build's")]);
        assert!(check_probe_path(&in_another_case).is_ok());
    }

    #[test]
    fn a_folder_at_the_probe_s_path_is_not_a_file_there() {
        // The folder's page is at `/__dyno/probe.js/`, which is not the probe's URL.
        let folder = build_of(&[("__dyno/probe.js/index.html", "the build's page")]);
        assert!(check_probe_path(&folder).is_ok());
        assert_eq!(
            named(&folder, "/__dyno/probe.js/"),
            Some("__dyno/probe.js/index.html")
        );
    }

    #[test]
    fn a_whole_file_and_a_part_of_one_point_at_the_held_bytes_with_no_copy() {
        let build = build_of(&[("music.mp3", "0123456789"), ("index.html", "<head></head>")]);
        let held = build.file("/music.mp3").unwrap().bytes;
        let whole = file_response(Path::new("music.mp3"), held, None, Pages::WithProbe);
        let Body::Shared(body) = whole.body else {
            panic!("{whole:?}");
        };
        assert!(std::ptr::eq(body, held));
        let range = Some("bytes=2-4");
        let part = file_response(Path::new("music.mp3"), held, range, Pages::AsTheyAre);
        let Body::Shared(body) = part.body else {
            panic!("{part:?}");
        };
        assert!(std::ptr::eq(body, &held[2..=4]));
        // A page with the probe points at the page too: the tag is the only other part.
        let page = build.file("/").unwrap().bytes;
        let tagged = file_response(Path::new("index.html"), page, None, Pages::WithProbe);
        let Body::Tagged { page: body, at, .. } = tagged.body else {
            panic!("{tagged:?}");
        };
        assert!(std::ptr::eq(body, page));
        assert_eq!(at, 6);
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
        assert_eq!(response.body.to_vec(), b"234");
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
        assert!(text(&response.body.to_vec()).contains("data-dyno-relay"));
    }

    #[test]
    fn an_html_page_as_it_is_has_no_probe_and_is_a_file_like_any_other() {
        let whole = file_response(
            Path::new("index.html"),
            b"<head></head>",
            None,
            Pages::AsTheyAre,
        );
        assert_eq!(whole.body.to_vec(), b"<head></head>");
        assert_eq!(whole.content_type, "text/html; charset=utf-8");
        let part = file_response(
            Path::new("index.html"),
            b"<head></head>",
            Some("bytes=0-5"),
            Pages::AsTheyAre,
        );
        assert_eq!(part.status, "206 Partial Content");
        assert_eq!(part.body.to_vec(), b"<head>");
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
