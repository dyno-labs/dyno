# Changelog

The changes in each release of dyno: the `dyno` binary and the `@dyno-cloud/schema` types, which
share one version. The release pull request writes each section from the change files in
`.changeset/`, and the section becomes the notes of the GitHub Release. See
[`docs/releasing.md`](docs/releasing.md).

## 0.1.2 (2026-11-11)

### Fixes

#### The types' npm package is `@dyno-cloud/schema`

The npm package of the JSON Schema and the TypeScript types is named `@dyno-cloud/schema`. Earlier releases named it `@dyno-labs/schema`, but no version of it was published under that name.

## 0.1.1 (2026-11-10)

### Features

#### Check the empty build with `--baseline`

`dyno check --baseline` checks a one-page build inside dyno instead of a target. Its build id never changes, so a device run of it, `dyno check --baseline --driver serve --device`, gives the phone's footprint with no game, which a game's footprint can be measured over.

#### Run a build folder in Chromium with `--browser`

`dyno check <build> --browser` serves the build folder on 127.0.0.1 and loads it in dyno's own Chromium. So the browser loads the files that the static scan read, and `run.build_link` is `served`, not `asserted` as with a build and a launch URL that you serve yourself. The dyno site counts a served run toward the build. The launch URL is the folder's root, and the run gives the same measures as a CDP run with a launch URL. `--browser` needs a build folder and no launch URL, and works only with the `cdp` driver. With `--games`, every selected game needs a build folder and no launch URL.

#### Run Chromium without its sandbox in a container that has none

A browser run in a container with no user namespaces, such as a self-hosted CI runner in Docker, failed with "Chromium exited before its DevTools endpoint was ready" and no reason. The error now quotes Chromium's last lines, and when Chromium found no sandbox it names `DYNO_CHROMIUM_SANDBOX=off`, which runs Chromium without one. The sandbox stays on otherwise.

#### `device.phys_footprint_peak`: the game's largest footprint on the phone

A device run now gives `device.phys_footprint_peak`, the largest physical footprint of the game's process from the navigation to the end of the run. It is required with `--device`. `device.phys_footprint` keeps the peak as an item, for tools that read it there.

#### `--device` opens the page in the phone's Safari, and stops on a locked phone

Before it records, `dyno check --driver serve --device` opens the harness page in the phone's Safari with `xcrun devicectl`, so there is no URL to type. `--no-open` leaves that to you, and a launch that fails only says to open the URL by hand. A locked phone is a usage error that says to unlock it and set Auto-Lock to Never. A phone that disconnects from xctrace during the run, as one that locks does, no longer leaves the check waiting minutes for the trace: the device measures are missing, and the reason asks whether the phone locked or is on Wi-Fi only. A trace that ended before the run did no longer counts the game as killed.

#### Record the game's memory on a connected iPhone with `--device`

`dyno check <build> --driver serve --device`, on a Mac with Xcode, records an Activity Monitor trace of the connected iPhone with `xcrun xctrace` while the phone's Safari runs the served check. The result gets `device.phys_footprint`, the footprint of the game's Safari process when the frame went idle, with its peak during the run, and `device.kills`. `--device-id` chooses the device when more than one is connected. With `--out`, the trace is kept beside the result.

#### `--runs N` measures the game's own footprint on the phone

`dyno check <build> --driver serve --device --runs 3` runs the empty build and the game three times each, in turn, each in a fresh Safari with its own recording. The result gives the median footprint at idle and the median peak of the game runs, how many of them the phone killed, and `device.footprint_over_baseline`: the median game peak minus the median peak of the empty build, which is the game's own footprint. Each measure has an item per run, and `inputs.device_runs` records N. A run that gave no reading leaves the measures that need it missing, and the reason names the run. Without `--runs`, `device.footprint_over_baseline` is unsupported.

#### `--device` reads the iPhone itself: no Xcode, Linux too, and Wi-Fi

`dyno check --driver serve --device` no longer records with `xcrun xctrace`, which lost a phone on Wi-Fi 3 to 4 s into every recording. dyno now reads the phone with the `idevice` crate, through usbmuxd or straight at the phone's address, on Linux or macOS. `--device-ip <IP>` with `--pairing-file <FILE>` records a phone on the network with no cable and no usbmuxd. The phone needs iOS 17 or later and Developer Mode on.

