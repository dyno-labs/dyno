//! The dyno site: who a token belongs to, result uploads, and games.
//!
//! Every request goes to `{base}/api/...` with the token as a bearer. The base is `DYNO_API_URL`,
//! or else the site itself. An answer that is not a success has an `error` text, which dyno shows
//! as it is.

use std::env;
use std::time::Duration;

use dyno_schema::TOOL_VERSION;
use serde::Deserialize;
use ureq::Agent;
use ureq::http::Response;

use crate::exit::CliError;

/// The site, when `DYNO_API_URL` does not name another.
const DEFAULT_BASE_URL: &str = "https://dynoapp.io";

/// The environment variable that names another site, such as a local one.
const BASE_URL_VAR: &str = "DYNO_API_URL";

/// How long a connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long one request may take, from the connection to the last byte of the answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The site, and the client that talks to it.
pub struct Site {
    /// The base URL, with no `/` at the end.
    base: String,
    agent: Agent,
}

/// Why a request to the site did not succeed.
#[derive(Debug)]
pub enum SiteError {
    /// The site answered with an error: its HTTP status and its `error` text.
    Refused { status: u16, message: String },
    /// The site could not be reached, or its answer could not be read.
    Unreachable(String),
}

/// An answer to `GET /api/me`.
#[derive(Deserialize)]
struct Me {
    email: String,
}

/// An answer to `POST /api/results`. A site from before games gives only the id.
#[derive(Deserialize)]
pub struct Uploaded {
    pub id: String,
    /// The game the site linked the run to. Null when it linked none.
    #[serde(default)]
    pub game: Option<Game>,
    /// The link to print: the game's report when the site linked the run, else the run's page.
    #[serde(default)]
    pub url: Option<String>,
}

/// A game on the site, as `POST /api/games` and `GET /api/games/:id` give it. The answers hold
/// more fields, which dyno does not need.
#[derive(Clone, Debug, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
}

/// An answer that holds one game.
#[derive(Deserialize)]
struct GameAnswer {
    game: Game,
}

/// The body of an answer that is not a success.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// HTTP 401: the token is not valid.
pub const UNAUTHORIZED: u16 = 401;

/// HTTP 404: no such page, or no such game.
pub const NOT_FOUND: u16 = 404;

impl Site {
    /// The site that `DYNO_API_URL` names, or else the dyno site. A value that is not an http or
    /// https URL is a usage error.
    pub fn from_env() -> Result<Site, CliError> {
        Site::new(env::var(BASE_URL_VAR).ok())
    }

