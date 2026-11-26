# Run evidence design

A result's measures say what a value is. They do not say whether the run under them really ran. Someone who runs dyno for other people must decide, from the result alone, whether a run measured the game or failed for a reason of the lab's own: a page that never loaded, a browser that lost its link to dyno, a recording that lost a feed. `run.runs` answers that. It is one small record per run, of the game and of the empty build, in the order the runs went, with what dyno itself saw.

Two rules run through it. A record writes what dyno observed, a time, a pid, a count, an error, and writes a conclusion only where the observations settle it for every order of events; where they do not, it keeps the observations and draws no conclusion. And the record is evidence only: for every input, every measure, the run coverage, the status, the verdicts, the result hash, the exit code and everything that a run sends are what they were before it. Where the record and a measure seem to disagree, the measure stays as it is, and the question is the maintainer's (below).

## The cases it answers

Before this record, each of these was found only by reading the code:

1. **A browser run that ended early.** A Chromium run (`--browser`, or a launch URL) that opened the game but never had a quiet period, or kept loading, times out. Its measures are all missing, with the same reason as a run that ended on Chromium's own error page, or on a 404 page. The `frame_document` mark in the messages does not tell them apart.
2. **A device run whose page stopped answering.** With `--runs`, a game run whose browser stopped taking dyno's commands after its `hello`, while the recording over the cable went on, still gives every device measure. The lab measures come from another game run that completed. Nothing said which runs completed as a page.
3. **An empty build run whose log stopped.** `device.kills` counts the game runs only. A recording that lost its log in a run of the empty build left no mark.
4. **A page killed before its first sample.** When the kernel killed the page before sysmontap sampled it, every device measure is missing, though the log gave a kill.

## The shape

`run.runs` is in the run part, so `result_hash` excludes it. A lab's signed receipt is another matter: it signs the SHA-256 of the whole result file (`result_sha256`), so it covers `run.runs` with the rest, and no longer matches a file whose records were added, changed or removed. It is left out of the JSON when the check had no browser run, so a build-only result is byte for byte what it was; a reader takes null as left out. A check with a browser run has one record. A check with `--runs N` has 2N: empty build 1, game 1, empty build 2, game 2, and so on.

```json
{
  "of": "game",
  "index": 1,
  "page": {
    "hello": true,
    "document": "not_observed",
    "probe": "heard",
    "end": "command_not_taken"
  },
  "recording": {
    "samples": { "read": 31, "last_ms": 12940, "stopped": null },
    "log": { "read": 2, "last_ms": 13010, "stopped": null },
    "pages": [{ "pid": 412, "first_ms": -880, "last_ms": 12940, "peak_bytes": 209715200 }],
    "pages_not_listed": 0,
    "kill": null,
    "error": null
  }
}
```

