---
default: minor
---

# Check the empty build with `--baseline`

`dyno check --baseline` checks a one-page build inside dyno instead of a target. Its build id never changes, so a device run of it, `dyno check --baseline --driver serve --device`, gives the phone's footprint with no game, which a game's footprint can be measured over.
