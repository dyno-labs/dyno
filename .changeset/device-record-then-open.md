---
default: patch
---

# `--device` records first, then opens Safari, with no pause

`dyno check --driver serve --device` now starts the recording before it opens the page in the phone's Safari, and no longer waits 5 s between the two. On an iPhone 14 Pro Max on a cable, a Safari launch during a recording left the recording on.
