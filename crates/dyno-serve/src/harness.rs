//! The harness page of a served run: a host page that embeds the game in one full-screen frame,
//! the way its host does, and talks to dyno over HTTP on its own origin.
//!
//! The page posts `hello`, and dyno answers once it has started its clock. Only then does the
//! page load the game, so every game request comes after the navigation. A second page that
//! posts `hello` in the same run is turned away. The page relays what
//! the game frame posts to it (the probe's events, the probe's document, and the game's own
//! messages to its host) to `/__dyno/events` in small batches, and polls `/__dyno/next` for what
//! to do: run the self-check, send the game a message, send what it holds, or end the run. An
//! answer it posts goes last in the same batch as the events before it, so dyno sees them first.
//!
//! Each of the page's requests carries the run's key in a header ([`crate::key`]). The page sends
//! no referrer, and neither does its frame, so the game's first request does not name the page's
//! URL, which holds the key. The game gets its own URL and the page's messages, and no key.

use crate::key::RunKey;

/// The path the page posts `hello` to.
pub const HELLO_PATH: &str = "/__dyno/hello";

/// The path the page posts its batches of events to.
pub const EVENTS_PATH: &str = "/__dyno/events";

/// The path the page polls for its next command.
pub const NEXT_PATH: &str = "/__dyno/next";

