//! The build alone on 127.0.0.1, with this test as the browser that the CDP driver drives.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;

use dyno_schema::manifest::BuildManifest;
use dyno_serve::files::Build;
use dyno_serve::{BuildServer, ServeError};
use dyno_static::model::BuildFiles;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

/// One HTTP exchange: the head and the body.
fn exchange(url: &str, request: &str) -> (String, Vec<u8>) {
    let address = url.trim_start_matches("http://").trim_end_matches('/');
    let mut socket = TcpStream::connect(address).unwrap();
    socket.write_all(request.as_bytes()).unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).unwrap();
    let end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8(response[..end].to_vec()).unwrap();
    (head, response[end + 4..].to_vec())
}

fn get(url: &str, path: &str) -> (String, Vec<u8>) {
    exchange(url, &format!("GET {path} HTTP/1.1\r\n\r\n"))
}

/// A build folder with a page, a sound and a file beside the build that no request may reach.
fn build() -> tempfile::TempDir {
    let outer = tempfile::tempdir().unwrap();
    fs::write(outer.path().join("secret.txt"), "secret").unwrap();
    let build = outer.path().join("build");
    fs::create_dir(&build).unwrap();
    fs::write(
        build.join("index.html"),
        "<!doctype html><head><title>game</title></head>",
    )
    .unwrap();
    fs::write(build.join("music.mp3"), "0123456789").unwrap();
    outer
}

#[test]
fn a_build_server_serves_each_page_as_it_is_on_the_loopback_address() {
    let outer = build();
    let server = BuildServer::start(&outer.path().join("build")).unwrap();
    assert!(
        server.url().starts_with("http://127.0.0.1:"),
        "{}",
        server.url()
    );
    let (head, body) = get(server.url(), "/");
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert!(
        head.contains("Content-Type: text/html; charset=utf-8"),
        "{head}"
    );
    assert!(head.contains("Cache-Control: no-store"), "{head}");
    assert_eq!(body, b"<!doctype html><head><title>game</title></head>");
    assert!(
        get(server.url(), "/__dyno/probe.js")
            .0
            .starts_with("HTTP/1.1 404")
    );
}

#[test]
fn a_build_server_serves_byte_ranges_and_only_files_inside_the_build() {
    let outer = build();
    let server = BuildServer::start(&outer.path().join("build")).unwrap();
    let (head, body) = exchange(
        server.url(),
        "GET /music.mp3 HTTP/1.1\r\nRange: bytes=2-4\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 206 Partial Content"), "{head}");
    assert!(head.contains("Content-Type: audio/mpeg"), "{head}");
    assert_eq!(body, b"234");
    let (head, body) = exchange(server.url(), "HEAD /music.mp3 HTTP/1.1\r\n\r\n");
    assert!(head.contains("Content-Length: 10"), "{head}");
    assert!(body.is_empty());
    for path in ["/../secret.txt", "/%2e%2e/secret.txt", "/missing.png"] {
        assert!(
            get(server.url(), path).0.starts_with("HTTP/1.1 404"),
            "{path}"
        );
    }
    let (head, _) = exchange(server.url(), "POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 405"), "{head}");
}

#[test]
fn a_build_server_stops_when_it_is_dropped() {
    let outer = build();
    let server = BuildServer::start(&outer.path().join("build")).unwrap();
    let address = server
        .url()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn a_zip_is_not_a_build_folder_to_serve() {
    let folder = tempfile::tempdir().unwrap();
    let zip = folder.path().join("build.zip");
    fs::write(&zip, "PK").unwrap();
    assert!(matches!(
        BuildServer::start(&zip),
        Err(ServeError::Build(_))
    ));
}

/// Operating system litter and version control, which the scan's inventory leaves out.
const SKIPPED: [&str; 5] = [".DS_Store", "Thumbs.db", "desktop.ini", "__MACOSX", ".git"];

fn write_file(folder: &Path, path: &str, contents: &str) {
    let path = folder.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A build folder with files in folders, a name that a URL must encode, and every kind of file
/// that the scan skips: litter, a version control folder and symbolic links.
fn build_with_skipped_files() -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    write_file(root, "index.html", "<!doctype html><title>game</title>");
    write_file(root, "js/main.js", "console.log('game')");
    write_file(root, "assets/reel strip #1.png", "png");
    write_file(root, "assets/sounds/win.mp3", "mp3");
    for name in SKIPPED {
        write_file(root, &format!("assets/{name}"), "litter");
        write_file(root, &format!("{name}/inside.js"), "litter");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("js/main.js"), root.join("same.js")).unwrap();
        std::os::unix::fs::symlink(root.join("js"), root.join("same-js")).unwrap();
    }
    folder
}

