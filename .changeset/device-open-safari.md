---
default: minor
---

# `--device` opens the page in the phone's Safari, and stops on a locked phone

Before it records, `dyno check --driver serve --device` opens the harness page in the phone's Safari with `xcrun devicectl`, so there is no URL to type. `--no-open` leaves that to you, and a launch that fails only says to open the URL by hand. A locked phone is a usage error that says to unlock it and set Auto-Lock to Never. A phone that disconnects from xctrace during the run, as one that locks does, no longer leaves the check waiting minutes for the trace: the device measures are missing, and the reason asks whether the phone locked or is on Wi-Fi only. A trace that ended before the run did no longer counts the game as killed.
