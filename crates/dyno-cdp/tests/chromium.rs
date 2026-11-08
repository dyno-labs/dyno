//! A real run in Chromium, on a small game served from 127.0.0.1. Ignored by default, because it
//! needs Chromium: run it with `cargo test -p dyno-cdp -- --ignored`. `DYNO_CHROMIUM` names the
//! binary when it is not on the PATH.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use dyno_schema::probe::{AudioState, ContextKind, MessageDirection, ProbeEvent, SourceKind};
use dyno_schema::profile::{HostProfile, Scenario};
use dyno_schema::runner::{DriverMessage, RUNNER_PROTOCOL_VERSION, RunMark, RunRequest};

const PAGE: &str = r#"<!doctype html>
<canvas id="c" width="402" height="874"></canvas>
<iframe srcdoc="<script>const g = document.createElement('canvas').getContext('webgl'); g.bindTexture(g.TEXTURE_2D, g.createTexture()); g.texImage2D(g.TEXTURE_2D, 0, g.RGBA, 77, 77, 0, g.RGBA, g.UNSIGNED_BYTE, null);</script>"></iframe>
<script src="game.js"></script>
"#;

/// A WebGL canvas with antialias off, sized to the viewport at DPR 3 after it gets its context
/// (the profile's viewport times its pixel ratio), a worker that fetches a file, a data URL, one 64 x 32 RGBA
/// texture, 0.1 s of mono audio at 22,050 Hz decoded, a fetch, and `reels:ready` once the
/// audio is decoded.
const GAME: &str = r#"
const canvas = document.getElementById("c");
const gl = canvas.getContext("webgl", { antialias: false });
addEventListener("load", () => {
  canvas.width = innerWidth * devicePixelRatio;
  canvas.height = innerHeight * devicePixelRatio;
});
new Worker("worker.js");
fetch("data:text/plain," + "x".repeat(10000));
gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 64, 32, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(64 * 32 * 4));
function wav(rate, samples) {
  const view = new DataView(new ArrayBuffer(44 + samples * 2));
  const text = (at, value) => { for (let i = 0; i < value.length; i++) view.setUint8(at + i, value.charCodeAt(i)); };
  text(0, "RIFF"); view.setUint32(4, 36 + samples * 2, true); text(8, "WAVE"); text(12, "fmt ");
  view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
  view.setUint32(24, rate, true); view.setUint32(28, rate * 2, true); view.setUint16(32, 2, true);
  view.setUint16(34, 16, true); text(36, "data"); view.setUint32(40, samples * 2, true);
  return view.buffer;
}
const audio = new AudioContext();
audio.decodeAudioData(wav(22050, 2205)).then(() => parent.postMessage({ type: "reels:ready" }, "*"));
requestAnimationFrame(() => gl.clear(gl.COLOR_BUFFER_BIT));
const image = new Image();
image.onload = () => {
  const texture = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, texture);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image);
  gl.deleteTexture(texture);
  const gl2 = document.createElement("canvas").getContext("webgl2");
  gl2.bindTexture(gl2.TEXTURE_2D, gl2.createTexture());
  gl2.texImage2D(gl2.TEXTURE_2D, 0, gl2.RGBA, 32, 16, 0, gl2.RGBA, gl2.UNSIGNED_BYTE, image);
  const cube = gl2.createTexture();
  gl2.bindTexture(gl2.TEXTURE_CUBE_MAP, cube);
  gl2.texStorage2D(gl2.TEXTURE_CUBE_MAP, 1, gl2.RGBA8, 8, 8);
  // Refused: the storage made the cube map immutable.
  gl2.texImage2D(gl2.TEXTURE_CUBE_MAP_POSITIVE_X, 0, gl2.RGBA, 8, 8, 0, gl2.RGBA, gl2.UNSIGNED_BYTE, null);
  gl2.deleteTexture(cube);
};
image.src = "tex.svg?session=SECRET";
fetch("data.json").then((response) => response.json());
// Pixi 8's two ways to make an ImageBitmap: from a fetched blob on this thread, and in a blob
// worker that gets `{ data: [url], uuid, id }` and replies `{ data: bitmap, uuid, id }`.
const uploadBitmap = (bitmap) => {
  gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, bitmap);
};
fetch("bitmap.png?session=SECRET").then((response) => response.blob()).then(createImageBitmap).then(uploadBitmap);
const pixiWorker = "self.onmessage = async (event) => { const blob = await (await fetch(event.data.data[0])).blob(); const bitmap = await createImageBitmap(blob); self.postMessage({ data: bitmap, uuid: event.data.uuid, id: event.data.id }, [bitmap]); };";
const bitmaps = new Worker(URL.createObjectURL(new Blob([pixiWorker], { type: "application/javascript" })));
bitmaps.addEventListener("message", (event) => uploadBitmap(event.data.data));
bitmaps.postMessage({ data: [new URL("worker-bitmap.png?session=SECRET", location.href).href, undefined], uuid: 0, id: "loadImageBitmap" });
// A game that loads each file once and hands Pixi a `blob:` URL or a response made from the blob.
fetch("object-url.png").then((response) => response.blob()).then((blob) => {
  const fromObjectUrl = new Image();
  fromObjectUrl.onload = () => uploadBitmap(fromObjectUrl);
  fromObjectUrl.src = URL.createObjectURL(blob);
});
fetch("response.png").then((response) => response.blob())
  .then((blob) => new Response(blob).blob()).then(createImageBitmap).then(uploadBitmap);
