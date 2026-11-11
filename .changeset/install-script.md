---
default: minor
---

# Install dyno with one command

Each release now has an install script, `dyno-cli-installer.sh`. It gets the release's archive for your machine, a Mac or Linux on x86-64, and puts `dyno` in `~/.local/bin`, or in `$XDG_BIN_HOME` when that is set:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dyno-labs/dyno/releases/latest/download/dyno-cli-installer.sh | sh
```
