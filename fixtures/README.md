# Static scan fixtures

Synthetic game builds for testing the static scan in `crates/dyno-static`.
Every file here is made by `generate.sh` from our own sources. No third-party
game build is in this folder. The formulas that the expected values follow are
in [`docs/design/static-scan.md`](../docs/design/static-scan.md).

## Layout

```
fixtures/
  generate.sh               rebuilds every build folder
  src/                      inputs for the generator (our own code)
    apps/*.js               tiny fixture apps, bundled with esbuild
    html/*                  index.html templates and app.css
    tools/fixture_tool.rs   std-only Rust helper (KTX2 writer, Opus header patch, fake font)
    package.json            pinned npm versions for the bundles
    package-lock.json
  <name>/
    build/                  the scan input (a build folder)
    expected.toml           the values the formulas must give for build/
    THIRD_PARTY_NOTICES.txt licences of npm code inside the bundle, if any
  zip-input/
    build.zip               the scan input is a zip, not a folder
    expected.toml
```

## Fixtures

| Fixture | What it covers | Rules that fire at the default thresholds |
|---|---|---|
| `audio-formats` | WAV, Ogg Vorbis, Ogg Opus (with a 44.1 kHz input rate field and a 48 kHz mono file), MP3 CBR, MP3 VBR (MPEG-2), MP3 without a Xing header, AAC in ADTS, AAC in M4A (moov first and moov last), WebM Opus, live WebM with no Duration. Stereo and mono. | none (render is `incomplete`: no JS) |
| `howler-sprites` | A Howler sprite map as a literal in the bundle, a Howler JSON map and an audiosprite (jukebox) JSON map, with known silent gaps. Sound groups of 2 formats. No canvas, so it draws with HTML: Howler only plays audio. | `audio.silence_share` warns |
| `images` | PNG (RGBA, RGB, palette, 16-bit grey), JPEG (baseline, progressive), WebP (lossy, lossless, alpha), AVIF, GIF, SVG, KTX2 (raw RGBA8 with mips, ASTC 4x4, Basis ETC1S with mips, Basis UASTC with Zstandard), `.basis` (ETC1S), a Spine atlas with 2 pages, one with a MipMap filter. | textures `incomplete` (SVG is not covered) |
| `pixi8-inline` | Pixi 8.22.0 and Howler 2.2.4 bundled into an inline module script in `index.html`, the way SvelteKit inline builds do. `app.init` with antialias on and resolution `devicePixelRatio`. web-sdk markers (RGS paths, `rgs_url`, a `__sveltekit_` global). External URLs in JS, HTML and CSS, and one dynamic host. A fake font and a short video. | render `warn` |
| `pixi8-webgpu` | Pixi 8.22.0 with `preference: 'webgpu'`, antialias off, resolution capped at 2 through a `const`. A Basis KTX2 texture with mips. A protocol-relative URL. | none |
| `pixi7-aa` | Pixi 7.4.3 `new Application` with antialias on, resolution uncapped, `resizeTo`. Power of two images, so v7 mipmaps apply. | render `warn` |
| `pixi7-opts` | Pixi 7.4.3 with the options object in a variable, no antialias key (default off), resolution `window.devicePixelRatio \|\| 1`, a fixed 720 x 1280 stage. | render `warn` |
| `duplicates` | The same PNG three times, an MP3 twice, a JSON file twice, two empty files (they never count). A page with no script, so it draws with HTML. | `files.duplicate` warns |
| `broken` | One damaged file per family: PNG, JPEG, WebP, AVIF, KTX2, Basis, Ogg Vorbis, Ogg Opus, MP3, ADTS, M4A, WAV, WebM, a JS file with a syntax error and a sprite JSON that does not parse. Two good files keep the totals above zero. | audio, stereo, silence, render and textures are `incomplete` |
| `zip-input` | `duplicates/build` zipped under one top-level folder `game/`. The scan strips that folder, so every value, `build_id` included, equals `duplicates`. | `files.duplicate` warns |
| `over-budget` | 6 minutes of silent stereo Vorbis (36 KB, 138 MB decoded) and flat 4096 x 4096 PNGs. | audio `fail`, stereo `warn`, textures `fail` |

