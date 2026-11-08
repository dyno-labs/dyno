---
default: minor
---

# Upload results to the dyno site with `dyno login`, `dyno upload` and `check --upload`

`dyno login` signs in with a token from the site's Settings page, checks it with the site, and saves it, readable by you only. `DYNO_TOKEN` wins over the saved token, as in CI, and `dyno logout` removes it. `dyno upload <RESULT>...` uploads each result file as it is and prints the link to its page on the site. `dyno check --upload` uploads the result after the check, or each game's result with `--games`, and stops before the check when you are not signed in. A failed upload makes a check that passed exit with 70.
