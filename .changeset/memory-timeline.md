---
default: minor
---

# `run.timeline`: the memory of every browser process through the run

A browser run now records the memory of each of the browser's processes through the run, with the run's steps on the same clock, in `run.timeline`. Each process has its `pid`, its `kind`, its `role` (`game`, `page`, `gpu`, `browser` or `other`) and its samples. The CDP driver on Linux reads every Chromium process from `/proc` 4 times a second: `private_bytes` is the private memory footprint that Chromium reports on Linux, with `resident_bytes` and `shared_bytes`. A short trace at idle finds the game frame's renderer and the page's. `--device` fills the timeline from the phone's sysmontap samples: Safari's page, GPU and network processes and the app, with the game's WebContent process as `game`, and with the kernel's kill of the game's process (`killed`) or its going over a memory limit (`over_memory_limit`) as marks. The recording that `--out` saves beside the result keeps the same processes. An Activity Monitor trace from a device cloud gives the same timeline through `dyno_ios::trace::trace_timeline`. After the active watch, the CDP driver sends Chromium a critical memory pressure signal and forces a garbage collection, with the new `purged` mark, so the timeline shows the memory the game holds once garbage and caches go. No lab measure sees it. The runner protocol has two new messages for a driver that reads memory: `memory` and `roles`.

The CDP driver's Chromium now runs with no extensions. A launcher such as Arch's `chromium` adds the desktop's own, and each one adds a renderer and can put scripts in the game's page.
