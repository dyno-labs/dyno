---
default: patch
---

# A GitHub Actions token in `DYNO_TOKEN` that the site refuses says to check `DYNO_TOKEN`

`dyno upload` and `check --upload` send the token in `DYNO_TOKEN` as they always did, so the dyno Action can pass the short-lived OIDC token that GitHub Actions gives a run. When the site refuses a token that is not a CLI token, the error now says to check `DYNO_TOKEN`, not to run `dyno login` again, which cannot help in CI.
