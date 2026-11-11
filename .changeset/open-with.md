---
default: minor
---

# `--open-with`: a served run in a browser that dyno starts and measures

`dyno check <build> --driver serve --open-with PROGRAM` starts the program with the harness page's URL, reads the memory of each of its processes into `run.timeline` (Linux), and stops it, and everything it started, when the run ends. The game's process is the web process with the largest footprint at idle. `examples/webkit/open.sh` runs WebKitGTK on a virtual display at the reference phone's viewport and pixel ratio, for a lab copy of Safari's engine. A served run now names WebKitGTK as itself, with no version, not as Safari.
