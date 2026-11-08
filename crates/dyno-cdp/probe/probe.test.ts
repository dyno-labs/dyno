// Unit tests of the probe. probe.ts, with its types stripped, runs in a fresh context with fake
// browser APIs, and each test reads the events it sends. Stripping keeps every position, so
// coverage maps onto probe.ts. The Chromium tests in crates/dyno-cdp/tests/chromium.rs run the
// built probe in a real browser.
//
//   node --test "crates/dyno-cdp/probe/*.test.ts"
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { test } from "node:test";
import vm from "node:vm";

const PROBE = new URL("probe.ts", import.meta.url);
const LAB = stripTypeScriptTypes(readFileSync(PROBE, "utf8"), { mode: "strip" });
const FIELD = readFileSync(new URL("probe.field.js", import.meta.url), "utf8");

const TEXTURE_2D = 0x0de1;
const CUBE_MAP = 0x8513;
const CUBE_FACE_FIRST = 0x8515;
const RGB = 0x1907;
const RGBA = 0x1908;
const LUMINANCE = 0x1909;
const LUMINANCE_ALPHA = 0x190a;
const UNSIGNED_BYTE = 0x1401;
const FLOAT = 0x1406;
const HALF_FLOAT = 0x140b;
const HALF_FLOAT_OES = 0x8d61;
const UNSIGNED_SHORT_4_4_4_4 = 0x8033;
const UNSIGNED_INT = 0x1405;
const R8 = 0x8229;
const RGBA8 = 0x8058;

/** One event the probe sent, as JSON. */
type ProbeEvent = { type: string; [field: string]: unknown };
type Listener = (event: unknown) => void;
type Attributes = { antialias?: boolean };

/** The item at `index`, which the test needs to be there. */
function at<T>(list: readonly T[], index: number): T {
  const item = list.at(index);
  if (item === undefined) throw new Error(`no item at ${index}`);
  return item;
}

class FakeEventTarget {
  listeners: Record<string, Listener[]> = {};
  addEventListener(type: string, listener: Listener): void {
    (this.listeners[type] ??= []).push(listener);
  }
  dispatch(type: string, event: unknown = {}): void {
    for (const listener of this.listeners[type] ?? []) listener(event);
  }
}

/**
 * A fake WebGL context: its texture bindings, and the textures texStorage2D made immutable. Each
 * frame makes its own WebGL1 and WebGL2 classes from it, as siblings: in a browser neither
 * interface inherits the other.
 */
class FakeGl {
  attributes: { antialias: boolean };
  bound = new Map<number, object>();
  immutable = new Set<object | undefined>();
  constructor(attributes: Attributes = {}) {
    this.attributes = { antialias: true, ...attributes };
  }
  getContextAttributes(): { antialias: boolean } {
    return this.attributes;
  }
  bindTexture(target: number, texture: object): void {
    this.bound.set(target === CUBE_MAP ? CUBE_MAP : TEXTURE_2D, texture);
  }
  getParameter(name: number): object | null {
    return (name === 0x8514 ? this.bound.get(CUBE_MAP) : this.bound.get(TEXTURE_2D)) ?? null;
  }
  getTexParameter(target: number, name: number): boolean {
    return name === 0x912f && this.immutable.has(this.bound.get(target));
  }
  texImage2D(..._args: unknown[]): void {}
  compressedTexImage2D(..._args: unknown[]): void {}
  texStorage2D(target: number, ..._args: unknown[]): void {
    this.immutable.add(this.bound.get(target === CUBE_MAP ? CUBE_MAP : TEXTURE_2D));
  }
  deleteTexture(_texture: object | null): void {}
}

/**
 * A game frame with the APIs the probe wraps, and the probe in it. `run` is the probe build to
 * run; `topLevel` makes the frame its own parent. `script` gives the frame a document whose script
 * tag asks for the relay or not; `nested` puts the frame inside another frame.
 */