The measures keep their meaning. `device.phys_footprint` and its peak come from the phone's sysmontap samples, the source of Instruments' Activity Monitor, and `device.kills` now comes from the kernel's jetsam line in the phone's log, so a process that only leaves the samples is not a kill. A soft memory limit that the game went over is kept in the evidence. Safari opens the page through the phone's CoreDevice app service, with `devicectl` as a fallback on a Mac. With `--out`, the recording is saved as `<name>.device.json` in place of a `.trace`. The OS version now has the build, such as `26.6.2 (23G90)`, and `memory_bytes` is the memory that iOS manages, a little under the RAM. `--device-id` takes a UDID, and no longer a device name.

The evidence of the footprint has the new method `sysmontap`. The device catalogue has each model's identifiers, such as `iPhone15,3`, and the iPhone 17e, 18 Pro and 18 Pro Max. `dyno_ios::trace` still reads an Activity Monitor trace that xctrace exported, for traces that a device cloud records.

#### `dyno link` links a build to its game on the dyno site

`dyno link <BUILD> --name <NAME>` finds the game of that name on the dyno site, or makes it, and `dyno link <BUILD> --game-id <UUID>` takes a game the site already has. Either way, dyno writes the build's entry in `dyno.toml` (or the file `--config` names), keeping the file's comments and other entries, so that every later `dyno check` of the build names the game. It needs `dyno login` or `DYNO_TOKEN`.

`dyno upload` and `dyno check --upload` now print the link that the site gives, which is the game's report when the site linked the result to a game, with the game's name. A site that gives no link gets the result's own page, as before.

#### `dyno.toml` can hold the command that builds the games

An optional `[build]` table, with one `command`, says how CI makes the build folders from a fresh checkout. The workflow that the dyno site's setup pull request writes runs it before it checks each game. dyno checks that the command is not empty, and does nothing else with it. `dyno link` keeps the table. A dyno from before this version refuses a file that has one.

#### The schema package lists every measure and every known device

`@dyno-cloud/schema` now has `json/measures.json`, every measure id with its source, phase, unit and scope, and what it counts (`quantity`) and which reading it is (`statistic`), and `json/devices.json`, every iPhone from the iPhone XS and XR to the iPhone 17 family, by the key a device run gives its phone. It also has `json/dyno-toml.schema.json`, the shape of `dyno.toml`.

#### Results say which run made them, what ran it, and which game they are about

Every check result now has `run.run_id`, a UUID v4 made when the check starts, so a retried upload stays one run. `run.build_link` says how the run ties to the build: `none`, `scanned`, `served` or `asserted`. `run.environment` says what ran: the browser, the OS and the device. The CDP driver gives its Chromium and this machine's OS, `--driver serve` reads the browser's user agent, `--device` adds the phone's model, iOS version and memory from the trace, and an exec driver gives the browser its `hello` names. None of these fields changes the result hash.

`--game-id <UUID>` sets `run.release.game_id`, the game's id on the dyno site. Without it, dyno reads the nearest `dyno.toml` in the build's parent folders, and takes the game's id from its entry for the build, and the entry's name as the `--game` label when none is given. A `dyno.toml` that does not parse is a usage error.

#### Run a game in any browser on the network with `--driver serve`

`dyno check <build> --driver serve` serves the build folder and a harness page, then waits for a browser to open the page, such as Safari on an iPhone, Safari in the iOS Simulator or a desktop browser. dyno adds the probe to each HTML page, and the run gives the same lab measures as the Chromium driver, but the JS heap. `--serve-host` and `--serve-port` say where dyno serves, and `--serve-query` gives the game its launch query string, such as a demo session. No result records the query string.

#### Decoded textures count an SVG at its own size

`static.textures.decoded` now counts an SVG whose root element gives an absolute `width` and `height`, such as `width="80" height="40"`, as an RGBA8 image of that size: `w * h * 4`, with Pixi 7's mips for sides that are powers of two. Before, every SVG was left out, so `textures.over_budget` was incomplete for any build with one.

