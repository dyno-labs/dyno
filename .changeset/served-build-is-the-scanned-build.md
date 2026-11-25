---
default: patch
---

# A served build exposes exactly the files its build id covers

The static scan leaves some files out of a build, and so out of its build id: `.DS_Store`, `Thumbs.db`, `desktop.ini`, anything in a `__MACOSX` or `.git` folder, and anything reached through a symbolic link. But the file server of `--driver serve` and `--browser` read the folder itself, by a rule of its own, and gave a browser any file in it. So a page under test could load a file that the build id did not cover, and a result with `run.build_link` of `served` then named a build that was not all of what ran.

The file server now gives the bytes that the scan read, from memory, and nothing else. A file that the scan skips gets a 404. So does a file that was added to the folder after the scan, and a file that was edited or swapped after the scan is served as the scan read it. A game that loaded a file through a symbolic link in its build folder must now have that file in the folder itself. dyno keeps the scan's one copy of the build in memory until the check ends, and writes each response from it.

A served build now answers exact names only, whatever volume its folder is on. A request names a file by its path as the scan listed it, in the same case: `/` finds `index.html`, and not `Index.html`. A Mac's default volume gives a file under a name in another case, so a game that asked for a file by another spelling loaded from a folder on a Mac. It now gets a 404, as it would on most hosts.

On the game's port of a served run, only `/__dyno/probe.js` is dyno's now, not everything under `/__dyno/`. A build file beside it is served, and a build with a file of its own at that very path is a usage error before the run.