| Field                       | What it holds                                                                                                                                                                                                                                                                                                                                                                                     |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `of`                        | `game`, or `empty_build`: a run of dyno's own empty build with `--runs`, or the check of `--baseline`.                                                                                                                                                                                                                                                                                            |
| `index`                     | The run's number among the runs of the same build, from 1. The items of the device measures name runs the same way: "game run 2" is `of` game, `index` 2.                                                                                                                                                                                                                                         |
| `page.hello`                | Whether the run got its `hello`: from the harness page of a served run, or from the driver.                                                                                                                                                                                                                                                                                                       |
| `page.document`             | The document that the game frame held at the end, as far as the driver saw. `build`, `http_error` (a status of 400 or more), `error_page`, `other_url`, `no_document`, or `not_observed`: the driver cannot tell.                                                                                                                                                                                 |
| `page.probe`                | A served run only: whether a probe's document message reached dyno's servers during the run, `heard` or `not_heard`. Null for the other drivers.                                                                                                                                                                                                                                                  |
| `page.end`                  | `ended`: with `done`. `timed_out`: the deadline passed or cut a step short. `command_not_taken`: a served run's harness page did not take a command in its step's whole wait. `no_request`: no request came to a served run's harness address for more than 25 s. `connection_closed`: the CDP driver's DevTools connection closed. `failed`: any other error. `never_started`: no `hello`.       |
| `recording`                 | Null when the check recorded no phone.                                                                                                                                                                                                                                                                                                                                                            |
| `recording.samples`, `.log` | Each feed: what dyno `read` from it in the run's window (samples of Safari's page processes, or kernel memory lines about any process), when the last thing it sent arrived (`last_ms`), and the stop it reported, in its own words (`stopped`).                                                                                                                                                  |
| `recording.pages`           | One summary for each distinct pid of a page process (`com.apple.WebKit.WebContent`) that the samples had in the window: when its first and last sample arrived and its largest footprint, at most 16; `pages_not_listed` counts the other distinct pids. A summary is not one process: iOS can give a pid to a new process, and the samples of each process with that pid are in its one summary. |
| `recording.kill`            | The first kill of a page process that the log gave in the window: its `pid`, the kernel's `line` up to the page counts, and when the line arrived (`received_ms`). Null when the log gave none, which is no proof that none was killed.                                                                                                                                                           |
| `recording.error`           | Why the recorder gave back no recording, in its own words. Null when it gave one.                                                                                                                                                                                                                                                                                                                 |

A time is in ms since dyno answered the page's `hello`, on this machine's clock, below 0 before it; with no run, since the recording started. The window is the one the device measures read: from that answer to the end of the run, with 1.5 s more at each end, since a sample comes about once a second. A run that never started has no window: each feed is then read over the whole recording, and no kill is read.

No field holds an estimate, a time on another clock or a value from a model. Every text is cut to 200 characters, and the page pids to 16, so a run's record is at most 16 KiB of JSON, with every text at its longest and escaped; a test builds that record and checks it. A check of `--runs N` has 2N records.

## How each driver fills the page

**The CDP driver** sees each document that the game frame commits, in `Page.frameNavigated`. It follows the frame by its id on its own session and on the harness page's session: a redirect into the harness page's own site moves the frame into the page's process, where only the page's session reports it. With no game target at all, the game frame is the harness page's only child frame. Each commit names its loader, and each document response (`Network.responseReceived`) names its loader and its HTTP status. A response goes to the current commit when it is that loader's; others wait, at most eight, the oldest going first. At the end, the last commit gives the document:

- `error_page` when the frame names a URL it could not reach (`unreachableUrl`), or has the `chrome-error` scheme;
- at the launch URL's scheme, host and port, `build` when its response's status is below 400, `http_error` when it is 400 or more, and `not_observed` when the run saw no response for its loader;
- `other_url` anywhere else, a `blob:` document too. dyno compares the three parts itself and does not ask the `url` crate for the origin, which overflows the stack on a `blob:` URL nested deep enough (RUSTSEC-2026-0592), and the game chooses the URL;
- `not_observed` when the frame's target detached, unless the page's session itself said afterwards where the frame went. Every detach that the run observed counts, one still queued when the run ended too, as behind a call that failed. The run notes the frame's target as it attaches, before its set-up calls, so a detach queued behind a failed set-up counts as well;
- `no_document` when the frame committed nothing and did not leave, as in a run that never started.

A run is `connection_closed` when the DevTools connection closed during it, whatever error that gave the run. That holds for a run whose messages end with `done`, too: a watch takes a closed event stream for a quiet one, as before, so the run goes on to `done`, and its measures are what they were. Otherwise a run that fails after its `hello` is `timed_out` on the run's deadline and `failed` on any other error.