The fixture apps use hosts under `.invalid`, so nothing in them can reach a real
server. The web-sdk markers in `pixi8-inline` are our own code. They use the
same public RGS path names and query parameter name that a web-sdk build
contains, and nothing else from that project.

`pixi8-inline/build/assets/fonts/fixture.woff2` is not a font. It is the
`wOF2` magic followed by zeros, so the byte buckets see a font file.

## expected.toml

Each file has one top-level table per module, so a module's tests read only
their own table:

| Table | Owner (see the design doc) |
|---|---|
| `[profile]` | shared: the proposed device profile the numbers assume |
| `[inventory]`, `[bytes]`, `[duplicates]`, `[rules]` | inventory, bytes and rules |
| `[audio]`, `[sprites]` | audio |
| `[textures]` | textures |
| `[bundle]` | bundle |

All sizes are integer bytes. File lists are sorted by path in byte order. A
Howler `src` list keeps its written order. A rule's `items` put the most bytes
saved first, then sort by path. Statuses use the plan's words:
`estimated`, `unsupported`, `missing`, `invalid`. Rule outcomes are `pass`,
`warn`, `fail` and `incomplete`.

The gzip and brotli sizes in `[bytes]` are exact for flate2 1.1.10 (miniz_oxide
backend, level 9) and brotli 9.1.1 (quality 11, window 22). If either crate
changes its output, tests may compare those two numbers with a 2% tolerance.
The decode values in `[audio]` (`silence_bp`, `stereo_class` and
`silence_by_decode_bp`) are exact for symphonia 0.6.1. Every other number must
match exactly.

## Rebuild

```sh
brew install ffmpeg vorbis-tools basis_universal   # ffmpeg brings webp, jpeg-turbo, lame, opus, svt-av1, x264
fixtures/generate.sh                # all fixtures
fixtures/generate.sh images broken  # or only some
git status fixtures
```

The generator needs bash, ffmpeg, oggenc, cwebp, jpegtran, basisu, rustc, npm,
npx, node, zip and shasum. It works in a temporary folder outside the repo and
deletes it on exit, `node_modules` included. It only deletes and rewrites
`fixtures/<name>/build` (and `zip-input/build.zip`). It never touches
`expected.toml`.

The output is byte-stable. Two full runs on the same tools give the same
SHA-256 for every file: ffmpeg runs with `-bitexact` and no metadata, Vorbis and
Opus streams have fixed serial numbers, the zip uses fixed times and no extra
fields, and npm installs from the lock file.

Tool versions used for the committed files:

| Tool | Version |
|---|---|
| ffmpeg | 9.0.2 (libopus 1.6.1, LAME 4.0, SVT-AV1 4.3.0, x264 r3222, native AAC) |
| oggenc (vorbis-tools) | 1.4.3 (libvorbis 1.3.7) |
| cwebp (webp) | 1.6.0 |
| jpegtran (jpeg-turbo) | 3.2.0 |
| basisu (basis_universal) | 2.50.0 |
| rustc | 1.99.1 |
| node, npm | 26.12.1, 11.20.0 |
| esbuild | 0.28.2 (from `src/package.json`) |
| pixi.js | 7.4.3 and 8.22.0; howler 2.2.4 |

A different version of any of these can change the bytes. After a rebuild,
if `git status fixtures` shows a changed file under `build/`, the
`expected.toml` of that fixture is stale. Re-derive its values from the
formulas in the design doc. The committed values were derived with a scratch
reference reader written to the doc's rules, and cross-checked against
symphonia 0.6.1, imagesize 0.15.0, the ktx2 0.5.0 crate and ffprobe. The
differences found in that cross-check are listed in the design doc.

## Licences

The bundles contain open-source npm code (pixi.js, howler and their MIT, ISC
and similar dependencies). Each fixture with a bundle has a
`THIRD_PARTY_NOTICES.txt` with the full licence of every bundled package. The
generator writes it from the esbuild metafile and the packages' own licence
files. Everything else in `fixtures/` is ours.

## Size

All fixtures together are about 4.1 MB. Keep them under 5 MB. Flat colours and
digital silence give large decoded sizes from small files.
