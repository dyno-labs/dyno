//! `dyno upload` and `dyno check --upload`: send result files to the dyno site, each to a page of
//! its own, or to its game's report when the site links it to a game.
//!
//! The site gets each result's bytes as they are, so its `result_hash` still matches.

use std::fs;
use std::path::Path;

use crate::cli::UploadArgs;
use crate::credentials::{self, TOKEN_VAR};
use crate::exit::CliError;
use crate::site::{Site, SiteError, UNAUTHORIZED};

/// The site, and the token that uploads to it.
pub struct Uploader {
    site: Site,
    token: String,
}

/// Runs `dyno upload`. Each file is read before any upload starts, so a bad path uploads nothing.
/// A file the site does not take is an error line, and the upload goes on to the next file.
pub fn run(args: UploadArgs) -> Result<(), CliError> {
    let uploader = Uploader::signed_in()?;
    let files = args
        .results
        .iter()
        .map(|path| read_json(path).map(|json| (path, json)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut failed = 0;
    for (path, json) in &files {
        let shown = path.display();
        match uploader.upload(json) {
            Ok(page) => eprintln!("dyno upload: {shown}: {page}"),
            Err(error) => {
                eprintln!("dyno upload: {shown}: {error}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(CliError::internal(format!(
            "{failed} of {} results did not upload",
            files.len()
        )));
    }
    Ok(())
}

impl Uploader {
    /// The site with the token of `DYNO_TOKEN` or `dyno login`. With neither, it is a usage error.
    pub fn signed_in() -> Result<Uploader, CliError> {
        let site = Site::from_env()?;
        let Some(token) = credentials::token()? else {
            return Err(CliError::usage(format!(
                "sign in first: run dyno login, or set {TOKEN_VAR}"
            )));
        };
        Ok(Uploader { site, token })
    }

    /// Uploads one result and gives the link to print: the game's report when the site linked the
    /// run to a game, and the run's own page otherwise, with the game's name.
    pub fn upload(&self, json: &[u8]) -> Result<String, SiteError> {
        let uploaded = self.site.upload(&self.token, json).map_err(sign_in_again)?;
        let page = self.site.uploaded_page(&uploaded);
        Ok(match &uploaded.game {
            Some(game) => format!("{page} ({})", game.name),
            None => page,
        })
    }

    /// The site, and the token that talks to it, for `dyno link`.
    pub fn site_and_token(&self) -> (&Site, &str) {
        (&self.site, &self.token)
    }
}

/// A token the site does not take says to sign in again.
pub fn sign_in_again(error: SiteError) -> SiteError {
    match error {
        SiteError::Refused {
            status: UNAUTHORIZED,
            message,
        } => SiteError::Refused {
            status: UNAUTHORIZED,
            message: format!("{message} (run dyno login again)"),
        },
        error => error,
    }
}

/// A result file's bytes. A file that cannot be read or is not JSON is a usage error.
fn read_json(path: &Path) -> Result<Vec<u8>, CliError> {
    let shown = path.display();
    let bytes = fs::read(path)
        .map_err(|error| CliError::usage(format!("cannot read result file {shown}: {error}")))?;
    serde_json::from_slice::<serde::de::IgnoredAny>(&bytes)
        .map_err(|error| CliError::usage(format!("{shown} is not JSON: {error}")))?;
    Ok(bytes)
}
