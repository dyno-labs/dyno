---
default: patch
---

# A .zip that names a file twice is refused

A zip file can name one file twice in its central directory, the list of entries at its end. Programs that unpack it differ in which of the two entries they keep, so such a zip is not one build: dyno could scan one file while a server or a player gets the other. `dyno check` and `dyno verify --build` now refuse such a .zip with a usage error (64) that says so. Before, dyno read one of the two entries and said nothing. A zip that names a folder twice is refused too.

A zip's content is what its central directory names. dyno does not compare it with the headers before each entry's bytes, so a program that unpacks a build must follow the central directory.
