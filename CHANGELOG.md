# Changelog

The changes in each release of dyno: the `dyno` binary and the `@dyno-cloud/schema` types, which
share one version. The release pull request writes each section from the change files in
`.changeset/`, and the section becomes the notes of the GitHub Release. See
[`docs/releasing.md`](docs/releasing.md).

## 0.1.4 (2026-11-28)

### Features

#### A result says what dyno saw of each run

A result's measures say what each value is, but not whether the run under them really ran. Someone who runs dyno for other people could not tell, from the result alone, a run that measured the game from one that failed for a reason of the lab's own. A new field in the run part, outside the result hash, says it, and says only what dyno observed: where it cannot tell, it says `not_observed`, or null. It is evidence only: no measure, run coverage, status, verdict, result hash or exit code changes with it. A lab's receipt signs the whole result file, so it covers the records too.

- `run.runs`: one record per run, in the order the runs went. A browser run has one; `--runs N` with `--device` has one per run of the empty build and of the game. Each says which build it was and its number; whether the page said `hello`; the game frame's document at the end (`build`, `http_error`, `error_page`, `other_url`, `no_document`, or `not_observed`); for a served run, whether a probe's document message was heard; and how the page's run ended (`ended`, `timed_out`, `command_not_taken`, `no_request`, `connection_closed`, `failed` or `never_started`). With `--device` it also says what the phone's samples and its log each gave in the run, when each last sent, and the stop each reported; a summary of the samples of each distinct page process pid, and when, which is of the pid and not of one process; and the first kill of a page process that the log gave, with when its line arrived. It writes what dyno saw, and no conclusion that this does not settle. A check with no browser run leaves the field out.

So a Chromium run that timed out on a game that never went quiet now shows that the game's own document came, where one on Chromium's error page shows `error_page` and one on a 404 page `http_error`. A served run whose page stopped taking dyno's commands after its `hello` shows `command_not_taken`, even when the phone's recording gave every device measure. A run of the empty build whose log stopped shows it.

When a later run of `--runs` stops the check, as a phone that left does, dyno still writes no result and exits as before, but with `--out` it keeps the records of the runs already made beside it, as `<name>.runs.json`, with each game's id under `--games`. It never replaces a file of that name.

The fake phone of the tests (`DYNO_FAKE_PHONE`) takes three more settings: `sampled-after-ms`, for a page that the samples do not have yet; `log-fails-in`, for a log that closes in chosen recordings only; and `connect-fails-in`, for a phone that does not connect for chosen recordings.

[`docs/design/run-evidence.md`](docs/design/run-evidence.md) gives each rule, what a kill in the record rests on, the open questions where the record and a measure disagree, and what is not yet proven on a real phone.

#### A phone run serves its page over the device link, and a result names its lab

`dyno check <build> --driver serve --device` on a Mac now serves the harness page and the game at the Mac's address inside the device link's tunnel to the phone, when the Mac has such a tunnel. The phone's Safari then needs no network path to the Mac, which a guest Wi-Fi or a cloud device lab often does not give. dyno finds the address with `xcrun devicectl list devices` and the Mac's own routing, with no interface name, and takes it only when it and the phone's address are two different addresses in one /64 network. `--serve-host` still wins, and is then the only address.

A tunnel can go away before Safari asks for the page. So dyno serves at the Mac's local network address too, and prints both URLs. It asks Safari for the tunnel address, once. When no request has come there after 30 s, and dyno has finished asking Safari, dyno says so and waits on: open the local network URL on the phone by hand, or run again with `--serve-host <address>`. When there is no tunnel address, dyno serves at the local network address as before, and says why. On Linux, dyno serves on the local network as before: the software tunnel that the `idevice` crate builds takes no connections from the phone. [`docs/cli.md`](docs/cli.md#where-the-page-is-served) says what it would need, and what is not yet proven on a real Mac.

A device run now goes in this order: record the phone, serve the page, open it in the phone's Safari once the servers answer. So `dyno check` prints the "recording" line and the URL before the "opened the page" line.

Two new fields in the run part, outside the result hash:

