---
default: minor
---

# Results say which run made them, what ran it, and which game they are about

Every check result now has `run.run_id`, a UUID v4 made when the check starts, so a retried upload stays one run. `run.build_link` says how the run ties to the build: `none`, `scanned`, `served` or `asserted`. `run.environment` says what ran: the browser, the OS and the device. The CDP driver gives its Chromium and this machine's OS, `--driver serve` reads the browser's user agent, `--device` adds the phone's model, iOS version and memory from the trace, and an exec driver gives the browser its `hello` names. None of these fields changes the result hash.

`--game-id <UUID>` sets `run.release.game_id`, the game's id on the dyno site. Without it, dyno reads the nearest `dyno.toml` in the build's parent folders, and takes the game's id from its entry for the build, and the entry's name as the `--game` label when none is given. A `dyno.toml` that does not parse is a usage error.
