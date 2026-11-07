//! Launch URLs: how dyno reads them, and how a result records them.
//!
//! A launch URL can carry a password in its user info and a session id in its query string or
//! fragment. The browser driver gets the full URL. A result records it without those parts.
//! dyno parses every launch URL by the WHATWG URL Standard, as a browser does, so it strips the
//! same user info the browser would see. Error messages never repeat the URL, and `scrub` gives
//! a URL in other text the same recorded form.

use std::fmt;

use url::{Position, Url};

use crate::scrub::scrub;

/// A parsed http or https launch URL.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchUrl(Url);

/// Why a launch URL was rejected.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum LaunchUrlError {
    #[error("the launch URL is not a valid URL: {0}")]
    NotAUrl(url::ParseError),
    #[error("the launch URL must start with http:// or https://")]
    NotHttp,
}

impl LaunchUrl {
    /// Parses a launch URL. Only http and https URLs are launch URLs.
    pub fn parse(text: &str) -> Result<LaunchUrl, LaunchUrlError> {
        let url = Url::parse(text).map_err(LaunchUrlError::NotAUrl)?;
        match url.scheme() {
            "http" | "https" => Ok(LaunchUrl(url)),
            _ => Err(LaunchUrlError::NotHttp),
        }
    }

    /// The full URL, user info and query string included. Only the browser driver gets it.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The URL as a result records it: the scheme, host, port and path. The user info, query
    /// string and fragment are gone, and a URL inside the path gets the same treatment.
    pub fn recorded(&self) -> String {
        recorded_form(&self.0)
    }
}

/// Shows the recorded form, so a debug print never shows a password or session id.
impl fmt::Debug for LaunchUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("LaunchUrl")
            .field(&self.recorded())
            .finish()
    }
}

/// Any URL as a result records it: the scheme, host, port and path. The path can hold another
/// URL, as a proxy's path can, so it is scrubbed too.
pub(crate) fn recorded_form(url: &Url) -> String {
    let host_and_port = &url[Position::BeforeHost..Position::BeforePath];
    let path = &url[Position::BeforePath..Position::AfterPath];
    let scheme = url.scheme();
    let path = scrub(path);
    format!("{scheme}://{host_and_port}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded(text: &str) -> String {
        LaunchUrl::parse(text).unwrap().recorded()
    }

    #[test]
    fn the_record_drops_the_user_info_query_and_fragment() {
        assert_eq!(recorded("https://u:p@h/g?x#y"), "https://h/g");
        assert_eq!(recorded("https://u@h/g"), "https://h/g");
    }

    #[test]
    fn the_record_keeps_the_host_port_and_path() {
        assert_eq!(
            recorded("https://cdn.example:8443/g/index.html?token=abc"),
            "https://cdn.example:8443/g/index.html"
        );
        assert_eq!(recorded("https://h/path@with-at"), "https://h/path@with-at");
    }

    #[test]
    fn a_url_in_the_path_is_recorded_the_same_way() {
        assert_eq!(
            recorded("https://proxy.example/fetch/https://u:pw@games.example/g?t=1"),
            "https://proxy.example/fetch/https://games.example/g"
        );
    }

    #[test]
    fn a_url_with_no_path_records_the_root_path() {
        assert_eq!(recorded("https://u:p@h"), "https://h/");
    }

    #[test]
    fn user_info_that_ends_the_authority_early_is_rejected() {
        // A browser reads "u" as the host and the rest as a bad port, so these never load.
        for text in [
            "https://u:abc/def@h/g",
            "https://u:p#ss@h/",
            "https://u:p?ss@h/",
        ] {
            assert!(
                matches!(LaunchUrl::parse(text), Err(LaunchUrlError::NotAUrl(_))),
                "{text}"
            );
        }
    }

    #[test]
    fn only_http_and_https_are_launch_urls() {
        assert_eq!(
            LaunchUrl::parse("ftp://h/g").unwrap_err(),
            LaunchUrlError::NotHttp
        );
        assert!(LaunchUrl::parse("http://h/g").is_ok());
    }

    #[test]
    fn debug_shows_only_the_recorded_url() {
        let url = LaunchUrl::parse("https://u:secret@h/g?session=secret").unwrap();
        assert_eq!(format!("{url:?}"), r#"LaunchUrl("https://h/g")"#);
        assert_eq!(url.as_str(), "https://u:secret@h/g?session=secret");
    }

    #[test]
    fn errors_never_repeat_the_url() {
        let error = LaunchUrl::parse("https://u:secret/x@h/").unwrap_err();
        assert!(!error.to_string().contains("secret"), "{error}");
    }
}