/// The harness page. `{{GAME_URL}}` is the game's URL and `{{KEY}}` the run's key, each as a JSON
/// string. The referrer policy is in the page and on the frame, and the server sends it in a
/// header too, so a browser that reads only one of them still sends none. The message the harness
/// sends the game is `{type: name, value: true}`, as in the CDP harness: a provisional shape
/// until the host profile fixes it. The self-check's WebGL context is lost again at once. Posts do
/// not use `keepalive`, which caps a body at 64 KiB: a batch from a loading burst can be larger.
/// One batch is in flight at a time, so dyno gets the events in order and an answer such as
/// `flushed` never overtakes the events before it. A batch that did not arrive goes back first
/// in the queue, for the next send.
///
/// The page takes a message only from its frame's window while that window holds the game's
/// origin, and posts to the frame only at that origin: a frame that went to another site keeps
/// its window, and must neither add to the run nor hear the active message. A 403 means the
/// page's key opens no run, because its run is over or it was never asked for. The page then
/// stops, and removes its frame, so a tab that missed `done` cannot go on and reach a later run.
const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="referrer" content="no-referrer">
<title>dyno</title>
<style>html,body{margin:0;height:100%;background:#000;color:#aaa;font:15px system-ui,sans-serif}iframe{display:block;border:0;width:100%;height:100%}p{margin:24px}</style>
<p id="note">dyno: starting the run</p>
<script>
"use strict";
const GAME_URL = {{GAME_URL}};
const KEY = {{KEY}};
const GAME_ORIGIN = new URL(GAME_URL).origin;
const note = document.getElementById("note");
const queue = [];
let frame = null;
let stopped = false;
const stop = (text) => {
  stopped = true;
  queue.length = 0;
  if (frame) frame.remove();
  frame = null;
  document.body.append(note);
  note.textContent = text;
};
const refused = () => stop("dyno: this page's URL opens no run. Close this tab, and open the URL that dyno printed for the run.");
const post = (path, body) =>
  fetch(path, { method: "POST", headers: { "Content-Type": "text/plain", "X-Dyno-Key": KEY }, body: JSON.stringify(body) });
let sending = false;
const flush = async () => {
  if (sending || stopped) return;
  sending = true;
  while (queue.length && !stopped) {
    const batch = queue.splice(0);
    let answer;
    try {
      answer = await post("/__dyno/events", batch);
    } catch {
      queue.unshift(...batch);
      break;
    }
    if (answer.status === 403) refused();
  }
  sending = false;
};
setInterval(flush, 250);
addEventListener("message", (event) => {
  if (!frame || event.source !== frame.contentWindow || event.origin !== GAME_ORIGIN) return;
  const data = event.data;
  if (data && data.dyno === "probe") {
    queue.push({ kind: "probe", payload: String(data.payload) });
  } else if (data && data.dyno === "document") {
    queue.push({ kind: "document", time_origin: data.time_origin, device_pixel_ratio: data.device_pixel_ratio });
  } else {
    const name = typeof data === "string" ? data : data && (data.type || data.name);
    if (typeof name === "string") queue.push({ kind: "host", name, epoch_ms: performance.timeOrigin + performance.now() });
  }
});
const webglRenderer = () => {
  const gl = document.createElement("canvas").getContext("webgl");
  if (!gl) return null;
  const info = gl.getExtension("WEBGL_debug_renderer_info");
  const renderer = info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
  const lose = gl.getExtension("WEBGL_lose_context");
  if (lose) lose.loseContext();
  return renderer;
};
const obey = async () => {
  while (!stopped) {
    let answer;
    let command;
    try {
      answer = await fetch("/__dyno/next", { headers: { "X-Dyno-Key": KEY } });
      if (answer.status !== 403) command = await answer.json();
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      continue;
    }
    if (answer.status === 403) return refused();
    if (stopped) return;
    if (command.command === "self_check") {
      queue.push({ kind: "self_check", webgl_renderer: webglRenderer() });
      flush();
    } else if (command.command === "send") {
      frame.contentWindow.postMessage({ type: command.name, value: true }, GAME_ORIGIN);
    } else if (command.command === "flush") {
      queue.push({ kind: "flushed" });
      flush();
    } else if (command.command === "done") {
      // The last batch leaves now, before stop clears the queue.
      flush();
      return stop("dyno: the run is done. Close this tab before the next run.");
    }
  }
};
(async () => {
  let answer;
  try {
    answer = await post("/__dyno/hello", { user_agent: navigator.userAgent });
  } catch {
    note.textContent = "dyno: cannot reach dyno. Is the run still waiting?";
    return;
  }
  if (answer.status === 403) return refused();
  if (!answer.ok) {
    note.textContent = "dyno: a run is already under way here. Close this tab.";
    return;
  }
  frame = document.createElement("iframe");
  frame.allow = "autoplay; fullscreen";
  frame.referrerPolicy = "no-referrer";
  const navigatedAt = performance.timeOrigin + performance.now();
  frame.src = GAME_URL;
  note.remove();
  document.body.append(frame);
  queue.push({ kind: "navigated_at", epoch_ms: navigatedAt });
  obey();
})();
</script>
"#;

/// The header that the server sends with the page, so the page and its frame send no referrer.
pub const REFERRER_POLICY: (&str, &str) = ("Referrer-Policy", "no-referrer");

/// The harness page for a game at this URL, in a run with this key.
pub fn page(game_url: &str, key: &RunKey) -> String {
    fill(
        PAGE,
        &[
            ("{{GAME_URL}}", script_string(game_url)),
            ("{{KEY}}", script_string(key.as_str())),
        ],
    )
}

/// The template with each placeholder replaced by its value, in one pass from the start: a value
/// that was put in is never read again, so a game URL whose query holds a placeholder's text,
/// which `--serve-query` allows, stays as it is.
fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut filled = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((at, name, value)) = values
        .iter()
        .filter_map(|(name, value)| rest.find(name).map(|at| (at, *name, value)))
        .min_by_key(|(at, ..)| *at)
    {
        filled.push_str(&rest[..at]);
        filled.push_str(value);
        rest = &rest[at + name.len()..];
    }
    filled.push_str(rest);
    filled
}

