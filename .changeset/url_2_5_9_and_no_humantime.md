---
default: patch
---

# The url crate is 2.5.9, and humantime is gone

`url` 2.5.8 has a security advisory, RUSTSEC-2026-0592: a URL with many `blob:` prefixes could stop a program that asks for its origin. dyno does not ask for a URL's origin, but the release binary now has `url` 2.5.9, which has the fix. With 2.5.9, a `^` in the path of a launch URL is written as `%5E` in a result, as a browser sends it.

`humantime` has no maintainer (RUSTSEC-2026-0542), so the `time` crate now writes a result's `started_at`. The text is the same as before.
