---
default: minor
---

# A phone run serves its page over the device link, and a result names its lab

`dyno check <build> --driver serve --device` on a Mac now serves the harness page and the game at the Mac's address inside the device link's tunnel to the phone, when the Mac has such a tunnel. The phone's Safari then needs no network path to the Mac, which a guest Wi-Fi or a cloud device lab often does not give. dyno finds the address with `xcrun devicectl list devices` and the Mac's own routing, with no interface name, and takes it only when it and the phone's address are two different addresses in one /64 network. `--serve-host` still wins, and is then the only address.

A tunnel can go away before Safari asks for the page. So dyno serves at the Mac's local network address too, asks Safari for the tunnel address first, and asks it for the local network address when no request has come to the harness page in 15 s. When there is no tunnel address, dyno serves at the local network address as before, and says why. On Linux, dyno serves on the local network as before: the software tunnel that the `idevice` crate builds takes no connections from the phone. [`docs/cli.md`](docs/cli.md#where-the-page-is-served) says what it would need, and what is not yet proven on a real Mac.

A device run now goes in this order: record the phone, serve the page, open it in the phone's Safari once the servers answer. So `dyno check` prints the "recording" line and the URL before the "opened the page" line.

Two new fields in the run part, outside the result hash:

- `run.serve_path`: the path the browser took to the page of a served run: `tunnel`, `local_network`, `loopback` or `given` (the address that `--serve-host` gave). It is the address that the browser's first request to the harness page came to, so it is the path that was used, and it is null when no browser asked for the page.
- `run.lab`: the kind of lab that ran the check, from the new `--lab <NAME>` flag, such as `desk`, `rig` or `devicefarm`. The name is a short word in lower case: it starts with a letter, and has at most 32 letters a to z, digits and `-`. No flag records no name.

The served run's two servers now refuse a TLS connection at once, with a TLS alert. Safari tries HTTPS on a port before it falls back to plain HTTP, and the servers used to leave that try without an answer until Safari gave up.
