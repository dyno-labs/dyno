// The dyno probe. It runs in the game frame before any game script, records raw facts as
// `ProbeEvent`s and never scores: Rust turns the events into measures.
//
// It wraps only calls that run at load or once per frame, never per-draw calls. Each event goes
// to the `__dynoProbe` binding that the driver adds to the frame. Build `probe.js` from this file
// with `node crates/dyno-cdp/probe/build.mjs`; it only strips the types.

import type { ProbeEvent } from "../../../ts/ProbeEvent";
import type { ContextKind } from "../../../ts/ContextKind";
import type { AudioState } from "../../../ts/AudioState";
import type { SourceKind } from "../../../ts/SourceKind";

(() => {
  "use strict";
  const installed = Symbol.for("dyno.probe");
  const scope = globalThis as unknown as Record<PropertyKey, unknown>;
  if (scope[installed]) return;
  Object.defineProperty(scope, installed, { value: true });

  const now = (): number => performance.now();
  const send = (event: ProbeEvent): void => {
    try {
      (scope.__dynoProbe as (payload: string) => void)(JSON.stringify(event));
    } catch {
      // No driver is listening.
    }
  };
  const wrap = (owner: object | undefined, name: string, around: (original: Function) => Function): void => {
    if (!owner) return;
    const original = (owner as Record<string, unknown>)[name];
    if (typeof original === "function") (owner as Record<string, unknown>)[name] = around(original);
  };

  // Frames: the first requestAnimationFrame callback that runs, then a histogram of frame times
  // every second. Callbacks of one frame share its time, so a frame counts once.
  // The edges and the period must match FRAME_TIME_BUCKETS_MS and FRAME_TIMES_EVERY_MS in Rust.
  const BUCKETS_MS = [17, 34, 50, 100, 250, 1000, 4294967295];
  const EVERY_MS = 1000;
  let counts = BUCKETS_MS.map(() => 0);
  let lastFrame = -1;
  const counted = (time: number): void => {
    if (time === lastFrame) return;
    if (lastFrame < 0) {
      send({ type: "first_frame", t_ms: now() });
    } else {
      const took = time - lastFrame;
      const bucket = BUCKETS_MS.findIndex((upper) => took <= upper);
      counts[bucket < 0 ? counts.length - 1 : bucket]++;
    }
    lastFrame = time;
  };
  wrap(scope, "requestAnimationFrame", (original) => function (this: unknown, callback: FrameRequestCallback) {
    return original.call(this, (time: number) => {
      counted(time);
      callback(time);
    });
  });
  setInterval(() => {
    send({ type: "frame_times", bucket_upper_ms: BUCKETS_MS, counts, t_ms: now() });
    counts = BUCKETS_MS.map(() => 0);
  }, EVERY_MS);

  // Messages from the host page, such as `reels:active`.
  addEventListener("message", (event: MessageEvent) => {
    if (event.source !== parent || parent === window) return;
    const data = event.data;
    const name = typeof data === "string" ? data : data && (data.type || data.name);
    if (typeof name === "string") send({ type: "host_message", direction: "from_host", name, t_ms: now() });
  });

  // Decoded audio is held at the context's rate: length x channels x 4 bytes.
  const decoded = (buffer: AudioBuffer): void => {
    send({
      type: "audio_decoded",
      length: buffer.length,
      channels: buffer.numberOfChannels,
      sample_rate: buffer.sampleRate,
      t_ms: now(),
    });
  };
  wrap(scope.BaseAudioContext && (scope.BaseAudioContext as { prototype: object }).prototype, "decodeAudioData",
    (original) => function (this: unknown, data: ArrayBuffer, onSuccess?: (b: AudioBuffer) => void, onError?: (e: unknown) => void) {
      const success = (buffer: AudioBuffer): void => {
        decoded(buffer);
        if (onSuccess) onSuccess(buffer);
      };
      const promise = original.call(this, data, success, onError) as Promise<AudioBuffer> | undefined;
      return promise;
    });

  // Each audio context's state, when it is made and when it changes.
  const watchContext = (context: BaseAudioContext): void => {
    const report = (): void => {
      send({ type: "audio_context_state", state: context.state as AudioState, sample_rate: context.sampleRate, t_ms: now() });
    };
    report();
    context.addEventListener("statechange", report);
  };
  for (const name of ["AudioContext", "webkitAudioContext"]) {
    const Original = scope[name] as (new (...args: unknown[]) => BaseAudioContext) | undefined;
    if (typeof Original !== "function") continue;
    const Watched = new Proxy(Original, {
      construct(target, args, newTarget) {
        const context = Reflect.construct(target, args, newTarget) as BaseAudioContext;
        watchContext(context);
        return context;
      },
    });
    scope[name] = Watched;
  }

  // A rendering context, once per canvas, with its size, antialias and the pixel ratio.
  const kinds: Record<string, ContextKind> = {
    webgl: "webgl",
    "experimental-webgl": "webgl",
    webgl2: "webgl2",
    webgpu: "webgpu",
    "2d": "canvas2d",
  };
  // Each canvas with a context gets a number, so its resizes can follow it.
  const numbers = new WeakMap<object, number>();
  let nextNumber = 0;
  const aroundGetContext = (original: Function) => function (this: HTMLCanvasElement | OffscreenCanvas, kind: string, ...rest: unknown[]) {
    const context = original.call(this, kind, ...rest);
    const known = kinds[kind];
    if (context && known && !numbers.has(this)) {
      const canvas = nextNumber++;
      numbers.set(this, canvas);
      const attributes = typeof context.getContextAttributes === "function" ? context.getContextAttributes() : null;
      send({
        type: "context_created",
        canvas,
        kind: known,
        width: this.width,
        height: this.height,
        antialias: attributes && typeof attributes.antialias === "boolean" ? attributes.antialias : null,
        device_pixel_ratio: devicePixelRatio,
        t_ms: now(),
      });
      this.addEventListener("webglcontextlost", () => send({ type: "context_lost", t_ms: now() }));
    }
    return context;
  };
  for (const name of ["HTMLCanvasElement", "OffscreenCanvas"]) {
    const owner = scope[name] as { prototype: object } | undefined;
    if (!owner) continue;
    wrap(owner.prototype, "getContext", aroundGetContext);
    for (const side of ["width", "height"]) {
      const property = Object.getOwnPropertyDescriptor(owner.prototype, side);
      const set = property && property.set;
      if (!set) continue;
      Object.defineProperty(owner.prototype, side, {
        ...property,
        set(this: HTMLCanvasElement, value: number) {
          set.call(this, value);
          const canvas = numbers.get(this);
          if (canvas !== undefined) send({ type: "canvas_resized", canvas, width: this.width, height: this.height, t_ms: now() });
        },
      });
    }
  }

  // The URL of an image source that carries none, such as an ImageBitmap, and the file each
  // `blob:` object URL stands for. Only the lab build fills them.
  const sourceUrls = new WeakMap<object, string>();
  const objectUrls = new Map<string, string>();
  // What an upload with no source held, from its image: null is no pixels. Only the lab build
  // tells; the field build sends no kind.
  let kindOf = (..._: unknown[]): SourceKind | void => {};
  // lab: begin
  // Facts only: an allocation with no pixels is `empty`, not a render target. A canvas is told by
  // the browser's own `width` getter, taken before game code runs: it accepts only a real canvas,
  // also one made in another frame, whatever the object says its type is. A failed check never
  // stops the upload.
  const canvasWidths = ["HTMLCanvasElement", "OffscreenCanvas"].map((name) => {
    const owner = scope[name] as { prototype: object } | undefined;
    return owner && Object.getOwnPropertyDescriptor(owner.prototype, "width")?.get;
  });
  const isCanvas = (image: unknown): boolean =>
    canvasWidths.some((width) => {
      try {
        return width !== undefined && (width.call(image), true);
      } catch {
        return false;
      }
    });
  kindOf = (source, image) => (source ? undefined : image === null ? "empty" : isCanvas(image) ? "canvas" : undefined);
  // Pixi 8 makes ImageBitmaps from a fetched blob, on this thread or in its own worker, and a
  // game can load a file once and hand Pixi a `blob:` URL or a `new Response(blob)`. So the URL
  // follows each step: a response to its blob, a blob to its bitmap, its object URL or a response
  // made from it. A worker gets the URL in a request `{ data: [url, ...], uuid }`, and its reply
  // with the same uuid holds the bitmap.
  type Named = { currentSrc?: string; src?: string; url?: string };
  const urlOf = (value: unknown): string | undefined => {
    if (!value || typeof value !== "object") return undefined;
    const url = sourceUrls.get(value) || (value as Named).currentSrc || (value as Named).src || (value as Named).url;
    return url && (objectUrls.get(url) || url);
  };
  const Response = scope.Response as { prototype: object } | undefined;
  wrap(Response && Response.prototype, "blob", (original) => function (this: object) {
    const url = urlOf(this);
    return original.call(this).then((blob: Blob) => {
      if (url) sourceUrls.set(blob, url);
      return blob;
    });
  });
  wrap(scope, "Response", (original) => new Proxy(original, {
    construct(target, args, newTarget) {
      const response = Reflect.construct(target, args, newTarget);
      const url = urlOf(args[0]);
      if (url) sourceUrls.set(response, url);
      return response;
    },
  }));
  wrap(scope.URL as object, "createObjectURL", (original) => function (this: unknown, blob: Blob) {
    const objectUrl = original.call(this, blob) as string;
    const url = urlOf(blob);
    if (url) objectUrls.set(objectUrl, url);
    return objectUrl;
  });
  wrap(scope, "createImageBitmap", (original) => function (this: unknown, ...args: unknown[]) {
    const url = urlOf(args[0]);
    return original.apply(this, args).then((bitmap: ImageBitmap) => {
      if (url) sourceUrls.set(bitmap, url);
      return bitmap;
    });
  });
  type WorkerMessage = { data?: unknown; uuid?: unknown } | null;
  wrap(scope, "Worker", (original) => new Proxy(original, {
    construct(target, args, newTarget) {
      const worker = Reflect.construct(target, args, newTarget) as Worker;
      const requested = new Map<unknown, string>();
      // Added before the game's listeners, so the bitmap has its URL when the game gets it.
      worker.addEventListener("message", (event: MessageEvent) => {
        const reply = event.data as WorkerMessage;
        const url = reply && requested.get(reply.uuid);
        if (url && reply.data && typeof reply.data === "object") sourceUrls.set(reply.data, url);
      });
      const post = worker.postMessage;
      worker.postMessage = (message: WorkerMessage, ...rest: unknown[]) => {
        const url = message && Array.isArray(message.data) ? message.data[0] : null;
        if (typeof url === "string" && message.uuid !== undefined) {
          try {
            requested.set(message.uuid, new URL(url, location.href).href);
          } catch {
            // Not a URL.
          }
        }
        return (post as Function).call(worker, message, ...rest);
      };
      return worker;
    },
  }));
  // lab: end

  // Texture uploads, with their bytes as the upload states them.
  const UNSIGNED_BYTE = 0x1401;
  const FLOAT = 0x1406;
  const HALF_FLOATS = [0x140b, 0x8d61];
  const PACKED = [0x8033, 0x8034, 0x8363];
  const COMPONENTS: Record<number, number> = { 0x1906: 1, 0x1909: 1, 0x1903: 1, 0x190a: 2, 0x8227: 2, 0x1907: 3, 0x1908: 4 };
  const bytesPerPixel = (format: number, type: number): number => {
    if (PACKED.includes(type)) return 2;
    const components = COMPONENTS[format] ?? 4;
    if (type === FLOAT) return components * 4;
    if (HALF_FLOATS.includes(type)) return components * 2;
    if (type === UNSIGNED_BYTE) return components;
    return 4;
  };
  const sizeOf = (source: unknown): [number, number] => {
    const image = source as Record<string, number> | null;
    if (!image) return [0, 0];
    const width = image.naturalWidth || image.videoWidth || image.displayWidth || image.width || 0;
    const height = image.naturalHeight || image.videoHeight || image.displayHeight || image.height || 0;
    return [width, height];
  };
  // Bits per pixel of the sized formats texStorage2D takes; another format counts as 32. A
  // compressed format counts by its average bits, without its block padding.
  const STORAGE_BITS: Record<number, number> = {
    0x8229: 8, 0x822b: 16, 0x8051: 24, 0x8058: 32, 0x8c43: 32, 0x8d62: 16, 0x8056: 16, 0x8057: 16,
    0x8059: 32, 0x8c3a: 32, 0x822d: 16, 0x822f: 32, 0x881a: 64, 0x881b: 48, 0x822e: 32, 0x8230: 64,
    0x8814: 128, 0x8815: 96, 0x81a5: 16, 0x81a6: 32, 0x88f0: 32, 0x8cac: 32,
    0x93b0: 8, 0x93d0: 8, 0x9274: 4, 0x9278: 8,
  };
  // Each texture gets a number when it is first uploaded to, from the binding of its target, so
  // a re-upload replaces its level and a delete frees it.
  const textureNumbers = new WeakMap<object, number>();
  let nextTexture = 0;
  const TEXTURE_2D = 0x0de1;
  const CUBE_MAP = 0x8513;
  const CUBE_FACE_FIRST = 0x8515;
  const CUBE_FACE_LAST = 0x851a;
  const BINDING_2D = 0x8069;
  const BINDING_CUBE_MAP = 0x8514;
  const IMMUTABLE_FORMAT = 0x912f;
  const WebGL2 = scope.WebGL2RenderingContext as (new () => WebGL2RenderingContext) | undefined;
  const uploaded = (gl: WebGLRenderingContext, target: number, level: number | null, width: number, height: number, bytes: number, image?: unknown): void => {
    if (!(width > 0 && height > 0)) return;
    const isFace = target >= CUBE_FACE_FIRST && target <= CUBE_FACE_LAST;
    const isCube = isFace || target === CUBE_MAP;
    const bound = gl.getParameter(isCube ? BINDING_CUBE_MAP : BINDING_2D) as object | null;
    // WebGL2 refuses any upload to an immutable texture, so it allocates nothing.
    if (bound && WebGL2 && gl instanceof WebGL2 && gl.getTexParameter(isCube ? CUBE_MAP : TEXTURE_2D, IMMUTABLE_FORMAT)) return;
    let texture: number | null = null;
    if (bound) {
      texture = textureNumbers.get(bound) ?? nextTexture++;
      textureNumbers.set(bound, texture);
    }
    // Only the path of an http image: a query can hold a session token.
    const named = image && (sourceUrls.get(image as object) || (image as HTMLImageElement).currentSrc || (image as HTMLImageElement).src);
    const url = typeof named === "string" ? objectUrls.get(named) || named : null;
    const source = url && /^https?:/.test(url) ? new URL(url).pathname : null;
    // A whole cube map, as texStorage2D allocates it, is every face: no one face.
    const face = isFace ? target - CUBE_FACE_FIRST : isCube ? null : 0;
    send({ type: "texture_uploaded", texture, face, level, width, height, bytes, source, source_kind: kindOf(source, image), t_ms: now() });
  };
  for (const name of ["WebGLRenderingContext", "WebGL2RenderingContext"]) {
    const owner = scope[name] as { prototype: object } | undefined;
    const prototype = owner && owner.prototype;
    wrap(prototype, "texImage2D", (original) => function (this: unknown, ...args: unknown[]) {
      const [target, level] = [args[0] as number, args[1] as number];
      if (args.length >= 9) {
        const [width, height] = [args[3] as number, args[4] as number];
        // WebGL2 also takes an image as the ninth argument. No pixels at all is null.
        const image = args[8] == null ? null : typeof args[8] === "object" && !ArrayBuffer.isView(args[8]) ? args[8] : undefined;
        uploaded(this as WebGLRenderingContext, target, level, width, height, width * height * bytesPerPixel(args[6] as number, args[7] as number), image);
      } else if (args.length === 6) {
        const [width, height] = sizeOf(args[5]);
        uploaded(this as WebGLRenderingContext, target, level, width, height, width * height * bytesPerPixel(args[3] as number, args[4] as number), args[5]);
      }
      return original.apply(this, args);
    });
    wrap(prototype, "compressedTexImage2D", (original) => function (this: unknown, ...args: unknown[]) {
      const data = args[6] as ArrayBufferView | number | undefined;
      const bytes = typeof data === "number" ? data : data ? data.byteLength : 0;
      uploaded(this as WebGLRenderingContext, args[0] as number, args[1] as number, args[3] as number, args[4] as number, bytes);
      return original.apply(this, args);
    });
    wrap(prototype, "texStorage2D", (original) => function (this: unknown, ...args: unknown[]) {
      const [levels, format, width, height] = [args[1] as number, args[2] as number, args[3] as number, args[4] as number];
      const bits = STORAGE_BITS[format] ?? 32;
      let bytes = 0;
      for (let level = 0; level < levels; level++) bytes += Math.ceil((Math.max(1, width >> level) * Math.max(1, height >> level) * bits) / 8);
      // A cube map's storage holds all six faces.
      const faces = args[0] === CUBE_MAP ? 6 : 1;
      uploaded(this as WebGLRenderingContext, args[0] as number, null, width, height, bytes * faces, null);
      return original.apply(this, args);
    });
    wrap(prototype, "deleteTexture", (original) => function (this: unknown, texture: object | null) {
      const number = texture ? textureNumbers.get(texture) : undefined;
      if (number !== undefined) send({ type: "texture_deleted", texture: number, t_ms: now() });
      return original.call(this, texture);
    });
  }
})();
