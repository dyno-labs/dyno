//! The memory of every process of a browser, read from Linux's `/proc` while a run goes on, for
//! `run.timeline`. The CDP driver reads the Chromium it launches; the serve driver reads a browser
//! that `--open-with` starts.
//!
//! A thread reads every process under the browser 4 times a second, on its own clock, so a busy
//! event loop never holds a sample back. `private_bytes` is `RssAnon` plus `VmSwap`: the private
//! memory footprint that Chromium's task manager reports on Linux. On another OS the sampler
//! reads nothing, and the run has no timeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dyno_schema::runner::{DriverMessage, ProcessMemory};
use dyno_schema::timeline::TimelineMethod;

/// The time between samples.
pub const EVERY_MS: u32 = 250;

/// The kind Chromium gives its GPU process.
pub const GPU_KIND: &str = "gpu-process";

/// Every process under the browser at one moment.
pub struct Reading {
    /// When the reading started.
    pub at: Instant,
    pub processes: Vec<ProcessMemory>,
}

/// A thread that reads the memory of every process under one process until it is stopped.
pub struct Sampler {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Vec<Reading>>>,
}

impl Sampler {
    /// Starts reading every process under `root`, the browser's process, once now and then every
    /// [`EVERY_MS`]. The thread stops by itself when `root` is gone.
    pub fn start(root: u32) -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let every = Duration::from_millis(u64::from(EVERY_MS));
            let mut readings = Vec::new();
            let mut next = Instant::now();
            while !stopped.load(Ordering::Relaxed) {
                let at = Instant::now();
                let processes = read_tree(root);
                if processes.is_empty() {
                    break;
                }
                readings.push(Reading { at, processes });
                // A reading that took longer than the gap is not caught up with a burst.
                next = (next + every).max(Instant::now());
                std::thread::park_timeout(next.saturating_duration_since(Instant::now()));
            }
            readings
        });
        Sampler {
            stop,
            thread: Some(thread),
        }
    }

    /// Stops the thread and gives back every reading, oldest first.
    pub fn stop(mut self) -> Vec<Reading> {
        self.stop.store(true, Ordering::Relaxed);
        let Some(thread) = self.thread.take() else {
            return Vec::new();
        };
        thread.thread().unpark();
        thread.join().unwrap_or_default()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

/// The readings taken since `start`, the run's navigation, as `memory` messages on the run's
/// clock. A reading from before it is left out.
pub fn memory_messages(readings: Vec<Reading>, start: Instant) -> Vec<DriverMessage> {
    readings
        .into_iter()
        .filter_map(|reading| {
            let since = reading.at.checked_duration_since(start)?;
            Some(DriverMessage::Memory {
                method: TimelineMethod::Proc,
                every_ms: EVERY_MS,
                processes: reading.processes,
                t_ms: since.as_secs_f64() * 1_000.0,
            })
        })
        .collect()
}

/// Every process that any reading gives this kind, each once, in the order first seen.
pub fn pids_of_kind(readings: &[Reading], kind: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for process in readings.iter().flat_map(|reading| &reading.processes) {
        if process.kind == kind && !pids.contains(&process.pid) {
            pids.push(process.pid);
        }
    }
    pids
}

/// The memory of `root` and every process under it. Empty when `root` is gone, and on an OS
/// without `/proc`.
#[cfg(target_os = "linux")]
pub fn read_tree(root: u32) -> Vec<ProcessMemory> {
    tree(root).into_iter().filter_map(read_process).collect()
}

#[cfg(not(target_os = "linux"))]
pub fn read_tree(_root: u32) -> Vec<ProcessMemory> {
    Vec::new()
}

/// `root` and every process under it, `root` first. Empty when `root` is gone.
#[cfg(target_os = "linux")]
fn tree(root: u32) -> Vec<u32> {
    use std::collections::HashMap;

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut root_seen = false;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        // A process can exit between the listing and the read.
        let Some(parent) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| parent_of(&stat))
        else {
            continue;
        };
        root_seen |= pid == root;
        children.entry(parent).or_default().push(pid);
    }
    if !root_seen {
        return Vec::new();
    }
    let mut pids = vec![root];
    let mut next = 0;
    while let Some(&pid) = pids.get(next) {
        pids.extend(children.get(&pid).into_iter().flatten());
        next += 1;
    }
    pids
}

/// One process's memory and kind, or None when it is gone or holds no memory, as a process
/// that is exiting does.
#[cfg(target_os = "linux")]
fn read_process(pid: u32) -> Option<ProcessMemory> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let memory = status_memory(&status)?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    Some(ProcessMemory {
        pid,
        kind: kind_of(&String::from_utf8_lossy(&cmdline)),
        ..memory
    })
}

/// The parent's pid from `/proc/<pid>/stat`. The name in parentheses can hold spaces and
/// parentheses, so the fields are read after the last ')'.
fn parent_of(stat: &str) -> Option<u32> {
    let (_, after_name) = stat.rsplit_once(')')?;
    after_name.split_whitespace().nth(1)?.parse().ok()
}

/// The memory fields of `/proc/<pid>/status`, in bytes, with no pid or kind. None when the
/// process has no `RssAnon`, as a process that is exiting does.
fn status_memory(status: &str) -> Option<ProcessMemory> {
    let field = |name: &str| {
        status.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix(':')?;
            let kilobytes: u64 = value.trim().strip_suffix("kB")?.trim().parse().ok()?;
            Some(kilobytes * 1024)
        })
    };
    let anonymous = field("RssAnon")?;
    Some(ProcessMemory {
        pid: 0,
        kind: String::new(),
        private_bytes: anonymous + field("VmSwap").unwrap_or(0),
        resident_bytes: field("VmRSS"),
        shared_bytes: field("RssShmem"),
    })
}

