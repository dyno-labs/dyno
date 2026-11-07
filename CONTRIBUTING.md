# Contributing to dyno

Thank you for helping. This page says how to build dyno, what a pull request needs, and the terms
of a contribution.

## Build and test

Needs Rust 1.97 or later. CI uses 1.99.1.

```sh
cargo build
cargo test --workspace
```

Before you open a pull request, run the same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Some tests compare the output with saved copies in the repo: the JSON Schema in `schema/`, the
TypeScript types in `ts/`, the CLI snapshots and `parity/golden-hashes.json`. When you change the
output on purpose, update the saved copies, then read the diff before you commit:

```sh
DYNO_UPDATE_GENERATED=1 INSTA_UPDATE=always cargo test --workspace
```

The tests that start Chromium are ignored by default. Run them with
`cargo test -p dyno-cdp -- --ignored`.

## What a change needs

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
