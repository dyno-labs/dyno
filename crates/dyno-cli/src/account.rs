//! `dyno login` and `dyno logout`: save and remove the token that uploads results to the dyno
//! site.

use std::io::{self, BufRead};

use crate::cli::LoginArgs;
use crate::credentials::{self, TOKEN_VAR};
use crate::exit::CliError;
use crate::site::Site;

/// Runs `dyno login`: asks the site who the token belongs to, then saves it.
pub fn login(args: LoginArgs) -> Result<(), CliError> {
    let site = Site::from_env()?;
    let path = credentials::path()?;
    let token = match args.token {
        Some(token) => token,
        None => {
            eprintln!(
                "Paste a token from {}, then press Enter.",
                site.account_page()
            );
            read_line()?
        }
    };
    let token = token.trim();
    if token.is_empty() {
        return Err(CliError::usage(format!(
            "no token given: make one at {}",
            site.account_page()
        )));
    }
    if !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(CliError::usage(format!(
            "that is not a token from {}",
            site.account_page()
        )));
    }
    let email = site.me(token).map_err(|error| error.into_cli_error())?;
    credentials::save(&path, token)?;
    eprintln!("dyno login: signed in as {email}.");
    if credentials::env_token().is_some() {
        eprintln!("dyno login: {TOKEN_VAR} is set, so dyno uses it, not the saved token.");
    }
    Ok(())
}

/// Runs `dyno logout`: removes the saved token, if there is one.
pub fn logout() -> Result<(), CliError> {
    let path = credentials::path()?;
    if credentials::remove(&path)? {
        eprintln!("dyno logout: removed the saved token.");
    } else {
        eprintln!("dyno logout: there was no saved token.");
    }
    if credentials::env_token().is_some() {
        eprintln!("dyno logout: {TOKEN_VAR} is still set, so dyno still uploads with it.");
    }
    Ok(())
}

/// One line from stdin, without its line ending.
fn read_line() -> Result<String, CliError> {
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| CliError::internal(format!("cannot read stdin: {error}")))?;
    Ok(line)
}
