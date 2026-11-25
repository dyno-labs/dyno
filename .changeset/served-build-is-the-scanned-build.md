---
default: patch
---

# A served build exposes exactly the files its build id covers

The static scan leaves some files out of a build, and so out of its build id: `.DS_Store`, `Thumbs.db`, `desktop.ini`, anything in a `__MACOSX` or `.git` folder, and anything reached through a symbolic link. But the file server of `--driver serve` and `--browser` read the folder itself, by a rule of its own, and gave a browser any file in it. So a page under test could load a file that the build id did not cover, and a result with `run.build_link` of `served` then named a build that was not all of what ran.

The file server now gives the bytes that the scan read, from memory, and nothing else. A file that the scan skips gets a 404. So does a file that was added to the folder after the scan, and a file that was edited or swapped after the scan is served as the scan read it. A game that loaded a file through a symbolic link in its build folder must now have that file in the folder itself. dyno keeps the scan's one copy of the build in memory until the check ends.

On a volume that gives a file under a name in another case, as the default volume of a Mac does, a request whose path differs from one scanned file's only in case still gets that file, so `/` finds a build's `Index.html` there as before.

On the game's port of a served run, only `/__dyno/probe.js` is dyno's now, not everything under `/__dyno/`. A build file beside it is served, and a build with a file of its own at that path is a usage error before the run.
