//! The harness page: a host page on its own local site that embeds the game the way its host
//! does, in one full-screen frame.
//!
//! The page is served from 127.0.0.1, or from `localhost` when the game itself is on 127.0.0.1,
//! so the game frame is always cross-site and gets its own target. The page reports each message
//! the game sends it through the `__dynoHost` binding, and the run makes the game active with
//! `__dyno.send(name)`.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::CdpError;

/// The binding the harness page calls with each message the game sends it.
pub const HOST_BINDING: &str = "__dynoHost";

/// The loopback address the server binds.
const LOOPBACK: &str = "127.0.0.1";
/// The other name for the loopback address, a different site.
const LOCALHOST: &str = "localhost";

/// The most of a request the server reads: one request line and its headers.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// The harness page. `{{GAME_URL}}` is the launch URL, escaped for an attribute. The message the
/// harness sends is `{type: name, value: true}`: a provisional shape until the host profile
/// fixes it.
const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>dyno harness</title>
<style>html,body{margin:0;height:100%;background:#000}iframe{display:block;border:0;width:100%;height:100%}</style>
<iframe id="game" allow="autoplay; fullscreen" src="{{GAME_URL}}"></iframe>
<script>
"use strict";
const frame = document.getElementById("game");
addEventListener("message", (event) => {
  if (event.source !== frame.contentWindow) return;
  const data = event.data;
  const name = typeof data === "string" ? data : data && (data.type || data.name);
  if (typeof name !== "string") return;
  globalThis.__dynoHost(JSON.stringify({ name, epoch_ms: performance.timeOrigin + performance.now() }));
});
globalThis.__dyno = {
  send(name) { frame.contentWindow.postMessage({ type: name, value: true }, "*"); },
};
</script>
"#;

/// A harness server that runs until it is dropped.
pub struct Harness {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Harness {
    /// Serves the harness page for `launch_url` on a free port.
    pub async fn serve(launch_url: &Url) -> Result<Harness, CdpError> {
        let listener = TcpListener::bind((LOOPBACK, 0))
            .await
            .map_err(|error| CdpError::Harness(error.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|error| CdpError::Harness(error.to_string()))?
            .port();
        let host = if launch_url.host_str() == Some(LOOPBACK) {
            LOCALHOST
        } else {
            LOOPBACK
        };
        let page: Arc<str> = PAGE
            .replace("{{GAME_URL}}", &attribute_text(launch_url.as_str()))
            .into();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let page = Arc::clone(&page);
                tokio::spawn(async move {
                    let Some(path) = request_path(&mut socket).await else {
                        return;
                    };
                    let (status, body) = if path == "/" {
                        ("200 OK", page.as_bytes())
                    } else {
                        ("404 Not Found", &b""[..])
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
                         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    // A client that hangs up early gets nothing more.
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Ok(Harness {
            url: format!("http://{host}:{port}/"),
            task,
        })
    }

    /// The harness page's URL.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The path of an HTTP request, from its request line.
async fn request_path(socket: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buffer = vec![0_u8; MAX_REQUEST_BYTES];
    let mut read = 0;
    while read < buffer.len() {
        match socket.read(&mut buffer[read..]).await {
            Ok(0) | Err(_) => return None,
            Ok(count) => read += count,
        }
        if buffer[..read]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buffer[..read]);
    let target = head.split_whitespace().nth(1)?;
    Some(target.split('?').next().unwrap_or(target).to_owned())
}

/// Text safe inside a double-quoted HTML attribute.
fn attribute_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_url_is_escaped_for_the_attribute() {
        assert_eq!(
            attribute_text(r#"https://g.example/?a=1&b="x"<y>"#),
            "https://g.example/?a=1&amp;b=&quot;x&quot;&lt;y&gt;"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_game_on_the_loopback_address_gets_a_harness_on_localhost() {
        let on_loopback = Url::parse("http://127.0.0.1:5173/").unwrap();
        let harness = Harness::serve(&on_loopback).await.unwrap();
        assert!(harness.url().starts_with("http://localhost:"));
        let elsewhere = Url::parse("https://games.example/").unwrap();
        let harness = Harness::serve(&elsewhere).await.unwrap();
        assert!(harness.url().starts_with("http://127.0.0.1:"));
    }
}
