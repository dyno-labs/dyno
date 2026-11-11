//! A run's memory timeline, `run.timeline`, from its `memory`, `roles` and `mark` messages.

use std::collections::{BTreeMap, HashMap};

use dyno_schema::runner::DriverMessage;
use dyno_schema::scrub::scrub;
use dyno_schema::timeline::{MemorySample, ProcessRole, Timeline, TimelineMark, TimelineProcess};

/// The timeline of a run, or None when the driver sent no `memory` message.
///
/// The method and the time between samples come from the first `memory` message. A sample from
/// before the navigation has no place on the run's clock, so it is left out. A process's role is
/// the last one a `roles` message gave it; a process that no message names is `other`. Its kind
/// is the last one a sample gave it, scrubbed, since it comes from the driver.
pub fn read(messages: &[DriverMessage]) -> Option<Timeline> {
    let mut method = None;
    let mut processes: BTreeMap<u32, TimelineProcess> = BTreeMap::new();
    let mut roles: HashMap<u32, ProcessRole> = HashMap::new();
    let mut marks = Vec::new();
    for message in messages {
        match message {
            DriverMessage::Memory {
                method: read_by,
                every_ms,
                processes: read,
                t_ms,
            } => {
                method.get_or_insert((*read_by, *every_ms));
                let Some(t_ms) = run_ms(*t_ms) else {
                    continue;
                };
                for process in read {
                    let entry = processes
                        .entry(process.pid)
                        .or_insert_with(|| TimelineProcess {
                            pid: process.pid,
                            role: ProcessRole::Other,
                            kind: String::new(),
                            samples: Vec::new(),
                        });
                    entry.kind = scrub(&process.kind);
                    entry.samples.push(MemorySample {
                        t_ms,
                        private_bytes: process.private_bytes,
                        resident_bytes: process.resident_bytes,
                        shared_bytes: process.shared_bytes,
                    });
                }
            }
            DriverMessage::Roles { roles: named, .. } => {
                roles.extend(named.iter().map(|named| (named.pid, named.role)));
            }
            DriverMessage::Mark { mark, t_ms } => {
                if let Some(t_ms) = run_ms(*t_ms) {
                    marks.push(TimelineMark { mark: *mark, t_ms });
                }
            }
            _ => {}
        }
    }
    let (method, every_ms) = method?;
    let mut timeline = Timeline {
        method,
        every_ms,
        marks,
        processes: processes
            .into_values()
            .map(|process| TimelineProcess {
                role: roles
                    .get(&process.pid)
                    .copied()
                    .unwrap_or(ProcessRole::Other),
                ..process
            })
            .collect(),
    };
    timeline.sort_lists();
    Some(timeline)
}

/// A time on the driver's clock in whole ms, or None before the navigation or when it is not a
/// number.
fn run_ms(t_ms: f64) -> Option<u64> {
    (t_ms.is_finite() && t_ms >= 0.0).then(|| t_ms.round() as u64)
}

#[cfg(test)]
mod tests {
    use dyno_schema::runner::{AssignedRole, ProcessMemory, RunMark};
    use dyno_schema::timeline::TimelineMethod;

    use super::*;

    fn memory(t_ms: f64, processes: &[(u32, &str, u64)]) -> DriverMessage {
        DriverMessage::Memory {
            method: TimelineMethod::Proc,
            every_ms: 250,
            processes: processes
                .iter()
                .map(|&(pid, kind, private_bytes)| ProcessMemory {
                    pid,
                    kind: kind.to_owned(),
                    private_bytes,
                    resident_bytes: Some(private_bytes * 2),
                    shared_bytes: Some(1),
                })
                .collect(),
            t_ms,
        }
    }

    fn roles(roles: &[(u32, ProcessRole)]) -> DriverMessage {
        DriverMessage::Roles {
            roles: roles
                .iter()
                .map(|&(pid, role)| AssignedRole { pid, role })
                .collect(),
            t_ms: 0.0,
        }
    }

    #[test]
    fn a_run_with_no_memory_message_has_no_timeline() {
        let messages = [DriverMessage::Mark {
            mark: RunMark::Idle,
            t_ms: 10.0,
        }];
        assert_eq!(read(&messages), None);
    }

    #[test]
    fn samples_are_grouped_by_process_with_the_runs_marks() {
        let messages = [
            memory(-5.0, &[(1, "browser", 99)]),
            DriverMessage::Mark {
                mark: RunMark::Navigated,
                t_ms: 0.0,
            },
            memory(0.4, &[(1, "browser", 10), (7, "renderer", 20)]),
            memory(250.6, &[(7, "renderer", 30), (1, "browser", 11)]),
            DriverMessage::Mark {
                mark: RunMark::Idle,
                t_ms: 240.0,
            },
        ];
        let timeline = read(&messages).unwrap();
        assert_eq!(timeline.method, TimelineMethod::Proc);
        assert_eq!(timeline.every_ms, 250);
        let marks: Vec<(RunMark, u64)> = timeline
            .marks
            .iter()
            .map(|mark| (mark.mark, mark.t_ms))
            .collect();
        assert_eq!(marks, [(RunMark::Navigated, 0), (RunMark::Idle, 240)]);
        let pids: Vec<u32> = timeline
            .processes
            .iter()
            .map(|process| process.pid)
            .collect();
        assert_eq!(pids, [1, 7]);
        let browser = &timeline.processes[0];
        assert_eq!(browser.kind, "browser");
        // The sample from before the navigation is left out.
        let samples: Vec<(u64, u64)> = browser
            .samples
            .iter()
            .map(|sample| (sample.t_ms, sample.private_bytes))
            .collect();
        assert_eq!(samples, [(0, 10), (251, 11)]);
        assert_eq!(browser.samples[0].resident_bytes, Some(20));
        assert_eq!(browser.samples[0].shared_bytes, Some(1));
    }

    #[test]
    fn the_last_role_wins_and_an_unnamed_process_is_other() {
        let messages = [
            memory(
                0.0,
                &[(1, "browser", 1), (7, "renderer", 2), (8, "renderer", 3)],
            ),
            roles(&[(1, ProcessRole::Browser), (7, ProcessRole::Page)]),
            roles(&[(7, ProcessRole::Game)]),
        ];
        let timeline = read(&messages).unwrap();
        let roles: Vec<(u32, ProcessRole)> = timeline
            .processes
            .iter()
            .map(|process| (process.pid, process.role))
            .collect();
        assert_eq!(
            roles,
            [
                (1, ProcessRole::Browser),
                (7, ProcessRole::Game),
                (8, ProcessRole::Other)
            ]
        );
    }

    #[test]
    fn a_kind_is_scrubbed() {
        let messages = [memory(0.0, &[(3, "https://u:secret@example.com/?t=1", 1)])];
        let timeline = read(&messages).unwrap();
        assert!(!timeline.processes[0].kind.contains("secret"));
    }
}
