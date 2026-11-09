---
default: patch
---

# A served run is not incomplete for a heap reading Safari never gives, and no result holds the phone's own name

Under `--driver serve`, `lab.js_heap` is now `unsupported` ("the browser gives the page no heap reading"), not `missing`. Safari gives the page no heap reading, so a served run on an iPhone was always `incomplete`. It now gets the status of its rules. The CDP driver still reads the heap, and a missing reading there still makes the run incomplete.

The evidence of `device.phys_footprint` now reads `an Activity Monitor trace of the phone: <model>, iOS <version>, process com.apple.WebKit.WebContent (<pid>)`. It no longer names the phone, which is often its owner's name, and a device error's reason has the phone's name taken out. The lab measures' evidence reads `one run in the harness page`, as it is not always Chromium.
