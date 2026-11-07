# Contributing to dyno

Thank you for helping. This page says how to build dyno, what a pull request needs, and the terms
of a contribution.

## Build and test

Needs Rust 1.97 or later. CI uses 1.99.1.

```sh
cargo build
cargo test --workspace
```

Before you open a pull request, run the same checks as CI. [mise](https://mise.jdx.dev) installs
the tools at the versions CI uses, and runs the tasks in `mise.toml` that CI runs:

```sh
mise install
mise run health   # formatting, lint and types in every language, licences and advisories, the workflows
mise run test     # the tests, and a build with the minimum Rust version
```

Each linter and formatter runs with its own defaults, and a warning fails the check: rustfmt and
clippy for Rust, oxfmt and oxlint for TypeScript (oxfmt also formats JSON, Markdown, YAML, TOML,
CSS and HTML), tsc for the types, shellcheck and shfmt for shell scripts, and actionlint and zizmor
for the workflows. To fix the formatting, run `cargo fmt --all`, `mise exec -- oxfmt` and
`mise exec -- shfmt -w <script>`. Disable a rule on one line only when it is wrong for that line,
with a comment that says why.

Without mise, run at least:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Some tests compare the output with saved copies in the repo: the JSON Schema in
`packages/schema/json/`, the TypeScript types in `packages/schema/ts/`, the CLI snapshots and
`fixtures/golden-hashes.json`. When you change the output on purpose, update the saved copies,
then read the diff before you commit:

```sh
DYNO_UPDATE_GENERATED=1 INSTA_UPDATE=always cargo test --workspace
```

The tests that start Chromium are ignored by default, and so is the one that rebuilds the probe
from `probe.ts`, which needs Node 22.18 or later. Run them with
`cargo test -p dyno-cdp -- --ignored`.
The probe has unit tests too, which run it in a fake browser frame:
`node --test 'crates/dyno-cdp/probe/*.test.ts'`, with Node 24 or later.

## Coverage

CI measures how much of the code the tests run, and fails a pull request that lowers it. Get the
same report on your machine:

```sh
mise run coverage                 # every part, then the report and its check
mise run coverage:rust            # one part: target/coverage/rust.lcov
mise run coverage:probe           # target/coverage/probe.lcov
mise run coverage-report:engine   # the report from the files above, as CI checks it
```

`coverage:rust` runs every Rust test with cargo-llvm-cov, also the ignored ones, so it needs
Chromium and Node. It leaves out one timing test, which a small CI machine is too slow for (see
`mise.toml`). The report is a table on the terminal, and `target/coverage/engine/`.

The check fails a part when:

- its lines, or its branches where a floor names them, are under the floor in
  `.github/coverage/engine.json`; or
- on a pull request, its lines are more than 0.1 points below main's at the pull request's base
  commit. With no report from main at that commit, only the floors apply.

To lose coverage on purpose, such as when you remove code that had good tests, add the label
`coverage-drop-ok` to the pull request and run the coverage job again. The drop is then a note in
the report. The floors still apply.

**Raise a floor** when your tests raise the coverage. The report says when a part is a point or
more over its floor, and the whole number the floor can go up to. Change it in the config, in the
same pull request as the tests. Do not lower a floor to make a pull request pass. Stable Rust gives
no branch coverage, so the Rust floor is for lines only.

## What a change needs

- **A change file** when the change touches `crates/` or `packages/schema/` and users will
  notice it: run `knope document-change`, or write one in `.changeset/` by hand. It becomes a line of the
  release notes. A check on the pull request asks for it. See
  [`docs/releasing.md`](docs/releasing.md#change-files).
- **Keep the result shape.** Add fields and measures. Do not rename or remove them. A breaking
  change bumps `SCHEMA_VERSION`. See [`docs/architecture.md`](docs/architecture.md).
- **Facts only.** dyno reports values that it reads from a build, or works out from them by fixed
  arithmetic. It does not report a value that rests on an assumed constant.
- **Synthetic fixtures only.** Every file in `fixtures/` is made by `fixtures/generate.sh` from our
  own sources. Do not add a game build that you did not make.
- **Plain words** in code, docs and output. Docs use plain English, with short sentences.
- **Tests that read like a spec:** one behaviour each, and each able to fail.
- **New dependencies** must pass `cargo deny check`. After a dependency change, regenerate
  `THIRD_PARTY_NOTICES.md` with
  `cargo about generate --frozen --fail about.hbs --output-file THIRD_PARTY_NOTICES.md`.

## Sign your commits (DCO)

Every commit needs a `Signed-off-by` line. It says that you agree to the
[Developer Certificate of Origin](https://developercertificate.org/): you wrote the change, or
you have the right to submit it under the project's licence.

```sh
git commit -s
```

## Licence of contributions

dyno is under the MIT licence or the Apache License 2.0, at your option. Unless you say otherwise,
a contribution that you submit is under the same two licences, with no other terms, as section 5
of the Apache License says.

The name and the logos are not covered by the code licence. See [`TRADEMARKS.md`](TRADEMARKS.md).

## Security problems

Do not open a public issue for a security problem. See [`SECURITY.md`](SECURITY.md).
