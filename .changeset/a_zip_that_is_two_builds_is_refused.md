---
default: patch
---

# A .zip that two programs can unpack as two builds is refused

A zip file can hold two file entries with one name, and programs that unpack it differ in which of the two they keep. A zip also names each entry twice, in its central directory and in a header before the entry's bytes, and the two can disagree. Either way the zip is not one build: dyno could scan one file while a server or a player gets another. `dyno check` and `dyno verify --build` now refuse such a .zip with a usage error (64) that says so. Before, dyno read one of the two and said nothing. A zip that names a folder twice is still read.
