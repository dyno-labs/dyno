//! The dyno site: who a token belongs to, and result uploads.
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

/// An answer to `POST /api/results`.
#[derive(Deserialize)]
struct Uploaded {
    id: String,
}

/// The body of an answer that is not a success.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// HTTP 401: the token is not valid.
pub const UNAUTHORIZED: u16 = 401;

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
        let lower = base.to_ascii_lowercase();
        if !lower.starts_with("https://") && !lower.starts_with("http://") {
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

    /// Uploads one result, as its JSON bytes, and gives its id on the site.
    pub fn upload(&self, token: &str, json: &[u8]) -> Result<String, SiteError> {
        let response = self
            .agent
            .post(self.api("results"))
            .header("Authorization", bearer(token))
            .content_type("application/json")
            .send(json)
            .map_err(|error| self.unreachable(error))?;
        let uploaded: Uploaded = self.success(response)?;
        Ok(uploaded.id)
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

    #[test]
    fn another_base_loses_its_last_slash() {
        let site = Site::new(Some("http://localhost:8000/".to_owned())).unwrap();
        assert_eq!(site.api("results"), "http://localhost:8000/api/results");
    }
}
