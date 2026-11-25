---
default: minor
---

# Only the page that dyno asked for can drive a served run

With `--driver serve`, the harness page took a run's events from any browser that reached its port. On a shared network, such as a lab where several phones run checks on one host, the game of one check could start or end another check's run, or feed it false events. Now each run has a key of 128 random bits, a new one for each run of `--runs` too, and the harness page's URL holds it: `http://<host>:7370/?key=<32 hex digits>`. dyno opens that URL in the phone's Safari, and prints it for you to open by hand. A request to the harness server without the key gets `403 Forbidden` and changes nothing.

The game cannot learn the key: it is on another origin, the page and its frame send no referrer, and the probe in the frame holds no key. It reports to the page around it, which takes only events of its own frame, so the game can neither say hello nor end a run.

The game server now answers no one before the page says hello, and after it only the address that the hello came from. So no other device can add game requests to the run, or load the build while it runs.

The game is no longer on the port after `--serve-port`: it is on a free port that the system gives, a new one for each run. A page whose key is refused stops and removes its game frame. So a tab that missed the end of its run reaches neither the next run's harness page nor its game, even on the same phone.

Each run's URL is new, so a bookmark of the harness page no longer opens a run: open the URL that dyno prints. The key is in no result and no saved recording. It is no secret from a party that reads the plain HTTP on the network. [`docs/cli.md`](docs/cli.md#who-can-drive-a-served-run) says what the key guards against, and what it does not. The CDP driver and `--driver exec:` are as before.
