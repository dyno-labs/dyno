---
default: patch
---

# A busy game no longer fails the browser run's idle reads

Once the game's network goes idle, dyno reads its heap and runs its self-checks in the game frame. A heavy game on a small machine that draws in software can still be busy then, and these reads failed after 15 s with "no reply in time", which left every run measure missing. They now wait until the run's own deadline.
