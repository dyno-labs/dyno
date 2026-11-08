---
default: minor
---

# Run a game in any browser on the network with `--driver serve`

`dyno check <build> --driver serve` serves the build folder and a harness page, then waits for a browser to open the page, such as Safari on an iPhone, Safari in the iOS Simulator or a desktop browser. dyno adds the probe to each HTML page, and the run gives the same lab measures as the Chromium driver, but the JS heap. `--serve-host` and `--serve-port` say where dyno serves, and `--serve-query` gives the game its launch query string, such as a demo session. No result records the query string.
