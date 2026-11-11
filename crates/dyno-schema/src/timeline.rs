//! Memory over one run: each browser process's memory, sampled through the run, with the run's
//! steps on the same clock.
//!
//! A timeline keeps every sample, so the peak, the settled value or any other point can be worked
//! out later. It differs on every run, so it is in the run part of a result, outside the hash.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::runner::RunMark;

/// The memory of each browser process through one run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Timeline {
    pub method: TimelineMethod,
    /// The time between samples that the sampler asked for. A busy machine can make it longer.
    pub every_ms: u32,
    /// The run's steps, on the samples' clock. Sorted by time, then step.
    pub marks: Vec<TimelineMark>,
    /// Every process the sampler saw. Sorted by pid.
    pub processes: Vec<TimelineProcess>,
}

/// How a timeline's samples were read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TimelineMethod {
    /// Linux's `/proc/<pid>/status`: `private_bytes` is `RssAnon` plus `VmSwap`, the private
    /// memory footprint that Chromium reports on Linux.
    Proc,
    /// An Activity Monitor trace that xctrace exported, such as a device cloud's: `private_bytes`
    /// is the physical footprint that jetsam reads.
    Xctrace,
    /// The iPhone's sysmontap service, which `--device` reads: `private_bytes` is the physical
    /// footprint that jetsam reads.
    Sysmontap,
}

/// One step of the run, on the timeline's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TimelineMark {
    pub mark: RunMark,
    /// Since the navigation, on the driver's clock.
    pub t_ms: u64,
}

/// One process of the browser, and its memory through the run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TimelineProcess {
    /// The OS process id. It only tells processes apart within this run.
    pub pid: u32,
    pub role: ProcessRole,
    /// What the browser calls the process, such as "renderer", "gpu-process" or
    /// "utility:network.mojom.NetworkService", or its name, such as
    /// "com.apple.WebKit.WebContent".
    pub kind: String,
    /// Sorted by time.
    pub samples: Vec<MemorySample>,
}

/// The part a process plays in the run, as far as the driver can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRole {
    /// It runs the game frame.
    Game,
    /// It runs the harness page.
    Page,
    /// The GPU process.
    Gpu,
    /// The browser's main process.
    Browser,
    /// Any other process, such as a spare renderer or the network service.
    Other,
}

/// One process's memory at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MemorySample {
    /// Since the navigation, on the driver's clock.
    pub t_ms: u64,
    /// The memory that only this process holds, as the OS counts it for the process (see
    /// [`TimelineMethod`]).
    pub private_bytes: u64,
    /// Resident memory, shared pages included. Null when the method does not read it.
    pub resident_bytes: Option<u64>,
    /// Resident shared memory, such as the buffers the GPU process and the renderers share. Null
    /// when the method does not read it.
    pub shared_bytes: Option<u64>,
}

impl Timeline {
    /// Sorts every list: marks by time, then step; processes by pid; samples by time.
    pub fn sort_lists(&mut self) {
        self.marks.sort_by_key(|mark| (mark.t_ms, mark.mark));
        self.processes.sort_by_key(|process| process.pid);
        for process in &mut self.processes {
            process.samples.sort_by_key(|sample| sample.t_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t_ms: u64) -> MemorySample {
        MemorySample {
            t_ms,
            private_bytes: 1,
            resident_bytes: None,
            shared_bytes: None,
        }
    }

    #[test]
    fn sorting_orders_marks_processes_and_samples() {
        let mut timeline = Timeline {
            method: TimelineMethod::Proc,
            every_ms: 250,
            marks: vec![
                TimelineMark {
                    mark: RunMark::Navigated,
                    t_ms: 500,
                },
                TimelineMark {
                    mark: RunMark::Idle,
                    t_ms: 0,
                },
            ],
            processes: vec![
                TimelineProcess {
                    pid: 9,
                    role: ProcessRole::Game,
                    kind: "renderer".to_owned(),
                    samples: vec![sample(250), sample(0)],
                },
                TimelineProcess {
                    pid: 2,
                    role: ProcessRole::Browser,
                    kind: "browser".to_owned(),
                    samples: vec![],
                },
            ],
        };
        timeline.sort_lists();
        assert_eq!(timeline.marks[0].mark, RunMark::Idle);
        assert_eq!(timeline.processes[0].pid, 2);
        let times: Vec<u64> = timeline.processes[1]
            .samples
            .iter()
            .map(|sample| sample.t_ms)
            .collect();
        assert_eq!(times, [0, 250]);
    }

    #[test]
    fn the_method_and_role_are_snake_case() {
        assert_eq!(
            serde_json::to_string(&TimelineMethod::Xctrace).unwrap(),
            "\"xctrace\""
        );
        assert_eq!(serde_json::to_string(&ProcessRole::Gpu).unwrap(), "\"gpu\"");
    }
}