/// The files of the build as the static scan lists them, and the bytes it read.
fn scan(build: &Path) -> (BuildManifest, BuildFiles) {
    let (manifest, files) = dyno_static::inventory::scan(build)
        .unwrap()
        .summary
        .into_manifest_and_files();
    (manifest.unwrap(), files)
}

/// The URL path of a build file, with every character that a URL would read as its own encoded.
fn url_path(path: &str) -> String {
    let parts: Vec<String> = path
        .split('/')
        .map(|part| utf8_percent_encode(part, NON_ALPHANUMERIC).to_string())
        .collect();
    format!("/{}", parts.join("/"))
}

/// The paths that the scan skips in [`build_with_skipped_files`].
fn skipped_paths() -> Vec<String> {
    let mut paths = Vec::new();
    for name in SKIPPED {
        paths.push(format!("/assets/{name}"));
        paths.push(format!("/{name}/inside.js"));
    }
    if cfg!(unix) {
        paths.push("/same.js".to_owned());
        paths.push("/same-js/main.js".to_owned());
    }
    paths
}

/// Checks that a server of the build gives every file of the manifest with its own bytes, and a
/// 404 for every path that the scan skips.
fn assert_serves_the_inventory_and_no_more(
    server: &BuildServer,
    build: &Path,
    files: &BuildManifest,
) {
    assert_eq!(files.files.len(), 4, "{files:?}");
    for file in &files.files {
        let (head, body) = get(server.url(), &url_path(file.path.as_str()));
        assert!(
            head.starts_with("HTTP/1.1 200 OK"),
            "{:?}: {head}",
            file.path
        );
        assert_eq!(
            body,
            fs::read(build.join(file.path.as_str())).unwrap(),
            "{:?}",
            file.path
        );
    }
    for path in skipped_paths() {
        let (head, body) = get(server.url(), &path);
        assert!(head.starts_with("HTTP/1.1 404 Not Found"), "{path}: {head}");
        assert_eq!(body, b"404 Not Found", "{path}");
    }
}

#[test]
fn a_build_server_gives_every_file_the_scan_lists_and_none_that_it_skips() {
    let folder = build_with_skipped_files();
    let files = scan(folder.path()).0;
    // With no scan to go by, the server scans the folder itself, by the same rule.
    let server = BuildServer::start(folder.path()).unwrap();
    assert_serves_the_inventory_and_no_more(&server, folder.path(), &files);
}

#[test]
fn a_build_server_of_a_scan_gives_the_bytes_of_the_scan_and_no_more() {
    let folder = build_with_skipped_files();
    let (files, bytes) = scan(folder.path());
    let build = Build::of_scan(bytes);
    let server = BuildServer::start_build(build).unwrap();
    assert_serves_the_inventory_and_no_more(&server, folder.path(), &files);
    // After the scan: a file is added, one is edited, and one is swapped for a link out of the
    // build. None of it is in the build id, so none of it is served.
    write_file(folder.path(), "js/late.js", "late");
    write_file(folder.path(), "js/main.js", "console.log('edited')");
    fs::remove_file(folder.path().join("assets/sounds/win.mp3")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hostname", folder.path().join("assets/sounds/win.mp3"))
        .unwrap();
    let (head, _) = get(server.url(), "/js/late.js");
    assert!(head.starts_with("HTTP/1.1 404 Not Found"), "{head}");
    assert_eq!(get(server.url(), "/js/main.js").1, b"console.log('game')");
    assert_eq!(get(server.url(), "/assets/sounds/win.mp3").1, b"mp3");
    // A byte range is a part of the scanned bytes too.
    let (head, body) = exchange(
        server.url(),
        "GET /js/main.js HTTP/1.1\r\nRange: bytes=0-6\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 206 Partial Content"), "{head}");
    assert_eq!(body, b"console");
    // A HEAD request gets the file's length, and none of its bytes.
    let (head, body) = exchange(server.url(), "HEAD /js/main.js HTTP/1.1\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert!(head.contains("Content-Length: 19"), "{head}");
    assert_eq!(body, b"");
}

#[test]
fn a_build_server_answers_exact_names_only() {
    let folder = tempfile::tempdir().unwrap();
    write_file(folder.path(), "Index.html", "page");
    write_file(folder.path(), "JS/Main.js", "1");
    let server = BuildServer::start(folder.path()).unwrap();
    assert_eq!(get(server.url(), "/Index.html").1, b"page");
    assert_eq!(get(server.url(), "/JS/Main.js").1, b"1");
    // On any volume: `/` asks for `index.html`, which is not this build's `Index.html`.
    for path in ["/", "/index.html", "/js/main.js"] {
        let (head, _) = get(server.url(), path);
        assert!(head.starts_with("HTTP/1.1 404 Not Found"), "{path}: {head}");
    }
}