function frame({
  run = LAB,
  topLevel = false,
  driver = true,
  script = undefined as "relay" | "plain" | undefined,
  nested = false,
} = {}) {
  const events: ProbeEvent[] = [];
  /** What the frame posted to its parent. */
  const posted: unknown[] = [];
  const clock = { now: 0 };
  const intervals: (() => void)[] = [];
  const animationFrames: ((time: number) => void)[] = [];
  const window = new FakeEventTarget();

  // The probe wraps their prototypes, so each frame has its own.
  class WebGLRenderingContext extends FakeGl {}
  class WebGL2RenderingContext extends FakeGl {}

  // The width getter checks that `this` is a real canvas, as a browser's does.
  class HTMLCanvasElement extends FakeEventTarget {
    #width = 300;
    #height = 150;
    get width(): number {
      return this.#width;
    }
    set width(value: number) {
      this.#width = value;
    }
    get height(): number {
      return this.#height;
    }
    set height(value: number) {
      this.#height = value;
    }
    getContext(kind: string, attributes?: Attributes): FakeGl | object | null {
      if (kind === "webgl") return new WebGLRenderingContext(attributes);
      if (kind === "webgl2") return new WebGL2RenderingContext(attributes);
      if (kind === "2d") return {};
      return null;
    }
  }

  type Decoded = { length: number; numberOfChannels: number; sampleRate: number };
  class BaseAudioContext extends FakeEventTarget {
    state = "suspended";
    sampleRate = 48000;
    decodeAudioData(data: ArrayBuffer, onSuccess?: (buffer: Decoded) => void): Promise<Decoded> {
      const buffer = { length: data.byteLength, numberOfChannels: 2, sampleRate: this.sampleRate };
      if (onSuccess) onSuccess(buffer);
      return Promise.resolve(buffer);
    }
  }
  class AudioContext extends BaseAudioContext {}
  class webkitAudioContext extends BaseAudioContext {}

  class Response {
    body: unknown;
    url?: string;
    constructor(body?: unknown) {
      this.body = body;
    }
    blob(): Promise<object> {
      return Promise.resolve({ blob: true });
    }
  }
  let objectUrls = 0;
  class FakeURL extends URL {
    static override createObjectURL(): string {
      objectUrls += 1;
      return `blob:https://cdn.game.test/${objectUrls}`;
    }
  }
  class Worker extends FakeEventTarget {
    posted: unknown[] = [];
    postMessage(message: unknown): void {
      this.posted.push(message);
    }
  }

  const context = vm.createContext({
    performance: { now: () => clock.now, timeOrigin: 1_000 },
    setInterval: (callback: () => void) => intervals.push(callback),
    requestAnimationFrame: (callback: (time: number) => void) => animationFrames.push(callback),
    addEventListener: (type: string, listener: Listener) => window.addEventListener(type, listener),
    devicePixelRatio: 3,
    location: { href: "https://cdn.game.test/game/index.html" },
    HTMLCanvasElement,
    WebGLRenderingContext,
    WebGL2RenderingContext,
    BaseAudioContext,
    AudioContext,
    webkitAudioContext,
    Response,
    URL: FakeURL,
    createImageBitmap: (source: unknown) =>
      Promise.resolve({ bitmapOf: source, width: 4, height: 4 }),
    Worker,
  });
  context.window = context;
  context.parent = topLevel ? context : { postMessage: (data: unknown) => posted.push(data) };
  context.top = nested ? {} : context.parent;
  if (script) {
    context.document = {
      currentScript: {
        hasAttribute: (name: string) => script === "relay" && name === "data-dyno-relay",
      },
    };
  }
  if (driver) {
    context.__dynoProbe = (payload: string) => events.push(JSON.parse(payload) as ProbeEvent);
  }
  new vm.Script(run, { filename: run === LAB ? PROBE.href : "probe.field.js" }).runInContext(
    context,
  );

  return {
    context,
    events,
    posted,
    clock,
    /** Events of one type, without their time. */
    sent(type: string): Record<string, unknown>[] {
      return events
        .filter((event) => event.type === type)
        .map((event) =>
          Object.fromEntries(Object.entries(event).filter(([key]) => key !== "t_ms")),
        );
    },
    /** Runs the frame callbacks the game asked for, at `time`. */
    animationFrame(time: number): void {
      const callbacks = animationFrames.splice(0);
      for (const callback of callbacks) callback(time);
    },
    everySecond(): void {
      for (const callback of intervals) callback();
    },
    message(data: unknown, source: unknown = context.parent): void {
      window.dispatch("message", { source, data });
    },
    /** A new canvas and its WebGL context. */
    webgl(kind = "webgl", attributes?: Attributes) {
      const canvas = new HTMLCanvasElement();
      const gl = canvas.getContext(kind, attributes);
      if (!(gl instanceof FakeGl)) throw new Error(`${kind} is not a WebGL context`);
      return { canvas, gl };
    },
    /** A fetched response, which the page did not construct. */
    fetched(url: string): Response {
      return Object.assign(Object.create(Response.prototype) as Response, { url });
    },
  };
}