// Uploads with no source: a canvas the game draws text on, and an allocation with no pixels.
const label = document.createElement("canvas");
label.width = 40;
label.height = 20;
label.getContext("2d").fillText("WIN", 2, 15);
uploadBitmap(label);
// Canvases made in another frame of the same origin, then uploaded here.
const other = document.body.appendChild(document.createElement("iframe")).contentWindow;
const foreign = other.document.createElement("canvas");
foreign.width = 36;
foreign.height = 18;
foreign.getContext("2d").fillRect(0, 0, 4, 4);
uploadBitmap(foreign);
const foreignOffscreen = new other.OffscreenCanvas(30, 15);
foreignOffscreen.getContext("2d").fillRect(0, 0, 4, 4);
uploadBitmap(foreignOffscreen);
// Type tags a game sets: a canvas whose tag getter throws, and ImageData that claims to be one.
const tagged = document.createElement("canvas");
tagged.width = 34;
tagged.height = 17;
Object.defineProperty(tagged, Symbol.toStringTag, { get() { throw new Error("no tag"); } });
uploadBitmap(tagged);
const posing = new ImageData(26, 13);
Object.defineProperty(posing, Symbol.toStringTag, { value: "HTMLCanvasElement" });
uploadBitmap(posing);
gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 48, 24, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
"#;

/// A game that asks for WebGPU and takes WebGL 2 when the browser gives no adapter, as Pixi 8
/// does with `preference: 'webgpu'`. On WebGPU it presents 3 frames before `reels:ready`, so a
/// device that is lost stops it first.
const WEBGPU_GAME: &str = r#"<!doctype html>
<canvas id="c"></canvas>
<script>
navigator.gpu.requestAdapter().then(async (adapter) => {
  const canvas = document.getElementById("c");
  if (adapter) {
    const device = await adapter.requestDevice();
    const context = canvas.getContext("webgpu");
    context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat() });
    for (let frame = 0; frame < 3; frame++) {
      const encoder = device.createCommandEncoder();
      const view = context.getCurrentTexture().createView();
      encoder.beginRenderPass({ colorAttachments: [{ view, loadOp: "clear", storeOp: "store" }] }).end();
      device.queue.submit([encoder.finish()]);
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
    await device.queue.onSubmittedWorkDone();
  } else {
    canvas.getContext("webgl2");
  }
  parent.postMessage({ type: "reels:ready" }, "*");
});
</script>
"#;

