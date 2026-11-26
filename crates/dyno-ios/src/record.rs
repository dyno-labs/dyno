//! What a recording gave one run, for the run's record in `run.runs`.
//!
//! This is evidence only. It reads the same recording as [`crate::device_measures`], and changes
//! no measure. It writes what was seen, and draws no conclusion that the observations do not
//! settle for every order of events: what each feed gave and when it last sent, a summary of the
//! samples of each distinct page process pid, and the first kill of a page process that the log
//! gave, with when its line arrived. A summary is of a pid, not of one process: iOS can give a
//! pid to a new process. Whether that kill was the game's is not said: a pid can come back to another
//! process, a sample can arrive before the log line of a kill that came first, and nothing ties a
//! page process to the game's frame but its footprint.

use dyno_schema::run_record::{
    FeedRecord, KillRecord, MAX_PAGES, RecordingRecord, SampledPage, bounded_text,
};

use crate::recording::{Coverage, LoggedLine, Recording};
use crate::{RunWindow, SLACK_MS, WEB_CONTENT};

/// What a recording gave one run. With no run, as when no page said hello, each feed is read
/// over the whole recording, times are from the recording's start, and no kill is read.
pub fn recording_record(recording: &Recording, run: Option<&RunWindow>) -> RecordingRecord {
    let zero = run.map_or(recording.started_epoch_ms, |run| run.navigated_epoch_ms);
    let ms = |epoch_ms: f64| (epoch_ms - zero).round() as i64;
    let counted = |at: f64| run.is_none_or(|run| in_window(run, at));
    let pages: Vec<_> = recording
        .samples
        .iter()
        .filter(|sample| sample.process == WEB_CONTENT && counted(sample.epoch_ms))
        .collect();
    let lines = recording
        .memory_lines
        .iter()
        .filter(|logged| counted(logged.epoch_ms))
        .count();
    let feed = |cover: &Coverage, read: usize| FeedRecord {
        read: u32::try_from(read).unwrap_or(u32::MAX),
        last_ms: cover.last_epoch_ms.map(ms),
        stopped: cover.ended.as_deref().map(bounded_text),
    };
    // Each distinct pid of a page process, in the order of its first sample. The samples of every
    // process that had the pid go into its one summary. Samples come in the order they arrived.
    let mut sampled: Vec<SampledPage> = Vec::new();
    for sample in &pages {
        match sampled.iter_mut().find(|page| page.pid == sample.pid) {
            Some(page) => {
                page.last_ms = ms(sample.epoch_ms);
                page.peak_bytes = page.peak_bytes.max(sample.footprint_bytes);
            }
            None => sampled.push(SampledPage {
                pid: sample.pid,
                first_ms: ms(sample.epoch_ms),
                last_ms: ms(sample.epoch_ms),
                peak_bytes: sample.footprint_bytes,
            }),
        }
    }
    let not_listed = sampled.len().saturating_sub(MAX_PAGES);
    sampled.truncate(MAX_PAGES);
    let kill = run.and_then(|run| {
        recording
            .memory_lines
            .iter()
            .find(|logged| is_page_kill(logged) && in_window(run, logged.epoch_ms))
            .map(|killed| KillRecord {
                pid: killed.line.pid,
                line: bounded_text(killed.line.short_text()),
                received_ms: ms(killed.epoch_ms),
            })
    });
    RecordingRecord {
        samples: feed(&recording.samples_cover, pages.len()),
        log: feed(&recording.log_covers, lines),
        pages: sampled,
        pages_not_listed: u32::try_from(not_listed).unwrap_or(u32::MAX),
        kill,
        error: None,
    }
}

/// What a recorder that gave back no recording gave a run: nothing was read, and nothing is
/// known of either feed. The recorder's own error is the recording's.
pub fn failed_recording_record(reason: &str) -> RecordingRecord {
    let feed = || FeedRecord {
        read: 0,
        last_ms: None,
        stopped: None,
    };
    RecordingRecord {
        samples: feed(),
        log: feed(),
        pages: Vec::new(),
        pages_not_listed: 0,
        kill: None,
        error: Some(bounded_text(reason)),
    }
}

