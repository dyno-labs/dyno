# Changelog

The changes in each release of dyno: the `dyno` binary and the `@dyno-labs/schema` types, which
share one version. The release pull request writes each section from the change files in
`.changeset/`, and the section becomes the notes of the GitHub Release. See
[`docs/releasing.md`](docs/releasing.md).

## 0.1.0

The first release.

- `dyno check <build>` scans a build folder or `.zip` and gives the static measures, rules and
  fixes.
- `dyno check <launch-url>` runs the game in Chromium and gives the lab measures.
  `--driver exec:<program>` runs it through an outside program instead.
- `dyno compare <base> <head>` gives the difference between two results.
- `dyno mcp` lets an AI coding tool check, compare and read results, over stdio.
- The JSON Schema and the TypeScript types of each result, as the npm package `@dyno-labs/schema`.
