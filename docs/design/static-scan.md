# Static scan design (Phase 1a)

Status: design for implementation. It covers the static measures and the 6 Phase 1
rules. Values marked **proposal** need a decision or device calibration. Where
this document and the code differ, the code in `crates/` is the contract
([decision R1](#decisions)).

The static scan reads a build folder or zip and reports, from file headers
only: decoded audio, decoded textures, the canvas the renderer will allocate,
engine versions, bytes by type, duplicates, external URLs and a `build_id`.
It never decodes audio or pixels, never runs JS, and gives the same output on
every run.

Test data is in [`fixtures/`](../../fixtures/README.md). Each fixture has an
`expected.toml` with the values the formulas below must give.

## Decisions

These were decided on 2026-11-06 and 2026-11-07.

| Id  | Decision                                                                                                                                                                                                                                                                                                                                                                            |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| R1  | The skeleton is the contract. Ids stay in `dyno-schema/src/ids.rs`, statuses are `MeasureStatus` and the device numbers are `HostProfile`. `model.rs` is a crate-internal module of `dyno-static` that holds the types passing between families, with no second status enum. Where a name differs, `ids.rs` wins (for example `render.antialias_or_resolution`).                    |
| R2  | Byte counts and file hashes are `estimated`, with uncertainty kind `exact`.                                                                                                                                                                                                                                                                                                         |
| R3  | `build_id` hashes every file's path with its SHA-256, so a rename changes it. [Section 3](#3-inventory-and-build-id) gives the bytes.                                                                                                                                                                                                                                               |
| R4  | Audio uses our own header readers for length and channels. `symphonia` (MPL-2.0) decodes each sound group for the silence and stereo analysis ([section 5.4](#54-decoding)), with its files unmodified (2026-11-07; this replaces "a dev-dependency for cross-check tests only"). The readers stay the authority on length, and the tests still cross-check them against symphonia. |
| R5  | Mipmaps use the exact chain where the engine makes mips ([section 7.3](#73-mipmaps)), not a flat third.                                                                                                                                                                                                                                                                             |
| R6  | `HostProfile::reels()` is the one source for the frame: an iPhone 17, 402 x 874 CSS px at DPR 3. Fixture tests pass each fixture's own `[profile]`, and the fixtures use the same frame.                                                                                                                                                                                            |
| R7  | Facts only. dyno reports values read from the build and values worked out from them by fixed arithmetic, never a value that rests on an assumed constant. So the canvas is a floor of one colour buffer ([section 8.4](#84-canvas-bytes)), and the device run measures the antialias buffers and the GPU context.                                                                   |
| R8  | Every threshold is a proposal.                                                                                                                                                                                                                                                                                                                                                      |

`model` is a `pub` module so that the tests in `tests/` can build summaries.
Its types never enter the result. That is how this design reads
"crate-internal" in R1.

## Contents

1. [Shared terms](#1-shared-terms)
2. [Modules and ownership](#2-modules-and-ownership)
3. [Inventory and build id](#3-inventory-and-build-id)
4. [Bytes by type and duplicates](#4-bytes-by-type-and-duplicates)
5. [Audio](#5-audio)
6. [Sprite maps and silence](#6-sprite-maps-and-silence)
7. [Textures](#7-textures)
8. [Bundle analysis](#8-bundle-analysis)
9. [Rules](#9-rules)
10. [Crates](#10-crates)
11. [Cross-check results](#11-cross-check-results)
12. [Open questions](#12-open-questions)
13. [Not covered](#13-not-covered)

## 1. Shared terms

### Units

- Every size is an integer number of bytes (`u64`). MB in text output means
  MiB (2^20 bytes), shown with one decimal, and KB means KiB (1,024 bytes).
  JSON keeps bytes.
- A share is in basis points (1 bp = 0.01%), an integer.
- A resolution or DPR is in milli-units: `resolution 1.5` is `1500`.
- Audio length is `ticks` at `tick_rate` ticks per second, both integers. The
  tick rate is the container's own clock, so no float enters the math.
- Integer division rounds down unless the formula says `ceil` or `round`.
  `round(a / b)` is `(a + b / 2) / b`.

### Statuses

A measure's status is `MeasureStatus` in `dyno-schema`. Every static value is
`estimated`, never `measured`: `measured` is for browser and device readings.

| Status        | Meaning in the static scan                                                                                                                                                                                     |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `estimated`   | The header was read and the formula applied. Byte counts and hashes are `estimated` too, with uncertainty kind `exact` (R2).                                                                                   |
| `unsupported` | The file is a format we know but do not model (for example SVG, FLAC, KTX1). The reason says which.                                                                                                            |
| `missing`     | An input the measure needs is not in the build (for example a sprite map whose audio file is absent, or no render config found). A measure that is not built yet is `missing` with the reason "not built yet". |
| `invalid`     | The file claims to be a format but the header is damaged, or the header promises bytes past the end of the file.                                                                                               |

A per-file status (in a summary or a fixture row) uses the same four words.

### Aggregates and `not_covered`

An aggregate (total decoded audio, total textures) sums only the files with
status `estimated`. It carries status `estimated`. Every other file in its
bucket goes, sorted, in the summary's `not_covered`. The measure's
`scope.not_covered` says the same in plain English, one line per file with its
reason. A rule then decides:

- the sum of the covered files is a lower bound, so if it is already above a
  threshold, the rule fails or warns as normal;
- if it is not above any threshold and `not_covered` is not empty, the rule is
  `incomplete`, never `pass`.

### Rule outcomes

A finding's verdict is `pass`, `warn`, `fail` or `incomplete`. A missing or
invalid input gives `incomplete` (plan: a missing measure is never a pass).

### Profile

The numbers assume one device profile: a standard late-model iPhone, the
iPhone 17, at DPR 3. `HostProfile::reels()` in `dyno-schema/src/profile.rs` is
the one source (R6), at version 1. Code reads the fields; it never repeats the
numbers. The profile holds no assumed cost (R7).

| `[profile]` key in fixtures | `HostProfile` field                    | `reels()`    | Source                                                                              |
| --------------------------- | -------------------------------------- | ------------ | ----------------------------------------------------------------------------------- |
| `dpr_milli`                 | `device_scale_factor` (a whole number) | 3            | iPhone 17                                                                           |
| `frame_css`                 | `viewport`                             | 402 x 874    | the iPhone 17 screen. Render scales of 1/2 and 2/3, as Stake Reels uses, come later |
| `webgpu`                    | `webgpu`                               | false        | iOS 18 Safari has WebGPU off                                                        |
| none                        | `pcm_rate_hz`, `pcm_low_rate_hz`       | 48000, 24000 | The measure ids name 48 kHz and 24 kHz                                              |

Every `expected.toml` has these in `[profile]`. Tests turn the table into a
`HostProfile` with `Fixture::profile()` (`reels()` with the table's values in
place), so a change to the frame in `reels()` does not move fixture numbers. The PCM rates still come from `reels()`.

### Sound groups

A game often ships one sound in several formats and the browser plays one of
them. A **sound group** is the union of:

- audio files in the same folder with the same stem (`sfx.webm`, `sfx.mp3`);
- all files named in one Howler `src` list (bundle literal or sprite JSON).

Use union-find over both rules. A group counts once, at its **representative**:
the member with the largest `decoded_bytes_48k`; on a tie, the first path in
byte order. Only `estimated` files join groups.

## 2. Modules and ownership

The crate is `crates/dyno-static`. Agents build it in parallel. Each owns whole
files, so no two agents edit the same file.

| Agent     | Owns                                                                                                                                                                                                                                     | Produces                                                               |
| --------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| A         | `src/lib.rs`, `src/inventory.rs`, `src/sniff.rs` (new), `src/bytes.rs`, `src/duplicates.rs`, `src/rules.rs`, `tests/common/mod.rs`, `tests/fixtures.rs`, `tests/inventory.rs`, `tests/bytes.rs`, `tests/duplicates.rs`, `tests/rules.rs` | inventory, `build_id`, bytes by type, duplicates, rules, orchestration |
| C         | `src/audio/` (`mod.rs`, `wav.rs`, `ogg.rs`, `mp3.rs`, `adts.rs`, `mp4.rs`, `webm.rs`, `groups.rs`, `sprites.rs`), `tests/audio.rs`, `tests/sprites.rs`                                                                                   | audio, sound groups, sprite maps and silence                           |
| D         | `src/bundle/` (`mod.rs`, `html.rs`, `css.rs`, `js.rs`, `eval.rs`, `render.rs`, `versions.rs`, `urls.rs`, `howl.rs`), `tests/bundle.rs`                                                                                                   | render sites, canvas bytes, engine versions, external URLs, Howl sites |
| B (later) | `src/textures/` (`mod.rs`, `raster.rs`, `ktx2.rs`, `basis.rs`, `spine.rs`, `mips.rs`), `tests/textures.rs`                                                                                                                               | texture costs and totals                                               |

**Shared files.** The contract step landed the crate `Cargo.toml` with every
dependency in [section 10](#10-crates), `src/model.rs`, the data flow in
`lib.rs`, `tests/common/mod.rs` and a stub `mod.rs` in `audio/`, `textures/`
and `bundle/`. Each stub now belongs to its agent. No agent edits
`Cargo.toml`, `Cargo.lock`, `src/model.rs` or `dyno-schema`: a change to any
of them goes through the foreman. An agent may add `pub` items inside its own
files for its tests. Only the entry functions and `model.rs` are the contract.

### Data flow

`lib.rs` runs the families in this order. Each entry function returns its
measures, and a summary when later steps read one.

```rust
inventory::scan(build: &Path) -> Result<FamilyScan<Inventory>, ScanError>
bundle::scan(inventory: &Inventory, profile: &HostProfile) -> FamilyScan<BundleSummary>
textures::scan(inventory: &Inventory, engine: Engine) -> FamilyScan<TextureSummary>
audio::scan(inventory: &Inventory, howl_sites: &[HowlSite], profile: &HostProfile) -> FamilyScan<AudioSummary>
bytes::scan(inventory: &Inventory) -> Vec<Measure>
duplicates::scan(inventory: &Inventory, decoded_bytes: &BTreeMap<RelPath, u64>) -> FamilyScan<Vec<DuplicateGroup>>
rules::check(summaries: &Summaries, profile: &HostProfile, budget: Option<&Budget>) -> Vec<Finding>
```

- Textures take `bundle.summary.engine` for mipmaps. Audio takes
  `bundle.summary.howl_sites` for sound groups and sprite maps.
- `decoded_bytes` is built in `lib.rs` from `TextureSummary.files`
  (`gpu_bytes`) and `AudioSummary.files` (`decoded_bytes_48k`), so duplicates
  need no texture or audio types.
- `rules::check` reads only `Summaries`, the profile and the budget file.
- `dyno_static::scan(build, host_profile, budget) -> Result<StaticScan,
  ScanError>` gives `StaticScan { manifest, measures, findings }`.
  `dyno_static::without_build()` gives every static measure `unsupported` and
  `rules::without_build()` for the findings.
- `dyno check` passes its budget file to the scan, then gives budget verdicts
  from `scan.measures`.

Every family has a `MEASURES` list, and a test checks that each static measure
id is in exactly one list. A stub measure is `missing` with "not built yet",
and a stub summary is empty.

### Types

The contract types come from `dyno-schema`:

| Type                                      | Use                                                                                                 |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `RelPath`                                 | every path: relative, with `/`, sorted in byte order                                                |
| `FileKind`                                | a file's bucket: `js`, `css`, `html`, `image`, `texture`, `audio`, `video`, `font`, `data`, `other` |
| `ManifestFile`, `BuildManifest`           | a file's path, bytes, lowercase hex SHA-256 and kind; the files and `build_id`                      |
| `Measure`, `MeasureStatus`, `Uncertainty` | the result rows                                                                                     |
| `HostProfile`, `Viewport`                 | the device numbers ([section 1](#profile))                                                          |
| `Budget`                                  | the budget file, with the rule thresholds                                                           |
| `Finding`, `Fix`, `FixAction`             | the rule results                                                                                    |

`src/model.rs` holds the rest:

| Type                            | Fields or methods                                                                                                                                                            | Written by                                               | Read by                          |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- | -------------------------------- |
| `FamilyScan<S>`                 | `measures`, `summary`                                                                                                                                                        | every family                                             | `lib.rs`                         |
| `Inventory`                     | `new(manifest, contents)`, `files()`, `files_of_kind(kind)`, `contents(path)`, `into_manifest()`                                                                             | inventory                                                | every family                     |
| `Engine`                        | `Unknown`, `Pixi7`, `Pixi8`                                                                                                                                                  | bundle                                                   | textures                         |
| `BundleSummary`                 | `engine`, `render_sites`, `howl_sites`, `invalid_scripts`, `draws_with_html`                                                                                                 | bundle                                                   | textures, audio, rules           |
| `RenderSite`                    | `file`, `offset`, `antialias` (`None` when not static), `draws_in_2d`, `resolution: Resolution`, `canvas_css`, `canvas: Option<CanvasFloor>` (`None` when Pixi cannot start) | bundle                                                   | rules                            |
| `Resolution`                    | `low_milli`, `high_milli` (each `None` when the build does not bound it); `exactly`, `fixed`, `is_above`, `floor_milli`                                                      | bundle                                                   | rules                            |
| `HowlSite`, `SpriteRange`       | `file`, `offset`, `src`, `sprites`; `name`, `start_us`, `end_us`, `looped`                                                                                                   | bundle                                                   | audio                            |
| `TextureSummary`, `TextureCost` | `low_bytes`, `high_bytes`, `files`, `not_covered`; `path`, `gpu_bytes`, `astc_bytes`                                                                                         | textures                                                 | `lib.rs` (for duplicates), rules |
| `AudioSummary`, `DecodedAudio`  | `decoded_bytes_48k`, `decoded_bytes_24k`, `files`, `stereo_groups`, `sprite_maps`, `not_covered`; `path`, `decoded_bytes_48k`                                                | audio                                                    | `lib.rs` (for duplicates), rules |
| `SpriteMapSummary`              | `source`, `status`, `audio`, `silence_bp`, `silence_bytes_48k`                                                                                                               | audio                                                    | rules                            |
| `DuplicateGroup`                | `sha256`, `bytes`, `paths`, `wasted_bytes`, `decoded_bytes_saved`                                                                                                            | duplicates                                               | rules                            |
| `Summaries`                     | `bundle`, `textures`, `audio`, `duplicates`, `measures_without_value`                                                                                                        | `lib.rs`                                                 | rules                            |
| `MeasureWithoutValue`           | `id`, `reason`: a measure whose status is not `estimated` or `measured`                                                                                                      | `lib.rs`                                                 | rules                            |
| `CanvasFloor`, `canvas_floor()` | `backing_width`, `backing_height`, `pixels`, `bytes`                                                                                                                         | the one canvas formula ([section 8.4](#84-canvas-bytes)) | bundle, rules                    |
| `RESOLUTION_CAP_MILLI`          | 2000, a **proposal**                                                                                                                                                         |                                                          | bundle, rules                    |

The `Inventory` keeps every file's contents, because it reads every file in
full to hash it. Other families read files only through it, so a folder and a
.zip look the same to them.

### Measures

Each static measure, the agent that owns it, and what it holds. Items sum to
the value unless the row says otherwise. An item's key is a path unless the
row says otherwise.

| Measure                             | Agent | Value                                                                                                                                                      | Items                                                                                                                                                  | Uncertainty                                                                                                                                                                                                      |
| ----------------------------------- | ----- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `static.inventory.files`            | A     | number of files                                                                                                                                            | none: the manifest lists them                                                                                                                          | `exact`                                                                                                                                                                                                          |
| `static.bytes.by_type`              | A     | bytes of every file                                                                                                                                        | one per non-empty kind, keyed by its `FileKind` name                                                                                                   | `exact`                                                                                                                                                                                                          |
| `static.bytes.compressed`           | A     | gzip bytes of the js, css, html and data files                                                                                                             | per kind of those four with files: its gzip bytes keyed by the kind, then its brotli bytes keyed `<kind>.brotli`. Only the gzip items sum to the value | `exact` for the pinned flate2 and brotli                                                                                                                                                                         |
| `static.bytes.duplicates`           | A     | sum of `wasted_bytes`                                                                                                                                      | one per extra copy (every path of a group but the first), its bytes                                                                                    | `exact`                                                                                                                                                                                                          |
| `static.textures.decoded`           | B     | `low_bytes`                                                                                                                                                | one per estimated file, its `gpu_bytes`                                                                                                                | `range` from `low_bytes` to `high_bytes`                                                                                                                                                                         |
| `static.audio.decoded_48k`          | C     | the total over sound groups at `pcm_rate_hz`                                                                                                               | one per sound group, keyed by its representative                                                                                                       | `floor`: "0 to 23% below what the browser holds"                                                                                                                                                                 |
| `static.audio.decoded_24k`          | C     | the same at `pcm_low_rate_hz`                                                                                                                              | as above                                                                                                                                               | `floor`, as above                                                                                                                                                                                                |
| `static.audio.stereo_files`         | C     | number of stereo sound groups                                                                                                                              | one per representative, value 1                                                                                                                        | `exact`                                                                                                                                                                                                          |
| `static.audio.stereo_mono_safe`     | C     | number of decoded stereo groups that are `safe_to_make_mono`                                                                                               | one per decoded stereo group, keyed by its representative, its class as text: `safe_to_make_mono`, `changes_the_sound` or `cancels`                    | `exact`, with a note that gives the two thresholds. `not_covered` lists the stereo groups that did not decode, then the files with no header                                                                     |
| `static.audio.sprite_silence`       | C     | the largest `silence_bp` of the estimated sprite maps; 0 when there are none                                                                               | one per sprite map, keyed by its source (a Howl site adds `@offset`), its `silence_bp`. Not a sum                                                      | `exact` for time no sprite covers; the note says it is an upper bound on what trimming saves                                                                                                                     |
| `static.audio.silence_by_decode`    | C     | silent windows over all windows of the decoded groups: the share of their time that is silent, rounded half up                                             | one per decoded group, keyed by its representative, its `silence_bp`                                                                                   | `exact`, with a note that gives the window and the level. `not_covered` lists the groups that did not decode, then the files with no header                                                                      |
| `static.render.antialias`           | D     | true when a site that may draw on the GPU (not a 2D canvas) proves it on; else unknown when such a site sets it at run time; else false                    | one per such site with a known flag, keyed by its file                                                                                                 | `exact`                                                                                                                                                                                                          |
| `static.render.resolution_uncapped` | D     | true when a site proves its resolution above `RESOLUTION_CAP_MILLI`; else unknown when a site's resolution is not bounded at or below the cap; else false  | one per site with a known answer                                                                                                                       | `exact`                                                                                                                                                                                                          |
| `static.render.backend`             | D     | what the site with the largest canvas floor asks for: `webgl`, `webgpu` or `canvas2d`, before any fallback. `none` for a game that draws with HTML and CSS | one per render site with a known preference, as text                                                                                                   | `exact`                                                                                                                                                                                                          |
| `static.render.canvas`              | D     | the largest site's `canvas.bytes`: a floor of one colour buffer                                                                                            | one per render site with a canvas, its bytes                                                                                                           | `floor`, with a note: "one colour buffer, 4 bytes a backing pixel; antialias buffers and the GPU context are not included", and what the floor took because the build does not fix it (the resolution, the size) |
| `static.bundle.engines`             | D     | number of libraries found                                                                                                                                  | one per library, keyed `<bucket>.<library>` (`rendering.pixi`, `audio.howler`, `other.web_sdk`), its version as text, or `unsupported`                 | `exact`                                                                                                                                                                                                          |
| `static.bundle.external_urls`       | D     | number of external URLs                                                                                                                                    | one per URL, value 1. `scope.not_covered` says how many hosts are built at run time                                                                    | `exact` for what the bundle holds                                                                                                                                                                                |
| `static.bundle.source_map`          | D     | Phase 2: stays `missing`, "not built yet"                                                                                                                  |                                                                                                                                                        |                                                                                                                                                                                                                  |

Each render measure says what the build proves, on its own: an option set at
run time makes only its own measure unknown. An unknown flag is `unsupported`
with reason `option_not_static`, since the browser run can read it. The
largest site is the one with the largest `canvas.bytes`, the first of equal
ones. The render measures are `missing` when no render site is found, unless
the game draws with HTML and CSS ([section 8.4](#84-canvas-bytes)).
`static.render.antialias` is `unsupported` with reason `canvas2d` when every
site is a 2D canvas.

### Tests

`tests/common/mod.rs` (A) loads the fixtures. A test file adds `mod common;`
and reads only its own table of `expected.toml`:

| Helper                                                             | Gives                                                                                                                                             |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fixtures()`                                                       | every folder under `fixtures/` with an `expected.toml`, sorted by name                                                                            |
| `fixture(name)`                                                    | one fixture                                                                                                                                       |
| `Fixture::table(name)`                                             | one top-level table, such as `"audio"`                                                                                                            |
| `Fixture::files()`                                                 | every build file's bytes by path. A zip keeps its top folder                                                                                      |
| `Fixture::profile()`                                               | the `[profile]` table as a `HostProfile`                                                                                                          |
| `Fixture::manifest()`                                              | the `BuildManifest` that `[inventory]` lists                                                                                                      |
| `Fixture::inventory()`                                             | the `Inventory` that `[[inventory.file]]` lists, with the build's bytes. Folder fixtures only. B, C and D test with it before A's inventory lands |
| `Fixture::scanned_inventory()`                                     | the `Inventory` that `inventory::scan` gives for the build, a folder or a zip                                                                     |
| `integer(table, key)`, `text(table, key)`, `sub_table(table, key)` | one value of a table                                                                                                                              |

| Test                  | Table                                                                                     |
| --------------------- | ----------------------------------------------------------------------------------------- |
| `tests/fixtures.rs`   | every fixture loads                                                                       |
| `tests/inventory.rs`  | `[inventory]`                                                                             |
| `tests/bytes.rs`      | `[bytes]`                                                                                 |
| `tests/duplicates.rs` | `[duplicates]`                                                                            |
| `tests/audio.rs`      | `[audio]`                                                                                 |
| `tests/sprites.rs`    | `[sprites]`                                                                               |
| `tests/textures.rs`   | `[textures]`                                                                              |
| `tests/bundle.rs`     | `[bundle]`, with `[profile]`                                                              |
| `tests/rules.rs`      | `[rules]`; it builds `Summaries` from the other tables, so it does not wait for B, C or D |

## 3. Inventory and build id

Owner: A. Files: `inventory.rs`, `sniff.rs`. Entry: `inventory::scan`.

**Input.** A folder, or a `.zip` file. In a zip, if every entry sits under one
top-level folder and there is no `index.html` at the root, strip that folder.
Skip directory entries and symbolic links. A zip entry name with a NUL byte
is an error, because the build id puts a NUL after each path. Two entries for one
path are an error. A zip with more than 100,000 entries, or whose central
directory states more than 1 GiB unpacked, is an error before any entry is
read, so a zip bomb cannot fill memory. The zip crate refuses an entry that
unpacks past the size it states, so the stated sizes hold. Each of these
errors is a usage error.

**Walk.** Recursive, sorted. Skip symlinks (never follow them). Skip these
names anywhere: `.DS_Store`, `Thumbs.db`, `desktop.ini`, and the folders
`__MACOSX` and `.git`. Paths are `RelPath`: relative, with `/`, and valid
UTF-8. A folder with a non-UTF-8 name is a scan error. A zip entry name that is
not UTF-8 is read as CP437, as the zip format says. Sort paths by byte order.

**Hash.** SHA-256 of every file (`sha2`). Every file is read in full once for
the hash; "header only" below means the parsers look at headers, not that I/O
is limited.

**build_id** (R3). SHA-256 over one line per file, in path order:

```
path + "\0" + lowercase_hex(sha256(file)) + "\n"
```

In bytes, each line is the path's UTF-8 bytes, one `0x00` byte, the file's
SHA-256 as 64 lowercase ASCII hex digits, and one `0x0A` byte. The lines are
joined with nothing between them, in byte order of path. Empty files have a
line too. `build_id` is the SHA-256 of the joined bytes, as 64 lowercase hex
digits. The fixtures' `build_id` values follow this.

The plan says "SHA-256 of the sorted file hashes". Including the path makes a
rename change the id, which a join key needs (a moved file can change what
loads).

**Bucket** (`FileKind`). Sniff the magic bytes first, then fall back to the
extension.

| Magic                                                                                                                                          | Bucket                                                                                                                                                                                              |
| ---------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `89 50 4E 47`, `FF D8 FF`, `GIF87a`, `GIF89a`, `RIFF....WEBP`                                                                                  | image                                                                                                                                                                                               |
| `RIFF....WAVE`, `OggS` (unless the first packet is `\x80theora`), `ID3`, `fLaC`, `caff`, `FORM....AIFF`/`AIFC`                                 | audio                                                                                                                                                                                               |
| `FF Ex`/`FF Fx` frame sync, only when the extension is mp3, aac, mp2 or mpga                                                                   | audio                                                                                                                                                                                               |
| ISO-BMFF (`ftyp` at offset 4)                                                                                                                  | brand `avif`, `avis`, `heic`, `heix`, `mif1`, `msf1`: image. Else any `hdlr` of type `vide`: video; else `soun`: audio. If `moov` is missing or unreadable: extension (m4a, aac: audio; else video) |
| EBML `1A 45 DF A3`                                                                                                                             | any `TrackType` 1: video; else 2: audio. Unreadable: extension (weba, mka: audio; else video). Read only up to `Tracks`; do not check sizes past it                                                 |
| KTX2 `AB 4B 54 58 20 32 30 BB`, KTX1 `AB 4B 54 58 20 31 31 BB`, ASTC `13 AB A1 5C`, `DDS `, PVR3 `50 56 52 03`, Basis `sB` with header size 77 | texture                                                                                                                                                                                             |
| `wOFF`, `wOF2`, `OTTO`; `true` and `00 01 00 00` only with ttf or otf, since `true` can start a text file                                      | font                                                                                                                                                                                                |
| `\0asm`                                                                                                                                        | other                                                                                                                                                                                               |

Extensions, when no magic matched: `js mjs cjs` js; `css`; `html htm`; `json
map txt xml csv atlas fnt skel plist yaml yml` data; `png jpg jpeg webp avif
gif svg bmp ico` image; `ktx2 ktx basis astc dds pvr` texture; `wav ogg oga
opus mp3 aac m4a weba flac caf aif aiff` audio; `mp4 m4v mov webm ogv mkv`
video; `woff woff2 ttf otf eot` font; anything else other.

A damaged file keeps the bucket its magic or extension gives. Its own module
marks it `invalid`.

**Output.** An `Inventory` with the manifest (`files` sorted by path, and
`build_id`) and every file's contents, plus the measure
`static.inventory.files`. Fixtures: every `[inventory]` table; `zip-input`
must give the same `build_id` as `duplicates`.

## 4. Bytes by type and duplicates

Owner: A. Files: `bytes.rs`, `duplicates.rs`. Entries: `bytes::scan` and
`duplicates::scan`.

**Bytes by type.** For each bucket in the fixed order js, css, html, image,
texture, audio, video, font, data, other: file count and raw bytes. Leave out
empty buckets. For js, css, html and data also compute:

- `gzip_bytes`: flate2, `Compression::new(9)`, written through
  `GzBuilder::new().mtime(0)` with no file name. That is the deflate stream
  plus 18 bytes.
- `brotli_bytes`: brotli, quality 11, `lgwin` 22, default mode.

Sum per bucket. Status `estimated`, uncertainty `exact` (R2). An inline
script is part of its HTML file's bytes. The raw bytes give
`static.bytes.by_type`. The gzip bytes give `static.bytes.compressed`, and its
items hold the brotli bytes too, keyed `<kind>.brotli`. Source maps (`.map`)
count in the raw bytes but are not compressed: no player downloads them, and
brotli at quality 11 on a large map is slow. When a build has maps, the
compressed measure's `scope.not_covered` says how many.

**Duplicates.** Group files by SHA-256. Ignore empty files. A group of `n >= 2`
files gives:

- `paths`, sorted; groups sorted by their first path;
- `keep`: the copy to keep, when the build's HTML, JavaScript, CSS or JSON
  files (source maps left out) name exactly one copy. A copy is named when a
  text holds its shortest path suffix that no other file in the build ends
  with (`coin.png`, or `a/coin.png` when `c/coin.png` is there too), as a whole
  name: the characters on each side are not letters, digits, `-`, `_` or `.`.
  None when no copy or more than one is named, since deleting a copy by name
  could then delete the one the game loads;
- `wasted_bytes = (n - 1) * bytes`;
- `decoded_bytes_saved = (n - 1) * decoded`, where `decoded` is the first
  path's value in `decoded_bytes` (a texture's `gpu_bytes` or an audio file's
  `decoded_bytes_48k`), else 0.

The groups are the `DuplicateGroup` summary, and their wasted bytes give
`static.bytes.duplicates`.

Fixture: `duplicates` (3 groups, 2 empty files that do not count),
`zip-input`.

## 5. Audio

Owner: C. Files: `audio/*.rs`. Entry: `audio::scan`. Input: every file of kind
audio, the `.json` files for sprite maps (section 6), the `HowlSite` list from
bundle, and the profile's two PCM rates.

### 5.1 Formula

Each reader returns `channels`, `ticks` and `tick_rate`: the playable length
the container states, in the container's own clock. Then, for R in
`profile.pcm_rate_hz` (48000) and `profile.pcm_low_rate_hz` (24000):

```
frames_R          = ceil(ticks * R / tick_rate)
decoded_bytes_R   = frames_R * channels * 4
```

`decodeAudioData` returns 32-bit float PCM at the context rate, one buffer per
channel. The source sample rate does not matter, only length and channels.
That also makes SBR (HE-AAC) and Opus's internal rate harmless.

The plan calls this number a floor (0 to 23% low): a browser can hold more
than the PCM. It is status `estimated`.

### 5.2 Readers

Own readers, one file per container. They read headers and walk frame or page
headers; they never decode. The tests cross-check them against symphonia
0.6.1's demuxers (R4). [Section 11](#11-cross-check-results) lists where it
differs. Decoding is separate ([section 5.4](#54-decoding)).

| Format            | Channels                                                                                                  | ticks                                                                                                                                                                      | tick_rate                                                 | `invalid` when                                                                                             |
| ----------------- | --------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| WAV (`RIFF/WAVE`) | `fmt.channels`                                                                                            | PCM (1), float (3), extensible (0xFFFE): `data_size / block_align`. Other codecs: `fact.sample_length`                                                                     | `fmt.sample_rate`                                         | no `fmt` or `data`; `data` runs past the end                                                               |
| Ogg Vorbis        | ident header byte 11                                                                                      | last granule position of the first logical stream                                                                                                                          | ident `sample_rate`                                       | first page incomplete; last page runs past the end; no granule                                             |
| Ogg Opus          | OpusHead byte 9                                                                                           | last granule minus `pre_skip`                                                                                                                                              | always 48000                                              | as Vorbis, and last granule not above `pre_skip`                                                           |
| MP3               | header mode 3 is mono, else 2                                                                             | Xing/Info frames x spf minus (delay + padding) when a LAME, Lavf or Lavc tag follows; Xing frames x spf without one; VBRI frames x spf; else walk every frame, count x spf | header sample rate                                        | no frame sync followed by a second valid frame; the walk loses sync; Xing byte count past the end          |
| AAC in ADTS       | `channel_configuration` (7 means 8)                                                                       | sum over frames of `(number_of_raw_data_blocks + 1) * 1024`                                                                                                                | sampling frequency index                                  | last frame runs past the end; lost sync                                                                    |
| AAC in MP4/M4A    | AudioSpecificConfig `channelConfiguration`; object type 29 (PS) means 2; 0 means the sample entry's count | with an edit list: sum of `segment_duration` of entries with `media_time >= 0`; without: `mdhd.duration`                                                                   | `mvhd.timescale` with an edit list; else `mdhd.timescale` | no `moov`; a box runs past the end; no `soun` track                                                        |
| WebM (Matroska)   | `Audio/Channels`                                                                                          | `round((Duration * TimestampScale - CodecDelay) / 1000)`                                                                                                                   | 1,000,000 (microseconds)                                  | an element runs past the end, `Segment` included; a video track (then the file is bucket video, not audio) |

Details per reader:

- **WAV.** Chunks are padded to an even size. `data_size` of `0xFFFFFFFF` or 0
  is a streamed file: use the rest of the file and add note `streamed_wav`.
  `RF64` is `unsupported`. An extensible `fmt` takes its codec from the first
  2 bytes of its sub-format GUID, at byte 24. One too short to hold them
  counts as PCM.
- **Ogg.** Read the first page fully (it holds the ident or OpusHead packet).
  For the end, scan the last 65,307 + 27 bytes (one maximum page) for
  `OggS`. Accept a page only if its CRC is right (polynomial `0x04C11DB7`,
  initial 0, not reflected, CRC field zeroed while computing) and its serial
  is the first page's serial. Pages of other streams are skipped. Take the
  granule of the last such page. If the last `OggS` found starts a page that
  runs past the end, the file is `invalid` (`truncated`). If the tail holds
  only other streams' pages, the file is `unsupported` (`chained ogg`).
- **Opus.** The OpusHead field `input_sample_rate` is informational. Never use
  it. Opus always decodes at 48 kHz and granules count 48 kHz samples. The
  `pre_skip` samples are dropped at the start. Fixture `tone-stereo.opus` has
  `input_sample_rate` 44100 on purpose: last granule 48312, pre-skip 312,
  gives 48000 ticks at 48000.
- **MP3.** Skip an ID3v2 tag (10 bytes plus its syncsafe size, plus 10 for a
  footer). Ignore a trailing ID3v1 `TAG` (128 bytes). Samples per frame: 1152
  for MPEG-1 Layer III, 576 for MPEG-2 and 2.5. The Xing or Info tag sits after
  the side info: 32 bytes (MPEG-1 stereo), 17 (MPEG-1 mono or MPEG-2 stereo), 9
  (MPEG-2 mono), counted from 4 bytes after the frame start, with or without a
  CRC. After its optional
  fields (frames, bytes, 100-byte TOC, quality) comes a 9-byte encoder string.
  If it starts with `LAME`, `Lavf` or `Lavc`, the encoder delay and padding are
  3 bytes at offset 21 from that string: 12 bits each. The VBRI tag sits 32
  bytes after the frame header; its frame count is at offset 14. A file with
  no tag is walked frame by frame (`tone-noxing.mp3`: 40 frames, 46080 ticks,
  45 ms of encoder delay and padding included).
- **ADTS.** The header is 7 bytes, or 9 with CRC. `frame_length` includes the
  header. The count includes the encoder's priming and padding, so it is a
  little high (fixture: 1.045 s for a 1 s tone). HE-AAC v2 in ADTS signals PS
  only inside the raw data, so a PS stream reads as mono (see
  [section 13](#13-not-covered)).
- **MP4.** Walk boxes with 32-bit or 64-bit sizes (`size == 0` runs to the end
  of its parent). Path: `moov/mvhd`, `moov/trak/edts/elst`,
  `moov/trak/mdia/{mdhd,hdlr}`, `.../minf/stbl/stsd/mp4a/esds`. The edit list
  removes the AAC priming: fixture `tone-stereo.m4a` has `mdhd` 45124 and an
  edit list of 44100 at timescale 44100. Use the first `soun` track. `moov`
  may come before or after `mdat` (both fixtures exist). Opus or ALAC in MP4 is
  `unsupported`. The sample entry's channel count sits at byte 16 of its body,
  but a QuickTime version 2 sound description keeps a placeholder there and
  the real count as 32 bits at byte 40.
- **WebM.** Element IDs keep their length marker; sizes drop it. An all-ones
  size is unknown and runs to the end of the parent. Read `Info`
  (`TimestampScale` 0x2AD7B1, default 1,000,000 ns; `Duration` 0x4489, a 4 or
  8 byte float) and `Tracks` (`TrackType` 0x83, `CodecID` 0x86, `CodecDelay`
  0x56AA in ns, `DefaultDuration` 0x23E383 in ns, `Channels` 0x9F). Live files
  have no `Duration`. Then the end is the last block's time plus its length:
  `(cluster Timestamp + block relative time) * TimestampScale`, plus
  `BlockDuration * TimestampScale` if the block group has one, else
  `DefaultDuration * laced frames - DiscardPadding`; minus `CodecDelay`. Only
  a positive `DiscardPadding` counts: RFC 9559 puts a negative one at the
  block's start. Add note `duration_from_blocks`. A `SamplingFrequency` that
  is not a finite, non-negative number keeps the default, since the formula
  never reads it. Both paths give 1,001,500 us for the fixture tone
  (`tone-stereo.webm` and `tone-live.webm`).

### 5.3 Output

Per file, as the `[[audio.file]]` rows list it: `path, status, reason?,
container, codec, channels, ticks, tick_rate, decoded_bytes_48k,
decoded_bytes_24k`. Codec names: `pcm`, `pcm_float`, `vorbis`, `opus`, `mp3`,
`aac`. Then sound groups (section 1), and the total over groups with
`not_covered`.

Each `[[audio.group]]` row also gives what decoding found: `silence_bp`,
`stereo_class` for a stereo group, and `decoded_from` when a file other than
the representative decoded. A group that did not decode has `not_decoded` with
the reason. `[audio.total]` has `silence_by_decode_bp`.

In the `AudioSummary`: `files` holds every estimated file with its own
`decoded_bytes_48k`, `stereo_groups` the representative of every stereo group
with its decoded bytes and its stereo class (none when it did not decode), and
`decoded_bytes_48k` and `decoded_bytes_24k` the totals.
The measures are in [section 2](#measures).

Fixtures: `audio-formats` (every reader), `broken` (one damaged file per
reader), `howler-sprites`, `over-budget`.

### 5.4 Decoding

Since 2026-11-07, dyno decodes audio, to
tell which stereo sounds can be mono and to measure silence. `audio/listen.rs`
decodes each sound group with symphonia 0.6.1, as a stream, in two passes. It
keeps only running sums, never the samples or a list of windows, so memory
does not grow with the length of a sound. It decodes the representative, or
else the first other file of the group that decodes.

The codecs and containers are only those the readers know: PCM, ADPCM, MP3,
AAC, Vorbis, in WAV, MP3, ADTS, MP4, Ogg and WebM. symphonia's licence is
MPL-2.0. It is file-level copyleft, which is fine in a closed-source binary
while its files stay unmodified. `deny.toml` and `about.toml` allow MPL-2.0
for the symphonia crates only, and `THIRD_PARTY_NOTICES.md` gives the link to
their source.

**Length.** The header readers stay the authority on the playable length.
The first pass counts the frames symphonia gives. symphonia can return frames
that a browser does not play, so the second pass skips a lead-in and measures
the header's length after it, to the exact frame. For an M4A whose edit list
plays one media segment, the lead-in is where that segment starts, so the
priming at the start and the padding at the end both stay out. symphonia 0.6
ignores edit lists, so it always gives that lead-in; a cross-check test fails
if a later symphonia applies them. An M4A whose edit list plays more than one
media segment is `not_decoded`: its playable part is not one range. For other
formats, or an M4A with no edit list, the lead-in is every extra frame, taken
from the start, where encoder priming is.
A decode that gives fewer frames than the header, by more than 1% or one
window, is `not_decoded` ("symphonia decoded less than the header's length"),
so a part of a sound never stands for all of it.

**Silence.** A window is silent when its mean energy is at or below -60 dBFS
(10^-6 a sample). `silence_bp` is the silent share of a group's windows. The
measure weighs each group by its time.

**Stereo.** Over the whole sound, for a sound with two or more channels: the
mono mix is the mean of the channels, the channel energy is the mean energy of
the channels, and what the channels do not share is the difference. For two
channels that is mid = (L + R) / 2 against side = (L - R) / 2. Every channel
counts, so a centre channel that cancels left and right is seen.

| Class               | When                                                                                                 | What the `audio.stereo` rule does                             |
| ------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| `safe_to_make_mono` | what the channels do not share is at least 40 dB below the mono mix, or the sound is digital silence | a `downmix_to_mono` fix                                       |
| `changes_the_sound` | neither of the others                                                                                | a `downmix_to_mono` fix, and the text asks for a listen first |
| `cancels`           | the mono mix keeps 25% of the channel energy or less                                                 | no fix: keep it stereo                                        |

The `cancels` bar is a proposal (R8). The 40 dB bar for `safe_to_make_mono` is
decided: it does not get looser without a listening test. A
"nearly mono" label from -40 to -20 dB was suggested; it is not decided and not
built. On the 2026-11-05 real builds, the side of the 56 stereo sounds is 21 to
28 dB below the mid at best and 12 dB below at the median, so all of them are
`changes_the_sound`.

**Known gaps.**

| Gap                                                                                                                  | How dyno handles it                                                                                                                                                                                       |
| -------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| symphonia has no Opus decoder                                                                                        | An Opus group is decoded from another file of the group when there is one. Else it is `not_decoded`, listed in `not_covered`, and the stereo rule judges it by its channel count                          |
| Opus pre-skip                                                                                                        | No Opus decode, so it does not apply                                                                                                                                                                      |
| M4A edit lists: symphonia ignores them and returns the priming and padding frames                                    | With one media segment, the MP4 reader gives its start, so the analysis skips the priming and stops before the padding: the frames ffmpeg plays. With more than one, the sound is `not_decoded`           |
| Other formats with padding at the end and no stated start                                                            | The extra frames are skipped from the start, so some padding stays and the same count of frames from the start goes. Silence and stereo energy are sums over the sound, so a few ms move them very little |
| symphonia trims an MP3 to its own length, from the Xing header or, with none, from the first frame's bit rate        | A decode shorter than the header's length is `not_decoded`, not measured in part                                                                                                                          |
| A stream whose channels change, such as MP3s joined end to end: symphonia refuses the first packet with new channels | The packet is tried once more with a new decoder, and decoding goes on                                                                                                                                    |
| A damaged packet                                                                                                     | It is skipped, as a browser skips it. The length check then judges what decoded                                                                                                                           |

## 6. Sprite maps and silence

Owner: C. File: `audio/sprites.rs`. Its output is `AudioSummary.sprite_maps`.

A Howler sprite is one audio file with named ranges. Howler decodes the whole
file at load, so the gaps between ranges cost decoded memory for nothing.

### 6.1 Where sprite maps appear

| Source                                                                                               | Shape                                                                              | Units   |
| ---------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | ------- |
| JS literal (from bundle as a `HowlSite`)                                                             | `new <anything>({src: "..." or [...], sprite: {name: [offset, duration, loop?]}})` | ms      |
| Howler JSON (any `.json` in the build; web-sdk ships one as `sounds.json`; audiosprite `-e howler2`) | `{"src": "..." or [...], "sprite": {name: [offset, duration, loop?]}}`             | ms      |
| Howler 1 JSON (audiosprite `-e howler`)                                                              | `{"urls": [...], "sprite": {...}}`                                                 | ms      |
| audiosprite jukebox JSON (`-e jukebox`, also Phaser)                                                 | `{"resources": [...], "spritemap": {name: {"start", "end", "loop"}}}`              | seconds |

Rules:

- A JSON file is a sprite map only if it has that exact shape. Other keys are
  allowed. A `.json` file that does not parse but contains the text `"sprite"`
  or `"spritemap"` is reported as a sprite map with status `invalid`.
- Convert to microseconds: Howler `start_us = offset * 1000`,
  `end_us = (offset + duration) * 1000`. Jukebox: `round(seconds * 1e6)`.
  JS numbers like `3e3` are fine (bundle evaluates them).
- Resolve each `src` path: drop a query string or fragment, decode `%XX`
  escapes, and strip a leading `./`. A leading `/` starts at the build root.
  Try the path relative to the folder of every HTML page in the build,
  shallowest first, then the build root, then the JSON file's folder (or the
  JS file's folder for a literal). The first candidate that exists wins. So
  a root `game.html` comes before `help/index.html`, and a root launcher
  `index.html` still finds `game/sfx/a.mp3` from `game/index.html`. If no
  candidate exists in the build, the map is `missing`.
- All files in one `src` list join one sound group (section 1).
- A map with no sprites (an empty `sprite` or `spritemap` object) is not a
  sprite map. It only joins its `src` files into one sound group, as a Howl
  site without `sprite` does.

### 6.2 Silence share

Use the group's representative file (section 1). Then:

```
duration_us  = floor(ticks * 1_000_000 / tick_rate)
covered_us   = length of the union of [start_us, end_us), each clipped to [0, duration_us]
silence_us   = duration_us - covered_us
silence_bp   = floor(silence_us * 10000 / duration_us)
silence_bytes_48k = floor(decoded_bytes_48k * silence_us / duration_us)
```

When the map's `src` files are in the build but none was read, the map takes
the first file's status (`invalid` or `unsupported`) with reason `audio not
readable`. When `duration_us` is 0, the map is `invalid` (`empty audio`).

"Silence" here means time no sprite covers. It is an upper bound on what
trimming saves: a game can also play a file by offset in code. True silence by
decoding is in section 5.4.

Fixture `howler-sprites`: the bundle literal over `sfx.webm` (6.0015 s, sprites
cover 2.25 s) gives 6250 bp and 1,440,576 bytes; `music.json` gives 2500 bp;
the jukebox `ui.json` gives 6250 bp. `broken/bad/sprite-broken.json` is an
`invalid` map.

## 7. Textures

Owner: B (later). Files: `textures/*.rs`. Entry: `textures::scan`. Input:
every file of kind image and texture, and `BundleSummary.engine`.

### 7.1 Formula

Per file, `gpu_bytes` is the sum over the levels the GPU will hold:

| Source                           | Bytes per level                                                | Levels            |
| -------------------------------- | -------------------------------------------------------------- | ----------------- |
| PNG, JPEG, WebP, AVIF, GIF       | `w * h * 4` (the browser decodes to RGBA8; Pixi uploads RGBA8) | see 7.3           |
| KTX2 with Basis (ETC1S or UASTC) | ASTC 4x4: `ceil(w/4) * ceil(h/4) * 16`                         | the file's levels |
| `.basis` (ETC1S or UASTC)        | ASTC 4x4, as above                                             | the file's levels |
| KTX2, raw (no Basis)             | `uncompressedByteLength` from the level index                  | the file's levels |

Totals: `low_bytes = sum of gpu_bytes` over `estimated` files;
`high_bytes = 2 * low_bytes` (the browser may keep a second, CPU-side copy;
plan: "1 to 2 copies until calibrated"). Each `TextureCost` also carries
`astc_bytes`, the same levels in ASTC 4x4, used for the rule's saving. The
rule takes the 5 largest by `gpu_bytes`, ties by path.

All images count, not only the ones the game loads at start (plan: "all
supplied assets, an inventory, not an upper bound"). Images used only as
`<img>` or CSS backgrounds also count here; the static scan cannot tell them
apart.

### 7.2 Readers

| Format         | Read                             | Crate                                                                                                                          | Odd headers                                                                                                          |
| -------------- | -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------- |
| PNG            | IHDR width and height            | `imagesize` 0.15.0 (`blob_size`)                                                                                               | APNG counts one frame. 16-bit and palette images still upload as RGBA8. IHDR must be the first chunk, else `invalid` |
| JPEG           | SOFn marker                      | `imagesize`                                                                                                                    | EXIF rotation swaps sides but not the product. No SOF before the end: `invalid`                                      |
| WebP           | `VP8 `, `VP8L` or `VP8X` header  | `imagesize`                                                                                                                    | animated WebP counts the canvas once                                                                                 |
| AVIF           | `ispe` of the primary item       | `imagesize` (HEIF path)                                                                                                        | grids and `irot` do not change `w * h`. `ftyp` only: `invalid`                                                       |
| GIF            | logical screen size              | `imagesize`                                                                                                                    | one frame                                                                                                            |
| KTX2           | 80-byte header, level index, DFD | `ktx2` 0.5.0: `Reader::new` (it checks every section and level against the file length), `header()`, `levels()`, `basic_dfd()` | Do not trust `imagesize` for KTX2: it reports a size for a truncated file; `Reader::new` returns `UnexpectedEnd`     |
| `.basis`       | own reader                       | none needed                                                                                                                    | see 7.4                                                                                                              |
| Spine `.atlas` | own text reader                  | none needed                                                                                                                    | see 7.5                                                                                                              |
| SVG            | none                             |                                                                                                                                | `unsupported` (its raster size depends on how the game draws it)                                                     |

KTX2 details:

- `vkFormat` 0 (undefined) with supercompression BasisLZ (1) and DFD colour
  model 163 is ETC1S. Colour model 166 is UASTC (supercompression none,
  Zstandard 2 or ZLIB 3). Both transcode on the phone. Anything else with
  `vkFormat` 0, such as UASTC HDR, is not modelled.
- Any other `vkFormat` is uploaded as is: `gpu_bytes` is the sum of
  `uncompressedByteLength`. BC formats (vkFormat 131 to 146) cannot be sampled
  on an iPhone: status `unsupported`, reason `bc_on_iphone`.
- `levelCount` 0 means "generate mips at runtime": count level 0 only and add
  note `runtime_mips`.
- Layers and faces multiply the level size (already in
  `uncompressedByteLength`; for Basis, multiply by `max(1, layerCount) *
  faceCount`). A volume's depth multiplies each level by its own depth.

### 7.3 Mipmaps

The plan says "plus 1/3 for mipmaps". The real rule depends on the engine, so
this design adds the exact chain only where we know mips exist (R5):

| Case                                                     | Mips                                                                                        |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Pixi 7 (`Engine::Pixi7`)                                 | Default `MIPMAP_MODES.POW2`: mips for images whose width and height are both powers of two. |
| Pixi 8 (`Engine::Pixi8`)                                 | None. `autoGenerateMipmaps` defaults to false.                                              |
| Unknown engine                                           | None.                                                                                       |
| A Spine atlas page whose min filter starts with `MipMap` | Full chain, any engine.                                                                     |
| KTX2 and `.basis`                                        | The levels in the file.                                                                     |

A full chain for `w x h` has `floor(log2(max(w, h))) + 1` levels; level `i` is
`max(1, w >> i) x max(1, h >> i)`.

Fixture `pixi7-aa`: `atlas-1024x1024.png` is 5,592,404 bytes with mips;
`panel-1000x600.png` is 2,400,000 without. Fixture `images`: Spine page
`hero_2.png` (512 x 256, MipMap filter) is 699,052.

### 7.4 Basis Universal `.basis` files

All fields are little-endian and packed. The header is 77 bytes:

| Offset   | Size | Field                                                                |
| -------- | ---- | -------------------------------------------------------------------- |
| 0        | 2    | signature `sB`                                                       |
| 2        | 2    | version (0x13)                                                       |
| 4        | 2    | header size (77)                                                     |
| 6        | 2    | header CRC-16                                                        |
| 8        | 4    | data size                                                            |
| 12       | 2    | data CRC-16                                                          |
| 14       | 3    | total slices                                                         |
| 17       | 3    | total images                                                         |
| 20       | 1    | texture format: 0 ETC1S, 1 UASTC LDR 4x4; others (HDR) `unsupported` |
| 21       | 2    | flags: bit 0 ETC1S, bit 2 has alpha slices                           |
| 23       | 1    | texture type                                                         |
| 24 to 64 |      | frame rate, user data, codebook and table offsets (not needed)       |
| 65       | 4    | slice descriptions file offset                                       |
| 69       | 8    | extended offset and size                                             |

Each slice description is 23 bytes: image index (3), level index (1), flags (1,
bit 0 means alpha slice), original width (2), original height (2), blocks x
(2), blocks y (2), file offset (4), file size (4), CRC-16 (2). Group the
slices by (image, level). Only in ETC1S does flag bit 0 mark a separate alpha
slice, which transcodes into the same ASTC texture, so ETC1S alpha slices are
skipped; in UASTC the bit only says the slice has alpha, and the slice counts.
A 2D texture, and video frames (one shows at a time), hold image 0 only. An
array, a cube map or a volume holds every image, so every image's levels
count. `invalid` when the header is short, the slice table runs
past the end, or a slice's data runs past the end.

### 7.5 Spine atlas

A Spine atlas is text. Pages are separated by blank lines. The first line of a
page is the image file name, relative to the atlas file. Then `key: value`
lines (`size`, `format`, `filter`, `repeat`, `pma`), then regions. Both Spine
3.x and 4.x use this layout. The reader only needs each page's file name and
its `filter` min value. A page image missing from the build adds note
`spine_page_missing`. The `.atlas` file itself is bucket data.

### 7.6 Output

Per file, as the `[[textures.file]]` rows list it: `path, status, reason?,
format, encoding?, width, height, levels, mips (none, pixi7_pow2,
spine_atlas_filter), gpu_format (rgba8, astc_4x4, or `vk_format_N` for
another uploaded format N), gpu_bytes, astc_bytes`.
Total, as `[textures.total]` lists it: `lo_bytes, hi_bytes, top5,
not_covered`. In the `TextureSummary` they are `low_bytes`, `high_bytes`,
`files` (the rule takes the top 5) and `not_covered`.

Fixtures: `images` (every reader), `broken`, `pixi7-aa`, `pixi8-*`,
`over-budget`.

## 8. Bundle analysis

Owner: D. Files: `bundle/*.rs`. Entry: `bundle::scan`. Its summary gives the
engine to textures and the Howl sites to audio.

### 8.1 What gets parsed

- HTML: every file in bucket html. Tokenise with `html5gum`. Collect
  `<script>` (inline text, `src`, `type`), `<link href>` and `<link
  imagesrcset>` when the `rel` loads it, `src`, `srcset`, `poster` and `data`
  attributes, `href` and `xlink:href` on SVG `<image>`, `<use>` and
  `<feImage>`, `<style>` text and `style` attributes. Ignore `<a href>`. An
  inline script is named after its HTML file in the output (several inline
  scripts in one file are told apart by byte offset).
- JS: every file in bucket js, and every inline script with no `src` that a
  browser runs, by the HTML standard's rules. Its `type` is absent, empty,
  `module` or a JavaScript MIME type (`text/javascript`,
  `application/ecmascript` and the rest), in any case and with surrounding
  whitespace trimmed. With no `type`, a `language` attribute reads as the type
  `text/` plus its value. A classic script with `nomodule` is skipped, since
  iOS Safari runs modules.
- CSS: every file in bucket css, `<style>` text and `style` attributes.
- JSON: every file in bucket data that parses as JSON, and every inline
  script with another type, such as an import map or `application/json`. A
  byte order mark is skipped. JSON gives URLs; audio reads sprite maps itself.
  A source map (extension `.map`, in any case) is not read: a browser fetches
  it only for its developer tools, and one can be megabytes.

Parse JS with `oxc_parser` 0.157.0, `SourceType::mjs()`. If that fails, retry
as a classic script. If both fail, add the file to `invalid` and go on.
Build semantics with
`SemanticBuilder::new().with_build_nodes(true).build(&program)`: without
`with_build_nodes` there is no node store, and `symbol_declaration` and
parent lookups panic.

oxc parses and walks nested code by recursion, with no depth limit, and a
stack overflow aborts the whole process. So each script is scanned on a thread
with a 64 MiB stack, and a script whose brackets nest deeper than 1,000 is
listed in `invalid` without parsing. The fixture bundles nest 16 deep. When no
such thread can be started, the script is listed in `invalid` too, since the
calling thread's smaller stack could overflow.

oxc 0.157 notes: string literal values are `JSStr` (they can hold lone
surrogates); call `.as_str()` and skip `None`. Property keys come from
`PropertyKey::static_name()`, also via `.as_str()`. oxc changes its API often,
so pin the exact version.

### 8.2 Evaluating expressions

A small evaluator (`eval.rs`) turns an option value into a known value or
`Unknown`. It handles:

- number, string, boolean and `null` literals, templates with no
  expressions, and array literals;
- the globals `undefined`, `NaN` and `Infinity`, and `void x`, which is
  `undefined` whatever `x` is;
- `!x`, `-x` and `+x`: `!0` is true and `!1` is false (minifiers write
  booleans this way);
- `devicePixelRatio`, `window.devicePixelRatio`, `self.devicePixelRatio`,
  `globalThis.devicePixelRatio`, when `window`, `self` or `globalThis` is not
  a local binding: the profile's `device_scale_factor`;
- `Math.min(...)` and `Math.max(...)` over known numbers;
- a call to a function whose body is one `return` of an expression: a
  function declaration, or a variable bound once to an arrow function or a
  function expression. Each parameter takes what its argument gives, read
  where the call is; a missing argument is `undefined`. The function is not
  followed when its name is written again, when it calls itself, when a
  parameter is a rest, has a default value or is destructured, when two
  parameters share a name, or when any code writes a parameter (`g(x = 3, x)`
  would change it between arguments);
- `a || b`, `a && b` and `a ?? b` when `a` is known: JavaScript's rule picks
  the side. When `a` is `Unknown`, so is the result, because it could be
  either side (`r.resolution || 1` is `Unknown`, not 1);
- `<`, `<=`, `>`, `>=`, `==`, `===`, `!=` and `!==` on two known numbers, and
  `test ? x : y` where `test` is known;
- an identifier: follow its symbol (oxc_semantic) to the one value written to
  it, a declarator `init` or a single plain assignment. More than one write,
  `+=`, `++`, a parameter or a destructured binding gives `Unknown`. A binding
  to an object literal whose keys the script writes elsewhere is not followed:
  `o.x = v`, `o.x += v`, `o.x++`, `delete o.x`, `o.x` as a destructuring or
  `for` target, or `o` or `o.x` as the first argument of `Object.assign`,
  `Object.defineProperty` or `Object.defineProperties`. So a site behind it is
  not read, as with a computed key;
- an object literal's keys, with each spread of an object literal (or a
  binding to one) read in place. A spread it cannot read, or a computed key
  such as `[k]: v`, hides every key before it and every absent key.

`number_range` bounds a number that is not static. A static number is a range
of one value. `test ? x : y` with an unknown test is the union of the two
branches. `Math.min` and `Math.max` combine the ranges of their arguments, and
an argument with no range counts as any number or NaN, so `Math.min(x, 2)` is
at most 2, or NaN. `a ?? b` with a range for `a` is `a`, since a number is never
nullish. `a || b` is `a` when the range of `a` holds neither zero nor NaN, else
the union of both sides, and NaN only if `b` may be NaN. A resolution that may
be NaN is `Unknown`. Some games pass `resolution: Sf(window.devicePixelRatio,
e)` with `function Sf(e,t){return Math.min(e||1,t?1.5:2)}`, so their
resolution is 1.5 to 2.

Two limits stop the evaluator, so deep, circular or fanning-out bindings end
as `Unknown`. The depth limit is 16, counting nested expressions, bindings and
spreads together. The step limit is 1,000 expressions and spreads for one
value or one object's keys, which stops arrays of arrays that grow at every
level.

### 8.3 Render config sites

Minified code has no names, so a site is found by shape. An object literal
that is the first argument of a call or `new` (or an identifier bound to one
object literal) is a **render site** when all three hold:

1. It has at least one signal key: `antialias`, `resolution`, `preference`,
   `autoDensity`, `resizeTo`, `webgl`, `webgpu`.
2. Every non-spread key is a Pixi Application option name (v7 and v8 union,
   built from the published `.d.ts` of pixi.js 7.4 and 8.x): `accessibilityOptions,
   antialias, autoDensity, autoStart, background, backgroundAlpha,
   backgroundColor, bezierSmoothness, canvas, canvasOptions, clearBeforeRender,
   context, culler, depth, eventFeatures, eventMode,
   failIfMajorPerformanceCaveat, forceCanvas, forceFallbackAdapter, gcActive,
   gcFrequency, gcMaxUnusedTime, gpu, height, hello, manageImports, multiView,
   powerPreference, preferWebGLVersion, preference, premultipliedAlpha,
   preserveDrawingBuffer, renderableGCActive, renderableGCFrequency,
   renderableGCMaxUnusedTime, resizeTo, resolution, roundPixels, sharedTicker,
   skipExtensionImports, textureGCAMaxIdle, textureGCActive,
   textureGCCheckCountMax, textureGCMaxIdle, transparent, useBackBuffer,
   useContextAlpha, view, webgl, webgpu, width`.
3. The callee is a member call named `init` (v8 `app.init`), or the object has
   at least one key that only an Application takes: `autoDensity, autoStart,
   background, backgroundAlpha, backgroundColor, canvas, clearBeforeRender,
   failIfMajorPerformanceCaveat, forceCanvas, hello, powerPreference,
   preference, premultipliedAlpha, preserveDrawingBuffer, resizeTo,
   sharedTicker, useBackBuffer, view, webgl, webgpu`.

Checked on real minified bundles of pixi.js 7.4.3 and 8.22.0: rule 2 rejects
v7's internal `initFromOptions` calls (they have `alpha` and `stencil`), and
rule 3 rejects Pixi's own texture and render target calls such as `new
X({width, height, resolution, antialias})`. Each fixture bundle gives exactly
one site, the app's.

**Values.**

| Option            | Read                                                                                                                                                                                                                           | Default when the key is absent                                        |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------- |
| `antialias`       | evaluator, as JavaScript truthiness                                                                                                                                                                                            | false (v7 and v8)                                                     |
| `resolution`      | evaluator, stored as milli-units: a positive number, rounded, that fits in 32 bits                                                                                                                                             | 1000 (v7 `settings.RESOLUTION`, v8 default)                           |
| `forceCanvas`     | evaluator, as truthiness. True makes the order 2D only. A value the evaluator cannot read makes the order `Unknown`                                                                                                            | absent                                                                |
| `preference`      | a string goes first, then the rest of webgl, webgpu, canvas; an array is the whole order. A falsy value is the default order. An unknown name, or a value the evaluator cannot read, is `Unknown`. The site asks for the first | v8: `webgl` (auto-detect order webgl, webgpu, canvas); v7: WebGL only |
| `autoDensity`     | evaluator                                                                                                                                                                                                                      | false                                                                 |
| `resizeTo`        | top level only, as Pixi's resize plugin reads it. A falsy value is no resize. A value the evaluator cannot read counts as a resize. A spread the evaluator cannot read may set it, so the frame is assumed                     | absent                                                                |
| `width`, `height` | evaluator: a positive number, rounded                                                                                                                                                                                          | absent                                                                |

Pixi 8 merges the chosen renderer's nested options over the rest
(`{...options, ...options.webgl}`), so a key in `webgl` (or `webgpu`, or
`canvasOptions`) wins for `antialias`, `resolution`, `autoDensity`, `width`
and `height`. A falsy nested value adds nothing. A nested value the evaluator
cannot read makes each of those five options `Unknown`. When the order is
`Unknown`, any backend's nested options may win, so any of the three keys
makes those five options `Unknown`.

**Defaults set by a script.** A script can change a default instead of passing
the option: Pixi 7's `settings.RESOLUTION = x`, Pixi 7.0 and 7.1's
`settings.RENDER_OPTIONS.antialias`, or `X.defaultOptions.antialias` and
`X.defaultOptions.resolution` (Pixi 7.2 and later, and Pixi 8). They are
matched by property name only: any `X.RESOLUTION`, and `antialias` or
`resolution` on any `X.defaultOptions` or `X.RENDER_OPTIONS`. When any
script in the build assigns one of these, every site without that key reads it
as `Unknown`, since the write may run before or after the site. The resolution
note then shows the write, such as `default: PIXI.settings.RESOLUTION =
devicePixelRatio`. The fixture bundles, Pixi included, make no such write.

A site keeps each option on its own. An `antialias` or `preference` that is
`Unknown` makes only its own measure unknown. A `resolution` keeps the range
the evaluator gives, rounded outward to milli-units (a low bound down, a high
bound up), so the range only widens. A static value counts too: 1.5006 is 1.500
to 1.501, and only a whole number of thousandths is one value.

### 8.4 Canvas bytes

The canvas measure is a floor: the colour buffer of the backing store, which
every canvas that draws holds. Antialias buffers, depth and stencil buffers and
the GPU context come on top. Their size rests on numbers that only a device
run can measure, so dyno does not report them (R7).

```
css size   = profile.viewport      if resizeTo is set                     size_from = "resizeTo"
           = (width, height)       if both are positive known numbers,    size_from = "width and height"
                                   rounded
           = profile.viewport      otherwise                              size_from = "assumed"
floor res  = the low bound of the resolution, else 1 (or the high bound
             when it is below 1)
backing    = round(css_w * res_milli / 1000) x round(css_h * res_milli / 1000)
bytes      = backing_pixels * 4
```

`model::canvas_floor(css, resolution_milli)` is this formula. Bundle and rules
both call it, and nothing else computes a canvas. The profile frame is
`HostProfile::reels().viewport`, 402 x 874 (R6). When the resolution or the
size is set at run time, the floor takes resolution 1 or the profile's frame,
and the measure's note says so.

`autoDensity` changes only the CSS size of the element, not the backing store,
so it does not enter the formula. The modelled renderer is the first one in
the site's order that the profile has: WebGL and 2D always, WebGPU only when
`webgpu = true`. So a `preference` of `webgpu` is modelled as WebGL while the
profile has `webgpu = false`, and `["webgpu", "canvas"]` is modelled as 2D. The
site records `modelled_as`. Every backend draws into a backing store, so every
site whose Pixi can start has a floor, a 2D canvas and an `Unknown` order
included. When the profile has none of the site's backends (`["webgpu"]` with
`webgpu = false`), Pixi throws at start: `modelled_as` is none and the site
has no canvas, reason `no_available_backend`.

Several sites: report all, sorted by file and offset, as
`BundleSummary.render_sites`. The canvas and backend measures use the one with
the largest floor (the first of equal ones). Canvases that live at once hold
at least the largest one, so it is a floor of the total too. A site whose Pixi
cannot start adds a `scope.not_covered` line, such as `a site with no canvas:
no_available_backend at byte 32 of main.js`. When no site can start, every
render measure is `unsupported` with reason `no_available_backend`.

**A game that draws with HTML and CSS.** A game with no canvas does not get a
canvas of 0 bytes, because 0 reads as free. When the build
has an HTML page and no render site, no library in the rendering bucket (see
below), no
script that asks for a canvas context, no script that did not parse, and no
external URL, `static.render.backend` is `none`. The other three render
measures are `unsupported` with reason "no canvas: the game draws with HTML
and CSS", so the browser run measures the page's layers. A script asks for a
canvas when it reads `getContext` or `transferControlToOffscreen` as a property
of any object (a call, `.call`, `.bind` or an alias all do), names
`OffscreenCanvas` or `HTMLCanvasElement`, or holds the string `canvas` or a
`<canvas` tag; a page has one when its markup holds a `<canvas` tag. A plain
call `getContext(...)` does not count: Svelte has its own function with that
name. A context reached through a key built at run time, such as
`c["get" + "Context"]`, is not seen. The scope says that scripts from hosts
built at run time are not checked: every Stake Engine game builds its RGS URL
at run time.

Fixtures: `pixi8-inline` and `pixi7-aa` (1206 x 2622 backing, 12,648,528
bytes), `pixi8-webgpu` (804 x 1748, 5,621,568), `pixi7-opts` (2160 x 3840 from
a fixed 720 x 1280 stage, 33,177,600). `duplicates`, `over-budget` and
`zip-input` have a page and no script, so they draw with HTML; `howler-sprites`
has a page with Howler and no canvas, so it does too.

### 8.5 Engine versions

**Library buckets**. Each library the scan detects belongs
to one bucket, by what it does: `rendering` (Pixi), `audio` (Howler) or
`other` (web-sdk, an SDK that bundles its own Pixi, which is detected as Pixi).
`dyno_schema::library` holds the libraries and their buckets, with no default:
a library added later gets a bucket in the same change. Only the rendering
bucket, with canvas use, decides whether a game draws with HTML. The scan
detects a library only by a signature it knows, so it never detects, or
buckets, a library it does not know; a renderer it does not know is still
caught by its canvas use. No more buckets: the scan detects no other library.

The Pixi banner is a template literal whose first quasi starts with `PixiJS `
and whose last quasi ends with ` - https://pixijs.com` (Pixi 7) or ` -
http://www.pixijs.com/` (Pixi 8). Its major comes from the version when the
version is read, else from that site.

| Engine                | Detection                                                                                                                                                                                                                              | Version                                                                                                                                                                                    | Reliability                                                                                                                                   |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Pixi 7                | the banner; 7.4.3 has `` `PixiJS 7.4.3 - ${...} - https://pixijs.com` ``                                                                                                                                                               | a semver right after `PixiJS ` in the first quasi                                                                                                                                          | reliable for 7.x                                                                                                                              |
| Pixi 8                | the banner; 8.22.0 has `` `PixiJS ${Ci} - ${...} - http://www.pixijs.com/` ``                                                                                                                                                          | resolve the first expression with the evaluator. `Ci` is declared `var Ci;` and assigned `Ci="8.22.0"` inside an esbuild lazy-init function, so the evaluator's single-write rule finds it | reliable when it resolves. Do not grep for semver strings: the same bundle has deprecation constants such as `"8.0.0"`, `"8.3.4"`, `"8.21.0"` |
| Pixi, fallback        | a comment containing `pixi.js` and a semver on the same line                                                                                                                                                                           | the semver, unless the script's banner gives another major                                                                                                                                 | heuristic: only if the build kept comments                                                                                                    |
| Pixi, last resort     | the banner without a version the evaluator can read                                                                                                                                                                                    | major only, from the site in the banner                                                                                                                                                    | heuristic; version `unsupported`                                                                                                              |
| Howler                | the legal comment `howler.js v2.2.4` (esbuild keeps `/*!` comments, inline or at the end)                                                                                                                                              | the semver after `howler.js v`                                                                                                                                                             | reliable when comments are kept                                                                                                               |
| Howler, fallback      | the string `HTML5 Audio pool exhausted, returning potentially locked audio object.` or a key or property `html5PoolSize`                                                                                                               | none                                                                                                                                                                                       | detection is reliable; version `unsupported`                                                                                                  |
| web-sdk (StakeEngine) | at least 2 of the string literals `/wallet/authenticate`, `/wallet/play`, `/wallet/end-round`, `/bet/event`, plus the literal `rgs_url`, plus a `__sveltekit_` name read as a global or as a property of any object, in the HTML or JS | always `unsupported`: its packages are versioned 0.0.0, so no version is in the bundle                                                                                                     | heuristic. Its Pixi and Howler copies are reported by their own rows (the plan notes record Pixi 8.8.1 at web-sdk commit 1843d60)             |

`Engine` for textures is `Pixi7` or `Pixi8` from the major. With two Pixi
copies, use the one in the file of the render site with the largest canvas
(else of the first site), else the first copy by file.

### 8.6 External URLs

A URL is external if it points outside the build, and it counts only where
the game can request it. HTML attributes, CSS and JSON name what they load, so
their URL values count. A script also holds text it never requests, such as
the documentation links in a library's error and log messages, so a
JavaScript string counts only in a **request position**:

- an argument of a call or `new` whose function, method or constructor is
  named `fetch`, `open` (XHR and `window.open`), `sendBeacon`,
  `importScripts`, `register` (service workers), `load` (`Assets.load`),
  `from` (`Texture.from`), `URL`, `Request`, `WebSocket`, `EventSource`,
  `Worker`, `SharedWorker` or `Audio`. A minified callee such as `(0,
  x.fetch)` counts by its last name;
- the source of `import(...)`, of a static `import` and of `export ... from`;
- the value of an object key, or of an assigned property, named `src`,
  `srcset`, `href`, `poster` or `url`, such as Howl's `src` or `img.src = u`;
- the second argument of `setAttribute` when the first is one of those names,
  in any case.

From a request position the scan follows parentheses, both branches of `?:`,
both operands of `||`, `&&` and `??`, the last expression of a comma list,
array elements, the start of `a + b + ...` and of a template, a variable
written once, and `o.key` where `o` is a variable bound to an object literal
(with section 8.2's rules for bindings). It stops after 16 levels or 1,000
steps for one position. A chain of `+` uses one level, however long, but each
`+` and each part read takes a step. A text longer than 8 KiB is not read:
servers commonly refuse a longer request line. Each string it reaches is read
by the first three rows:

| Where                                                | What counts                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| JS string literal, or a template with no expressions | the **whole** value is a URL: `http`, `https`, `ws` or `wss` scheme, or protocol-relative `//host`, with a host that has a dot (or is an IPv6 literal) and no whitespace                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| JS template with expressions                         | read as `a + b + ...` is: a template that starts with an expression starts with what that expression reads as, and each expression that reads as one whole text is joined in, so with `const h = 'x.com'`, `` `https://${h}/a` `` is the URL `https://x.com/a`. At the first expression read at run time, or one that would make the text longer than 8 KiB, the text so far is the start: if it holds a full host and a `/` (`` `https://cdn.x.com/a/${n}.png` ``), record it. If it is a scheme and part of a host only (`` `https://${host}${path}` ``), count one `dynamic_host_site`. Ignore a start that is protocol-relative (`` `//----${x}` `` appears in shader code) |
| JS `a + b + ...`                                     | the start, the leftmost part, is read first. A whole start joined with parts that each read as one whole text is whole again, so `'https://api.' + 'x.com/a'` is the URL `https://api.x.com/a`. At the first part read at run time, or one that would make the text longer than 8 KiB, the text so far is only the start and follows the template rule: `'https://api.' + env` counts one `dynamic_host_site`                                                                                                                                                                                                                                                                   |
| HTML                                                 | `src`, `srcset` (each candidate's URL, read by the HTML standard's rules, so a URL may hold commas; descriptors are skipped, not checked), `poster`, `data`, SVG `href` and `xlink:href`, and `href` and `imagesrcset` on a `<link>` whose `rel` loads it: `apple-touch-icon`, `apple-touch-icon-precomposed`, `dns-prefetch`, `icon`, `manifest`, `mask-icon`, `modulepreload`, `preconnect`, `prefetch`, `preload` or `stylesheet`                                                                                                                                                                                                                                            |
| CSS                                                  | `url(...)` and `@import`, in one pass that skips comments and strings, so `content: "url(x.png)"` does not count. A quoted value ends at its closing quote; a raw newline (LF, CR or form feed) or the end of the text first means no value. An escaped newline, CR LF as one, joins the value                                                                                                                                                                                                                                                                                                                                                                                  |
| JSON                                                 | string values that are whole URLs, in data files other than source maps and in import maps and other data blocks in HTML                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |

Drop W3C namespace URIs (`http://www.w3.org/2000/svg`, `.../1999/xhtml`,
`.../1999/xlink`, `.../XML/1998/namespace`, `.../2000/xmlns/`,
`.../1998/Math/MathML`). The URL rules are a subset of a browser's. The
scheme needs two or more slashes after it, and a run of `/` and `\` is read as
`//`. The host ends at the first `/`, `\`, `?` or `#`. The port is empty or
digits for a number up to 65535. Strip the user info, the query and the
fragment. A URL inside the path, as a proxy's path can hold, goes through
`dyno_schema::scrub`, so it loses its own user info too. The stored URL is written as a browser reads it: the scheme and host
in lowercase; the port as a number without leading zeros, dropped when it is
empty or the scheme's default (80 for `http` and `ws`, 443 for `https` and
`wss`; a protocol-relative URL keeps it); and `\` in the path as `/`. Output: `hosts` (lowercase, sorted), `urls` (sorted, each with its
sorted `sources`) and `dynamic_host_sites`. A template or `+` chain that
builds a host is one site, however many request positions reach it.

**What is reliable.** The whole-literal rule never reports text inside a
longer string, so library banners and error messages (`"see
https://pixijs.com/..."`) drop out: the Pixi 7 and 8 bundles give no URLs of
their own. The request-position rule drops whole URLs that are only
messages, such as Svelte 5's error and warning links
(`https://svelte.dev/e/<code>`, passed to `throw Error(...)` and
`console.warn(...)`). A URL found sits where the game can request it; it is
not proof the game does. **Heuristic gaps:** hosts built from variables are
counted, not named; URLs assembled from pieces with no whole literal are
missed; a URL that reaches a request through a function parameter, a wrapper
the list does not name (a game's own `get(url)`), a key the list does not
name, or another script is missed; a `<base href>` is not applied.

Fixtures: `pixi8-inline` (6 URLs from JS, HTML and CSS, 1 dynamic host),
`pixi8-webgpu` (one protocol-relative URL), others none.

### 8.7 Howl sites

A `HowlSite` is an object literal (or a binding to one) passed to `new`, with
key `src` (a string or an array of string literals) and every other key a Howl
option, Howler 2.0's `xhrWithCredentials` and the spatial plugin's options
included. A key `sprite`, when there, is an object literal whose values are
arrays of 2 or 3 elements, the first two known numbers with a start that is
not negative. A falsy `sprite`, such as `null`, is no sprites, as Howler reads
`sprite || {}`. The callee name does not matter; minified bundles write `new
k.Howl(...)`. A plain call such as `Assets.load({src})` is not a site. A `src`
with no `sprite` is still a site: it ties its files into one sound group. An
object with a computed key or a spread the evaluator cannot read is not a
site. Sites whose `src` is not literal (for example `{src: t.src}` fed from
JSON) are skipped; the JSON is read by audio. One options object passed to
several `new` calls is one site, at the first call, since the site depends
only on its keys.

### 8.8 Output

`[bundle]` lists `scanned`, `invalid`, `versions`, `render_site` rows and
`external`. `scanned` is the files scanned for scripts: every JS file, and
every HTML file with an inline script. `invalid` is the scanned files with a
script that did not parse. A render site row has `file, callee, antialias,
antialias_from, resolution_milli, resolution_from, preference, modelled_as,
auto_density, size_from, canvas_css, canvas_backing, canvas_pixels,
canvas_bytes`.

- `antialias_from` is `literal`, `default` or `option_not_static`.
- `size_from` is `resizeTo`, `width and height` or `assumed`.
- `modelled_as` is none when the order is `Unknown`, or when the profile has
  none of the site's backends.
- `resolution_from` is a short note for people; tests do not compare it.

The `static.bundle.engines` evidence counts the files scanned. A script that
did not parse is named in the `scope.not_covered` of every measure the bundle
scan builds.

The `BundleSummary` keeps what later steps read: `engine`, a `RenderSite` per
site, the `HowlSite` list, `invalid_scripts` and `draws_with_html`. A fixture's
`[bundle]` table lists `draws_with_html = true` when the scan gives it. The measures are in
[section 2](#measures). D may expose richer row types from `bundle/` for its
own tests: `bundle::analyse` gives a `BundleReport` with a `RenderConfig` row
per site.

The stub scan passes `Inventory::default()`. For it, `bundle::scan` keeps its
stub result, so the stub scan stays all `missing`.

### 8.9 What is reliable and what is heuristic

Reliable, given the build as supplied:

- Parsing. oxc parses each script as a module, then as a classic script. A
  script that fails both is listed in `invalid` and named in every built
  bundle measure's `scope.not_covered`.
- Which inline scripts run, by the HTML standard's `type`, `language` and
  `nomodule` rules.
- Antialias and resolution that a site writes. A value the evaluator cannot
  read is `Unknown`, and its measure then says `option_not_static`. A
  resolution that is not one value keeps the range the evaluator proves.
- The backend order. An order the evaluator cannot read is `Unknown`, never
  WebGL by default, and the backend measure then says `option_not_static`.
- The canvas floor, once the resolution and the CSS size are known. It is the
  shared `canvas_floor` formula: 4 bytes a backing pixel. It is a floor, not
  the canvas's cost (R7).
- Pixi 7 versions from the console banner, Howler versions from its legal
  comment, and URLs that are a whole string value in HTML, CSS, JSON or a
  script's request position. These are text the libraries and the game write
  themselves.
- URL privacy. User info, the query and the fragment are dropped before a URL
  is stored, so tokens in them never reach the result. A port other than the
  scheme's default is kept.

Heuristic:

- Finding render sites. Minified code has no names, so a site is found by the
  shape of its options object (section 8.3). It holds for the Pixi 7.4.3 and
  8.22.0 bundles in the fixtures. A wrapper that builds the options from
  function parameters is not seen, and a site in a lazy chunk the HTML never
  loads still counts. Another WebGL library's options, such as
  `{antialias: true, powerPreference: "high-performance"}` for Three.js, can
  pass all three rules, and so can any library's `x.init({resolution: 2})`,
  since rule 3 takes every `init`. A Pixi 7 `new Application({width, height,
  resolution, antialias})` with no Application-only key fails rule 3 and is
  missed.
- Absent keys. An absent `antialias` or `resolution` is taken as Pixi's
  default unless a script writes that default in a form section 8.3 lists.
  Those writes are matched by property name only, so a write to another
  library's `RESOLUTION` makes the option `Unknown` too.
- One script at a time. Each JS file and each inline script is parsed alone,
  so a binding declared in another script is not followed.
- Writes the evaluator does not see. A binding whose keys the script writes
  directly (`o.antialias = true`, `Object.assign(o, more)`) is not followed,
  so its site is not seen. A write through another name (`p = o;
  p.antialias = true`) or inside a function `o` is passed to still leaves the
  site with the literal value. Defaults set by
  `Object.assign(X.defaultOptions, ...)` or by replacing `defaultOptions` are
  not seen.
- When a write runs. The single-write rule reads a variable written once as
  that value everywhere, even where the code reads it before the write runs,
  or in a branch that may never run. Pixi 8's banner version is read through
  this rule, so it is heuristic too. A banner a transpiler rewrote as
  `"PixiJS ".concat(...)` or `+` is not detected.
- The CSS size. `resizeTo` is taken as the profile's frame, whatever it names,
  and a `resizeTo` the evaluator cannot read counts as one. Without `resizeTo`
  or a static `width` and `height`, the frame is assumed, not Pixi's default
  of 800 x 600. This guesses that the game sizes its canvas to the screen in
  code the scan does not follow. A fractional `width` or `height` is rounded
  before the resolution scales it, since `canvas_floor` takes whole pixels.
- The backend. `forceCanvas` is read as a 2D canvas without a check for
  pixi.js-legacy, the only Pixi 7 build with a 2D renderer, and although Pixi 8
  ignores it. A `canvas` preference is
  modelled as Pixi 8.22 runs it, with its 2D renderer; older Pixi 8 releases
  throw instead.
- Howl sites. An options object passed to any `new` is a Howl site when every
  key is a Howl option and `src` is a string or an array of strings. A
  `sprite` that is not fully static skips the site, so audio never sees half a
  sprite map. Sprites come out sorted by name. One options object passed to
  several `new` calls is one site.
- Pixi from comments, the last-resort Pixi major, and web-sdk detection
  (section 8.5). A Pixi comment version counts only on the line that names
  `pixi.js`, and only when its major agrees with the banner, because license
  blocks list other packages' versions.
- URLs built from pieces, hosts built at run time (counted, not named) and
  `<base href>`, as section 8.6 says. `srcset` descriptors are not checked.
- Request positions. A script's URL counts only where section 8.6's list
  hands it to a request. A URL passed through a function parameter, an API
  the list does not name, or another script is missed, and a listed name
  used for something else (any method called `load`, `open` or `from`) counts
  its URL arguments anyway. A number in a template or `+` chain, and `o.key`
  through another variable or `?.`, are read as run-time parts. A script's
  `srcset` value counts only when it is one whole URL, with no descriptor.
- Deep nesting. Brackets are counted over the raw text, strings and comments
  included, so a script can be listed as invalid without being deep. Long
  chains with no brackets, such as `a ? b : c ? d : ...`, still recurse; an
  unoptimised build handles 10,000 links on the 64 MiB stack.
- Byte offsets assume UTF-8. A file with invalid UTF-8 is read with
  replacement characters, which can move offsets after the first bad byte.

## 9. Rules

Owner: A. File: `rules.rs`. Entry: `rules::check`. It reads only
`model::Summaries`, the profile and the budget file. Rule ids are `RuleId` in
`dyno-schema/src/ids.rs`, shared with lab and field. The budget file
is `dyno-schema`'s `Budget`, and `examples/budget.toml` shows every key.

All thresholds are **proposals** (R8), to be set from device runs. For scale,
a draft budget is 250 to 300 MB per iOS frame.

### 9.1 Budget keys

A limit is in whole MB (1 MB is 1,048,576 bytes), in basis points, or a
severity (`allow`, `warn` or `fail`). A value warns or fails when it is
**above** the limit, never when it equals it, as in `dyno-score`. No fixture
value sits on a default.

Each budget id says in `ids.rs` who checks it (`CheckedBy`):

- `Score`: `dyno-score` gives a budget verdict from the measure, and the
  verdict can set the status. The rule reads the same limit and adds the
  fixes. The verdict reads `not_covered`: a partial value gives a verdict
  only when no bigger value could give a worse one. So the verdict can be
  `incomplete` where the budget rule below warns.
- `Rule`: the limit is the rule's threshold, because `dyno-score` cannot
  compare it with the measure's value: a saving, a share per sprite map, or a
  severity for any duplicate. It gives no budget verdict, but the rule's
  finding counts toward the status (see `docs/cli.md`).

When a check has no budget file, or the file does not set a key, the rule uses
the key's default:

| Budget key                   | Checked by                 | Default (proposal) | Key in the earlier draft                                                   |
| ---------------------------- | -------------------------- | ------------------ | -------------------------------------------------------------------------- |
| `audio.decoded_mb`           | score                      | warn 64, fail 128  | `static.audio.pcm_48k_warn_bytes`, `static.audio.pcm_48k_fail_bytes`       |
| `audio.stereo_saving_mb`     | rule `audio.stereo`        | warn 4             | `static.audio.stereo_saving_warn_bytes`                                    |
| `audio.silence_share_bp`     | rule `audio.silence_share` | warn 1000          | `static.audio.silence_share_warn_bp`                                       |
| `audio.silence_saving_mb`    | rule `audio.silence_share` | warn 1             | `static.audio.silence_saving_warn_bytes`                                   |
| `textures.decoded_mb`        | score                      | warn 160, fail 256 | `static.textures.lo_warn_bytes` (192 MiB), `static.textures.lo_fail_bytes` |
| `render.antialias`           | score                      | `warn`             | `static.render.antialias`                                                  |
| `render.resolution_uncapped` | score                      | `warn`             | `static.render.resolution_warn_milli`                                      |
| `files.duplicate`            | rule `files.duplicate`     | `warn`             | `static.files.duplicate`                                                   |

- The texture warn is the skeleton's 160, not 192. No fixture has textures
  between 160 and 192 MiB, so no fixture outcome moves.
- The resolution cap is `RESOLUTION_CAP_MILLI` (2000) in `model.rs`, not a
  budget key, so a measure never depends on the budget file.
- The top 5 of `textures.over_budget` is a constant in `rules.rs`, not a key.
- `render.antialias` defaults to `warn`, as in `examples/budget.toml`.

### 9.2 The rules

| Rule id                          | Value                                                                          | Verdict (`outcome` in fixtures)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | `saved_bytes`                                                 | Fixes                                                                                                                                                                                                                                                                        | Fix text                                                                                                                                                                                                                                                                                       |
| -------------------------------- | ------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `audio.pcm_over_budget`          | `AudioSummary.decoded_bytes_48k`                                               | budget rule (below) on `audio.decoded_mb`, with audio's `not_covered`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | `decoded_bytes_48k - decoded_bytes_24k`                       | one `resample_to_24k`, no path                                                                                                                                                                                                                                               | "Decoded audio is {v} MB at 48 kHz. Run the AudioContext at 24 kHz to save {s} MB, or load long music when it is needed."                                                                                                                                                                      |
| `audio.stereo`                   | sum of `decoded_bytes_48k / 2` over the `stereo_groups` that are not `cancels` | budget rule on `audio.stereo_saving_mb`, with audio's `not_covered`. The default has no fail limit, so by default it never fails                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | the value                                                     | one `downmix_to_mono` per representative that halving shrinks and that is not `cancels`, so a sound of 0 or 1 byte gets none                                                                                                                                                 | "{n} stereo sounds decode to {d} MB. Export them as mono to save {s} MB.", then "Mono would change the sound of {c} of them, so listen first." when some are `changes_the_sound`, and "Keep {k} other stereo sounds as they are: mono would cancel them out of phase." when some are `cancels` |
| `audio.silence_share`            | the largest `silence_bp` of the sprite maps                                    | a level (warn or fail) applies when a map is above that level's limit in both silence keys. A level that either key leaves unset never applies. Else `incomplete` if a sprite map is not `estimated`, or `invalid_scripts` is not empty; else pass                                                                                                                                                                                                                                                                                                                                                                                                  | sum of `silence_bytes_48k` of the maps at the outcome's level | one `trim_sprite_silence` per such map, with its source                                                                                                                                                                                                                      | "{p}% of {file} is not covered by any sprite in {source}. Trim the gaps to save up to {s} MB."                                                                                                                                                                                                 |
| `render.antialias_or_resolution` | the largest site's floor `canvas.bytes`                                        | the worse of two checks on what the sites prove: the `render.antialias` severity if a site that may draw on the GPU has antialias on, and the `render.resolution_uncapped` severity if a site's resolution is above `RESOLUTION_CAP_MILLI`. `allow` skips a check. A proven warning or fail wins over a site that sets its options at run time. Else `incomplete` if no site was found, no site can start, or a site sets antialias (on the GPU) or the resolution at run time. A check that would pass is `incomplete` while `invalid_scripts` is not empty, since such a script may hold a worse site. A game that draws with HTML and CSS passes | the fixes' savings                                            | `cap_resolution` for each site whose resolution is one value above the cap, saving `floor(res) - floor(cap)`, which is w x h x 4 x (r^2 - 2^2). No path. Turning antialias off has no fix: what it saves needs a device run (R7), and `Fix.saves_bytes` is a required number | "{file} turns antialias on. Set `antialias: false`. A device run will measure what it saves, so this fix has no number yet." and "{file} sets resolution {r}. Cap `resolution` at 2 (`Math.min(devicePixelRatio, 2)`) to save {s} MB."                                                         |
| `textures.over_budget`           | `TextureSummary.low_bytes`                                                     | budget rule on `textures.decoded_mb`, with textures' `not_covered`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | sum over the top 5 of `gpu_bytes - astc_bytes`                | one `shrink_textures` per top file that ASTC would shrink, so a file already in an ASTC-sized format gets none                                                                                                                                                               | "Textures decode to {lo} to {hi} MB. The {n} largest: {list}. Ship them as KTX2 (Basis) to save about {s} MB."                                                                                                                                                                                 |
| `files.duplicate`                | number of duplicate groups                                                     | the `files.duplicate` severity if there is a group; else, or with `allow`, pass                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | sum of `wasted_bytes`                                         | for a group with `keep`, one `remove_duplicate` per other copy, saving its bytes. For a group without, one `keep_one_copy` named by the group's first path, saving the group's `wasted_bytes`                                                                                | "{n} files are copies of files the game names: {list}. Delete them." and "The build does not show which copy the game loads of {first paths}. Keep that copy and delete the others.", then "This saves {w} KB to download and up to {d} MB decoded."                                           |
| `files.unreferenced`             | Phase 2: stays `incomplete` with "not built yet"                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |                                                               |                                                                                                                                                                                                                                                                              |                                                                                                                                                                                                                                                                                                |

**Budget rule.** `fail` if the value is above the fail limit; `warn` if above
the warn limit; else `incomplete` if `not_covered` is not empty; else `pass`.

**Findings.** A `Finding` has the verdict, the reason when it is `incomplete`,
and the fixes. In `[rules]`, `saved_bytes` is the sum of the fixes'
`saves_bytes`, and a rule's `items` are its fixes' paths. There are two
exceptions: the render rule names the worst site's file, and `files.duplicate`
names each group by its first path. The render saving rests on the
antialias-off proposal (R7). A `remove_duplicate` fix also sets
`saves_decoded_bytes`, its share of the group's `decoded_bytes_saved`, and
`saved_decoded_bytes` is their sum. `Finding` has no value or text field, so:

- `rules::reports` gives a `RuleReport` per rule, with the finding, the value
  (`value_bytes`, `value_bp`, `value_groups`), the items and the fix text, for
  `tests/rules.rs`;
- the fix text column is the wording for a later text report.

In a `RuleReport`, items are sorted by saved bytes, largest first, then by
path. A fix text about one file uses the singular, such as "The largest" and
"Ship it". `CheckResult::new` sorts the fixes in the result by path. The audio
saving assumes the game changes the context rate, which is a code change; the
stereo saving is per file and needs a re-export.

A rule that would pass while a measure it reads has no value (any status but
`estimated` or `measured`) is `incomplete` with "<measure id>: <reason>". Its
family left the summary empty, which reads as a build with nothing to flag. A
rule that is `incomplete` on its own keeps its own reason, such as "no render
config found". The render rule is the exception: it reads each site itself, and
its measures have no value on purpose for a game that draws with HTML or a 2D
canvas, where the rule passes.

With no build, `rules::without_build()` gives every static rule `incomplete`
with the reason `NEEDS_A_BUILD`.

Fixture outcomes at the default thresholds: `over-budget` (audio fail, stereo
warn, textures fail), `howler-sprites` (silence warn), `pixi8-inline`,
`pixi7-aa` and `pixi7-opts` (render warn), `duplicates` and
`zip-input` (duplicate warn), `broken` (5 rules `incomplete`).

## 10. Crates

Versions checked on crates.io on 2026-11-06. Every crate below is in
`[workspace.dependencies]` and in `dyno-static`'s `Cargo.toml`, and
`Cargo.lock` resolves these exact versions. oxc is pinned exactly; the rest use
the usual caret.

- oxc 0.157 needs Rust 1.97, so the workspace `rust-version` and the CI
  minimum Rust job are 1.97.
- `symphonia` has `default-features = false` and only the codec and format
  features the readers know (section 5.4). Its MPL-2.0 files are used
  unmodified (R4).
- `zip` has `default-features = false` and the feature `deflate-flate2`. Its
  `deflate` feature would switch flate2 to the zlib-rs backend, and the
  fixtures' gzip sizes assume miniz_oxide.

| Crate                                                                                        | Version  | Licence              | Use                                                                                   | Notes                                                                                     |
| -------------------------------------------------------------------------------------------- | -------- | -------------------- | ------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `oxc_parser`, `oxc_ast`, `oxc_ast_visit`, `oxc_semantic`, `oxc_allocator`, `oxc_span`        | =0.157.0 | MIT                  | JS parse, visit, symbols                                                              | released 2026-11-02; the API changes between minor versions                               |
| `imagesize`                                                                                  | 0.15.0   | MIT                  | PNG, JPEG, WebP, AVIF (HEIF), GIF sizes                                               | also knows KTX2, but use `ktx2` for that                                                  |
| `ktx2`                                                                                       | 0.5.0    | Apache-2.0           | KTX2 header, levels, DFD                                                              | `Reader::new` validates all bounds                                                        |
| `sha2`                                                                                       | 0.11.1   | MIT or Apache-2.0    | file hashes, `build_id`                                                               |                                                                                           |
| `flate2`                                                                                     | 1.1.10   | MIT or Apache-2.0    | gzip size, and deflate inside a zip                                                   | default `miniz_oxide` backend; the gzip numbers in the fixtures depend on it              |
| `brotli`                                                                                     | 9.1.1    | BSD-3-Clause and MIT | brotli size                                                                           | in the items of `static.bytes.compressed`                                                 |
| `zip`                                                                                        | 9.4.0    | MIT                  | zip input                                                                             | features above                                                                            |
| `walkdir`                                                                                    | 2.5.0    | MIT or Unlicense     | folder walk                                                                           | `follow_links(false)`                                                                     |
| `html5gum`                                                                                   | 0.9.1    | MIT                  | HTML tokens                                                                           | a tokenizer is enough; `lol_html` 3.0.3 also works but is a rewriter; `tl` 0.7.8 is stale |
| `serde_json`                                                                                 | 1.0.154  | MIT or Apache-2.0    | sprite JSON, JSON URLs                                                                |                                                                                           |
| `toml`                                                                                       | 1.2.0    | MIT or Apache-2.0    | tests read `expected.toml`                                                            | dev-dependency                                                                            |
| `tempfile`                                                                                   | 3.27.0   | MIT or Apache-2.0    | tests make build folders                                                              | dev-dependency                                                                            |
| `memchr`                                                                                     | 2.8.3    | MIT or Unlicense     | `OggS` and sync scans                                                                 |                                                                                           |
| `symphonia` (features `aac`, `adpcm`, `isomp4`, `mkv`, `mp3`, `ogg`, `pcm`, `vorbis`, `wav`) | 0.6.1    | MPL-2.0              | decodes audio for silence and stereo (section 5.4); cross-checks the readers in tests | released 2026-08; no Opus decoder                                                         |

Considered and not used:

| Crate             | Version | Why not                                                                                                                                     |
| ----------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `ogg`             | 0.9.2   | a page reader; the tail scan with CRC is 60 lines, and we need no packets. The skeleton had it as planned; it is dropped from the workspace |
| `lewton`          | 0.10.2  | a Vorbis decoder, last release 2021; we only need the ident header                                                                          |
| `mp4parse`        | 0.17.0  | MPL, last release 2023; heavy for 6 boxes                                                                                                   |
| `mp4`             | 0.14.0  | last release 2023                                                                                                                           |
| `matroska`        | 0.30.1  | maintained, but reads the whole file for metadata; we need 6 elements                                                                       |
| `basis-universal` | 0.3.1   | C++ bindings for transcoding, last release 2023; we only read the header                                                                    |
| `basisu`          | 0.1.0   | new pure-Rust transcoder, too young                                                                                                         |
| `image`           | 0.25.10 | decodes pixels; we never decode                                                                                                             |
| `swc_ecma_parser` | 47.0.0  | the plan's fallback parser; oxc handled every bundle tested                                                                                 |

## 11. Cross-check results

The fixture values were derived with a scratch reader written to this
document, then compared with symphonia 0.6.1, imagesize 0.15.0, ktx2 0.5.0 and
ffprobe. Where they differ, this design is deliberate:

| Case                          | symphonia 0.6.1                                                                                         | This design                                                     |
| ----------------------------- | ------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| Ogg Opus                      | `num_frames` 48312 = last granule; pre-skip 312 reported as `delay`                                     | 48000: subtract pre-skip                                        |
| M4A                           | `num_frames` 45124 = `mdhd` duration; edit list ignored                                                 | 44100 from the edit list                                        |
| ADTS                          | `num_frames` from an estimate (`approximate_frame_count`)                                               | walk every frame (same result on the fixtures)                  |
| WebM                          | no `num_frames`; time base 1/1000                                                                       | Duration minus CodecDelay, in microseconds                      |
| MP3 with a Lavf tag           | `delay` 1105 and `padding` 875 (adds the 529-sample decoder delay to one and removes it from the other) | raw tag values 576 and 1404; the same total, so the same length |
| Truncated WAV, truncated ADTS | reads the header and reports the full length, no error                                                  | `invalid`                                                       |
| Truncated KTX2                | (imagesize) reports 64 x 64                                                                             | `invalid` (`ktx2::Reader::new` fails)                           |

## 12. Open questions

1. **Frame size and scale** (R6). Settled: the frame is an iPhone 17, 402 x
   874 at DPR 3. Render scales of 1/2 and 2/3, as Stake
   Reels scales games down, come later.
2. **Antialias and GPU context cost** (R7). Settled: dyno reports only the
   colour buffer floor. The device run measures the rest, and then a
   `turn_off_antialias` fix can come back with a measured saving.
3. **WebGPU.** The model treats `preference: 'webgpu'` as WebGL because the
   host profile assumes iOS 18 Safari. The reference phone is now the iPhone
   14 Pro Max, and its iOS version is recorded at the first run. Decided: the
   profile follows the phone. On iOS
   26 a new profile version turns WebGPU on, after a check that Chromium on
   Linux can run it, and games that ask for WebGPU also get a WebGL run to
   compare.
4. **Mipmaps** (R5). Pixi 8 games that turn on `autoGenerateMipmaps` per
   texture are under-counted.
5. **Brotli.** The brotli sizes are items of `static.bytes.compressed`, so no
   budget can check them. Do they need their own measure id?
6. **Rule thresholds and the status.** Settled: a static finding that warns
   or fails gives that status, so every rule threshold can set it.
7. **Defaults without a budget file.** Settled: the rules use the defaults in
   [section 9.1](#91-budget-keys), and their findings count toward the status.
   So a check with no budget file can warn or fail.
8. **Decoded saving of duplicates.** Settled: `Fix.saves_decoded_bytes` holds
   it, beside the download bytes in `saves_bytes`.
9. **Unknown canvas size.** With no `resizeTo` and no literal size, Pixi's
   default is 800 x 600, but games resize later. This design assumes the
   profile frame.
10. **Thresholds** (R8). Every default in section 9 is a proposal until device
    runs set it.
11. **Silence is uncovered time.** A sprite-mapped file played by offset from
    code would count as silence. `static.audio.silence_by_decode` gives the
    true silence of each sound ([section 5.4](#54-decoding)). The
    `audio.silence_share` rule still reads the sprite maps.
12. **Images that are not textures.** `<img>` and CSS images count as textures.

## 13. Not covered

Formats the readers do not handle (status `unsupported`), and gaps in the
fixtures:

- Audio: FLAC, AIFF, CAF, RF64, Opus or ALAC in MP4, chained Ogg streams (only
  the first logical stream counts), HE-AAC v2 with implicit PS in ADTS (reads
  as mono). No fixture has HE-AAC or a VBRI header. No fixture has a stereo
  sound that cancels; the unit tests in `audio/listen.rs` make one.
- Textures: KTX1, DDS, PVR, raw `.astc` files (the 16-byte header is easy; add
  when a game needs it), Basis HDR and XUASTC, cube maps and arrays (formula
  given, no fixture), SVG.
- Engines: Pixi 5 and 6, Phaser, Cocos, three.js. Spine runtime version is not
  detected.
- Zip input: entries compressed with anything but store or deflate (for
  example Deflate64, bzip2 or Zstandard). `zip` is built with deflate only.
- web-sdk version: never in the bundle.
- Evaluator: no fixture uses a ternary or `??`.
