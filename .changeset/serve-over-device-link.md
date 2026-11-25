---
default: minor
---

# A phone run serves its page over the device link, and a result names its lab

`dyno check <build> --driver serve --device` on a Mac now serves the harness page and the game at the Mac's address inside the device link's tunnel to the phone, when the Mac has such a tunnel. The phone's Safari then needs no network path to the Mac, which a guest Wi-Fi or a cloud device lab often does not give. dyno finds the address with `xcrun devicectl list devices` and the Mac's own routing, with no interface name. When there is no tunnel address, or the servers cannot be bound there, dyno serves at the local network address as before, and says why. `--serve-host` still wins. On Linux, dyno serves on the local network as before: the software tunnel that the `idevice` crate builds takes no connections from the phone. [`docs/cli.md`](docs/cli.md#where-the-page-is-served) says what it would need.

A device run now goes in this order: record the phone, serve the page, open it in the phone's Safari. So `dyno check` prints the "recording" line before the "opened the page" line.

Two new fields in the run part, outside the result hash:

- `run.serve_path`: the path the browser took to the page of a served run: `tunnel`, `local_network`, `loopback` or `given` (the address that `--serve-host` gave).
- `run.lab`: the kind of lab that ran the check, from the new `--lab <NAME>` flag, such as `desk`, `rig` or `devicefarm`. The name is a short word in lower case: it starts with a letter, and has at most 32 letters a to z, digits and `-`. No flag records no name.

The served run's two servers now refuse a TLS connection at once, with a TLS alert. Safari tries HTTPS on a port before it falls back to plain HTTP, and the servers used to leave that try without an answer until Safari gave up.
