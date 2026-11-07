# Releasing

A release is binary archives on this repository's GitHub Releases, plus the TypeScript types on
npm. Both come from one version tag. No source tarball, installer or crates.io package: every crate
has `publish = false`.

## What a release holds

One archive per target, each with the `dyno` binary, `README.md`, `LICENSE-MIT`,
`LICENSE-APACHE` and `THIRD_PARTY_NOTICES.md`, and a SHA-256 checksum per archive:

- `aarch64-apple-darwin` and `x86_64-apple-darwin`: Macs;
- `x86_64-unknown-linux-gnu`: Linux, such as CI runners and hosted runners.

`parity/golden-hashes.json` at the tag gives the status and `result_hash` that this release's
binary gives for each fixture. Anyone who runs a copy of the release, such as a hosted runner, can
check that it gives the same results.

`dist-workspace.toml` sets this. The build uses the `dist` profile: release with thin LTO.

## Checks before a release

CI runs these on every push (`.github/workflows/ci.yml`, job "Licences, advisories and notices"):

```sh
cargo deny check   # advisories, licences, bans and sources of what ships (deny.toml)
cargo about generate --frozen --fail about.hbs --output-file THIRD_PARTY_NOTICES.md
git diff --exit-code THIRD_PARTY_NOTICES.md
```

The licence allow-list is permissive, and dev-dependencies are left out. The one exception is
symphonia, the audio decoder (MPL-2.0, design decision R4): `deny.toml` and `about.toml` allow
MPL-2.0 for the symphonia crates only. MPL-2.0 lets the binary use its files under our own licence
while they stay unmodified, so never patch or vendor a changed copy of them. The notices give the link to
their source. After a dependency change, regenerate `THIRD_PARTY_NOTICES.md` with the second
command and commit it.

## Making a release

Only with a maintainer's OK. Set the version in `Cargo.toml` (`[workspace.package] version`) and
in `ts/package.json` (a test checks that they match), regenerate the saved output
(`DYNO_UPDATE_GENERATED=1 INSTA_UPDATE=always cargo test --workspace`: the version is in every
result hash), merge to `main`, then push a tag of that version, such as `v0.1.0`. `.github/workflows/release.yml`,
which cargo-dist generates, builds every target and makes the GitHub Release. On a pull request
it only checks the plan.

To check locally without releasing (the archive lands in `target/distrib/`):

```sh
dist plan
dist build --artifacts=local --target x86_64-unknown-linux-gnu
```

After a change to `dist-workspace.toml`, run `dist generate` and commit the new workflow. The
tools are cargo-deny 0.20.2, cargo-about 0.9.3 (`--features cli`) and cargo-dist 0.34.0.

## The npm types

`.github/workflows/npm.yml` publishes `ts/` as `@dyno-labs/schema` when a release is published. It
does nothing until the repository variable `NPM_PUBLISH` is `true`. It uses npm trusted
publishing, so no npm token is stored: on npmjs.com, the package's settings must name this
repository, `npm.yml` and the `release` environment. The job checks that the package version is
the tag's version.
