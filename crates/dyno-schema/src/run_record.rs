//! What dyno itself saw of each run of a check: one small record per run of the game, and per
//! run of the empty build with `--runs`, in the order they ran.
//!
//! A measure says what a value is. A record says whether the run under it really ran: whether the
//! page said hello, whether the game frame's own document came, how the page's run ended, and,
//! with a phone, whether each of the recording's feeds ran for the whole run and what the kernel
//! log said of the page's process. So someone who runs dyno for other people can tell, from the
//! result alone, a run that measured the game from one that failed for a reason of the lab's own.
//!
//! Every field is a fact that dyno read, or a fixed rule on facts it read. A thing dyno cannot see
//! is `not_observed`, or null, and never a guess: when dyno cannot tell, it says less. The records differ on every run, so they are in
//! the run part of a result, outside the hash.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One run of the game or of the empty build, as dyno saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RunRecord {
    /// Which build ran.
    pub of: RunOf,
    /// The run's number among the runs of the same build, from 1, as the items of the device
    /// measures name it: "game run 2" is `of` game, `index` 2.
    pub index: u32,
    /// The page's run in the browser.
    pub page: PageRecord,
    /// The phone's recording of the run. Null when the check recorded no phone.
    pub recording: Option<RecordingRecord>,
}

/// Which build a run loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunOf {
    /// The build under check.
    Game,
    /// dyno's own empty build: a run of `--runs` that measures what Safari takes for an empty
    /// page, or the build of `--baseline`.
    EmptyBuild,
}

/// What dyno saw of the page's run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PageRecord {
    /// Whether the run got its `hello`: from the harness page of a served run, or from the
    /// driver. A run with no `hello` never started.
    pub hello: bool,
    /// The document that the game frame held at the end of the run, as far as the driver saw.
    pub document: PageDocument,
    /// Whether a probe's document message reached dyno (served run). Null for a driver that sees
    /// the frame's documents itself, or none of them.
    pub probe: Option<ProbeHeard>,
    /// How the page's run ended.
    pub end: PageEnd,
}

/// The document that the game frame held at the end of the run, as the driver saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PageDocument {
    /// The build's own (CDP driver): the frame committed a document at the launch URL's scheme,
    /// host and port, from a response with an HTTP status below 400.
    Build,
    /// A document at the launch URL's scheme, host and port from a response with an HTTP status
    /// of 400 or more, such as a 404 page (CDP driver).
    HttpError,
    /// The browser's own error page, which a navigation that failed commits (CDP driver).
    ErrorPage,
    /// A document at another scheme, host or port than the launch URL's, such as one that a
    /// redirect led to, or a `blob:` document (CDP driver).
    OtherUrl,
    /// The driver saw the game frame commit no document.
    NoDocument,
    /// The driver cannot tell: an outside program's (`exec`), a served run's, which sees only what
    /// the probe sends, or a CDP run whose frame left the sessions it follows, or whose document's
    /// response it did not see.
    NotObserved,
}

/// Whether a probe was heard from in a served run: whether a document message from a probe
/// reached dyno during the run. The probe tag is only in the build's own pages, and dyno takes
/// the message only with the run's key, so no page of another run can send it. But the message
/// does not say which document sent it: `heard` says that one came, not that the build sent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProbeHeard {
    /// A document message from a probe reached dyno during the run.
    Heard,
    /// None did.
    NotHeard,
}

/// How the page's run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PageEnd {
    /// The run ended by itself, with `done`.
    Ended,
    /// The run's deadline passed, or cut a step short. In a served run whose deadline cut no
    /// step short, a request with the run's key came to the address that took the `hello` in
    /// the last 25 s.
    TimedOut,
    /// In a served run, a command that dyno gave the harness page was not taken in the whole of
    /// its step's wait: 15 s for the self-check, 5 s for the flush. That is all it says: the page
    /// may still have sent events.
    CommandNotTaken,
    /// In a served run, no request with the run's key came to the address that took the `hello`
    /// for more than 25 s, longer than the page's poll for its next command waits. A request
    /// without the key is refused and does not count. A second tab opened at the run's URL has
    /// the key, and counts as the page.
    NoRequest,
    /// The driver's connection to the browser closed during the run: with the CDP driver, the
    /// DevTools connection, as when the browser exits or crashes.
    ConnectionClosed,
    /// The run ended with any other error, such as a page that crashed or a harness page that
    /// took a command and did not answer it.
    Failed,
    /// The run failed before its `hello`, such as a browser that did not start, or no browser
    /// that opened the harness page.
    NeverStarted,
}

