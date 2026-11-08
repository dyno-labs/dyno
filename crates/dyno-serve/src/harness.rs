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

/// The path the page posts `hello` to.
pub const HELLO_PATH: &str = "/__dyno/hello";

/// The path the page posts its batches of events to.
pub const EVENTS_PATH: &str = "/__dyno/events";

/// The path the page polls for its next command.
pub const NEXT_PATH: &str = "/__dyno/next";

/// The harness page. `{{GAME_URL}}` is the game's URL as a JSON string. The message the harness
/// sends the game is `{type: name, value: true}`, as in the CDP harness: a provisional shape
/// until the host profile fixes it. The self-check's WebGL context is lost again at once. Posts do
/// not use `keepalive`, which caps a body at 64 KiB: a batch from a loading burst can be larger.
/// One batch is in flight at a time, so dyno gets the events in order and an answer such as
/// `flushed` never overtakes the events before it. A batch that did not arrive goes back first
/// in the queue, for the next send.
const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<title>dyno</title>
<style>html,body{margin:0;height:100%;background:#000;color:#aaa;font:15px system-ui,sans-serif}iframe{display:block;border:0;width:100%;height:100%}p{margin:24px}</style>
<p id="note">dyno: starting the run</p>
<script>
"use strict";
const GAME_URL = {{GAME_URL}};
const note = document.getElementById("note");
const queue = [];
let frame = null;
const post = (path, body) =>
  fetch(path, { method: "POST", headers: { "Content-Type": "text/plain" }, body: JSON.stringify(body) });
let sending = false;
const flush = async () => {
  if (sending) return;
  sending = true;
  while (queue.length) {
    const batch = queue.splice(0);
    try {
      await post("/__dyno/events", batch);
    } catch {
      queue.unshift(...batch);
      break;
    }
  }
  sending = false;
};
setInterval(flush, 250);
addEventListener("message", (event) => {
  if (!frame || event.source !== frame.contentWindow) return;
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
  for (;;) {
    let command;
    try {
      command = await (await fetch("/__dyno/next")).json();
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      continue;
    }
    if (command.command === "self_check") {
      queue.push({ kind: "self_check", webgl_renderer: webglRenderer() });
      flush();
    } else if (command.command === "send") {
      frame.contentWindow.postMessage({ type: command.name, value: true }, "*");
    } else if (command.command === "flush") {
      queue.push({ kind: "flushed" });
      flush();
    } else if (command.command === "done") {
      flush();
      frame.remove();
      document.body.append(note);
      note.textContent = "dyno: the run is done. Close this tab before the next run.";
      return;
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
  if (!answer.ok) {
    note.textContent = "dyno: a run is already under way here. Close this tab.";
    return;
  }
  frame = document.createElement("iframe");
  frame.allow = "autoplay; fullscreen";
  const navigatedAt = performance.timeOrigin + performance.now();
  frame.src = GAME_URL;
  note.remove();
  document.body.append(frame);
  queue.push({ kind: "navigated_at", epoch_ms: navigatedAt });
  obey();
})();
</script>
"#;

/// The harness page for a game at this URL.
pub fn page(game_url: &str) -> String {
    // A JSON string is a JS string. `</` is escaped so the URL cannot end the script element.
    let literal = serde_json::Value::String(game_url.to_owned())
        .to_string()
        .replace("</", "<\\/");
    PAGE.replace("{{GAME_URL}}", &literal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_loads_the_game_url_as_a_script_string() {
        let page = page("http://192.168.1.20:7371/");
        assert!(page.contains(r#"const GAME_URL = "http://192.168.1.20:7371/";"#));
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
}
