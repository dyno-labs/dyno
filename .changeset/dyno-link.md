---
default: minor
---

# `dyno link` links a build to its game on the dyno site

`dyno link <BUILD> --name <NAME>` finds the game of that name on the dyno site, or makes it, and `dyno link <BUILD> --game-id <UUID>` takes a game the site already has. Either way, dyno writes the build's entry in `dyno.toml` (or the file `--config` names), keeping the file's comments and other entries, so that every later `dyno check` of the build names the game. It needs `dyno login` or `DYNO_TOKEN`.

`dyno upload` and `dyno check --upload` now print the link that the site gives, which is the game's report when the site linked the result to a game, with the game's name. A site that gives no link gets the result's own page, as before.
