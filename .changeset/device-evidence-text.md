---
default: patch
---

# The device footprint's evidence names the model and iOS version in plain words

The evidence of `device.phys_footprint` now reads `an Activity Monitor trace of <name>: <model>, iOS <version>, process com.apple.WebKit.WebContent (<pid>)`, so a tool can read the model and version from it.
