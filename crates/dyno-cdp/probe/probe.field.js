// Generated from probe.ts by build.mjs. Do not edit.
;
(() => {
"use strict";
const installed = Symbol.for("dyno.probe");
const scope = globalThis ;
if (scope[installed]) return;
Object.defineProperty(scope, installed, { value: true });
const now = () => performance.now();
const send = (event ) => {
try {
(scope.__dynoProbe )(JSON.stringify(event));
} catch {
}
};
const wrap = (owner , name , around ) => {
if (!owner) return;
const original = (owner )[name];
if (typeof original === "function") (owner )[name] = around(original);
};
const BUCKETS_MS = [17, 34, 50, 100, 250, 1000, 4294967295];
const EVERY_MS = 1000;
let counts = BUCKETS_MS.map(() => 0);
let lastFrame = -1;
const counted = (time ) => {
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
wrap(scope, "requestAnimationFrame", (original) => function ( callback ) {
return original.call(this, (time ) => {
counted(time);
callback(time);
});
});
setInterval(() => {
send({ type: "frame_times", bucket_upper_ms: BUCKETS_MS, counts, t_ms: now() });
counts = BUCKETS_MS.map(() => 0);
}, EVERY_MS);
addEventListener("message", (event ) => {
if (event.source !== parent || parent === window) return;
const data = event.data;
const name = typeof data === "string" ? data : data && (data.type || data.name);
if (typeof name === "string") send({ type: "host_message", direction: "from_host", name, t_ms: now() });
});
const decoded = (buffer ) => {
send({
type: "audio_decoded",
length: buffer.length,
channels: buffer.numberOfChannels,
sample_rate: buffer.sampleRate,
t_ms: now(),
});
};
wrap(scope.BaseAudioContext && (scope.BaseAudioContext ).prototype, "decodeAudioData",
(original) => function ( data , onSuccess , onError ) {
const success = (buffer ) => {
decoded(buffer);
if (onSuccess) onSuccess(buffer);
};
const promise = original.call(this, data, success, onError) ;
return promise;
});
const watchContext = (context ) => {
const report = () => {
send({ type: "audio_context_state", state: context.state , sample_rate: context.sampleRate, t_ms: now() });
};
report();
context.addEventListener("statechange", report);
};
for (const name of ["AudioContext", "webkitAudioContext"]) {
const Original = scope[name] ;
if (typeof Original !== "function") continue;
const Watched = new Proxy(Original, {
construct(target, args, newTarget) {
const context = Reflect.construct(target, args, newTarget) ;
watchContext(context);
return context;
},
});
scope[name] = Watched;
}
const kinds = {
webgl: "webgl",
"experimental-webgl": "webgl",
webgl2: "webgl2",
webgpu: "webgpu",
"2d": "canvas2d",
};
const numbers = new WeakMap ();
let nextNumber = 0;
const aroundGetContext = (original ) => function ( kind , ...rest ) {
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
const owner = scope[name] ;
if (!owner) continue;
wrap(owner.prototype, "getContext", aroundGetContext);
for (const side of ["width", "height"]) {
const property = Object.getOwnPropertyDescriptor(owner.prototype, side);
const set = property && property.set;
if (!set) continue;
Object.defineProperty(owner.prototype, side, {
...property,
set( value ) {
set.call(this, value);
const canvas = numbers.get(this);
if (canvas !== undefined) send({ type: "canvas_resized", canvas, width: this.width, height: this.height, t_ms: now() });
},
});
}
}
const sourceUrls = new WeakMap ();
const objectUrls = new Map ();
let kindOf = (..._ ) => {};
const UNSIGNED_BYTE = 0x1401;
const FLOAT = 0x1406;
const HALF_FLOATS = [0x140b, 0x8d61];
const PACKED = [0x8033, 0x8034, 0x8363];
const COMPONENTS = { 0x1906: 1, 0x1909: 1, 0x1903: 1, 0x190a: 2, 0x8227: 2, 0x1907: 3, 0x1908: 4 };
const bytesPerPixel = (format , type ) => {
if (PACKED.includes(type)) return 2;
const components = COMPONENTS[format] ?? 4;
if (type === FLOAT) return components * 4;
if (HALF_FLOATS.includes(type)) return components * 2;
if (type === UNSIGNED_BYTE) return components;
return 4;
};
const sizeOf = (source ) => {
const image = source ;
if (!image) return [0, 0];
const width = image.naturalWidth || image.videoWidth || image.displayWidth || image.width || 0;
const height = image.naturalHeight || image.videoHeight || image.displayHeight || image.height || 0;
return [width, height];
};
const STORAGE_BITS = {
0x8229: 8, 0x822b: 16, 0x8051: 24, 0x8058: 32, 0x8c43: 32, 0x8d62: 16, 0x8056: 16, 0x8057: 16,
0x8059: 32, 0x8c3a: 32, 0x822d: 16, 0x822f: 32, 0x881a: 64, 0x881b: 48, 0x822e: 32, 0x8230: 64,
0x8814: 128, 0x8815: 96, 0x81a5: 16, 0x81a6: 32, 0x88f0: 32, 0x8cac: 32,
0x93b0: 8, 0x93d0: 8, 0x9274: 4, 0x9278: 8,
};
const textureNumbers = new WeakMap ();
let nextTexture = 0;
const TEXTURE_2D = 0x0de1;
const CUBE_MAP = 0x8513;
const CUBE_FACE_FIRST = 0x8515;
const CUBE_FACE_LAST = 0x851a;
const BINDING_2D = 0x8069;
const BINDING_CUBE_MAP = 0x8514;
const IMMUTABLE_FORMAT = 0x912f;
const WebGL2 = scope.WebGL2RenderingContext ;
const uploaded = (gl , target , level , width , height , bytes , image ) => {
if (!(width > 0 && height > 0)) return;
const isFace = target >= CUBE_FACE_FIRST && target <= CUBE_FACE_LAST;
const isCube = isFace || target === CUBE_MAP;
const bound = gl.getParameter(isCube ? BINDING_CUBE_MAP : BINDING_2D) ;
if (bound && WebGL2 && gl instanceof WebGL2 && gl.getTexParameter(isCube ? CUBE_MAP : TEXTURE_2D, IMMUTABLE_FORMAT)) return;
let texture = null;
if (bound) {
texture = textureNumbers.get(bound) ?? nextTexture++;
textureNumbers.set(bound, texture);
}
const named = image && (sourceUrls.get(image ) || (image ).currentSrc || (image ).src);
const url = typeof named === "string" ? objectUrls.get(named) || named : null;
const source = url && /^https?:/.test(url) ? new URL(url).pathname : null;
const face = isFace ? target - CUBE_FACE_FIRST : isCube ? null : 0;
send({ type: "texture_uploaded", texture, face, level, width, height, bytes, source, source_kind: kindOf(source, image), t_ms: now() });
};
for (const name of ["WebGLRenderingContext", "WebGL2RenderingContext"]) {
const owner = scope[name] ;
const prototype = owner && owner.prototype;
wrap(prototype, "texImage2D", (original) => function ( ...args ) {
const [target, level] = [args[0] , args[1] ];
if (args.length >= 9) {
const [width, height] = [args[3] , args[4] ];
const image = args[8] == null ? null : typeof args[8] === "object" && !ArrayBuffer.isView(args[8]) ? args[8] : undefined;
uploaded(this , target, level, width, height, width * height * bytesPerPixel(args[6] , args[7] ), image);
} else if (args.length === 6) {
const [width, height] = sizeOf(args[5]);
uploaded(this , target, level, width, height, width * height * bytesPerPixel(args[3] , args[4] ), args[5]);
}
return original.apply(this, args);
});
wrap(prototype, "compressedTexImage2D", (original) => function ( ...args ) {
const data = args[6] ;
const bytes = typeof data === "number" ? data : data ? data.byteLength : 0;
uploaded(this , args[0] , args[1] , args[3] , args[4] , bytes);
return original.apply(this, args);
});
wrap(prototype, "texStorage2D", (original) => function ( ...args ) {
const [levels, format, width, height] = [args[1] , args[2] , args[3] , args[4] ];
const bits = STORAGE_BITS[format] ?? 32;
let bytes = 0;
for (let level = 0; level < levels; level++) bytes += Math.ceil((Math.max(1, width >> level) * Math.max(1, height >> level) * bits) / 8);
const faces = args[0] === CUBE_MAP ? 6 : 1;
uploaded(this , args[0] , null, width, height, bytes * faces, null);
return original.apply(this, args);
});
wrap(prototype, "deleteTexture", (original) => function ( texture ) {
const number = texture ? textureNumbers.get(texture) : undefined;
if (number !== undefined) send({ type: "texture_deleted", texture: number, t_ms: now() });
return original.call(this, texture);
});
}
})();
