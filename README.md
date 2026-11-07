<picture>
  <source media="(prefers-color-scheme: dark)" srcset="brand/png/dyno-redhot-1024.png">
  <img alt="dyno" src="brand/png/dyno-redhot-light-1024.png" width="280">
</picture>

# dyno

A Rust command-line tool for casino games that run in iframes. Its first job is to say how much memory a game will use on an iPhone, before a player loads it.

**Status:** early, and the result shape may still change before 1.0. `dyno check ./dist` scans a build and gives the static measures, rules and fixes. `dyno check <launch-url>` runs the game in Chromium and gives the lab measures. `dyno compare` diffs two results. `dyno mcp` lets an AI coding tool run the same checks. The phone calibration, and so the memory model, comes next.


## Build

Needs Rust 1.97 or later. CI uses 1.99.1.

```sh
cargo build                      # the binary is target/debug/dyno
cargo test --workspace           # unit, golden and snapshot tests
cargo run -- check path/to/dist  # the static scan of a build folder or .zip
cargo run -- check path/to/dist http://127.0.0.1:5173/  # and a browser run of the game
cargo run -- compare base.json head.json  # the diff of two results
```

Some tests compare the output with saved copies in the repo, and fail when the two are different. When you change the output on purpose, run this command to replace the saved copies with the new output. Then read the diff before you commit:

```sh
DYNO_UPDATE_GENERATED=1 INSTA_UPDATE=always cargo test --workspace
```

`DYNO_UPDATE_GENERATED=1` rewrites the JSON Schema in `schema/`, the TypeScript types in `ts/` and the golden result hashes in `parity/`. `INSTA_UPDATE=always` rewrites the saved command output (the snapshots) in `crates/*/tests/snapshots/`.

A browser run needs Chromium. dyno looks for `chromium`, `chromium-browser`, `google-chrome` and `google-chrome-stable` on the PATH, then Google Chrome and Chromium in `/Applications`. Set `DYNO_CHROMIUM` to the binary to use another. The tests that start Chromium, and the one that needs Node 22.13 or later to check the probe, are ignored by default:

```sh
cargo test -p dyno-cdp -- --ignored
```

See [`docs/cli.md`](docs/cli.md) for the commands and [`docs/architecture.md`](docs/architecture.md) for the crates.

## Licence

dyno is under either of these licences, at your option:

- the Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE));
- the MIT licence ([`LICENSE-MIT`](LICENSE-MIT)).

The notices for the open source code inside the binary are in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). The name and the logos are not covered by the code licence: see [`TRADEMARKS.md`](TRADEMARKS.md). To contribute, see [`CONTRIBUTING.md`](CONTRIBUTING.md). To report a security problem, see [`SECURITY.md`](SECURITY.md).