/// Whether a log line is a kill of a Safari page process.
fn is_page_kill(logged: &LoggedLine) -> bool {
    logged.line.process == WEB_CONTENT && logged.line.is_kill()
}

/// Whether a time is in the run window, with [`SLACK_MS`] at each end, as the device measures
/// read it.
fn in_window(run: &RunWindow, at: f64) -> bool {
    at >= run.navigated_epoch_ms - SLACK_MS && at <= run.ended_epoch_ms + SLACK_MS
}

#[cfg(test)]
mod tests {
    use dyno_schema::run_record::MAX_TEXT_CHARS;

    use super::*;
    use crate::recording::Sample;
    use crate::recording::tests::phone;
    use crate::syslog::memory_line;

    const START: f64 = 1_000_000.0;
    const GAME: u32 = 412;
    const OTHER_TAB: u32 = 377;
    const KILL: &str = "memorystatus: killing process 412 [com.apple.WebKit.WebContent] in high \
        band FOREGROUND (100) - memorystatus_available_pages: 53907";
    const OTHER_KILL: &str = "memorystatus: killing process 377 [com.apple.WebKit.WebContent] in \
        high band FOREGROUND (100)";

    fn at(ms: u64) -> f64 {
        START + ms as f64
    }

    /// A recording from START to 14 s, with these page process samples, (ms, pid, bytes), and
    /// these log lines, (ms, line). Both feeds last sent at the stop.
    fn recording(samples: &[(u64, u32, u64)], lines: &[(u64, &str)]) -> Recording {
        let to_end = Coverage {
            until_epoch_ms: at(14_000),
            ended: None,
            last_epoch_ms: Some(at(14_000)),
        };
        Recording {
            phone: phone(),
            memory_bytes: Some(1),
            started_epoch_ms: START,
            stopped_epoch_ms: at(14_000),
            samples: samples
                .iter()
                .map(|&(ms, pid, footprint_bytes)| Sample {
                    epoch_ms: at(ms),
                    pid,
                    process: WEB_CONTENT.to_owned(),
                    footprint_bytes,
                })
                .collect(),
            samples_cover: to_end.clone(),
            memory_lines: lines
                .iter()
                .map(|&(ms, line)| LoggedLine {
                    epoch_ms: at(ms),
                    line: memory_line(line).unwrap(),
                })
                .collect(),
            log_covers: to_end,
        }
    }

    /// The page opens at 3 s, so the window opens at 1.5 s; the run ends at 13 s.
    fn run() -> RunWindow {
        RunWindow {
            navigated_epoch_ms: at(3_000),
            idle_epoch_ms: Some(at(10_400)),
            activated_epoch_ms: Some(at(10_600)),
            ended_epoch_ms: at(13_000),
        }
    }

    #[test]
    fn the_record_lists_each_page_process_with_when_it_was_sampled() {
        let recording = recording(
            &[
                (500, OTHER_TAB, 9),
                (2_000, GAME, 100),
                (4_000, OTHER_TAB, 50),
                (8_000, GAME, 900),
                (20_000, GAME, 1),
            ],
            &[],
        );
        let record = recording_record(&recording, Some(&run()));
        // Before the window and after it, samples are left out.
        assert_eq!(
            record.pages,
            [
                SampledPage {
                    pid: GAME,
                    first_ms: -1_000,
                    last_ms: 5_000,
                    peak_bytes: 900,
                },
                SampledPage {
                    pid: OTHER_TAB,
                    first_ms: 1_000,
                    last_ms: 1_000,
                    peak_bytes: 50,
                },
            ]
        );
        assert_eq!(record.samples.read, 3);
    }

    #[test]
    fn a_pid_that_came_back_to_a_new_process_is_one_summary_of_both() {
        // The game's process is killed at 6 s, and a new page process gets its pid at 7 s.
        let recording = recording(
            &[(4_000, GAME, 900), (7_000, GAME, 120), (9_000, GAME, 300)],
            &[(6_000, KILL)],
        );
        let record = recording_record(&recording, Some(&run()));
        // One summary of the pid: its first sample and its peak are of the first process, its
        // last sample of the second.
        assert_eq!(
            record.pages,
            [SampledPage {
                pid: GAME,
                first_ms: 1_000,
                last_ms: 6_000,
                peak_bytes: 900,
            }]
        );
        assert_eq!(record.pages_not_listed, 0);
        assert_eq!(record.samples.read, 3);
    }

