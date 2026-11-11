#!/usr/bin/env bash
# Renovate runs this after it changes Cargo.toml or Cargo.lock (.github/renovate.json5), so the
# same commit has the THIRD_PARTY_NOTICES.md that CI checks against the dependencies. It runs in
# Renovate's container, which has cargo but not cargo-about: it downloads cargo-about's release
# binary, checks its SHA-256, and runs the same command as CI.
set -euo pipefail

# The release binary of the cargo-about version in mise.toml. Renovate updates both lines.
version=0.9.3
sha256=3ad1e94ea1e63d69d1961f4fb15ccbbe9575d906111f96d2cb4dce6df48f71b4

archive="cargo-about-$version-x86_64-unknown-linux-musl.tar.gz"
tools="$(mktemp -d)"
trap 'rm -rf "$tools"' EXIT
curl -fsSLo "$tools/$archive" "https://github.com/EmbarkStudios/cargo-about/releases/download/$version/$archive"
echo "$sha256  $tools/$archive" | sha256sum --check --strict --quiet
tar -xzf "$tools/$archive" -C "$tools" --strip-components=1
export PATH="$tools:$PATH"

# cargo-about reads each crate's licence files from cargo's download folder.
cargo fetch --locked
cargo about generate --frozen --fail about.hbs --output-file THIRD_PARTY_NOTICES.md