/// A 4 x 2 RGBA PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x7f, 0xa8, 0x7d,
    0x63, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x19, 0x33, 0xa0, 0x0b, 0x00, 0x00, 0x0f, 0x21, 0x0f, 0xf1, 0xfe, 0x45, 0x14, 0x63, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Serves the game on a free port of 127.0.0.1 and returns its URL.
fn serve_game() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let head = String::from_utf8_lossy(&buffer[..read]);
            let target = head.split_whitespace().nth(1).unwrap_or("/");
            let path = target.split('?').next().unwrap_or(target).to_owned();
            let (kind, body): (&str, &[u8]) = match path.as_str() {
                "/" => ("text/html", PAGE.as_bytes()),
                "/game.js" => ("text/javascript", GAME.as_bytes()),
                "/data.json" => ("application/json", b"{\"lines\": 20}"),
                "/redirect" => ("text/html", b"<script>location.replace(\"/\")</script>"),
                "/webgpu" => ("text/html", WEBGPU_GAME.as_bytes()),
                "/stuck" => (
                    "text/html",
                    b"<script>fetch('data.json'); while (true) {}</script>",
                ),
                "/tex.svg" => (
                    "image/svg+xml",
                    b"<svg xmlns='http://www.w3.org/2000/svg' width='32' height='16'></svg>",
                ),
                "/bitmap.png" | "/worker-bitmap.png" | "/object-url.png" | "/response.png" => {
                    ("image/png", PNG)
                }
                "/worker.js" => ("text/javascript", b"fetch('worker-data.json')"),
                "/worker-data.json" => ("application/json", b"{\"from\": \"worker\"}"),
                _ => ("text/plain", b""),
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    format!("http://127.0.0.1:{port}/")
}

fn request(launch_url: String) -> RunRequest {
    RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url,
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: 1_000,
        timeout_ms: 30_000,
    }
}