/// A JS string literal that holds `text`. A JSON string is a JS string. `</` is escaped so the
/// text cannot end the script element.
fn script_string(text: &str) -> String {
    serde_json::Value::String(text.to_owned())
        .to_string()
        .replace("</", "<\\/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::KEY_HEADER;

    fn page(game_url: &str) -> String {
        super::page(game_url, &RunKey::new().unwrap())
    }

    #[test]
    fn the_page_loads_the_game_url_as_a_script_string() {
        let page = page("http://192.168.1.20:7371/");
        assert!(page.contains(r#"const GAME_URL = "http://192.168.1.20:7371/";"#));
    }

    #[test]
    fn a_game_url_that_holds_a_placeholder_s_text_is_put_in_as_it_is() {
        let key = RunKey::new().unwrap();
        let game_url = "http://h/?a={{KEY}}&b={{GAME_URL}}";
        let page = super::page(game_url, &key);
        assert!(page.contains(r#"const GAME_URL = "http://h/?a={{KEY}}&b={{GAME_URL}}";"#));
        assert!(page.contains(&format!(r#"const KEY = "{}";"#, key.as_str())));
        // The key is in its constant only, not in the game's URL.
        assert_eq!(page.matches(key.as_str()).count(), 1);
    }

    #[test]
    fn a_template_is_filled_in_one_pass_in_any_order() {
        let values = [("{{A}}", "{{B}}".to_owned()), ("{{B}}", "{{A}}".to_owned())];
        assert_eq!(fill("{{B}} {{A}} {{A}}", &values), "{{A}} {{B}} {{B}}");
        assert_eq!(fill("no placeholder", &values), "no placeholder");
    }

    #[test]
    fn the_page_hears_and_tells_only_the_game_s_origin_in_its_frame() {
        let page = page("http://h:7371/");
        assert!(page.contains("const GAME_ORIGIN = new URL(GAME_URL).origin;"));
        assert!(page.contains(
            "if (!frame || event.source !== frame.contentWindow || event.origin !== GAME_ORIGIN) return;"
        ));
        assert!(!page.contains(r#", "*")"#), "{page}");
    }

    #[test]
    fn a_refused_request_stops_the_page_and_removes_its_frame() {
        let page = page("http://h/");
        // Each of the page's three requests stops the page on a 403: the hello, the posts of
        // events and the poll.
        assert_eq!(page.matches("answer.status === 403").count(), 3, "{page}");
        assert_eq!(page.matches("refused()").count(), 3, "{page}");
        // A stopped page sends nothing more, polls no more and has no frame.
        assert!(page.contains("if (sending || stopped) return;"));
        assert!(page.contains("while (!stopped) {"));
        assert!(page.contains("  if (frame) frame.remove();\n  frame = null;"));
    }

    #[test]
    fn a_url_cannot_end_the_script() {
        let page = page(r#"http://h/</script><script>alert(1)"#);
        assert!(!page.contains("</script><script>alert"));
    }

    #[test]
    fn a_batch_of_any_size_can_be_sent() {
        assert!(!page("http://h/").contains("keepalive"));
    }

    #[test]
    fn the_page_uses_the_paths_the_server_answers() {
        let page = page("http://h/");
        for path in [HELLO_PATH, EVENTS_PATH, NEXT_PATH] {
            assert!(page.contains(path), "{path}");
        }
    }

    #[test]
    fn every_request_of_the_page_carries_the_key_in_the_header_the_server_reads() {
        let key = RunKey::new().unwrap();
        let page = super::page("http://h/", &key);
        assert!(page.contains(&format!(r#"const KEY = "{}";"#, key.as_str())));
        // The page makes two kinds of request, posts and the poll, and each sends the header.
        assert_eq!(page.matches("fetch(").count(), 2, "{page}");
        assert_eq!(page.matches(&format!(r#""{KEY_HEADER}": KEY"#)).count(), 2);
    }

    #[test]
    fn neither_the_page_nor_its_frame_sends_a_referrer() {
        let page = page("http://h/");
        assert!(page.contains(r#"<meta name="referrer" content="no-referrer">"#));
        assert!(page.contains(r#"frame.referrerPolicy = "no-referrer";"#));
        assert_eq!(REFERRER_POLICY, ("Referrer-Policy", "no-referrer"));
    }

    #[test]
    fn the_frame_gets_no_key_from_the_page() {
        let key = RunKey::new().unwrap();
        let page = super::page("http://h/", &key);
        // The key is in one place, its constant, and only the page's own requests read it.
        assert_eq!(page.matches(key.as_str()).count(), 1);
        assert_eq!(page.matches("KEY").count(), 3, "{page}");
        // What the page gives the frame: its URL, and messages of the host profile's shape.
        assert!(page.contains("frame.src = GAME_URL;"));
        assert!(page.contains(r#"postMessage({ type: command.name, value: true }, GAME_ORIGIN)"#));
        assert_eq!(page.matches("postMessage(").count(), 1, "{page}");
        // The page never lowers its origin to the host's, which would let a frame from the
        // game's port that does the same read it, key and all.
        assert!(!page.contains("document.domain"), "{page}");
    }
}