- `run.serve_path`: the path the browser took to the page of a served run: `tunnel`, `local_network`, `loopback` or `given` (the address that `--serve-host` gave). It is the address that the page which ran said hello at, so it is the path that was used, and it is null when no page ran.
- `run.lab`: the kind of lab that ran the check, from the new `--lab <NAME>` flag, such as `desk`, `rig` or `devicefarm`. The name is a short word in lower case: it starts with a letter, and has at most 32 letters a to z, digits and `-`. No flag records no name.

The served run's two servers now refuse a TLS connection at once, with a TLS alert. Safari tries HTTPS on a port before it falls back to plain HTTP, and the servers used to leave that try without an answer until Safari gave up.

#### Only the page that dyno asked for can drive a served run

With `--driver serve`, the harness page took a run's events from any browser that reached its port. On a shared network, such as a lab where several phones run checks on one host, the game of one check could start or end another check's run, or feed it false events. Now each run has a key of 128 random bits, a new one for each run of `--runs` too, and the harness page's URL holds it: `http://<host>:7370/?key=<32 hex digits>`. dyno opens that URL in the phone's Safari, and prints it for you to open by hand. A request to the harness server without the key gets `403 Forbidden` and changes nothing.

The game cannot learn the key: it is on another origin, the page and its frame send no referrer, and the probe in the frame holds no key. It reports to the page around it, which takes only events of its own frame, so the game can neither say hello nor end a run.

The game server now answers no one before the page says hello, and after it only the address that the hello came from. So no other device can add game requests to the run, or load the build while it runs.

Each run's game now has a port of its own: with `--serve-port` P, the first run's game is at P + 1, the second's at P + 2, and so on up to P + 64, with a busy port skipped. When those are used up, the next run stops with an error that says so. Open P to P + 64 on a firewall: 7370 to 7434 by default. A page whose key is refused stops and removes its game frame. So a tab that missed the end of its run reaches neither the next run's harness page nor its game, even on the same phone.

Each run's URL is new, so a bookmark of the harness page no longer opens a run: open the URL that dyno prints. The key is in no result and no saved recording. It is no secret from a party that reads the plain HTTP on the network. [`docs/cli.md`](docs/cli.md#who-can-drive-a-served-run) says what the key guards against, and what it does not. The CDP driver and `--driver exec:` are as before.

#### `dyno verify`: check a lab's signed receipt for a result

A result can come from a lab: someone who owns real phones and runs dyno for people who do not. A lab now signs a small receipt for each result, and `dyno verify` checks it, so that you can trust the lab's phone readings for exactly the files you hold, without running the check again:

```sh
dyno verify receipt.json --keys https://lab.example/keys.json --result result.json --build ./dist
```

