# Releasing

A release is binary archives on this repository's GitHub Releases, plus the TypeScript types on
npm. Both come from one version tag, and they share one version: `[workspace.package] version` in
`Cargo.toml`, which every crate uses, and `version` in `packages/schema/package.json` (a test
checks that they match). No source tarball, installer or crates.io package: every crate has `publish = false`.

The flow is the one changesets gives JavaScript projects, with [knope](https://knope.tech) 0.23.2
as the tool:

1. Each pull request that changes the engine adds a change file in `.changeset/`.
2. On `main`, a bot keeps a release pull request open. It bumps the version and writes the
   change files into `CHANGELOG.md`.
3. Merging the release pull request tags the new version. The tag builds the release, and the
   new section of `CHANGELOG.md` becomes its notes.

## Change files

A pull request that changes `crates/` or `packages/schema/` adds a change file: one Markdown file
in `.changeset/` that says, for users, what changed. `mise install` installs knope 0.23.2 (or see
[its installation page](https://knope.tech/installation/)). Run `knope document-change`, or write
the file by hand:

```md
---
default: minor
---

# Compare results from two different versions

Any more detail goes here, under the title.
```

- `default` is the one package, the engine. Its value is the type of change: `major` breaks
  something that worked, `minor` adds a feature, `patch` fixes a bug. Before 1.0, `major` bumps
  the middle number (0.1.0 to 0.2.0), and `minor` and `patch` bump the last (0.1.0 to 0.1.1).
- The title becomes a line under "Breaking Changes", "Features" or "Fixes". Text under the title
  makes the entry a section of its own.
- Name the file after the change, such as `.changeset/compare_across_versions.md`. One pull
  request can add more than one.
- Change files are public, as the changelog is. Commit messages never reach the changelog.

The `Change file` check (`.github/workflows/change-file.yml`) fails a pull request that changes
those folders and adds no change file. A change to the root `Cargo.toml` or `Cargo.lock` alone,
such as a dependency update, needs none. When users will not notice the change, such as a refactor
or a new test, add the label `no change file` to the pull request instead. knope cannot read an
empty change file, so the label is the only way to skip it. Renovate puts the label on the
dependency updates that it merges by itself. A person who merges a dependency update adds a change
file or the label.

## The release pull request

`.github/workflows/release-pr.yml` runs on every push to `main`. Its `Release PR` job runs
`mise run release:prepare`, which is `knope prepare-release`. It:

- works out the new version from the change files, and writes it in `Cargo.toml` and
  `packages/schema/package.json`;
- adds a section for that version at the top of `CHANGELOG.md`, and deletes the change files;
- runs `DYNO_UPDATE_GENERATED=1 INSTA_UPDATE=always cargo test --workspace`, because the version is
  in every result hash. This updates `Cargo.lock`, the snapshots and `fixtures/golden-hashes.json`.

The job commits all of it to the `release` branch and opens a pull request, "chore(release):
vX.Y.Z", with the new changelog section as its description. Each push to `main` makes the branch
again from scratch, so never push to it by hand: add or edit a change file on `main` instead. When
`main` has no change files, the job closes the pull request. It is one commit, so any merge method
works, a rebase merge too. Nothing in this flow pushes to `main`.

To see what the next release pull request would hold, without changing anything:

```sh
mise run release:preview   # knope prepare-release --dry-run
```

## The tag and the GitHub Release

The `Tag` job of `release-pr.yml` also runs on every push to `main`. When the version in
`Cargo.toml` has no tag yet, it tags that commit `v` and the version, such as `v0.2.0`. So merging
the release pull request tags the new version. A tag pushed by hand works the same way, but it must
be on a commit whose `Cargo.toml` has that version.

The tag starts `.github/workflows/release.yml`, which cargo-dist generates. It builds every target
and makes the GitHub Release. cargo-dist reads the release's title and notes from the section of
`CHANGELOG.md` with the tag's version. A version with a pre-release part, such as `0.2.0-rc.1`, is
marked as a pre-release. On a pull request, `release.yml` only checks the plan.

To see the notes a release would carry, without releasing:

```sh
dist plan --tag=v0.1.0 --output-format=json | jq -r .announcement_github_body
```

### Settings

Both jobs push with a token of the `dyno-bot` GitHub App, because a branch or tag that
`GITHUB_TOKEN` pushes starts no other workflow. Until the repository has the variable
`DYNO_BOT_CLIENT_ID` and the secret `DYNO_BOT_PRIVATE_KEY`, both jobs skip with a notice.

| Setting                                      | Kind             | What it does                                  |
| -------------------------------------------- | ---------------- | --------------------------------------------- |
| `RELEASE_PR`                                 | variable         | `true` turns on the release pull request.     |
| `RELEASE_TAGS`                               | variable         | `true` turns on the tag, and so the releases. |
| `DYNO_BOT_CLIENT_ID`, `DYNO_BOT_PRIVATE_KEY` | variable, secret | The app's client ID and private key.          |
| `NPM_PUBLISH`                                | variable         | `true` turns on the npm publish (see below).  |

The app needs these repository permissions: contents (read and write), to push the `release`
branch and the tags; pull requests (read and write), to open and close the release pull request;
and workflows (read and write), because moving the `release` branch onto a `main` that changed a
workflow counts as changing that workflow.

## What a release holds

One archive per target, each with the `dyno` binary, `README.md`, `LICENSE-MIT`,
`LICENSE-APACHE` and `THIRD_PARTY_NOTICES.md`, and a SHA-256 checksum per archive:

- `aarch64-apple-darwin` and `x86_64-apple-darwin`: Macs;
- `x86_64-unknown-linux-gnu`: Linux, such as CI runners and hosted runners.

`fixtures/golden-hashes.json` at the tag gives the status and `result_hash` that this release's
binary gives for each fixture. Anyone who runs a copy of the release, such as a hosted runner, can
check that it gives the same results.

`dist-workspace.toml` sets this. The build uses the `dist` profile: release with thin LTO.

## Checks before a release

CI runs these on every push (`.github/workflows/health.yml`, job "Licences, advisories and
notices"). `mise run health:licences` runs the same commands:

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

To build an archive locally without releasing (it lands in `target/distrib/`):

```sh
dist plan        # or: mise run build:dist-plan
dist build --artifacts=local --target x86_64-unknown-linux-gnu
```

After a change to `dist-workspace.toml`, run `dist generate` and commit the new workflow. Never
edit `release.yml` by hand. The tools are cargo-deny 0.20.2, cargo-about 0.9.3, cargo-dist 0.34.0
and knope 0.23.2. `mise install` installs them at these versions (`mise.toml`).

## The npm types

`.github/workflows/npm.yml` publishes `packages/schema/` as `@dyno-labs/schema` after `release.yml`
finishes the release of a tag: the TypeScript types in `ts/` and the JSON Schema in `json/`. It
does not wait for the "release published" event: `release.yml` makes the release with
`GITHUB_TOKEN`, and an event made that way starts no workflow. It does nothing until the
repository variable `NPM_PUBLISH` is `true`. It uses npm trusted publishing, so no npm token is
stored: on npmjs.com, the package's settings must name this repository, `npm.yml` and the
`release` environment. The job checks that the package version is the tag's version. A
pre-release version goes to the `next` dist-tag, so `latest` stays on the last full release. To
publish a tag again, run `npm.yml` by hand on that tag.
