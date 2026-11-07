# Architecture

dyno is one Rust binary built from a Cargo workspace. The skeleton has the full command line and the full result shape. Every measure is a stub that reports `missing` with the reason "not built yet", or `unsupported` when the input cannot give it. Each later step fills in stubs and does not change the shape.

## Crates

| Crate | What it owns |
|---|---|
| `dyno-schema` | The result shape: every type that crosses a process or language boundary. The measure, rule and budget ids, the budget and games files, canonical JSON, the result hash, the runner protocol messages, the probe events and the host's unclean exit record. It generates `schema/` (JSON Schema) and `ts/` (TypeScript). |
| `dyno-static` | The static scan of a build folder or .zip. One module per measure family: `inventory`, `bundle/`, `textures/`, `audio/`, `bytes` and `duplicates`, then `rules` for the static findings. `model` holds the types that pass between them. Each family has one entry function. `docs/design/static-scan.md` gives the design. |
| `dyno-run` | The browser run: the `Driver` trait, JSON lines framing for the runner protocol, `exec`, the driver that runs an outside program, and `lab`, which turns a run's messages into the lab measures and contract findings. |
| `dyno-cdp` | The Rust CDP driver. `browser` finds and launches Chromium, `connection` is the session layer over one WebSocket, `harness` serves the host page that embeds the game, and `run` drives one load-and-idle run. `probe/probe.ts` is the in-page probe, and `probe/build.mjs` strips its types into two builds: `probe/probe.js`, the lab build that dyno injects over CDP, and `probe/probe.field.js`, the field build for a platform, without the blocks between `// lab: begin` and `// lab: end`. `CdpDriver` is the driver `dyno check <launch-url>` uses. `cargo run -p dyno-cdp --example run -- <launch-url>` prints a run's messages. |
| `dyno-ios` | The iPhone lab (Phase 2): `doctor` and the device measures. |
| `dyno-score` | Budget verdicts, the overall status, run coverage, the memory model, the provisional tier and the compare diff. |
| `dyno-cli` | The `dyno` binary. It parses flags, calls the crates above in order, and writes the output. `mcp` gives the same check and compare to an MCP client through `rmcp`. |

## How a check runs

1. `dyno-static` scans the build, if there is one. The families run in the order of the data flow: inventory, bundle, textures, audio, bytes, duplicates. Then the static rules run on their summaries, the host profile and the budget file. Without a build, every static measure is `unsupported` and every static rule is `incomplete`.
2. `dyno-score` checks each budget in the budget file against its static measure. A value with a gap in its scope is a floor, so it gives a verdict only when no bigger value could give a worse one. Otherwise the budget is incomplete. A rule's threshold gives no verdict: its rule checks it.
3. `dyno-score` adds the model measures, which are `unsupported` without a launch URL. With a launch URL, `dyno-run` runs the browser through a driver. With `--device`, `dyno-ios` adds the device measures.
4. `dyno-score` works out from the inputs which measures each part must have. It sets the run coverage from the run measures, then the overall status from the static measures, the budget verdicts, the static findings and the run coverage.
5. `CheckResult::new` in `dyno-schema` makes a measure with a bad number invalid, sorts every list, rejects a repeated measure id, budget id or file path, and computes the hash.

## Rules for each step

- **Keep the shape.** Add fields and measures. Do not rename or remove them. A breaking change bumps `SCHEMA_VERSION`.
- **One place for ids.** Every measure, rule and budget id is defined in `dyno-schema/src/ids.rs`. A measure's entry in `ids.rs` also gives its source, phase, unit and what it covers. Each static measure belongs to exactly one family's `MEASURES` list, and a test checks it.
- **Static values are `estimated`.** Only browser and device readings are `measured`.
- **Nothing in the deterministic part may depend on the machine.** Use integer bytes, relative `/` paths and sorted lists. Timestamps, paths as given, URLs and labels go in the `run` part. The one exception is the run coverage, and the status that follows it. It stays in the hashed part so that the hash always gives the exit code. A build-only check has no run coverage to vary, so its hash reproduces on any machine. A check with a launch URL or `--device` gets a hash that identifies its result, but it is not expected to reproduce.
- **Static findings count toward the status.** A finding that warns or fails gives that status. An incomplete one counts only when a value from the build is missing, not when the rule is not built yet or reads an `unsupported` measure. Lab findings are in the run part, so they do not count.
- **Outside text is scrubbed where it comes in.** A driver error is a fixed code plus a detail that `DriverError::new` runs through `dyno_schema::scrub::scrub`. So no driver in another crate can put the user info, query string or fragment of a URL with a `://` into a reason. Use the same scrub for any other driver or subprocess text.
- **The runner protocol messages are in `dyno-schema`, not `dyno-run`.** The fallback driver is a TypeScript script, so the messages need generated TypeScript types like the probe events.

