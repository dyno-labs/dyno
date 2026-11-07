//! The parts of a result that differ between machines and runs: time, machine and tool build.

use std::time::{Instant, SystemTime};

use dyno_schema::TOOL_VERSION;
use dyno_schema::result::{Machine, ToolBuild};

/// When a command started, as a timestamp and as a clock for its duration.
pub struct Clock {
    /// Wall clock time, for the timestamp.
    wall_start: SystemTime,
    /// Monotonic time, for the duration. It never jumps when the wall clock is set.
    monotonic_start: Instant,
}

/// The commit a build comes from, read from `DYNO_GIT_SHA` at compile time. A release build sets
/// it. A local build leaves it unset, so the result records null.
const GIT_SHA: Option<&str> = option_env!("DYNO_GIT_SHA");

impl Clock {
    pub fn start() -> Clock {
        Clock {
            wall_start: SystemTime::now(),
            monotonic_start: Instant::now(),
        }
    }

    /// RFC 3339 UTC, to the millisecond.
    pub fn started_at(&self) -> String {
        humantime::format_rfc3339_millis(self.wall_start).to_string()
    }

    pub fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.monotonic_start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// This machine, without its host name or user name.
pub fn machine() -> Machine {
    // Some sandboxes hide the CPU count. The machine is a run detail, so 1 is a safe stand-in.
    let cpus = std::thread::available_parallelism().map_or(1, |count| count.get());
    Machine {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        cpus: u32::try_from(cpus).unwrap_or(u32::MAX),
    }
}

/// This dyno binary.
pub fn tool_build() -> ToolBuild {
    // Cargo turns debug assertions on in the dev profile and off in release, so they tell the two
    // builds apart.
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    ToolBuild {
        version: TOOL_VERSION.to_owned(),
        git_sha: GIT_SHA.map(str::to_owned),
        profile: profile.to_owned(),
    }
}