**The served run** (`--driver serve`) cannot see the game frame's document, so `document` is `not_observed`. `probe` is `heard` when a document message from a probe reached dyno's servers during the run: one that the run took, or one still in its queue when it ended. The probe tag is only in the build's own HTML pages, but the message does not say where it came from, and the harness page sends exactly what it sent before. The harness server takes a message only in a request with the run's key, and each run has a key of its own, so a page of an earlier run cannot send one: its key is refused. So `heard` records that a document message came with this run's key. It does not prove that the build's own document sent it: the message does not say which document in the game frame it came from, and a second tab opened at the run's URL has the key too.

A served run that fails after its `hello` says what dyno saw, in this order:

1. `timed_out`, when the run's deadline cut a step short, as a self-check that had less than its 15 s, whatever else was seen.
2. `command_not_taken`, when a command that dyno gave the harness page was not taken in its step's whole wait: 15 s for the self-check, 5 s for the flush. The page may still have sent events.
3. `no_request`, when no request came to the address that took the `hello` for more than 25 s. The page asks for its next command again as soon as an ask is answered, and dyno answers each ask within 20 s.
4. `timed_out`, when the frame never went idle: the wait for idle has no length of its own, so the deadline is its end.
5. `failed`, when the page took a command and did not answer it.

A request counts as contact when it came to the address that took the `hello`, with the run's key, and the server read the whole of it. A request without the key is refused before its body is read, and is not contact. So the other address of a run served at two does not hide a silence, and neither does a page of an earlier run, whose key is refused. dyno scopes nothing else to the page that said hello: a second tab opened at the run's URL has the key, so it can take a command meant for the page, and its requests count as the page's. Read `command_not_taken`, `no_request`, a `timed_out` that rests on recent requests, and `probe`, with that in mind.

**The exec driver** runs an outside program, whose messages are its own word, so the document is `not_observed`. dyno sees whether the program said `hello` before an error that dyno itself ended the run with, and whether that error was the run's deadline.

A driver clears what it saw of a run before the first step of the next one that can fail, so a run that fails at once never reports the run before it.

## How the recording is read

The record reads the recording that the device measures read, in `dyno_ios::record`, and changes nothing in it or in them. It says what each feed gave, when each last sent and the stop each reported, and draws no conclusion from them. Whether a feed ran to the end of the run is not settled by any of them: a feed that went quiet may have stopped, or had nothing to say, and the recorder has no keep-alive. When the recorder gives back no recording, nothing was read and nothing is known of either feed, and its error is the recording's `error`.

The kill is the first kill of a page process that the log gave in the window, with when its line arrived. Whether it was the game's is not said, because the observations do not settle it for every order of events: a pid can come back to another process; a sample of a new process with that pid can arrive before the log line of the kill that came first; and nothing ties a page process to the game's frame but its footprint. So the record lists the distinct pids of page processes that the samples had, each with a summary of its samples, and leaves the reading to the reader. A pid's summary can hold the samples of more than one process, so its first sample, its last and its peak can each be of a different one. The relay's line has the phone's own time on it, to the second, with no year, no time zone and no known offset from this machine's clock, so the record gives the time the line arrived, which is after the kill.

## What the measures do

Nothing changes. `device.kills` counts a kill as before: a kill line of the game's pid in the run window, with its slack, and every device measure is missing as before when the samples have no page process, though the record may have a kill. The CDP driver's watches take a closed event stream for a quiet one as before, so a run whose connection closed can end with `done` and measures, while its record says `connection_closed`. A browser run that ended early keeps the evidence of its document in the record: `page.document` is `build` and `page.end` is `timed_out`, where an error page gives `error_page` and a 404 page `http_error`.

Nothing that a run sends changes either. The harness page is the one before, byte for byte, and a test pins its template's SHA-256. dyno reads the page's events as it did: what the record hears of the probe, it notes beside the event, which then goes on as before.

Three proofs show this. The base branch's own tests of the measures (the device and served checks of the CLI, the CLI's browser checks, the served run's driver, and the Chromium runs) pass, unchanged, against this branch. Each function that makes a measure, a finding, the run coverage, the status or a message that the lab measures read is the base's, verbatim. And the base's binary and this one, given the same inputs by a script that plays the browser (a batch of events at exactly the server's body limit, a document message with an extra field of the wrong type, two document messages in one run, a whole run, each with and without a fake phone), agree on the deterministic part, the hash, the measures, the findings and the exit code.

