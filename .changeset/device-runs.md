---
default: minor
---

# `--runs N` measures the game's own footprint on the phone

`dyno check <build> --driver serve --device --runs 3` runs the empty build and the game three times each, in turn, each in a fresh Safari with its own recording. The result gives the median footprint at idle and the median peak of the game runs, how many of them the phone killed, and `device.footprint_over_baseline`: the median game peak minus the median peak of the empty build, which is the game's own footprint. Each measure has an item per run, and `inputs.device_runs` records N. A run that gave no reading leaves the measures that need it missing, and the reason names the run. Without `--runs`, `device.footprint_over_baseline` is unsupported.