/// What a process is, from its command line: for Chromium's children, the value of `--type`, with
/// the service's name for a utility process; for any other process, its program, such as
/// "chromium", "WebKitWebProcess" or "chrome_crashpad_handler". Chromium rewrites a child's
/// command line as one string with spaces, so both spaces and NULs split it.
fn kind_of(cmdline: &str) -> String {
    let args: Vec<&str> = cmdline
        .split(['\0', ' '])
        .filter(|arg| !arg.is_empty())
        .collect();
    let flag = |name: &str| args.iter().find_map(|arg| arg.strip_prefix(name));
    match flag("--type=") {
        Some("utility") => match flag("--utility-sub-type=") {
            Some(service) => format!("utility:{service}"),
            None => "utility".to_owned(),
        },
        Some(kind) => kind.to_owned(),
        None => args
            .first()
            .and_then(|program| program.rsplit('/').next())
            .unwrap_or("unknown")
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "Name:\tchromium\nVmPeak:\t 1000 kB\nVmRSS:\t   300 kB\nRssAnon:\t  200 kB\nRssFile:\t   90 kB\nRssShmem:\t   10 kB\nVmSwap:\t    5 kB\n";

    #[test]
    fn the_private_footprint_is_anonymous_memory_plus_swap() {
        let memory = status_memory(STATUS).unwrap();
        assert_eq!(memory.private_bytes, 205 * 1024);
        assert_eq!(memory.resident_bytes, Some(300 * 1024));
        assert_eq!(memory.shared_bytes, Some(10 * 1024));
    }

    #[test]
    fn a_process_with_no_anonymous_memory_has_no_reading() {
        assert_eq!(
            status_memory("Name:\tkthreadd\nState:\tS (sleeping)\n"),
            None
        );
    }

    #[test]
    fn the_parent_is_read_after_the_name_even_with_parentheses_in_it() {
        assert_eq!(parent_of("42 (a (b) c) S 7 42 42 0 -1"), Some(7));
        assert_eq!(parent_of("garbage"), None);
    }

    #[test]
    fn the_kind_is_the_type_flag_with_a_utility_service() {
        let renderer = "/usr/lib/chromium/chromium --type=renderer --renderer-client-id=10";
        assert_eq!(kind_of(renderer), "renderer");
        let network = "/usr/lib/chromium/chromium\0--type=utility\0--utility-sub-type=network.mojom.NetworkService\0";
        assert_eq!(kind_of(network), "utility:network.mojom.NetworkService");
        assert_eq!(kind_of("chromium --type=gpu-process"), GPU_KIND);
        assert_eq!(kind_of("chromium --type=utility"), "utility");
    }

    #[test]
    fn a_process_with_no_type_is_named_by_its_program() {
        assert_eq!(
            kind_of("/usr/lib/chromium/chromium\0--headless=new\0"),
            "chromium"
        );
        assert_eq!(
            kind_of("/usr/lib/webkit2gtk-4.1/WebKitWebProcess\0--x\0"),
            "WebKitWebProcess"
        );
        let crashpad = "/usr/lib/chromium/chrome_crashpad_handler\0--monitor-self\0";
        assert_eq!(kind_of(crashpad), "chrome_crashpad_handler");
        assert_eq!(kind_of(""), "unknown");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_sampler_reads_this_process_and_its_children() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let sampler = Sampler::start(std::process::id());
        std::thread::sleep(Duration::from_millis(u64::from(EVERY_MS) + 100));
        let readings = sampler.stop();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(readings.len() >= 2, "{} readings", readings.len());
        let first = &readings[0].processes;
        assert_eq!(first[0].pid, std::process::id());
        assert!(!first[0].kind.is_empty());
        assert!(first[0].private_bytes > 0);
        assert!(first.iter().any(|process| process.pid == child.id()));
    }

    fn reading(at: Instant, processes: &[(u32, &str)]) -> Reading {
        Reading {
            at,
            processes: processes
                .iter()
                .map(|&(pid, kind)| ProcessMemory {
                    pid,
                    kind: kind.to_owned(),
                    private_bytes: 1,
                    resident_bytes: None,
                    shared_bytes: None,
                })
                .collect(),
        }
    }

    #[test]
    fn readings_since_the_start_become_memory_messages_on_the_runs_clock() {
        let start = Instant::now();
        let readings = vec![
            reading(start - Duration::from_millis(10), &[(1, "chromium")]),
            reading(start + Duration::from_millis(250), &[(1, "chromium")]),
        ];
        let messages = memory_messages(readings, start);
        assert_eq!(messages.len(), 1);
        let DriverMessage::Memory { t_ms, every_ms, .. } = &messages[0] else {
            panic!("{messages:?}");
        };
        assert_eq!(*t_ms, 250.0);
        assert_eq!(*every_ms, EVERY_MS);
    }

    #[test]
    fn the_pids_of_a_kind_are_each_given_once() {
        let at = Instant::now();
        let readings = [
            reading(at, &[(1, "chromium"), (5, GPU_KIND)]),
            reading(at, &[(5, GPU_KIND), (6, GPU_KIND)]),
        ];
        assert_eq!(pids_of_kind(&readings, GPU_KIND), [5, 6]);
    }

    #[test]
    fn a_sampler_of_a_process_that_is_gone_stops_by_itself() {
        let readings = Sampler::start(u32::MAX).stop();
        assert!(readings.is_empty());
    }
}