## The browser run

`dyno_cdp::run_cdp` serves a harness page on a local site that is never the game's, launches Chromium headless with site isolation and `--force-device-scale-factor` from the host profile, gives the page the host profile's viewport by emulation, and attaches to it with every frame paused before its first script. The first frame the page attaches is the game frame; every target attached under it, such as a nested frame or a worker, joins its family, and the family's requests are the game's (`data:` and `blob:` URLs cross no network, so they do not count). Before it runs, the driver enables its network, adds the `__dynoProbe` binding and the probe, and auto-attaches its own children. The probe sends each event to the binding. It numbers each canvas and each WebGL texture, so a resize, a re-upload of a texture level or a deleted texture can be followed. The harness page reports each message the game sends it through `__dynoHost`; the driver puts it on the frame's clock with the frame's `performance.timeOrigin`. Requests are joined by request id across sessions, and a request counts when it ends on the game frame's session. The frame is idle when no request is in flight and none started for `idle_after_ms`. Then the driver reads the heap, runs the self-checks in an isolated world (so the probe's wrappers do not see them) and records when the frame's clock started on its own clock, sends the host profile's active message, watches for 2 s and ends with `done`. `timeout_ms` bounds the whole run, calls included; a run that reaches it ends with `error`. Chromium gets 5 s to exit after `Browser.close`, and its profile is deleted after it.

`dyno check --driver exec:<program>` uses `dyno_run::exec::ExecDriver` instead. It lets an outside program run the browser, such as a Playwright script or a driver that is not in this repo, with no change to dyno. dyno starts the program directly, with no shell and no arguments. It writes the `RunRequest` as one JSON line on the program's stdin and closes stdin. The program writes the same `DriverMessage` lines on stdout that the CDP driver gives: `hello` with `RUNNER_PROTOCOL_VERSION` first, and `done` or `error` last. dyno stops reading at that last message. So `lab` gives the same measures from both drivers. These are protocol errors: a line that is not a message, a first message that is not a `hello` with the same protocol version, and a stdout that closes before `done` or `error`. The last error quotes the end of the program's stderr. `timeout_ms` bounds the whole run. At the deadline, dyno kills the program and the run fails. A program that sent its last message gets 5 s to exit, then dyno kills it. dyno reads stderr on its own thread, so a program that writes much to stderr never stops. Every error detail goes through `DriverError::new`, so the scrub removes secrets from the quoted stderr too.

The CLI tests set `DYNO_CHROMIUM` to a path that does not exist, so a check with a launch URL fails the same way on every machine. The Chromium test in `crates/dyno-cdp/tests/chromium.rs` and the probe freshness test are ignored by default: they need Chromium and Node. Run them with `cargo test -p dyno-cdp -- --ignored`.

## The MCP server

`dyno mcp` is a local MCP server on stdin and stdout. It uses `rmcp`, the official MCP Rust SDK, with only its server, its tool macros and its stdio transport. Its tools call the same functions as the commands: `check::check_target`, `compare::compare_files` and `compare::read_result`. These functions return the result and write nothing. The commands write the JSON and the summary after the call, so in MCP mode only MCP messages go to stdout. The server runs on a tokio runtime with one thread. The CDP driver starts its own tokio runtime, and a runtime cannot start inside another. So each tool call runs on its own thread, and the server waits for its answer.

## Generated files

`schema/` and `ts/` are generated from `dyno-schema`, and the help and stub result snapshots are in `crates/dyno-cli/tests/snapshots/`. Tests fail when any of them is stale. After a change you make on purpose, run `DYNO_UPDATE_GENERATED=1 INSTA_UPDATE=always cargo test --workspace` to replace the saved copies with the new output, then read the diff.