The game can draw an SVG at another size or resolution, so each SVG counted this way is also a gap in the measure's scope (`svg_own_size`). An SVG with only a `viewBox`, or a size in `%` or `em`, has no size of its own and stays out of the total, now with the reason `svg_no_size`. A file named `.svg` with no SVG root element is `invalid`.

#### Upload results to the dyno site with `dyno login`, `dyno upload` and `check --upload`

`dyno login` signs in with a token from the site's Settings page, checks it with the site, and saves it, readable by you only. `DYNO_TOKEN` wins over the saved token, as in CI, and `dyno logout` removes it. `dyno upload <RESULT>...` uploads each result file as it is and prints the link to its page on the site. `dyno check --upload` uploads the result after the check, or each game's result with `--games`, and stops before the check when you are not signed in. A failed upload makes a check that passed exit with 70.

#### WebGPU is on in the Reels host profile

The reference iPhone runs iOS 26, whose Safari has WebGPU on, so the `reels` host profile is now version 2 with WebGPU on. The static scan models a build that asks for WebGPU as WebGPU, not WebGL. The Chromium run gives the game a WebGPU adapter only when the host profile has WebGPU. On Linux, every Chromium run now uses SwiftShader's Vulkan for its graphics, so a WebGPU canvas can draw. On a Mac, Chrome uses the Mac's GPU, which gives WebGPU whatever the profile says. A result made with version 2 has a new `result_hash`, even for a build that does not use WebGPU. A game that draws with WebGPU gets its texture measures as unsupported, since the probe sees only WebGL uploads.

### Fixes

#### A busy game no longer fails the browser run's idle reads

Once the game's network goes idle, dyno reads its heap and runs its self-checks in the game frame. A heavy game on a small machine that draws in software can still be busy then, and these reads failed after 15 s with "no reply in time", which left every run measure missing. They now wait until the run's own deadline.

#### The device footprint's evidence names the model and iOS version in plain words

The evidence of `device.phys_footprint` now reads `an Activity Monitor trace of <name>: <model>, iOS <version>, process com.apple.WebKit.WebContent (<pid>)`, so a tool can read the model and version from it.

#### `--device` records first, then opens Safari, with no pause

`dyno check --driver serve --device` now starts the recording before it opens the page in the phone's Safari, and no longer waits 5 s between the two. On an iPhone 14 Pro Max on a cable, a Safari launch during a recording left the recording on.

#### A served run is not incomplete for a heap reading Safari never gives, and no result holds the phone's own name

Under `--driver serve`, `lab.js_heap` is now `unsupported` ("the browser gives the page no heap reading"), not `missing`. Safari gives the page no heap reading, so a served run on an iPhone was always `incomplete`. It now gets the status of its rules. The CDP driver still reads the heap, and a missing reading there still makes the run incomplete.

The evidence of `device.phys_footprint` now reads `an Activity Monitor trace of the phone: <model>, iOS <version>, process com.apple.WebKit.WebContent (<pid>)`. It no longer names the phone, which is often its owner's name, and a device error's reason has the phone's name taken out. The lab measures' evidence reads `one run in the harness page`, as it is not always Chromium.

#### A GitHub Actions token in `DYNO_TOKEN` that the site refuses says to check `DYNO_TOKEN`

`dyno upload` and `check --upload` send the token in `DYNO_TOKEN` as they always did, so the dyno Action can pass the short-lived OIDC token that GitHub Actions gives a run. When the site refuses a token that is not a CLI token, the error now says to check `DYNO_TOKEN`, not to run `dyno login` again, which cannot help in CI.

## 0.1.0

The first release.

- `dyno check <build>` scans a build folder or `.zip` and gives the static measures, rules and
  fixes.
- `dyno check <launch-url>` runs the game in Chromium and gives the lab measures.
  `--driver exec:<program>` runs it through an outside program instead.
- `dyno compare <base> <head>` gives the difference between two results.
- `dyno mcp` lets an AI coding tool check, compare and read results, over stdio.
- The JSON Schema and the TypeScript types of each result, as the npm package `@dyno-cloud/schema`.
