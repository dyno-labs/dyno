---
default: minor
---

# Record the game's memory on a connected iPhone with `--device`

`dyno check <build> --driver serve --device`, on a Mac with Xcode, records an Activity Monitor trace of the connected iPhone with `xcrun xctrace` while the phone's Safari runs the served check. The result gets `device.phys_footprint`, the footprint of the game's Safari process when the frame went idle, with its peak during the run, and `device.kills`. `--device-id` chooses the device when more than one is connected. With `--out`, the trace is kept beside the result.
