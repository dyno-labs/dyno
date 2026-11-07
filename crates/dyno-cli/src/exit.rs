//! Exit codes and the errors that set them.
//!
//! A command that gives a result exits with the code for its status. The status comes from the
//! deterministic part alone, so the same `result_hash` always gives the same exit code. A command
//! that stops before it has a result exits with the code for its error.

use std::fmt;

use dyno_schema::status::Status;

/// An error that stops a command before it has a result.
#[derive(Debug)]
pub enum CliError {
    /// Bad flags, or an input file that cannot be read or is not valid.
    Usage(String),
    /// A fault in dyno, or an output that cannot be written.
    Internal(String),
}

/// Pass, or warn.
pub const PASS: u8 = 0;
/// A budget or a static rule fails.
pub const FAIL: u8 = 1;
/// A required measure is missing or has no row, or a budget or static rule could not be checked.
pub const INCOMPLETE: u8 = 2;
/// A required measure failed its check, a measure row does not fit the check, or an input result
/// was edited.
pub const INVALID: u8 = 3;
/// A usage or config error: bad flags, or an unreadable target, budget, games or result file.
pub const USAGE: u8 = 64;
/// An internal or I/O error, such as an output file that cannot be written.
pub const INTERNAL: u8 = 70;

/// The exit code for a result's status.
pub fn for_status(status: Status) -> u8 {
    match status {
        Status::Pass | Status::Warn => PASS,
        Status::Fail => FAIL,
        Status::Incomplete => INCOMPLETE,
        Status::Invalid => INVALID,
    }
}

impl CliError {
    pub fn usage(problem: impl fmt::Display) -> CliError {
        CliError::Usage(problem.to_string())
    }

    pub fn internal(problem: impl fmt::Display) -> CliError {
        CliError::Internal(problem.to_string())
    }

    /// The same error, with `context` before the problem.
    pub fn with_context(self, context: &str) -> CliError {
        match self {
            CliError::Usage(problem) => CliError::Usage(format!("{context}: {problem}")),
            CliError::Internal(problem) => CliError::Internal(format!("{context}: {problem}")),
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::Usage(_) => USAGE,
            CliError::Internal(_) => INTERNAL,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(problem) | CliError::Internal(problem) => formatter.write_str(problem),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_status_has_its_exit_code() {
        let cases = [
            (Status::Pass, PASS),
            (Status::Warn, PASS),
            (Status::Fail, FAIL),
            (Status::Incomplete, INCOMPLETE),
            (Status::Invalid, INVALID),
        ];
        for (status, code) in cases {
            assert_eq!(for_status(status), code, "{status:?}");
        }
    }
}