    #[test]
    fn the_record_has_the_first_page_kill_in_the_window_with_when_its_line_arrived() {
        let record = recording_record(
            &recording(
                &[(4_000, GAME, 900)],
                &[(500, OTHER_KILL), (6_000, KILL), (7_000, OTHER_KILL)],
            ),
            Some(&run()),
        );
        assert_eq!(
            record.kill,
            Some(KillRecord {
                pid: GAME,
                line: "memorystatus: killing process 412 [com.apple.WebKit.WebContent] in high \
                       band FOREGROUND (100)"
                    .to_owned(),
                received_ms: 3_000,
            })
        );
        // A line that names the pid with another process is no page kill.
        let reused = "memorystatus: killing process 412 [com.apple.mediaserverd] in high band \
            FOREGROUND (100)";
        let other = recording_record(&recording(&[], &[(6_000, reused)]), Some(&run()));
        assert_eq!(other.kill, None);
    }

    #[test]
    fn each_feed_says_what_it_read_when_it_last_sent_and_the_stop_it_reported() {
        let mut recording = recording(&[(4_000, GAME, 900)], &[(6_000, KILL), (13_400, KILL)]);
        recording.log_covers = Coverage {
            until_epoch_ms: at(11_000),
            ended: Some("the relay closed".to_owned()),
            last_epoch_ms: Some(at(11_000)),
        };
        let record = recording_record(&recording, Some(&run()));
        assert_eq!(
            record.log,
            FeedRecord {
                read: 2,
                last_ms: Some(8_000),
                stopped: Some("the relay closed".to_owned()),
            }
        );
        assert_eq!(record.samples.last_ms, Some(11_000));
    }

    #[test]
    fn a_feed_that_sent_nothing_has_no_last_time() {
        let mut recording = recording(&[], &[]);
        recording.samples_cover.last_epoch_ms = None;
        assert_eq!(
            recording_record(&recording, Some(&run())).samples.last_ms,
            None
        );
    }

    #[test]
    fn with_no_run_the_record_reads_the_whole_recording_from_its_start_and_no_kill() {
        let recording = recording(&[(100, GAME, 50), (13_900, GAME, 60)], &[(6_000, KILL)]);
        let record = recording_record(&recording, None);
        assert_eq!(record.samples.read, 2);
        assert_eq!(record.samples.last_ms, Some(14_000));
        assert_eq!(record.log.read, 1);
        assert_eq!(record.pages[0].first_ms, 100);
        assert_eq!(record.kill, None);
    }

    #[test]
    fn a_record_lists_at_most_its_most_page_processes_and_counts_the_rest() {
        let samples: Vec<(u64, u32, u64)> = (0..20).map(|pid| (4_000, pid, 1)).collect();
        let record = recording_record(&recording(&samples, &[]), Some(&run()));
        assert_eq!(record.pages.len(), MAX_PAGES);
        assert_eq!(record.pages_not_listed, 4);
    }

    #[test]
    fn every_text_from_the_phone_is_cut_to_its_most_characters() {
        let mut recording = recording(&[], &[]);
        recording.samples_cover.ended = Some("x".repeat(10 * MAX_TEXT_CHARS));
        let record = recording_record(&recording, Some(&run()));
        assert_eq!(
            record.samples.stopped.unwrap().chars().count(),
            MAX_TEXT_CHARS
        );
        let failed = failed_recording_record(&"y".repeat(10 * MAX_TEXT_CHARS));
        assert_eq!(failed.error.unwrap().chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn a_recorder_that_gave_back_no_recording_has_its_error_and_no_known_feed() {
        let record = failed_recording_record("the recording thread stopped");
        for feed in [&record.samples, &record.log] {
            assert_eq!(feed.read, 0);
            assert_eq!(feed.last_ms, None);
            assert_eq!(feed.stopped, None);
        }
        assert_eq!(record.kill, None);
        assert_eq!(
            record.error.as_deref(),
            Some("the recording thread stopped")
        );
    }
}