type Frame = ReturnType<typeof frame>;

/** A texture bound to `gl`'s 2D target, or to its cube map. */
function bound(gl: FakeGl, target = TEXTURE_2D): object {
  const texture = {};
  gl.bindTexture(target, texture);
  return texture;
}

/** The fields of the last texture upload, without its time. */
function lastUpload(probe: Frame): Record<string, unknown> {
  return at(probe.sent("texture_uploaded"), -1);
}

test("the probe installs once per frame", () => {
  const probe = frame();
  new vm.Script(LAB).runInContext(probe.context);
  probe.context.requestAnimationFrame(() => {});
  probe.animationFrame(0);
  assert.equal(probe.sent("first_frame").length, 1);
});

test("a frame callback still runs, with its time", () => {
  const probe = frame();
  const seen: number[] = [];
  probe.context.requestAnimationFrame((time: number) => seen.push(time));
  probe.animationFrame(16);
  assert.deepEqual(seen, [16]);
});

test("the first frame is sent once, then a histogram of frame times each second", () => {
  const probe = frame();
  for (const time of [0, 16, 50, 100, 2100]) {
    // Two callbacks in one frame share its time, so the frame counts once.
    probe.context.requestAnimationFrame(() => {});
    probe.context.requestAnimationFrame(() => {});
    probe.animationFrame(time);
  }
  probe.everySecond();
  probe.everySecond();
  assert.equal(probe.sent("first_frame").length, 1);
  const [first, second] = probe.sent("frame_times");
  assert.ok(first && second);
  assert.deepEqual(first.bucket_upper_ms, [17, 34, 50, 100, 250, 1000, 4294967295]);
  // 16 ms, 34 ms, 50 ms and 2000 ms.
  assert.deepEqual(first.counts, [1, 1, 1, 0, 0, 0, 1]);
  assert.deepEqual(second.counts, [0, 0, 0, 0, 0, 0, 0]);
});

test("a message from the host page is sent with its name", () => {
  const probe = frame();
  probe.message("reels:active");
  probe.message({ type: "reels:inactive" });
  probe.message({ name: "reels:spin" });
  assert.deepEqual(
    probe.sent("host_message").map((event) => event.name),
    ["reels:active", "reels:inactive", "reels:spin"],
  );
  assert.equal(at(probe.sent("host_message"), 0).direction, "from_host");
});

test("a message with no name, or not from the host page, is not sent", () => {
  const probe = frame();
  probe.message({ other: 1 });
  probe.message(null);
  probe.message("reels:active", {});
  assert.deepEqual(probe.sent("host_message"), []);
});

test("a frame that is its own parent sends no host messages", () => {
  const probe = frame({ topLevel: true });
  probe.message("reels:active");
  assert.deepEqual(probe.sent("host_message"), []);
});

test("decoded audio is sent with its length, channels and rate, and the game still gets it", async () => {
  const probe = frame();
  const audio = new probe.context.AudioContext();
  const given: { length: number }[] = [];
  const promised = await audio.decodeAudioData(
    new ArrayBuffer(4800),
    (buffer: { length: number }) => given.push(buffer),
  );
  assert.deepEqual(probe.sent("audio_decoded"), [
    { type: "audio_decoded", length: 4800, channels: 2, sample_rate: 48000 },
  ]);
  assert.equal(at(given, 0).length, 4800);
  assert.equal(promised, at(given, 0));
});

test("audio decoded without a callback is still sent", async () => {
  const probe = frame();
  await new probe.context.AudioContext().decodeAudioData(new ArrayBuffer(10));
  assert.equal(probe.sent("audio_decoded").length, 1);
});

test("each audio context's state is sent when it is made and when it changes", () => {
  const probe = frame();
  const audio = new probe.context.webkitAudioContext();
  audio.state = "running";
  audio.dispatch("statechange");
  assert.ok(audio instanceof probe.context.BaseAudioContext);
  assert.deepEqual(
    probe.sent("audio_context_state").map((event) => [event.state, event.sample_rate]),
    [
      ["suspended", 48000],
      ["running", 48000],
    ],
  );
});

