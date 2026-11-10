---
default: minor
---

# `--device` reads the iPhone itself: no Xcode, Linux too, and Wi-Fi

`dyno check --driver serve --device` no longer records with `xcrun xctrace`, which lost a phone on Wi-Fi 3 to 4 s into every recording. dyno now reads the phone with the `idevice` crate, through usbmuxd or straight at the phone's address, on Linux or macOS. `--device-ip <IP>` with `--pairing-file <FILE>` records a phone on the network with no cable and no usbmuxd. The phone needs iOS 17 or later and Developer Mode on.

The measures keep their meaning. `device.phys_footprint` and its peak come from the phone's sysmontap samples, the source of Instruments' Activity Monitor, and `device.kills` now comes from the kernel's jetsam line in the phone's log, so a process that only leaves the samples is not a kill. A soft memory limit that the game went over is kept in the evidence. Safari opens the page through the phone's CoreDevice app service, with `devicectl` as a fallback on a Mac. With `--out`, the recording is saved as `<name>.device.json` in place of a `.trace`. The OS version now has the build, such as `26.6.2 (23G90)`, and `memory_bytes` is the memory that iOS manages, a little under the RAM. `--device-id` takes a UDID, and no longer a device name.

The evidence of the footprint has the new method `sysmontap`. The device catalogue has each model's identifiers, such as `iPhone15,3`, and the iPhone 17e, 18 Pro and 18 Pro Max. `dyno_ios::trace` still reads an Activity Monitor trace that xctrace exported, for traces that a device cloud records.