It checks that a public key you gave signed the receipt (`--key`, or the lab's keys document with `--keys`), that the result file is the one the receipt is for (`--result`), and that your files have the receipt's build id (`--build`), with the code that `dyno check` uses. It writes a JSON verdict and exits with 0 when the receipt is verified and 1 when it is not. With no key it refuses: a receipt that brought its own key would prove nothing. It also refuses a receipt, a result or a keys document in which an object has one name twice, because parsers differ in which value they keep.

The receipt format is open, and anyone can run a lab. `dyno receipt keygen` makes a lab's Ed25519 key pair, and `dyno receipt sign` signs the receipt for a result of a check with a run on a device. `docs/receipts.md` gives the format, version 1, and a known-answer receipt in `fixtures/receipts/` for other implementations to test against.

`@dyno-cloud/schema` has the new types `ReceiptEnvelope`, `Receipt`, `PublicKeys` and `VerifyResult`, and the JSON Schema files `receipt.schema.json`, `receipt-keys.schema.json` and `verify.schema.json`.

### Fixes

#### A .zip that names a file twice is refused

A zip file can name one file twice in its central directory, the list of entries at its end. Programs that unpack it differ in which of the two entries they keep, so such a zip is not one build: dyno could scan one file while a server or a player gets the other. `dyno check` and `dyno verify --build` now refuse such a .zip with a usage error (64) that says so. Before, dyno read one of the two entries and said nothing. A zip that names a folder twice is refused too.

A zip's content is what its central directory names. dyno does not compare it with the headers before each entry's bytes, so a program that unpacks a build must follow the central directory.

#### A served build exposes exactly the files its build id covers

The static scan leaves some files out of a build, and so out of its build id: `.DS_Store`, `Thumbs.db`, `desktop.ini`, anything in a `__MACOSX` or `.git` folder, and anything reached through a symbolic link. But the file server of `--driver serve` and `--browser` read the folder itself, by a rule of its own, and gave a browser any file in it. So a page under test could load a file that the build id did not cover, and a result with `run.build_link` of `served` then named a build that was not all of what ran.

The file server now gives the bytes that the scan read, from memory, and nothing else. A file that the scan skips gets a 404. So does a file that was added to the folder after the scan, and a file that was edited or swapped after the scan is served as the scan read it. A game that loaded a file through a symbolic link in its build folder must now have that file in the folder itself. dyno keeps the scan's one copy of the build in memory until the check ends, and writes each response from it.

A served build now answers exact names only, whatever volume its folder is on. A request names a file by its path as the scan listed it, in the same case: `/` finds `index.html`, and not `Index.html`. A Mac's default volume gives a file under a name in another case, so a game that asked for a file by another spelling loaded from a folder on a Mac. It now gets a 404, as it would on most hosts.

On the game's port of a served run, only `/__dyno/probe.js` is dyno's now, not everything under `/__dyno/`. A build file beside it is served, and a build with a file of its own at that very path is a usage error before the run.

#### The url crate is 2.5.9, and humantime is gone

`url` 2.5.8 has a security advisory, RUSTSEC-2026-0592: a URL with many `blob:` prefixes could stop a program that asks for its origin. dyno does not ask for a URL's origin, but the release binary now has `url` 2.5.9, which has the fix. With 2.5.9, a `^` in the path of a launch URL is written as `%5E` in a result, as a browser sends it.

`humantime` has no maintainer (RUSTSEC-2026-0542), so the `time` crate now writes a result's `started_at`. The text is the same as before.

## 0.1.3 (2026-11-11)

### Features

#### Install dyno with one command

Each release now has an install script, `dyno-cli-installer.sh`. It gets the release's archive for your machine, a Mac or Linux on x86-64, and puts `dyno` in `~/.local/bin`, or in `$XDG_BIN_HOME` when that is set:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dyno-labs/dyno/releases/latest/download/dyno-cli-installer.sh | sh
```

#### `run.timeline`: the memory of every browser process through the run

A browser run now records the memory of each of the browser's processes through the run, with the run's steps on the same clock, in `run.timeline`. Each process has its `pid`, its `kind`, its `role` (`game`, `page`, `gpu`, `browser` or `other`) and its samples. The CDP driver on Linux reads every Chromium process from `/proc` 4 times a second: `private_bytes` is the private memory footprint that Chromium reports on Linux, with `resident_bytes` and `shared_bytes`. A short trace at idle finds the game frame's renderer and the page's. `--device` fills the timeline from the phone's sysmontap samples: Safari's page, GPU and network processes and the app, with the game's WebContent process as `game`, and with the kernel's kill of the game's process (`killed`) or its going over a memory limit (`over_memory_limit`) as marks. The recording that `--out` saves beside the result keeps the same processes. An Activity Monitor trace from a device cloud gives the same timeline through `dyno_ios::trace::trace_timeline`. After the active watch, the CDP driver sends Chromium a critical memory pressure signal and forces a garbage collection, with the new `purged` mark, so the timeline shows the memory the game holds once garbage and caches go. No lab measure sees it. The runner protocol has two new messages for a driver that reads memory: `memory` and `roles`.

The CDP driver's Chromium now runs with no extensions. A launcher such as Arch's `chromium` adds the desktop's own, and each one adds a renderer and can put scripts in the game's page.

#### `--open-with`: a served run in a browser that dyno starts and measures

`dyno check <build> --driver serve --open-with PROGRAM` starts the program with the harness page's URL, reads the memory of each of its processes into `run.timeline` (Linux), and stops it, and everything it started, when the run ends. The game's process is the web process with the largest footprint at idle. `examples/webkit/open.sh` runs WebKitGTK on a virtual display at the reference phone's viewport and pixel ratio, for a lab copy of Safari's engine. A served run now names WebKitGTK as itself, with no version, not as Safari.

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
