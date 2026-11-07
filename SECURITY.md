# Security

## Report a problem

Report a security problem privately, through GitHub: open the repository's **Security** tab and
click **Report a vulnerability**. Do not open a public issue or pull request for it.

Tell us what you found, how to make it happen, and which version of dyno you used. We aim to reply
within 7 days.

## What is in scope

- The `dyno` binary: the static scan of a build folder or `.zip`, and the Chromium run.
- The in-page probe (`crates/dyno-cdp/probe/`), in its lab and field builds. The probe runs inside
  other people's games, so a problem there matters most.
- The result shape and the result hash (`crates/dyno-schema`).

A game build is untrusted input. dyno must not run code from a build outside Chromium's sandbox,
and it must not write outside the paths you give it.

## Supported versions

Only the latest release gets security fixes.
