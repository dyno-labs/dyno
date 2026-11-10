---
default: minor
---

# Run Chromium without its sandbox in a container that has none

A browser run in a container with no user namespaces, such as a self-hosted CI runner in Docker, failed with "Chromium exited before its DevTools endpoint was ready" and no reason. The error now quotes Chromium's last lines, and when Chromium found no sandbox it names `DYNO_CHROMIUM_SANDBOX=off`, which runs Chromium without one. The sandbox stays on otherwise.