    /// The site at `base`, or the dyno site when it is None or empty.
    fn new(base: Option<String>) -> Result<Site, CliError> {
        let base = base
            .filter(|base| !base.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        if !is_http_url(&base) {
            return Err(CliError::usage(format!(
                "{BASE_URL_VAR} {base:?} must be an http or https URL"
            )));
        }
        let agent = Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .user_agent(format!("dyno/{TOOL_VERSION}"))
            .build()
            .into();
        Ok(Site {
            base: base.trim_end_matches('/').to_owned(),
            agent,
        })
    }

    /// The site's Account page, where people make tokens.
    pub fn account_page(&self) -> String {
        format!("{}/#/account", self.base)
    }

    /// The page of an uploaded result.
    pub fn check_page(&self, id: &str) -> String {
        format!("{}/#/checks/{id}", self.base)
    }

    /// The email of the person the token belongs to.
    pub fn me(&self, token: &str) -> Result<String, SiteError> {
        let response = self
            .agent
            .get(self.api("me"))
            .header("Authorization", bearer(token))
            .call()
            .map_err(|error| self.unreachable(error))?;
        let me: Me = self.success(response)?;
        Ok(me.email)
    }

    /// Uploads one result, as its JSON bytes, and gives the site's answer.
    pub fn upload(&self, token: &str, json: &[u8]) -> Result<Uploaded, SiteError> {
        let response = self
            .agent
            .post(self.api("results"))
            .header("Authorization", bearer(token))
            .content_type("application/json")
            .send(json)
            .map_err(|error| self.unreachable(error))?;
        self.success(response)
    }

    /// The link to print for an uploaded result: the site's own `url` when it gives one, and the
    /// result's page otherwise. A `url` that starts with `/` is on this site.
    pub fn uploaded_page(&self, uploaded: &Uploaded) -> String {
        match uploaded.url.as_deref().map(str::trim) {
            Some(url) if url.starts_with('/') => format!("{}{url}", self.base),
            Some(url) if is_http_url(url) => url.to_owned(),
            _ => self.check_page(&uploaded.id),
        }
    }

    /// The org's game with this name, made if it has none.
    pub fn create_game(&self, token: &str, name: &str) -> Result<Game, SiteError> {
        let body = serde_json::json!({ "name": name }).to_string();
        let response = self
            .agent
            .post(self.api("games"))
            .header("Authorization", bearer(token))
            .content_type("application/json")
            .send(body.as_bytes())
            .map_err(|error| self.unreachable(error))?;
        let answer: GameAnswer = self.success(response)?;
        Ok(answer.game)
    }

    /// The game with this id, which must be a UUID.
    pub fn game(&self, token: &str, id: &str) -> Result<Game, SiteError> {
        let response = self
            .agent
            .get(self.api(&format!("games/{id}")))
            .header("Authorization", bearer(token))
            .call()
            .map_err(|error| self.unreachable(error))?;
        let answer: GameAnswer = self.success(response)?;
        Ok(answer.game)
    }

    fn api(&self, path: &str) -> String {
        format!("{}/api/{path}", self.base)
    }

    fn unreachable(&self, error: ureq::Error) -> SiteError {
        SiteError::Unreachable(format!("cannot reach {}: {error}", self.base))
    }

    /// Reads a 2xx answer's body as `T`, or else the answer's error.
    fn success<T: for<'de> Deserialize<'de>>(
        &self,
        mut response: Response<ureq::Body>,
    ) -> Result<T, SiteError> {
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| self.unreachable(error))?;
        if !response.status().is_success() {
            let message = match serde_json::from_str::<ErrorBody>(&body) {
                Ok(ErrorBody { error }) => error,
                Err(_) => format!("the site answered HTTP {status}"),
            };
            return Err(SiteError::Refused { status, message });
        }
        serde_json::from_str(&body).map_err(|error| {
            SiteError::Unreachable(format!(
                "{} gave an answer dyno cannot read: {error}",
                self.base
            ))
        })
    }
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Whether the text starts with `http://` or `https://`, in any case.
fn is_http_url(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

impl SiteError {
    /// The error as a command's error: a bad token is a usage error, and anything else an I/O
    /// error.
    pub fn into_cli_error(self) -> CliError {
        match self {
            SiteError::Refused {
                status: UNAUTHORIZED,
                message,
            } => CliError::usage(message),
            SiteError::Refused { message, .. } | SiteError::Unreachable(message) => {
                CliError::internal(message)
            }
        }
    }
}

impl std::fmt::Display for SiteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SiteError::Refused { message, .. } | SiteError::Unreachable(message) => {
                formatter.write_str(message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dyno_site_is_the_default() {
        for base in [None, Some(String::new())] {
            let site = Site::new(base).unwrap();
            assert_eq!(site.account_page(), "https://dynoapp.io/#/account");
            assert_eq!(site.check_page("abc"), "https://dynoapp.io/#/checks/abc");
            assert_eq!(site.api("me"), "https://dynoapp.io/api/me");
        }
    }

    fn uploaded(url: Option<&str>) -> Uploaded {
        Uploaded {
            id: "abc".to_owned(),
            game: None,
            url: url.map(str::to_owned),
        }
    }

    #[test]
    fn the_site_s_url_is_the_page_of_an_upload_and_the_check_page_is_the_fallback() {
        let site = Site::new(Some("http://localhost:8000".to_owned())).unwrap();
        let cases = [
            (
                Some("https://dynoapp.io/#/games/g1"),
                "https://dynoapp.io/#/games/g1",
            ),
            (Some("/#/runs/r1"), "http://localhost:8000/#/runs/r1"),
            (None, "http://localhost:8000/#/checks/abc"),
            (Some(""), "http://localhost:8000/#/checks/abc"),
            (
                Some("javascript:alert(1)"),
                "http://localhost:8000/#/checks/abc",
            ),
        ];
        for (url, page) in cases {
            assert_eq!(site.uploaded_page(&uploaded(url)), page, "{url:?}");
        }
    }

    #[test]
    fn another_base_loses_its_last_slash() {
        let site = Site::new(Some("http://localhost:8000/".to_owned())).unwrap();
        assert_eq!(site.api("results"), "http://localhost:8000/api/results");
    }
}