/// What dyno saw of the phone's recording of one run (`--device`): what each feed gave, a summary
/// of the samples of each distinct page process pid, and the kill of a page process that the log
/// gave. It says what was seen, and draws no conclusion: no kill here is no proof that no process
/// was killed.
///
/// A time is in ms since dyno answered the page's `hello`, on this machine's clock, and is below
/// 0 for a time before it; with no run, since the recording started. Every text is cut to
/// [`MAX_TEXT_CHARS`] characters, and the page pids to [`MAX_PAGES`], so a record is at most
/// [`MAX_RECORD_BYTES`] of JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RecordingRecord {
    /// The memory samples: sysmontap's footprint of each of Safari's processes.
    pub samples: FeedRecord,
    /// The phone's log: the kernel's memory lines.
    pub log: FeedRecord,
    /// One summary for each distinct pid of a page process (`com.apple.WebKit.WebContent`) that
    /// the samples had in the run's window, in the order of each pid's first sample, at most
    /// [`MAX_PAGES`]. A summary is not one process: iOS can give a pid to a new process, and the
    /// samples of every process with that pid go into its one summary.
    pub pages: Vec<SampledPage>,
    /// How many more distinct pids of page processes the samples had in the window, past those
    /// listed.
    pub pages_not_listed: u32,
    /// The first kill of a page process that the log gave in the run's window. Null when it gave
    /// none, which is no proof that none was killed: the log may have stopped, or been late.
    pub kill: Option<KillRecord>,
    /// Why the recorder gave back no recording, in its own words. Null when it gave one back.
    pub error: Option<String>,
}

/// One feed of a recording, over the run's window: from dyno's answer to the page's `hello` to
/// the end of the run, with 1.5 s more at each end, as the device measures read it. A run that
/// never started has no window, so a feed is read over the whole recording.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FeedRecord {
    /// What dyno read from it in the window: the samples of page processes, or the kernel's
    /// memory lines about any process.
    pub read: u32,
    /// When the last thing that the feed sent arrived, at any time in the recording. Null when
    /// it sent nothing. A feed that went quiet may have stopped, or had nothing to say.
    pub last_ms: Option<i64>,
    /// The stop that the feed reported, in its own words: its error, or the recording's note of a
    /// long silence before the stop. Null when it reported none.
    pub stopped: Option<String>,
}

/// The samples of one distinct pid of a page process in the run's window, summed up. It is not
/// one process: the samples of every process that had the pid in the window are in it, so its
/// first sample, its last and its peak can each be of a different process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SampledPage {
    pub pid: u32,
    /// When the first and the last sample of the pid in the window arrived.
    pub first_ms: i64,
    pub last_ms: i64,
    /// The largest footprint of a sample of the pid in the window, in bytes.
    pub peak_bytes: u64,
}

/// A kill of a page process that the phone's log gave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct KillRecord {
    /// The pid that the kernel's line names.
    pub pid: u32,
    /// The kernel's words, up to the page counts, such as "memorystatus: killing process 412
    /// [com.apple.WebKit.WebContent] in high band FOREGROUND (100)".
    pub line: String,
    /// When the line arrived, which is after the kill: the phone's own time on it has no year, no
    /// time zone and no known offset from this machine's clock.
    pub received_ms: i64,
}

/// The most characters of any text in a record.
pub const MAX_TEXT_CHARS: usize = 200;

/// The most distinct pids of page processes that a record lists.
pub const MAX_PAGES: usize = 16;

/// The most bytes of JSON that one run's record holds, with every text at its most characters,
/// each of them escaped, and every page pid listed.
pub const MAX_RECORD_BYTES: usize = 16 * 1024;

