# The dyno command line

dyno has four commands. `check`, `compare` and `doctor` each write one JSON result to stdout, or to the file `--out` names. Each one also writes a short summary for people to stderr. `mcp` gives `check` and `compare` to an MCP client instead. The summary has no stable format, so no tool may parse it. There is no other output format.

The check summary gives the status and why, the decoded memory and the build size, and each rule that warns or fails with its 3 largest fixes. Sizes are in MB with one decimal. A floor shows as "at least", and a range shows its two ends. The last lines tell you where the full result is, and give its `result_hash`.

`--help` marks each command and flag with the plan phase that builds it. In the skeleton every command runs, but no measure is built yet. So every check, doctor report and unedited compare is `incomplete`.

## Commands

### `dyno check <TARGET>...` (Phase 1)

`TARGET` is a build folder or .zip, a launch URL, or one of each. A value that starts with `http://` or `https://` is a launch URL. Anything else is a build. dyno parses a launch URL as a browser does, and a URL that does not parse is a usage error.

- A build gives the static scan: what dyno can tell from the files alone.
- A launch URL adds a browser run: dyno loads the game in a harness page and waits for idle.

| Flag                                | Phase | What it does                                                                                                                                                                                                                                                            |
| ----------------------------------- | ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--budget <FILE>`                   | 1     | The budget file. TOML, or JSON when the name ends in `.json`. See [`examples/budget.toml`](../examples/budget.toml).                                                                                                                                                    |
| `--out <FILE>`                      | 1     | Write the JSON to this file instead of stdout.                                                                                                                                                                                                                          |
| `--host-profile <NAME>`             | 1     | How the host page embeds the game, and the contract it checks. Default `reels`, the only profile so far.                                                                                                                                                                |
| `--game <NAME>`                     | 1     | A game name label for the result. Not in the hash. Not with `--games`.                                                                                                                                                                                                  |
| `--release <VERSION>`               | 1     | A release version label for the result. Not in the hash. Not with `--games`.                                                                                                                                                                                            |
| `--games <FILE>`                    | 1     | Check every game in a games file instead of a target.                                                                                                                                                                                                                   |
| `--shard <I/N>`                     | 1     | With `--games`: check only shard I of N.                                                                                                                                                                                                                                |
| `--driver <DRIVER>`                 | 1     | The browser driver. `cdp`, the default, is the built-in Chromium driver. `exec:<PROGRAM>` runs PROGRAM, which must speak the runner protocol, and needs a launch URL. `serve` serves the build to a browser on the network, and needs a build folder and no launch URL. |
| `--serve-host <HOST>`               | 1     | With `--driver serve`: the address a browser reaches this machine at. Default: this machine's address on its local network.                                                                                                                                             |
| `--serve-port <PORT>`               | 1     | With `--driver serve`: the harness page's port. The game is on the next port. Default 7370.                                                                                                                                                                             |
| `--serve-query <QUERY>`             | 1     | With `--driver serve`: the query string the game loads with, such as a demo session's `sessionID` and `rgs_url`. No result records it.                                                                                                                                  |
| `--scenario <alone\|aged\|overlap>` | 2     | The harness scenario. Default `alone`. Any other scenario needs a launch URL.                                                                                                                                                                                           |
| `--spins <N>`                       | 2     | Play N demo spins after load, with N at least 1. Needs a launch URL.                                                                                                                                                                                                    |
| `--device`                          | 2     | Also run the game on the connected iPhone. Needs a launch URL.                                                                                                                                                                                                          |

A flag that needs a launch URL is checked before any work starts. With `--games`, such a flag needs a `launch_url` on every selected game.

With `--driver exec:<PROGRAM>`, dyno starts PROGRAM directly, with no shell and no arguments. A path with no `/` is found on `PATH`. dyno writes one `RunRequest` line of JSON on its stdin, then closes stdin. The program must write `DriverMessage` lines of JSON on stdout: `hello` first, with the `protocol_version` of the request, and `done` or `error` last. `packages/schema/json/run-request.schema.json` and `packages/schema/json/driver-message.schema.json` give their shapes. Its stderr is free text. The run fails when the program breaks the protocol, exits before `done` or `error`, or does not end within the run's `timeout_ms`. The reason then names the problem, with the end of stderr when the program exited early.

With `--driver serve`, dyno serves the build folder and a harness page, and waits up to 10 minutes for a browser to open the page. The browser can be on another device, such as Safari on an iPhone on the same Wi-Fi, Safari in the iOS Simulator, or a desktop browser. dyno prints the URL to open. The harness page is on `--serve-port` and the game is on the next port, so the game frame is cross-origin to the page around it. dyno adds the probe's script tag at the start of each HTML page's `<head>`, and the probe sends its events to the harness page, which sends them to dyno. A served run gives the same lab measures as the CDP driver, with these differences:

- The only requests it sees are requests for files in the build. Requests to other hosts, such as a CDN or the game's server, are not counted.
- `lab.js_heap` is missing, because Safari does not give the JS heap. The device run gives the phone's memory instead.
- `lab.self_check.frame_dpr` is the browser's own pixel ratio. A desktop browser at a ratio of 1 or 2 makes it invalid against the profile's 3, because it is not the phone.
- A game that needs a session gets it from `--serve-query`. The result records the build, not the URL or its query.
- A browser slows the timers of a page it does not show. Keep the page on screen, and keep the device awake, for the whole run.

The build must be a folder: unzip a .zip first. While the run waits and runs, anyone on the network can load the build from the two ports.

A games file starts with `schema_version = 0`, then lists one `[[game]]` table per game. Each has an `id` and a `build`, a `launch_url` or both. A `release` label is optional. Builds are relative to the games file. Shards split the games in id order: game k, counting from 0, falls in shard (k mod N) + 1. The output is a catalogue result. Its status is the worst status of its games. A selection with no games is `incomplete`, because it checked nothing.

### `dyno compare <BASE> <HEAD>` (Phase 1)

`BASE` and `HEAD` are result files from `dyno check`, not build folders. So compare never runs a check again, and a CI job can compare against the stored result of the last release.

| Flag              | Phase | What it does                                                               |
| ----------------- | ----- | -------------------------------------------------------------------------- |
| `--budget <FILE>` | 1     | Apply this budget file to both sides, instead of each side's own verdicts. |
| `--out <FILE>`    | 1     | Write the JSON to this file instead of stdout.                             |

Compare checks each side's `result_hash`. If a side does not match its content, someone edited it, and the compare is `invalid`. The check hashes the canonical form of the side's `deterministic` JSON as read, before dyno parses it into its own types. So a field that another dyno version added still counts, and a list in another order is an edit. A file that is not a result, or has another `schema_version`, is a usage error. So is a result with a measure id or other value that this dyno does not know: a newer field is read past, but a newer value is not.

The diff covers the deterministic parts only: every static measure whose status or value moved (with `delta`, head minus base, for whole numbers), every budget verdict that changed, and every file added, removed or changed in its SHA-256 (`suspect_assets`). Run measures differ from run to run, so they never enter a compare. The status is the worst of three: `fail` when a budget verdict got worse (pass to warn or fail, or warn to fail), `incomplete` when either side is incomplete, and `invalid` when either side was edited. The stderr summary lists the changed verdicts, the moved measures, and how many files changed.

### `dyno doctor` (Phase 2)

Checks that this Mac can run the game on an iPhone: Xcode, a connected iPhone, and trust. Until Phase 2 builds it, it reports its phase and `incomplete`. It runs no command against a device.

| Flag           | Phase | What it does                                   |
| -------------- | ----- | ---------------------------------------------- |
| `--out <FILE>` | 2     | Write the JSON to this file instead of stdout. |

### `dyno mcp` (Phase 3)

A local MCP server on stdin and stdout, in the same binary. It lets an MCP client, such as an AI coding tool, run dyno. There is no AI inside dyno. Only MCP messages go to stdout. dyno writes one line to stderr when it starts, and a client can keep stderr as a log. The server stops when the client closes stdin, and exits with 0. It exits with 70 when the session does not start or fails.

To add it to Claude Code:

```sh
claude mcp add dyno -- dyno mcp
```

A client that reads a JSON config, such as a project's `.mcp.json`, takes:

```json
{
  "mcpServers": {
    "dyno": { "command": "dyno", "args": ["mcp"] }
  }
}
```

The server has three tools. A relative path is relative to the folder the server runs in. A tool that works gives two text blocks: canonical JSON first, then the summary that the command writes to stderr.

| Tool          | Arguments                                                                                         | What it gives                                                                                                                  |
| ------------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `check`       | `build` or `launch_url` or both. Optional: `budget`, `host_profile`, `game`, `release` and `out`. | The result that `dyno check` gives. With `out`, dyno also writes the result to that file, so that `compare` can read it later. |
| `compare`     | `base` and `head`. Optional: `budget`.                                                            | The result that `dyno compare` gives.                                                                                          |
| `read_result` | `path`, a result file from `dyno check`                                                           | Its `status`, its `result_hash`, and `hash_matches`, which is false when the file was edited. Then the check summary.          |

A check or compare that ends `fail`, `warn`, `incomplete` or `invalid` is a tool call that worked, because the status is in the result. An error that stops the command, such as a bad path or flag, is a tool error (`isError` is true) with the message. A check with a `launch_url` uses the CDP driver and the `alone` scenario. The tools have no spins, device or driver argument.

## Statuses

Each measure has one of these statuses:

| Status        | Meaning                                                                                                        |
| ------------- | -------------------------------------------------------------------------------------------------------------- |
| `measured`    | Read from a browser or a device.                                                                               |
| `estimated`   | Arithmetic on what the build supplies. Every static value is estimated, never measured.                        |
| `unsupported` | dyno cannot measure it for this input, and the reason says why. For example, the footprint needs a launch URL. |
| `missing`     | It should be there and is not. A stub gives `missing` with the reason "not built yet".                         |
| `invalid`     | It failed its own check. For example, a run whose game frame's DPR was not 3.                                  |

Only a change in size, such as growth, can be negative. A negative value or bound in any other measure makes it `invalid`.

A measure is `required` when the result needs it. Each measure in a result is a row of a `measures` list. The check's inputs decide which measures are required, never the rows themselves:

- A build makes the phase 1 static measures required. A check with no build has no static measures to require, so they are `unsupported` and not required.
- A launch URL makes the phase 1 lab measures required.
- Phase 2 measures are required only when a flag asks for them: `--spins` for the play measures, and `--device` for the device measures.
- Model measures are never required, because the model is report-only in Phase 1.

A required measure with no row counts as `missing`. A row that does not fit the check makes the result `invalid`. A row does not fit when the inputs do not expect it (such as a lab measure with no launch URL), when it is in the wrong part of the result, or when its `required`, `source`, `phase`, `unit` or `scope.covers` is not what its id and the inputs give. Static measures go in `deterministic` and the others in `run`. A row also does not fit when its status disagrees with its value: `measured` and `estimated` need a value, and `missing` and `unsupported` have none. A result never holds the same measure id twice. A check that would write one stops with an internal error.

The overall status of a check comes from four things: the measures, the budget verdicts, the static rule findings and the run coverage. When several statuses apply, the first in this list wins:

| Status       | When                                                                                                                                                         |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `fail`       | A budget or a static rule fails.                                                                                                                             |
| `invalid`    | A required measure is invalid, or a measure row does not fit the check.                                                                                      |
| `incomplete` | A required measure is missing or has no row, a budget could not be checked, or a static rule could not be checked because a value from the build is missing. |
| `warn`       | A budget or a static rule warns.                                                                                                                             |
| `pass`       | None of the above. A stub never passes.                                                                                                                      |

An `unsupported` measure does not block a pass.

A static rule uses its limits from the budget file. A limit that the file does not set takes its default, and a check with no budget file uses every default. So a check with no budget file can warn or fail. A static rule's finding counts like this:

- A finding that warns or fails gives that status.
- An incomplete finding gives `incomplete`, because a value that dyno should read from the build is missing. Examples are a sound that did not decode, a script that did not parse, and a measure that is `missing`.
- An incomplete finding does not count when its rule is not built yet (the reason "not built yet"), or when a measure that the rule reads is `unsupported`. An `unsupported` value needs a browser or device run, so it does not make a static check incomplete. This is the rule for measures too: a `missing` measure counts and an `unsupported` one does not.

A lab finding is in the `run` part, so it does not set the status.

A budget checks one measure's value. When the measure's `scope.not_covered` names a gap, such as a sound that did not decode, the value is a floor: the full value can only be bigger. So the budget gives a verdict only when no bigger value could give a worse one:

- A floor above the fail limit fails.
- A floor above the warn limit warns when the budget has no fail limit.
- A flag that is on gets its severity. A flag set to `allow` passes, on or off.
- Otherwise the budget is `incomplete`. So a floor above the warn limit is `incomplete` when there is a fail limit, because the full value could be above it.

A `floor` uncertainty alone does not make a value partial. Only `scope.not_covered` does.

## Exit codes

| Code | Meaning                                                                                                             |
| ---- | ------------------------------------------------------------------------------------------------------------------- |
| 0    | `pass`, or `warn`                                                                                                   |
| 1    | `fail`: a budget or a static rule fails                                                                             |
| 2    | `incomplete`                                                                                                        |
| 3    | `invalid`: a required measure failed its check, a measure row does not fit the check, or an input result was edited |
| 64   | Usage or config error: bad flags, or an unreadable target, budget, games or result file                             |
| 70   | Internal or I/O error, such as an output file that cannot be written                                                |

`invalid` gets its own code, because a CI job must not treat a broken run or an edited result as a plain incomplete one.

The exit code comes from the status alone, and the status comes from the deterministic part alone. So the same `result_hash` always gives the same exit code.

## The result hash

A result has two parts:

- `deterministic`: everything the same build, tool version and budget file give on any machine. Static measures, budget verdicts, static findings, the build manifest, the inputs, the versions, the status and the run coverage. The run coverage is the one exception: with a launch URL or `--device`, it says whether this machine's run gave every required measure, and the status follows it.
- `run`: everything that differs between machines and runs, and the labels that must not change the hash. Timestamps, durations, the machine, the tool build, the target paths and URLs, the release labels, the driver, the browser, device and model measures, the lab findings and the tier.

What the hash promises depends on the inputs. A check with only a build gives the same `result_hash` and exit code on any machine. With a launch URL or `--device`, the run coverage depends on the machine and the browser, and the status follows it. The hash then identifies the result, but a check on another machine is not expected to give the same hash.

`result_hash` is the lowercase hex SHA-256 of the RFC 8785 canonical JSON of `deterministic`. To make the bytes the same on every machine:

- Sizes are integer bytes, so float formatting cannot change the output.
- Every integer is in the safe integer range, -(2^53 - 1) to 2^53 - 1, where a double holds every integer exactly. A measure with a number outside it is `invalid`, and the number is dropped. Any other integer outside it stops dyno with an internal error.
- Every list has a complete sort key, so its order never depends on the order dyno built it in: measures by id, files by path, verdicts by budget id, and findings by rule id, then by their canonical JSON. A result never holds the same measure id, budget id or file path twice.
- Paths are relative to the build root and use `/`.

The whole output is canonical JSON too, with one final newline.

A launch URL is recorded without its user info, query string or fragment, which can carry passwords and session ids, and it is in the `run` part. Its path is kept, so a session id in the path stays. Text from a driver, such as its error, enters a result only after every URL in it with a `://` gets the same treatment, including a URL inside another URL's path. A URL that does not parse keeps only its scheme. A part of the text with no `://`, such as a bare path and query string, is not changed.