## Open questions for the maintainer

These measures stay as they are. The record shows each case. A change of status would change the run coverage, and so the status, the hash and the exit code, of a result that had it.

- **A device measure of `--runs` that rests on a game run whose page did not end by itself** (`page.end` is not `ended`). It could be missing, with that run in the reason, as a run with no reading is now. That would make such a check `incomplete`, where today it can pass on device readings from a run whose page the lab did not see to its end.
- **`device.footprint_over_baseline` when a run of the empty build has a log that stopped or went quiet.** It could be missing, since a page that the phone killed lowers the empty build's peak and raises the game's footprint over it. That would make such a check `incomplete` where today it measures the footprint over a baseline that may be broken.
- **`device.kills` and which process a kill was.** `device.kills` counts a kill line of the game's pid in the run window with its slack, whatever process the line names, and also one that came before the process's first sample or in the 1.5 s after the run ended, or of a pid that came back to another process. A rule that settles which process instance a kill was, for every order of events, would need more than the samples and the log give.
- **`device.kills` when the samples have no page process.** Every device measure is then missing, though the log may have the kill of a page process. No sample ties that process to the game, so counting it would rest on no evidence, as with `--no-open` and another tab open.
- **`device.kills` from a log that was quiet.** A log that sent nothing near the end of the run, and reported no error, gives `device.kills` 0 as a measure, since the recording's own coverage takes it to have run to the stop. The record shows when the log last sent, and no kill. The measure could be missing there, with the reason that the log went quiet; a check whose log was quiet would then be `incomplete`, where today it reports no kill.
- **A bound on the served run's events.** The harness page's queue of events, and what dyno keeps of them, are not bounded across batches, and a body over the server's 4 MiB limit loses its whole batch. A bound would have to choose between dropping the game's events early, before a batch could reach the limit, and letting a body pass it. Either changes which events reach dyno, and so the lab measures of a game that sends that much. That choice is the maintainer's, in a change of its own.
- **The lab measures of a CDP run whose DevTools connection closed.** The run can go on to `done` and give lab measures from what it saw before the connection closed. The run could end as failed there, which would leave every lab measure missing and the check `incomplete`.

## When a check of `--runs` stops

A phone that dyno cannot record, a server that does not bind, or an opener that stops the check, in a later run of `--runs`, stops the check with an error and no result, as before, with the same exit code. With `--out`, dyno keeps the records of the runs already made beside it, as `<name>.runs.json` from the name of `--out` alone, and says so on stderr. The file is a list with one entry for each game: its `game`, the id in a games file or null for a check of one target, and its `runs`. With `--games`, the games checked before the error keep their records with the runs of the game that stopped. The file is written to a new file with a name of its own, created exclusively, then linked to its name only if no file is there: dyno never replaces a file, such as a games file of that name, and says on stderr when it keeps nothing for that reason. Without `--out` or `--runs`, nothing is kept.

## Not proven without a phone

The fake phone (`DYNO_FAKE_PHONE`) and the test's own browser stand in for the phone and its Safari. These are not proven on a real one:

- That a kill of the page's process, which Safari shares between the harness page and the game frame, stops the harness page's requests, so that the end reads `command_not_taken` or `no_request`. What Safari does next, such as load the page again, is not known: a second `hello` would be turned away, as it is now.
- That a Safari page in the background or a locked phone stops its requests for longer than 25 s, and so reads as `no_request`.
- How late the relay's lines arrive after the kill they report.

The fake phone's settings for the tests of these records (`sampled-after-ms`, `log-fails-in`, `connect-fails-in`) are in the same parser as its others, because the tests run the `dyno` binary as a process of its own and can reach the fake phone only through that variable.
