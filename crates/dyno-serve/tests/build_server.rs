//! The build alone on 127.0.0.1, with this test as the browser that the CDP driver drives.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;

use dyno_serve::{BuildServer, ServeError};

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