test("a rendering context is sent once per canvas, with its size, antialias and pixel ratio", () => {
  const probe = frame();
  const { canvas } = probe.webgl("webgl", { antialias: false });
  canvas.getContext("webgl");
  assert.deepEqual(probe.sent("context_created"), [
    {
      type: "context_created",
      canvas: 0,
      kind: "webgl",
      width: 300,
      height: 150,
      antialias: false,
      device_pixel_ratio: 3,
    },
  ]);
});

test("each canvas gets the next number, and a 2D context has no antialias", () => {
  const probe = frame();
  probe.webgl("webgl2");
  new probe.context.HTMLCanvasElement().getContext("2d");
  assert.deepEqual(
    probe.sent("context_created").map((event) => [event.canvas, event.kind, event.antialias]),
    [
      [0, "webgl2", true],
      [1, "canvas2d", null],
    ],
  );
});

test("a context of another kind, or none, is not sent", () => {
  const probe = frame();
  const canvas = new probe.context.HTMLCanvasElement();
  canvas.getContext("bitmaprenderer");
  canvas.getContext("webgpu");
  assert.deepEqual(probe.sent("context_created"), []);
});

test("a resize of a canvas with a context is sent; one without is not", () => {
  const probe = frame();
  const { canvas } = probe.webgl();
  canvas.width = 1206;
  canvas.height = 2622;
  new probe.context.HTMLCanvasElement().width = 10;
  assert.deepEqual(
    probe.sent("canvas_resized").map((event) => [event.canvas, event.width, event.height]),
    [
      [0, 1206, 150],
      [0, 1206, 2622],
    ],
  );
  assert.equal(canvas.width, 1206);
});

test("a lost WebGL context is sent", () => {
  const probe = frame();
  probe.webgl().canvas.dispatch("webglcontextlost");
  assert.equal(probe.sent("context_lost").length, 1);
});

test("an upload from pixels is width x height x the bytes per pixel of its format and type", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  const sizes: [number, number, number][] = [
    [RGBA, UNSIGNED_BYTE, 4],
    [RGB, UNSIGNED_BYTE, 3],
    [LUMINANCE, UNSIGNED_BYTE, 1],
    [LUMINANCE_ALPHA, UNSIGNED_BYTE, 2],
    [RGBA, FLOAT, 16],
    [RGB, HALF_FLOAT, 6],
    [RGBA, HALF_FLOAT_OES, 8],
    [RGBA, UNSIGNED_SHORT_4_4_4_4, 2],
    [RGBA, UNSIGNED_INT, 4],
    [0x1234, UNSIGNED_BYTE, 4],
  ];
  for (const [format, type, bytesPerPixel] of sizes) {
    gl.texImage2D(TEXTURE_2D, 0, format, 64, 32, 0, format, type, new Uint8Array(1));
    assert.equal(lastUpload(probe).bytes, 64 * 32 * bytesPerPixel, `format ${format} type ${type}`);
  }
});

test("an upload names its texture, face and level, and a pixel upload has no source", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  gl.texImage2D(TEXTURE_2D, 2, RGBA, 8, 8, 0, RGBA, UNSIGNED_BYTE, new Uint8Array(256));
  assert.deepEqual(lastUpload(probe), {
    type: "texture_uploaded",
    texture: 0,
    face: 0,
    level: 2,
    width: 8,
    height: 8,
    bytes: 256,
    source: null,
  });
});

test("a re-upload keeps the texture's number, and another texture gets the next one", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  const first = bound(gl);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, new Uint8Array(64));
  bound(gl);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, new Uint8Array(64));
  gl.bindTexture(TEXTURE_2D, first);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 8, 8, 0, RGBA, UNSIGNED_BYTE, new Uint8Array(256));
  assert.deepEqual(
    probe.sent("texture_uploaded").map((event) => event.texture),
    [0, 1, 0],
  );
});

test("an upload with no texture bound has no texture number", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, new Uint8Array(64));
  assert.equal(lastUpload(probe).texture, null);
});

test("an upload of no size is not sent", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 0, 4, 0, RGBA, UNSIGNED_BYTE, null);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, { width: 0, height: 0 });
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, null);
  assert.deepEqual(probe.sent("texture_uploaded"), []);
});

