---
default: minor
---

# Run a build folder in Chromium with `--browser`

`dyno check <build> --browser` serves the build folder on 127.0.0.1 and loads it in dyno's own Chromium. So the browser loads the files that the static scan read, and `run.build_link` is `served`, not `asserted` as with a build and a launch URL that you serve yourself. The dyno site counts a served run toward the build. The launch URL is the folder's root, and the run gives the same measures as a CDP run with a launch URL. `--browser` needs a build folder and no launch URL, and works only with the `cdp` driver. With `--games`, every selected game needs a build folder and no launch URL.
