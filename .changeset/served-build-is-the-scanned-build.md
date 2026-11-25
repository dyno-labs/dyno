---
default: patch
---

# A served build exposes exactly the files its build id covers

The static scan leaves some files out of a build, and so out of its build id: `.DS_Store`, `Thumbs.db`, `desktop.ini`, anything in a `__MACOSX` or `.git` folder, and anything reached through a symbolic link. But the file server of `--driver serve` and `--browser` gave a browser any of them that it asked for. So a page under test could load a file that the build id did not cover, and a result with `run.build_link` of `served` then named a build that was not all of what ran.

The file server now gives the files that the scan listed, and no others. It uses the scan's own rule, so the two cannot differ. A file that the scan skips gets a 404, and so does a file that was added to the folder after the scan. A game that loaded a file through a symbolic link in its build folder must now have that file in the folder itself.
