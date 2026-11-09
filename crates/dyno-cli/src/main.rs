//! `dyno`: predicts how much memory an HTML5 game in an iframe will use on an iPhone, from its
//! build.

use std::process::ExitCode;

use clap::Parser;
use clap::error::ErrorKind;
use dyno_schema::status::Status;

mod account;
mod banner;
mod baseline;
mod check;
mod cli;
mod compare;
mod credentials;
mod device;
mod doctor;
mod exit;
mod link;
mod mcp;
mod output;
mod run_info;
mod site;
mod upload;

use cli::{Cli, Command};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // Help and version requests are not errors.
            let code = if error.use_stderr() {
                ExitCode::from(exit::USAGE)
            } else {
                ExitCode::SUCCESS
            };
            if error.kind() == ErrorKind::DisplayVersion {
                banner::print();
            }
            let _ = error.print();
            return code;
        }
    };
    let outcome = match cli.command {
        Command::Check(args) => check::run(*args).map(exit_code_for),
        Command::Compare(args) => compare::run(args).map(exit_code_for),
        Command::Doctor(args) => doctor::run(args).map(exit_code_for),
        Command::Mcp => mcp::run().map(|()| ExitCode::SUCCESS),
        Command::Login(args) => account::login(args).map(|()| ExitCode::SUCCESS),
        Command::Logout => account::logout().map(|()| ExitCode::SUCCESS),
        Command::Upload(args) => upload::run(args).map(|()| ExitCode::SUCCESS),
        Command::Link(args) => link::run(args).map(|()| ExitCode::SUCCESS),
    };
    match outcome {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dyno: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

fn exit_code_for(status: Status) -> ExitCode {
    ExitCode::from(exit::for_status(status))
}
