---
default: minor
---

# A result says what dyno saw of each run

A result's measures say what each value is, but not whether the run under them really ran. Someone who runs dyno for other people could not tell, from the result alone, a run that measured the game from one that failed for a reason of the lab's own. A new field in the run part, outside the result hash, says it, and says only what dyno observed: where it cannot tell, it says `not_observed`, or null. It is evidence only: no measure, run coverage, status, verdict, result hash or exit code changes with it. A lab's receipt signs the whole result file, so it covers the records too.

- `run.runs`: one record per run, in the order the runs went. A browser run has one; `--runs N` with `--device` has one per run of the empty build and of the game. Each says which build it was and its number; whether the page said `hello`; the game frame's document at the end (`build`, `http_error`, `error_page`, `other_url`, `no_document`, or `not_observed`); for a served run, whether a probe's document message was heard; and how the page's run ended (`ended`, `timed_out`, `command_not_taken`, `no_request`, `connection_closed`, `failed` or `never_started`). With `--device` it also says what the phone's samples and its log each gave in the run, when each last sent, and the stop each reported; a summary of the samples of each distinct page process pid, and when, which is of the pid and not of one process; and the first kill of a page process that the log gave, with when its line arrived. It writes what dyno saw, and no conclusion that this does not settle. A check with no browser run leaves the field out.

So a Chromium run that timed out on a game that never went quiet now shows that the game's own document came, where one on Chromium's error page shows `error_page` and one on a 404 page `http_error`. A served run whose page stopped taking dyno's commands after its `hello` shows `command_not_taken`, even when the phone's recording gave every device measure. A run of the empty build whose log stopped shows it.

When a later run of `--runs` stops the check, as a phone that left does, dyno still writes no result and exits as before, but with `--out` it keeps the records of the runs already made beside it, as `<name>.runs.json`, with each game's id under `--games`. It never replaces a file of that name.

The fake phone of the tests (`DYNO_FAKE_PHONE`) takes three more settings: `sampled-after-ms`, for a page that the samples do not have yet; `log-fails-in`, for a log that closes in chosen recordings only; and `connect-fails-in`, for a phone that does not connect for chosen recordings.

[`docs/design/run-evidence.md`](docs/design/run-evidence.md) gives each rule, what a kill in the record rests on, the open questions where the record and a measure disagree, and what is not yet proven on a real phone.
