//! `dyno compare`: the diff between two result files from `dyno check`.
//!
//! Compare reads result files, never build folders, so it never re-runs a check. A result file
//! that cannot be read, or has another schema version, is a usage error. A result whose hash
//! does not match its content makes the compare invalid. [`compare_files`] gives the compare and
//! writes nothing, so the MCP `compare` tool calls it too.

use std::fs;
use std::path::Path;

use dyno_schema::SCHEMA_VERSION;
use dyno_schema::compare::{CompareResult, CompareRun};
use dyno_schema::result::LoadedResult;
use dyno_schema::status::Status;
use serde::Deserialize;
use serde_json::Value;

use crate::check::load_budget;
use crate::cli::CompareArgs;
use crate::exit::CliError;
use crate::output;
use crate::run_info::{Clock, machine, tool_build};

/// The one field read before the rest, so a result from another dyno gets a clear error.
#[derive(Deserialize)]
struct VersionOnly {
    schema_version: u32,
}

/// Runs `dyno compare` and returns the status for the exit code.
pub fn run(args: CompareArgs) -> Result<Status, CliError> {
    let compare = compare_files(&args.base, &args.head, args.budget.as_deref())?;
    output::write_json(&compare, args.out.as_deref())?;
    output::print_summary(&output::compare_summary(&compare));
    Ok(compare.deterministic.status)
}

/// Compares two result files, with the budget file if one is given. It writes nothing.
pub fn compare_files(
    base: &Path,
    head: &Path,
    budget: Option<&Path>,
) -> Result<CompareResult, CliError> {
    let clock = Clock::start();
    let base = read_result(base)?;
    let head = read_result(head)?;
    let budget = load_budget(budget)?;
    let deterministic =
        dyno_score::compare::diff(&base, &head, budget.as_ref()).map_err(CliError::internal)?;
    let run = CompareRun {
        started_at: clock.started_at(),
        duration_ms: clock.elapsed_ms(),
        machine: machine(),
        tool_build: tool_build(),
        base_release: base.result.run.release,
        head_release: head.result.run.release,
    };
    CompareResult::new(deterministic, run).map_err(CliError::internal)
}

/// Reads a result file. Another `schema_version` is a usage error. The hash is checked on the
/// parsed JSON, before it becomes a typed result.
pub fn read_result(path: &Path) -> Result<LoadedResult, CliError> {
    let shown = path.display();
    let text = fs::read_to_string(path)
        .map_err(|error| CliError::usage(format!("cannot read result file {shown}: {error}")))?;
    let not_a_result = |error: serde_json::Error| {
        CliError::usage(format!("{shown} is not a dyno check result: {error}"))
    };
    let json: Value = serde_json::from_str(&text).map_err(not_a_result)?;
    let VersionOnly { schema_version } = VersionOnly::deserialize(&json).map_err(not_a_result)?;
    if schema_version != SCHEMA_VERSION {
        return Err(CliError::usage(format!(
            "{shown} has schema_version {schema_version}, but this dyno reads schema_version {SCHEMA_VERSION}"
        )));
    }
    LoadedResult::from_json(json).map_err(not_a_result)
}