test("an upload from an image uses its natural size and the path of its URL, never the query", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  const image = {
    naturalWidth: 64,
    naturalHeight: 32,
    width: 1,
    height: 1,
    src: "https://cdn.game.test/img/a.png?token=secret",
  };
  gl.texImage2D(TEXTURE_2D, 0, RGB, RGB, UNSIGNED_BYTE, image);
  assert.deepEqual(
    [
      lastUpload(probe).width,
      lastUpload(probe).height,
      lastUpload(probe).bytes,
      lastUpload(probe).source,
    ],
    [64, 32, 64 * 32 * 3, "/img/a.png"],
  );
});

test("a video, a frame or a plain image gives its own size", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  for (const source of [
    { videoWidth: 2, videoHeight: 3 },
    { displayWidth: 4, displayHeight: 5 },
    { width: 6, height: 7 },
  ]) {
    gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, source);
  }
  assert.deepEqual(
    probe.sent("texture_uploaded").map((event) => [event.width, event.height]),
    [
      [2, 3],
      [4, 5],
      [6, 7],
    ],
  );
});

test("an image that is not from http has no source", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, {
    width: 4,
    height: 4,
    src: "data:image/png;base64,AAAA",
  });
  assert.equal(lastUpload(probe).source, null);
});

test("a cube face upload names its face", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl, CUBE_MAP);
  gl.texImage2D(CUBE_FACE_FIRST + 2, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, new Uint8Array(64));
  assert.deepEqual([lastUpload(probe).texture, lastUpload(probe).face], [0, 2]);
});

test("texStorage2D counts every level of its sized format", () => {
  const probe = frame();
  const { gl } = probe.webgl("webgl2");
  bound(gl);
  // R8: 8 x 4, 4 x 2 and 2 x 1 at one byte a pixel.
  gl.texStorage2D(TEXTURE_2D, 3, R8, 8, 4);
  assert.deepEqual(
    [lastUpload(probe).bytes, lastUpload(probe).level, lastUpload(probe).face],
    [32 + 8 + 2, null, 0],
  );
});

test("texStorage2D of a format it does not know counts 32 bits a pixel", () => {
  const probe = frame();
  const { gl } = probe.webgl("webgl2");
  bound(gl);
  gl.texStorage2D(TEXTURE_2D, 1, 0x1234, 4, 4);
  assert.equal(lastUpload(probe).bytes, 64);
});

test("texStorage2D of a cube map counts all six faces and names no face", () => {
  const probe = frame();
  const { gl } = probe.webgl("webgl2");
  bound(gl, CUBE_MAP);
  gl.texStorage2D(CUBE_MAP, 1, RGBA8, 16, 16);
  assert.deepEqual([lastUpload(probe).bytes, lastUpload(probe).face], [16 * 16 * 4 * 6, null]);
});

test("an upload to an immutable WebGL2 texture allocates nothing, so it is not sent", () => {
  const probe = frame();
  const { gl } = probe.webgl("webgl2");
  bound(gl);
  gl.texStorage2D(TEXTURE_2D, 1, RGBA8, 4, 4);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, new Uint8Array(64));
  assert.equal(probe.sent("texture_uploaded").length, 1);
});

test("a compressed upload states the bytes of its data", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  gl.compressedTexImage2D(TEXTURE_2D, 0, 0x93b0, 64, 64, 0, new Uint8Array(4096));
  gl.compressedTexImage2D(TEXTURE_2D, 1, 0x93b0, 32, 32, 0, 1024);
  gl.compressedTexImage2D(TEXTURE_2D, 2, 0x93b0, 16, 16, 0);
  assert.deepEqual(
    probe.sent("texture_uploaded").map((event) => event.bytes),
    [4096, 1024, 0],
  );
});

test("a deleted texture is sent by its number; one never uploaded to is not", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  const texture = bound(gl);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, new Uint8Array(64));
  gl.deleteTexture({});
  gl.deleteTexture(null);
  gl.deleteTexture(texture);
  assert.deepEqual(probe.sent("texture_deleted"), [{ type: "texture_deleted", texture: 0 }]);
});

test("the lab build calls an upload with no pixels empty, and one from a canvas a canvas", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, null);
  const canvas = new probe.context.HTMLCanvasElement();
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, canvas);
  // An object with a canvas's fields is not a canvas.
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, { width: 300, height: 150 });
  assert.deepEqual(
    probe.sent("texture_uploaded").map((event) => event.source_kind),
    ["empty", "canvas", undefined],
  );
});

test("the lab build gives no kind to an upload that has a source", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, {
    width: 4,
    height: 4,
    src: "https://cdn.game.test/a.png",
  });
  assert.equal(lastUpload(probe).source_kind, undefined);
});