fn probe_events(messages: &[DriverMessage]) -> Vec<&ProbeEvent> {
    messages
        .iter()
        .filter_map(|message| match message {
            DriverMessage::Probe { event } => Some(event),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "needs Chromium"]
fn a_run_sees_the_frame_from_its_first_script_to_its_activation() {
    let messages = dyno_cdp::run_cdp(&request(serve_game())).unwrap();
    for message in &messages {
        eprintln!("{}", serde_json::to_string(message).unwrap());
    }
    assert!(matches!(
        messages.first(),
        Some(DriverMessage::Hello { .. })
    ));
    assert!(
        matches!(messages.last(), Some(DriverMessage::Done { .. })),
        "{:?}",
        messages.last()
    );

    let marks: Vec<RunMark> = messages
        .iter()
        .filter_map(|message| match message {
            DriverMessage::Mark { mark, .. } => Some(*mark),
            _ => None,
        })
        .collect();
    assert_eq!(
        marks,
        [
            RunMark::Navigated,
            RunMark::FrameDocument,
            RunMark::Idle,
            RunMark::Activated
        ]
    );

    let urls: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            DriverMessage::Request { url, .. } => Some(url.as_str()),
            _ => None,
        })
        .collect();
    assert!(!urls.iter().any(|url| url.starts_with("data:")), "{urls:?}");
    for path in [
        "/",
        "/game.js",
        "/data.json",
        "/worker.js",
        "/worker-data.json",
    ] {
        assert!(
            urls.iter().any(|url| url.ends_with(path)),
            "{path} in {urls:?}"
        );
    }

    let events = probe_events(&messages);
    assert!(events.iter().any(|event| matches!(
        event,
        ProbeEvent::ContextCreated { kind: ContextKind::Webgl, width: 402, height: 874, antialias: Some(false), device_pixel_ratio, .. }
            if *device_pixel_ratio == 3.0
    )), "{events:?}");
    assert!(
        events.iter().any(|event| matches!(
            event,
            ProbeEvent::TextureUploaded {
                width: 64,
                height: 32,
                bytes: 8_192,
                ..
            }
        )),
        "{events:?}"
    );
    let from_image = events.iter().find_map(|event| match event {
        ProbeEvent::TextureUploaded {
            texture: Some(texture),
            source: Some(source),
            width: 32,
            height: 16,
            ..
        } => Some((*texture, source.clone())),
        _ => None,
    });
    let (texture, source) = from_image.expect("an upload from the image");
    assert_eq!(source, "/tex.svg", "the path only, no query");
    assert!(events.iter().any(|event| matches!(
        event,
        ProbeEvent::TextureDeleted { texture: deleted, .. } if *deleted == texture
    )));
    let from_image = events
        .iter()
        .filter(|event| matches!(event, ProbeEvent::TextureUploaded { source: Some(source), .. } if source == "/tex.svg"))
        .count();
    assert_eq!(
        from_image, 2,
        "the WebGL1 and the WebGL2 forms both keep the source"
    );
    let bitmap_sources: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ProbeEvent::TextureUploaded {
                source: Some(source),
                width: 4,
                height: 2,
                ..
            } => Some(source.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        bitmap_sources.contains(&"/bitmap.png"),
        "an ImageBitmap from a fetched blob keeps its path: {events:?}"
    );
    assert!(
        bitmap_sources.contains(&"/worker-bitmap.png"),
        "an ImageBitmap from a worker keeps its path: {events:?}"
    );
    assert!(
        bitmap_sources.contains(&"/object-url.png"),
        "an image from a blob: object URL keeps the path of the blob's file: {events:?}"
    );
    assert!(
        bitmap_sources.contains(&"/response.png"),
        "a response made from a blob keeps the path of the blob's file: {events:?}"
    );
    let kind_of = |wanted_width: u32| {
        events.iter().find_map(|event| match event {
            ProbeEvent::TextureUploaded {
                width,
                source: None,
                source_kind,
                ..
            } if *width == wanted_width => Some(*source_kind),
            _ => None,
        })
    };
    assert_eq!(
        kind_of(40),
        Some(Some(SourceKind::Canvas)),
        "a drawn canvas"
    );
    assert_eq!(
        kind_of(36),
        Some(Some(SourceKind::Canvas)),
        "a canvas from another frame"
    );
    assert_eq!(
        kind_of(30),
        Some(Some(SourceKind::Canvas)),
        "an OffscreenCanvas from another frame"
    );
    assert_eq!(
        kind_of(34),
        Some(Some(SourceKind::Canvas)),
        "a canvas whose type tag throws, uploaded all the same"
    );
    assert_eq!(
        kind_of(26),
        Some(None),
        "ImageData that claims to be a canvas"
    );
    assert_eq!(kind_of(48), Some(Some(SourceKind::Empty)), "no pixels");
    assert_eq!(kind_of(64), Some(None), "data is neither");
    let cube = events.iter().find_map(|event| match event {
        ProbeEvent::TextureUploaded {
            texture: Some(texture),
            face: None,
            level: None,
            width: 8,
            bytes,
            ..
        } => Some((*texture, *bytes)),
        _ => None,
    });
    let (cube, cube_bytes) = cube.expect("the cube map's storage");
    assert_eq!(cube_bytes, 8 * 8 * 4 * 6, "six faces");
    assert!(events.iter().any(|event| matches!(
        event,
        ProbeEvent::TextureDeleted { texture, .. } if *texture == cube
    )));
    assert!(
        !events.iter().any(|event| matches!(
            event,
            ProbeEvent::TextureUploaded {
                width: 8,
                level: Some(0),
                ..
            }
        )),
        "the refused upload is not counted"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProbeEvent::TextureUploaded { width: 77, .. })),
        "the child frame's probe is left out"
    );
    let profile = HostProfile::reels();
    let (width, height) = (
        profile.viewport.width * profile.device_scale_factor,
        profile.viewport.height * profile.device_scale_factor,
    );
    let last_size = events.iter().rev().find_map(|event| match event {
        ProbeEvent::CanvasResized {
            canvas: 0,
            width,
            height,
            ..
        } => Some((*width, *height)),
        _ => None,
    });
    assert_eq!(
        last_size,
        Some((width, height)),
        "the frame has the profile's viewport"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProbeEvent::AudioDecoded { channels: 1, .. })),
        "{events:?}"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        ProbeEvent::AudioContextState {
            state: AudioState::Running | AudioState::Suspended,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProbeEvent::FirstFrame { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        ProbeEvent::HostMessage { direction: MessageDirection::ToHost, name, .. } if name == "reels:ready"
    )), "{events:?}");
    assert!(events.iter().any(|event| matches!(
        event,
        ProbeEvent::HostMessage { direction: MessageDirection::FromHost, name, .. } if name == "reels:active"
    )), "{events:?}");

    let checks = messages.iter().find_map(|message| match message {
        DriverMessage::SelfCheck {
            device_pixel_ratio,
            webgl_renderer,
            ..
        } => Some((*device_pixel_ratio, webgl_renderer.clone())),
        _ => None,
    });
    let (device_pixel_ratio, webgl_renderer) = checks.expect("a self-check");
    assert_eq!(device_pixel_ratio, 3.0);
    assert!(webgl_renderer.is_some());
    assert!(messages.iter().any(
        |message| matches!(message, DriverMessage::Heap { used_bytes, .. } if *used_bytes > 0)
    ));
}

