---
default: patch
---

# A .zip with two entries of the same name is refused

A zip file can hold two entries with one name. Programs that unpack it differ in which of the two they keep, so such a zip is not one build: dyno could scan one file while a server or a player gets the other. `dyno check` and `dyno verify --build` now refuse such a .zip with a usage error (64) that says so. Before, dyno read one of the two entries and said nothing.