test("a bitmap from a fetched blob carries the URL of the fetch", async () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  const blob = await probe.fetched("https://cdn.game.test/img/b.png").blob();
  const bitmap = await probe.context.createImageBitmap(blob);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, bitmap);
  assert.equal(lastUpload(probe).source, "/img/b.png");
});

test("an object URL of a blob stands for the blob's file", async () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  const blob = await probe.fetched("https://cdn.game.test/img/c.png").blob();
  const objectUrl = probe.context.URL.createObjectURL(blob);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, { width: 4, height: 4, src: objectUrl });
  assert.equal(lastUpload(probe).source, "/img/c.png");
});

test("a response made from a blob carries the blob's URL", async () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  const blob = await probe.fetched("https://cdn.game.test/img/d.png").blob();
  const response = new probe.context.Response(blob);
  const bitmap = await probe.context.createImageBitmap(await response.blob());
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, bitmap);
  assert.equal(lastUpload(probe).source, "/img/d.png");
});

test("values that carry no URL are passed through as they are", async () => {
  const probe = frame();
  const blob = await new probe.context.Response("text").blob();
  assert.deepEqual(blob, { blob: true });
  assert.match(probe.context.URL.createObjectURL(blob), /^blob:/);
  const bitmap = await probe.context.createImageBitmap(null);
  assert.equal(bitmap.width, 4);
});

test("a worker's reply carries the URL its request named", () => {
  const probe = frame();
  const { gl } = probe.webgl();
  bound(gl);
  const worker = new probe.context.Worker("loader.js");
  worker.postMessage({ data: ["img/e.png"], uuid: 7 });
  const bitmap = { width: 4, height: 4 };
  worker.dispatch("message", { data: { data: bitmap, uuid: 7 } });
  gl.texImage2D(TEXTURE_2D, 0, RGBA, RGBA, UNSIGNED_BYTE, bitmap);
  assert.equal(lastUpload(probe).source, "/game/img/e.png");
  assert.deepEqual(worker.posted, [{ data: ["img/e.png"], uuid: 7 }]);
});

test("a worker message that names no URL, or a bad one, is still posted", () => {
  const probe = frame();
  const worker = new probe.context.Worker("loader.js");
  const messages = [
    { data: ["http://["], uuid: 1 },
    { data: "not a list", uuid: 2 },
    { data: ["a.png"] },
    null,
  ];
  for (const message of messages) worker.postMessage(message);
  worker.dispatch("message", { data: { data: {}, uuid: 1 } });
  worker.dispatch("message", { data: null });
  assert.deepEqual(worker.posted, messages);
});

test("the field build sends the same uploads with no source kind", () => {
  const probe = frame({ run: FIELD });
  const { gl } = probe.webgl();
  bound(gl);
  gl.texImage2D(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, null);
  assert.deepEqual(lastUpload(probe), {
    type: "texture_uploaded",
    texture: 0,
    face: 0,
    level: 0,
    width: 4,
    height: 4,
    bytes: 64,
    source: null,
  });
});

test("a served build's probe posts its document, then each event, to the harness page", () => {
  const probe = frame({ driver: false, script: "relay" });
  probe.context.requestAnimationFrame(() => {});
  probe.animationFrame(16);
  // The posts are the frame's own objects, so they are compared as JSON.
  assert.deepEqual(JSON.parse(JSON.stringify(probe.posted)), [
    { dyno: "document", time_origin: 1_000, device_pixel_ratio: 3 },
    { dyno: "probe", payload: JSON.stringify({ type: "first_frame", t_ms: 0 }) },
  ]);
});

test("the probe posts nothing without the relay tag, inside another frame, or at the top", () => {
  for (const options of [
    { script: "plain" as const },
    { script: "relay" as const, nested: true },
    { script: "relay" as const, topLevel: true },
  ]) {
    const probe = frame({ driver: false, ...options });
    probe.context.requestAnimationFrame(() => {});
    probe.animationFrame(16);
    assert.deepEqual(probe.posted, [], JSON.stringify(options));
  }
});

test("with no driver listening, the game runs as before", () => {
  const probe = frame({ driver: false });
  let ran = false;
  probe.context.requestAnimationFrame(() => (ran = true));
  probe.animationFrame(0);
  probe.webgl();
  assert.ok(ran);
  assert.deepEqual(probe.events, []);
});
