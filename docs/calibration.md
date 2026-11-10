# Phone calibration

The method that turns phone runs into the memory model and its error report. It is fixed before
any phone run. dyno runs no command against a phone: a person runs the phone and records each run.

## Decided

- **The reference phone** is the iPhone 14 Pro Max, 6 GB. Every run of the fit, the held-out
  games and the baseline is on this one phone, with one iOS version.
- **More phones later.** Calibration across several known iPhone models is a future phase. It
  starts from this phone's fit.
- **iOS and WebGPU.** The host profile follows the phone. The phone runs iOS 26, so `reels`
  version 2 has WebGPU on. Version 1 assumed iOS 18 Safari, with WebGPU off. The browser run gives
  the game a WebGPU adapter only when the profile has WebGPU: in headless Chromium on Linux it is
  SwiftShader's. On a Mac, Chrome uses the Mac's GPU and gives WebGPU whatever the profile says.
  The probe sees WebGL texture uploads only, so a game that draws with WebGPU has
  `lab.textures.uploaded`, `lab.textures.resident` and `lab.inactive.allocated` unsupported, not
  a false 0.

The template names the reference phone in `[device] phone`. The report adds a note when the runs
are on another phone, or when no iOS version is recorded.

## The method

- **Scenario:** the game alone in the Reels harness, in a fresh Safari process on the reference
  iPhone, until idle: no new requests for 10 s.
- **Baseline:** the same harness page with no game frame, on the same phone and iOS version, in 3
  fresh processes.
- **Phone value:** the median `phys_footprint` of 3 fresh runs of the game, minus the median
  baseline.
- **Kills:** a run the phone kills has no footprint; record it in `killed_runs`, with its syslog
  line in `kill_lines`. Two kills of 3 set the game's tier to `blocked`, and leave it out of the
  fit and the gate; it is still reported. One kill leaves a value biased low, and the report
  notes it.
- **Sound phone values only:** a game with no run left, or whose value is at or below the
  baseline, is reported but left out of the fit and the gate.
- **Error per game:** (model minus phone) divided by phone, signed. Negative is an
  underprediction.
- **Gate:** the total feeds the provisional tier only when all 3 held-out games have a sound
  phone value and their worst underprediction is under 20%. The `playable` line (500 MB, a placeholder until the
  fit) then loses that much headroom: 15% gives 425 MB.
- **Refit** for each iOS major; re-check 5 games for a new SDK or Pixi version.

## The model

footprint = decoded audio + GPU textures + canvas buffers + k x JS heap + b. The fit sets k and b
by least squares on the 10 fit games with a sound phone value. Decoded images that are not textures are not measured yet,
so they are not a part. `dyno-score/src/calibration.rs` holds the method; the parts come from
two recipes, so both can be compared on the same runs before one is chosen for the model
version:

| Recipe   | Audio                      | Textures                             | Canvas                         | JS heap       |
| -------- | -------------------------- | ------------------------------------ | ------------------------------ | ------------- |
| `static` | `static.audio.decoded_48k` | `static.textures.decoded` (one copy) | `static.render.canvas` (floor) | `lab.js_heap` |
| `lab`    | `lab.audio.decoded`        | `lab.textures.resident`              | `lab.canvas.bytes` (floor)     | `lab.js_heap` |

## Each game, before its phone runs

1. Run `dyno check <build> <launch-url> --out results/<game>.json` with the same build and launch
   URL the phone will load. The result needs every part above, and its browser run must pass
   its self-checks (the frame's pixel ratio, and requests from the frame): a part with no value
   or a failed self-check stops the report with its reason.
2. Put the result's dyno version and `result_hash` in the game's `dyno_version` and
   `result_hash`.

## The calibration file

Copy `crates/dyno-score/calibration/template.toml`, fill in `[device]` and one `[[game]]` per
game, and run:

```sh
cargo run -p dyno-score --example calibrate -- path/to/calibration.toml
```

It prints the report of both recipes as JSON: k and b, each game's model, phone value and signed
error, the worst fit and held-out underprediction, whether the total feeds the tier, the line
with headroom, and notes where the runs differ from the method (such as a game with 2 runs, or a
negative k).

## Not built yet

- A check of the device run on the reference phone. `dyno check <build> --driver serve --device`
  reads `device.phys_footprint` from the phone's sysmontap samples (docs/cli.md). It was tried on
  an iPhone 14 Pro Max over Wi-Fi, from Linux and from a Mac, with a test page; a calibration
  game run on it is still to do.

- Model versioning: the fitted k and b as a versioned data file in `dyno-score`, and
  `model.footprint` from it. It waits for the first fit.
