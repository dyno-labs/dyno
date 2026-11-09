---
default: minor
---

# `dyno.toml` can hold the command that builds the games

An optional `[build]` table, with one `command`, says how CI makes the build folders from a fresh checkout. The workflow that the dyno site's setup pull request writes runs it before it checks each game. dyno checks that the command is not empty, and does nothing else with it. `dyno link` keeps the table. A dyno from before this version refuses a file that has one.