#[test]
#[ignore = "needs Chromium"]
fn a_script_redirect_starts_a_new_document_and_the_run_follows_it() {
    let messages = dyno_cdp::run_cdp(&request(format!("{}redirect", serve_game()))).unwrap();
    assert!(
        matches!(messages.last(), Some(DriverMessage::Done { .. })),
        "{:?}",
        messages.last()
    );
    let documents = messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                DriverMessage::Mark {
                    mark: RunMark::FrameDocument,
                    ..
                }
            )
        })
        .count();
    assert_eq!(documents, 2);
    let events = probe_events(&messages);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProbeEvent::TextureUploaded { .. }))
    );
}

/// The kind of each context the WebGPU game made, and whether it sent `reels:ready`, in a run
/// with this profile's WebGPU.
fn webgpu_game_with_webgpu(webgpu: bool) -> (Vec<ContextKind>, bool) {
    let mut run = request(format!("{}webgpu", serve_game()));
    run.host_profile.webgpu = webgpu;
    let messages = dyno_cdp::run_cdp(&run).unwrap();
    let events = probe_events(&messages);
    let contexts = events
        .iter()
        .filter_map(|event| match event {
            ProbeEvent::ContextCreated { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    let ready = events.iter().any(|event| matches!(
        event,
        ProbeEvent::HostMessage { direction: MessageDirection::ToHost, name, .. } if name == "reels:ready"
    ));
    (contexts, ready)
}

#[test]
#[ignore = "needs Chromium"]
fn a_profile_with_webgpu_gives_the_game_a_webgpu_canvas_that_presents() {
    let (contexts, ready) = webgpu_game_with_webgpu(true);
    assert_eq!(contexts, [ContextKind::Webgpu]);
    assert!(ready, "the device was lost before the game was ready");
}

// On a Mac, Chrome's GPU gives WebGPU whatever the profile says, so only Linux can turn it off.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "needs Chromium"]
fn a_profile_without_webgpu_gives_the_game_no_webgpu_adapter() {
    let (contexts, _) = webgpu_game_with_webgpu(false);
    assert_eq!(contexts, [ContextKind::Webgl2]);
}

#[test]
#[ignore = "needs Chromium"]
fn a_game_stuck_in_a_loop_times_out_near_the_deadline() {
    let started = std::time::Instant::now();
    let mut stuck = request(format!("{}stuck", serve_game()));
    stuck.timeout_ms = 3_000;
    let messages = dyno_cdp::run_cdp(&stuck).unwrap();
    assert!(
        matches!(messages.last(), Some(DriverMessage::Error { message, .. }) if message.contains("timed out")),
        "{:?}",
        messages.last()
    );
    // The deadline, the browser's 5 s to exit, and a margin.
    assert!(started.elapsed().as_secs() < 12, "{:?}", started.elapsed());
}
