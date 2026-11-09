//! The build's files, as the game server serves them: a URL path to a file inside the build
//! folder, its content type, and each HTML page with the probe's script tag added, or as it is.

use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;

use crate::ServeError;
use crate::http::{RangeAsk, Request, Response, range_ask};

/// The path the game server serves the probe at. A request under it is dyno's, not the game's.
pub const DYNO_PATH: &str = "/__dyno/";

/// The probe's path on the game server.
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

/// A build folder to serve.
#[derive(Clone, Debug)]
pub struct Build {
    /// The folder, with every link resolved.
    root: PathBuf,
}

impl Build {
    /// The build in this folder. A .zip or a missing folder is an error.
    pub fn open(path: &Path) -> Result<Build, ServeError> {
        if !path.is_dir() {
            return Err(ServeError::Build(format!(
                "{} is not a folder: the served run serves a build folder, so unzip a .zip first",
                path.display()
            )));
        }
        let root = path
            .canonicalize()
            .map_err(|error| ServeError::Build(format!("{}: {error}", path.display())))?;
        Ok(Build { root })
    }

    /// The file a URL path names, inside the build. None for a path that leaves the build, such
    /// as through `..` or a link, and for a file that is not there. A folder serves its
    /// `index.html`.
    pub fn file(&self, url_path: &str) -> Option<PathBuf> {
        let mut path = self.root.clone();
        for segment in url_path.split('/') {
            let segment = percent_decode_str(segment).decode_utf8().ok()?;
            match segment.as_ref() {
                "" | "." => {}
                ".." => return None,
                name if name.contains(['/', '\\', '\0']) => return None,
                name => path.push(name),
            }
        }
        if path.is_dir() {
            path.push(INDEX);
        }
        let path = path.canonicalize().ok()?;
        let inside = path.starts_with(&self.root)
            && path
                .components()
                .all(|component| !matches!(component, Component::ParentDir));
        (inside && path.is_file()).then_some(path)
    }
}

/// The response to a request for a build file: the file, part of it, or an error. Only GET and
/// HEAD read a file.
pub(crate) fn build_response(build: &Build, request: &Request, pages: Pages) -> Response {
    if request.method != "GET" && request.method != "HEAD" {
        return Response::error("405 Method Not Allowed");
    }
    let Some(path) = build.file(&request.path) else {
        return Response::error("404 Not Found");
    };
    match std::fs::read(&path) {
        Ok(contents) => file_response(&path, contents, request.range.as_deref(), pages),
        Err(_) => Response::error("404 Not Found"),
    }
}

/// The response for a file: the whole file, or the part a `Range` header asks for. With the
/// probe, an HTML page gets the probe's tag and is always sent whole.
pub fn file_response(
    path: &Path,
    contents: Vec<u8>,
    range: Option<&str>,
    pages: Pages,
) -> Response {
    let content_type = content_type(path);
    if pages == Pages::WithProbe && is_html(path) {
        return Response::new("200 OK", content_type, with_probe(&contents));
    }
    let length = contents.len() as u64;
    let mut response = match range_ask(range, length) {
        RangeAsk::Whole => Response::new("200 OK", content_type, contents),
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
    fn a_path_names_a_file_inside_the_build() {
        let folder = tempfile::tempdir().unwrap();
        fs::create_dir(folder.path().join("assets")).unwrap();
        fs::write(folder.path().join("index.html"), "page").unwrap();
        fs::write(folder.path().join("assets/a b.png"), "png").unwrap();
        let build = Build::open(folder.path()).unwrap();
        let root = folder.path().canonicalize().unwrap();
        assert_eq!(build.file("/"), Some(root.join("index.html")));
        assert_eq!(
            build.file("/assets/a%20b.png"),
            Some(root.join("assets/a b.png"))
        );
        assert_eq!(build.file("/assets/missing.png"), None);
    }

    #[test]
    fn a_path_that_leaves_the_build_names_nothing() {
        let outer = tempfile::tempdir().unwrap();
        fs::write(outer.path().join("secret.txt"), "secret").unwrap();
        fs::create_dir(outer.path().join("build")).unwrap();
        let build = Build::open(&outer.path().join("build")).unwrap();
        for path in ["/../secret.txt", "/%2e%2e/secret.txt", "/..%2fsecret.txt"] {
            assert_eq!(build.file(path), None, "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_build_names_nothing() {
        let outer = tempfile::tempdir().unwrap();
        fs::write(outer.path().join("secret.txt"), "secret").unwrap();
        fs::create_dir(outer.path().join("build")).unwrap();
        std::os::unix::fs::symlink(
            outer.path().join("secret.txt"),
            outer.path().join("build/link.txt"),
        )
        .unwrap();
        let build = Build::open(&outer.path().join("build")).unwrap();
        assert_eq!(build.file("/link.txt"), None);
    }

    #[test]
    fn a_zip_is_not_a_build_folder() {
        let folder = tempfile::tempdir().unwrap();
        let zip = folder.path().join("build.zip");
        fs::write(&zip, "PK").unwrap();
        assert!(matches!(Build::open(&zip), Err(ServeError::Build(_))));
    }

    #[test]
    fn a_range_request_gets_part_of_a_media_file() {
        let response = file_response(
            Path::new("a.mp3"),
            b"0123456789".to_vec(),
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
            b"0123".to_vec(),
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
            b"<head>".to_vec(),
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
            b"<head></head>".to_vec(),
            None,
            Pages::AsTheyAre,
        );
        assert_eq!(whole.body, b"<head></head>");
        assert_eq!(whole.content_type, "text/html; charset=utf-8");
        let part = file_response(
            Path::new("index.html"),
            b"<head></head>".to_vec(),
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