/// The text, cut to [`MAX_TEXT_CHARS`] characters.
pub fn bounded_text(text: &str) -> String {
    text.chars().take(MAX_TEXT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_in_a_result_are_snake_case() {
        let names = [
            serde_json::to_string(&RunOf::EmptyBuild).unwrap(),
            serde_json::to_string(&PageDocument::ErrorPage).unwrap(),
            serde_json::to_string(&PageDocument::NotObserved).unwrap(),
            serde_json::to_string(&PageEnd::CommandNotTaken).unwrap(),
            serde_json::to_string(&PageEnd::NeverStarted).unwrap(),
            serde_json::to_string(&PageEnd::ConnectionClosed).unwrap(),
            serde_json::to_string(&PageDocument::HttpError).unwrap(),
            serde_json::to_string(&ProbeHeard::NotHeard).unwrap(),
        ];
        assert_eq!(
            names,
            [
                "\"empty_build\"",
                "\"error_page\"",
                "\"not_observed\"",
                "\"command_not_taken\"",
                "\"never_started\"",
                "\"connection_closed\"",
                "\"http_error\"",
                "\"not_heard\""
            ]
        );
    }

    #[test]
    fn a_text_is_cut_to_its_most_characters_on_a_character() {
        assert_eq!(
            bounded_text(&"é".repeat(MAX_TEXT_CHARS + 5)),
            "é".repeat(MAX_TEXT_CHARS)
        );
        assert_eq!(bounded_text("short"), "short");
    }

    #[test]
    fn a_record_with_every_text_and_list_at_its_most_stays_within_its_bound() {
        // Control characters escape to six bytes each in JSON.
        let text = || bounded_text(&"\u{1}".repeat(10 * MAX_TEXT_CHARS));
        let feed = || FeedRecord {
            read: u32::MAX,
            last_ms: Some(i64::MIN),
            stopped: Some(text()),
        };
        let record = RunRecord {
            of: RunOf::EmptyBuild,
            index: u32::MAX,
            page: PageRecord {
                hello: true,
                document: PageDocument::NotObserved,
                probe: Some(ProbeHeard::NotHeard),
                end: PageEnd::CommandNotTaken,
            },
            recording: Some(RecordingRecord {
                samples: feed(),
                log: feed(),
                pages: vec![
                    SampledPage {
                        pid: u32::MAX,
                        first_ms: i64::MIN,
                        last_ms: i64::MIN,
                        peak_bytes: u64::MAX,
                    };
                    MAX_PAGES
                ],
                pages_not_listed: u32::MAX,
                kill: Some(KillRecord {
                    pid: u32::MAX,
                    line: text(),
                    received_ms: i64::MIN,
                }),
                error: Some(text()),
            }),
        };
        let bytes = serde_json::to_vec(&record).unwrap().len();
        assert!(bytes <= MAX_RECORD_BYTES, "{bytes}");
    }

    #[test]
    fn a_record_survives_a_json_round_trip() {
        let record = RunRecord {
            of: RunOf::Game,
            index: 2,
            page: PageRecord {
                hello: true,
                document: PageDocument::NotObserved,
                probe: Some(ProbeHeard::Heard),
                end: PageEnd::NoRequest,
            },
            recording: Some(RecordingRecord {
                samples: FeedRecord {
                    read: 31,
                    last_ms: Some(12_900),
                    stopped: None,
                },
                log: FeedRecord {
                    read: 0,
                    last_ms: None,
                    stopped: Some("the relay closed".to_owned()),
                },
                pages: vec![SampledPage {
                    pid: 412,
                    first_ms: -900,
                    last_ms: 6_100,
                    peak_bytes: 1_048_576,
                }],
                pages_not_listed: 0,
                kill: Some(KillRecord {
                    pid: 412,
                    line: "memorystatus: killing process 412 [com.apple.WebKit.WebContent]"
                        .to_owned(),
                    received_ms: 6_200,
                }),
                error: None,
            }),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<RunRecord>(&json).unwrap(), record);
    }
}
