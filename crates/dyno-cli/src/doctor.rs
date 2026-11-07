//! `dyno doctor` (Phase 2): whether this Mac can run the game on an iPhone.

use dyno_schema::status::Status;

use crate::cli::DoctorArgs;
use crate::exit::CliError;
use crate::output;

/// Runs `dyno doctor` and returns the status for the exit code. The stub reports its phase.
pub fn run(args: DoctorArgs) -> Result<Status, CliError> {
    let report = dyno_ios::doctor();
    output::write_json(&report, args.out.as_deref())?;
    output::print_summary(&output::doctor_summary(&report));
    Ok(report.status)
}
