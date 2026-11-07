//! Runs one load-and-idle run on a launch URL and prints each runner protocol message as a JSON
//! line. For trying the driver by hand:
//!
//! ```sh
//! cargo run -p dyno-cdp --example run -- 'http://127.0.0.1:5173/?mode=demo'
//! ```

use std::process::ExitCode;

use dyno_run::protocol::encode_line;
use dyno_schema::profile::{HostProfile, Scenario};
use dyno_schema::runner::DriverMessage;
use dyno_schema::runner::{RUNNER_PROTOCOL_VERSION, RunRequest};
use dyno_schema::scrub::scrub;

/// Shorter than `dyno check`'s, for trying the driver by hand.
const IDLE_AFTER_MS: u32 = 3_000;
const TIMEOUT_MS: u32 = 60_000;

fn main() -> ExitCode {
    let Some(launch_url) = std::env::args().nth(1) else {
        eprintln!("usage: run <launch-url>");
        return ExitCode::from(64);
    };
    let request = RunRequest {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        launch_url,
        host_profile: HostProfile::reels(),
        scenario: Scenario::Alone,
        spins: None,
        idle_after_ms: IDLE_AFTER_MS,
        timeout_ms: TIMEOUT_MS,
    };
    match dyno_cdp::run_cdp(&request) {
        Ok(messages) => {
            // A request URL can hold the launch URL's session token, so every line is scrubbed.
            for message in &messages {
                print!(
                    "{}",
                    scrub(&encode_line(message).expect("a message is JSON"))
                );
            }
            match messages.last() {
                Some(DriverMessage::Done { .. }) => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            }
        }
        Err(error) => {
            eprintln!("run: {error}");
            ExitCode::FAILURE
        }
    }
}
