//! External URLs: what the scripts, HTML, CSS and JSON point to outside the build (design
//! section 8.6).
//!
//! A value counts only when the whole value is a URL, so text inside a longer string never
//! does. A URL is written as a browser reads it: the scheme and host in lowercase, the port as
//! a number unless it is the scheme's default, and the path with `\` read as `/`. User info, the
//! query and the fragment go, because they can hold tokens.

use std::collections::{BTreeMap, BTreeSet};

use dyno_schema::path::RelPath;
use dyno_schema::scrub::scrub;

/// One external URL, as the result lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalUrl {
    /// `scheme://` or `//`, then the host, port and path.
    pub url: String,
    /// The host in lowercase, without the port.
    pub host: String,
}

/// What the text before the first expression of a template or a concatenation says about a
/// URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateStart {
    /// The host is whole, so the start is recorded.
    Url(ExternalUrl),
    /// A scheme with no host, part of one, or a host with no `/` after it yet: the host may be
    /// built at run time.
    DynamicHost,
    NotAUrl,
}

/// Every external URL in a build, with the files that hold it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalUrls {
    /// Lowercase, sorted, without repeats.
    pub hosts: Vec<String>,
    /// Sorted by URL.
    pub urls: Vec<UrlSources>,
    /// Templates and concatenations that build a host at run time.
    pub dynamic_host_sites: u32,
}

/// One URL and the sorted files that hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlSources {
    pub url: String,
    pub sources: Vec<RelPath>,
}

/// Collects URLs from every file, then sorts them into `ExternalUrls`.
#[derive(Debug, Default)]
pub struct UrlBook {
    urls: BTreeMap<String, BookEntry>,
    dynamic_host_sites: u32,
}

/// One URL's host and the files that hold it.
#[derive(Debug)]
struct BookEntry {
    host: String,
    sources: BTreeSet<RelPath>,
}

/// The schemes of a URL that leaves the build. Anything else (`data:`, `blob:`, relative paths)
/// stays inside it.
const EXTERNAL_SCHEMES: &[&str] = &["http", "https", "ws", "wss"];
/// The port each external scheme uses when a URL names none.
const DEFAULT_PORTS: &[(&str, u16)] = &[("http", 80), ("https", 443), ("ws", 80), ("wss", 443)];

/// W3C namespace URIs. Libraries pass them to DOM calls; nothing fetches them.
const NAMESPACE_URIS: &[&str] = &[
    "http://www.w3.org/2000/svg",
    "http://www.w3.org/1999/xhtml",
    "http://www.w3.org/1999/xlink",
    "http://www.w3.org/XML/1998/namespace",
    "http://www.w3.org/2000/xmlns/",
    "http://www.w3.org/1998/Math/MathML",
];

const PROTOCOL_RELATIVE: &str = "//";

const SCHEME_END: char = ':';
const SCHEME_SEPARATOR: &str = "://";

/// After an external scheme, browsers read `\` as `/` and skip any run of slashes.
const SLASHES: [char; 2] = ['/', '\\'];
/// The fewest slashes after the scheme for a value to count, as in `https://`.
const MIN_SLASHES: usize = 2;

/// What ends the host and port.
const AUTHORITY_END: [char; 4] = ['/', '\\', '?', '#'];
const PATH_END: [char; 2] = ['?', '#'];
const BACKSLASH: char = '\\';
const PATH_SEPARATOR: &str = "/";
const USER_INFO_END: char = '@';
const PORT_START: char = ':';
const IPV6_START: char = '[';
const IPV6_END: char = ']';
const IPV6_GROUP_SEPARATOR: char = ':';
const LABEL_SEPARATOR: char = '.';
/// The characters a host name holds besides letters, digits and dots.
const HOST_NAME_PUNCTUATION: [char; 2] = ['-', '_'];

impl UrlBook {
    pub fn add(&mut self, url: ExternalUrl, source: &RelPath) {
        let entry = self.urls.entry(url.url).or_insert_with(|| BookEntry {
            host: url.host,
            sources: BTreeSet::new(),
        });
        entry.sources.insert(source.clone());
    }

    pub fn add_dynamic_host_sites(&mut self, count: u32) {
        self.dynamic_host_sites = self.dynamic_host_sites.saturating_add(count);
    }

    pub fn finish(self) -> ExternalUrls {
        let hosts: BTreeSet<String> = self.urls.values().map(|entry| entry.host.clone()).collect();
        let urls = self
            .urls
            .into_iter()
            .map(|(url, entry)| UrlSources {
                url,
                sources: entry.sources.into_iter().collect(),
            })
            .collect();
        ExternalUrls {
            hosts: hosts.into_iter().collect(),
            urls,
            dynamic_host_sites: self.dynamic_host_sites,
        }
    }
}

/// The URL that `value` is, when the whole value is an external URL.
pub fn whole_url(value: &str) -> Option<ExternalUrl> {
    if value.chars().any(char::is_whitespace) {
        return None;
    }
    let (scheme, after_scheme) = split_scheme(value)?;
    let authority_end = after_scheme
        .find(AUTHORITY_END)
        .unwrap_or(after_scheme.len());
    let (authority, rest) = after_scheme.split_at(authority_end);
    let host_and_port = authority
        .rsplit_once(USER_INFO_END)
        .map_or(authority, |(_, host_and_port)| host_and_port);
    let (host, port) = split_host(host_and_port)?;
    let path = rest[..rest.find(PATH_END).unwrap_or(rest.len())].replace(BACKSLASH, PATH_SEPARATOR);
    // A proxy's path can hold another URL with its own password.
    let path = scrub(&path);
    let host = host.to_lowercase();
    let scheme = scheme.map(str::to_ascii_lowercase);
    let port = port.filter(|port| default_port(scheme.as_deref()) != Some(*port));
    let start = match scheme {
        Some(scheme) => format!("{scheme}{SCHEME_SEPARATOR}"),
        None => PROTOCOL_RELATIVE.to_owned(),
    };
    let port = port.map_or_else(String::new, |port| format!("{PORT_START}{port}"));
    let url = format!("{start}{host}{port}{path}");
    if NAMESPACE_URIS.contains(&url.as_str()) {
        return None;
    }
    Some(ExternalUrl { url, host })
}

/// What the text before the first expression of a template or a concatenation says.
pub fn template_start(first_part: &str) -> TemplateStart {
    // `//` followed by an expression appears in shader code, so only schemes count here.
    if first_part.starts_with(PROTOCOL_RELATIVE) {
        return TemplateStart::NotAUrl;
    }
    let Some((_, after_scheme)) = split_scheme(first_part) else {
        return TemplateStart::NotAUrl;
    };
    let host_is_whole = after_scheme.contains(AUTHORITY_END);
    if !host_is_whole {
        return TemplateStart::DynamicHost;
    }
    match whole_url(first_part) {
        Some(url) => TemplateStart::Url(url),
        None => TemplateStart::NotAUrl,
    }
}

/// The scheme and the text after its slashes, or None and the text after `//`, when the value
/// starts like a URL that leaves the build.
fn split_scheme(value: &str) -> Option<(Option<&str>, &str)> {
    if let Some(rest) = value.strip_prefix(PROTOCOL_RELATIVE) {
        return Some((None, rest));
    }
    let (scheme, rest) = value.split_once(SCHEME_END)?;
    let known = EXTERNAL_SCHEMES
        .iter()
        .any(|known| scheme.eq_ignore_ascii_case(known));
    let after_slashes = rest.trim_start_matches(SLASHES);
    let slashes = rest.len() - after_slashes.len();
    (known && slashes >= MIN_SLASHES).then_some((Some(scheme), after_slashes))
}

/// `host[:port]` split into the host and the port number, when the host is a name with a dot or
/// an IPv6 literal, and the port is one a browser takes. An empty port is None.
fn split_host(host_and_port: &str) -> Option<(&str, Option<u16>)> {
    let is_ipv6 = host_and_port.starts_with(IPV6_START);
    let host_end = if is_ipv6 {
        host_and_port.find(IPV6_END)? + IPV6_END.len_utf8()
    } else {
        host_and_port
            .find(PORT_START)
            .unwrap_or(host_and_port.len())
    };
    let (host, after_host) = host_and_port.split_at(host_end);
    let port_digits = match after_host.strip_prefix(PORT_START) {
        Some(digits) => digits,
        None if after_host.is_empty() => after_host,
        None => return None,
    };
    let host_is_valid = if is_ipv6 {
        is_ipv6_literal(host)
    } else {
        is_host_name(host)
    };
    if !host_is_valid {
        return None;
    }
    let port = if port_digits.is_empty() {
        None
    } else {
        Some(port_number(port_digits)?)
    };
    Some((host, port))
}

/// A name such as `cdn.example.com` or `127.0.0.1`: letters, digits, `-`, `_` and dots, with at
/// least one dot between two other characters. Non-ASCII letters are allowed for IDN hosts.
fn is_host_name(host: &str) -> bool {
    let allowed = host
        .chars()
        .all(|c| c.is_alphanumeric() || c == LABEL_SEPARATOR || HOST_NAME_PUNCTUATION.contains(&c));
    let has_inner_dot = host.trim_matches(LABEL_SEPARATOR).contains(LABEL_SEPARATOR);
    allowed && has_inner_dot
}

/// `[` and `]` around hex digits and colons, with a dotted IPv4 tail allowed.
fn is_ipv6_literal(host: &str) -> bool {
    let Some(inner) = host
        .strip_prefix(IPV6_START)
        .and_then(|rest| rest.strip_suffix(IPV6_END))
    else {
        return false;
    };
    let allowed = inner
        .chars()
        .all(|c| c.is_ascii_hexdigit() || c == IPV6_GROUP_SEPARATOR || c == LABEL_SEPARATOR);
    allowed && inner.contains(IPV6_GROUP_SEPARATOR)
}

/// The number that ASCII digits spell, when it is at most 65535. Leading zeros do not count.
fn port_number(digits: &str) -> Option<u16> {
    // `parse` alone would also take a leading `+`.
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The port a browser uses for `scheme` when the URL names none. A protocol-relative URL takes
/// the page's scheme, so it has none.
fn default_port(scheme: Option<&str>) -> Option<u16> {
    let scheme = scheme?;
    DEFAULT_PORTS
        .iter()
        .find(|(name, _)| *name == scheme)
        .map(|(_, port)| *port)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Option<String> {
        whole_url(value).map(|found| found.url)
    }

    #[test]
    fn an_absolute_url_keeps_its_scheme_host_and_path() {
        assert_eq!(
            url("https://cdn.example.invalid/brand/logo.png"),
            Some("https://cdn.example.invalid/brand/logo.png".into())
        );
        assert_eq!(
            url("wss://live.example.invalid/feed"),
            Some("wss://live.example.invalid/feed".into())
        );
        assert_eq!(
            url("https://fonts.gstatic.com"),
            Some("https://fonts.gstatic.com".into())
        );
    }

    #[test]
    fn the_query_and_fragment_are_dropped() {
        assert_eq!(
            url("https://api.example.invalid/v1/config?session=abc#top"),
            Some("https://api.example.invalid/v1/config".into())
        );
        assert_eq!(
            url("https://example.invalid?token=abc"),
            Some("https://example.invalid".into())
        );
    }

    #[test]
    fn user_info_is_dropped_and_the_port_kept() {
        assert_eq!(
            url("https://user:secret@example.invalid:8443/a"),
            Some("https://example.invalid:8443/a".into())
        );
    }

    #[test]
    fn a_url_in_the_path_loses_its_user_info() {
        assert_eq!(
            url("https://proxy.example.invalid/https://user:secret@inner.example.invalid/a"),
            Some("https://proxy.example.invalid/https://inner.example.invalid/a".into())
        );
    }

    #[test]
    fn a_protocol_relative_url_counts() {
        assert_eq!(
            url("//cdn.example.invalid/bg/stars.png?w=64"),
            Some("//cdn.example.invalid/bg/stars.png".into())
        );
    }

    #[test]
    fn the_scheme_and_host_are_lowercase_and_the_path_as_written() {
        let found = whole_url("HTTPS://CDN.Example.Invalid/A.png").unwrap();
        assert_eq!(found.url, "https://cdn.example.invalid/A.png");
        assert_eq!(found.host, "cdn.example.invalid");
    }

    #[test]
    fn extra_slashes_after_the_scheme_are_skipped() {
        assert_eq!(
            url("https:\\\\/cdn.example.invalid/a.png"),
            Some("https://cdn.example.invalid/a.png".into())
        );
    }

    #[test]
    fn a_scheme_with_one_slash_does_not_count() {
        assert_eq!(url("https:/cdn.example.invalid/a.png"), None);
    }

    #[test]
    fn an_empty_port_is_dropped() {
        assert_eq!(
            url("https://example.invalid:/a"),
            Some("https://example.invalid/a".into())
        );
    }

    #[test]
    fn a_port_loses_its_leading_zeros() {
        assert_eq!(
            url("https://example.invalid:08443/a"),
            Some("https://example.invalid:8443/a".into())
        );
    }

    #[test]
    fn the_default_port_of_the_scheme_is_dropped() {
        assert_eq!(
            url("https://example.invalid:443/a"),
            Some("https://example.invalid/a".into())
        );
        assert_eq!(
            url("ws://example.invalid:0080/a"),
            Some("ws://example.invalid/a".into())
        );
    }

    #[test]
    fn the_default_port_of_another_scheme_is_kept() {
        assert_eq!(
            url("http://example.invalid:443/a"),
            Some("http://example.invalid:443/a".into())
        );
    }

    #[test]
    fn a_protocol_relative_url_keeps_its_port() {
        assert_eq!(
            url("//example.invalid:443/a"),
            Some("//example.invalid:443/a".into())
        );
    }

    #[test]
    fn a_port_with_a_sign_does_not_count() {
        assert_eq!(url("https://example.invalid:+8443/a"), None);
    }

    #[test]
    fn a_port_above_65535_does_not_count() {
        assert_eq!(
            url("https://example.invalid:65535/"),
            Some("https://example.invalid:65535/".into())
        );
        assert_eq!(url("https://example.invalid:65536/"), None);
    }

    #[test]
    fn a_backslash_in_the_path_is_read_as_a_slash() {
        assert_eq!(
            url("https://cdn.example.invalid\\brand\\logo.png"),
            Some("https://cdn.example.invalid/brand/logo.png".into())
        );
    }

    #[test]
    fn a_backslash_ends_the_host_as_a_browser_reads_it() {
        let found = whole_url("https://cdn.example.invalid\\@evil.example.invalid/x").unwrap();
        assert_eq!(found.host, "cdn.example.invalid");
    }

    #[test]
    fn an_ipv6_literal_is_a_host() {
        let found = whole_url("http://[::1]:8080/feed").unwrap();
        assert_eq!(found.host, "[::1]");
    }

    #[test]
    fn a_url_inside_longer_text_does_not_count() {
        assert_eq!(url("see https://pixijs.com/guide"), None);
    }

    #[test]
    fn a_value_with_whitespace_does_not_count() {
        assert_eq!(url("https://example.invalid/a b"), None);
    }

    #[test]
    fn a_scheme_without_a_host_does_not_count() {
        assert_eq!(url("https://"), None);
    }

    #[test]
    fn a_host_with_an_empty_label_before_its_only_dot_does_not_count() {
        assert_eq!(url("https://.invalid/"), None);
    }

    #[test]
    fn a_scheme_that_is_not_external_does_not_count() {
        assert_eq!(url("ftp://files.example.invalid/a"), None);
        assert_eq!(url("data:image/png;base64,AAAA"), None);
    }

    #[test]
    fn a_relative_path_does_not_count() {
        assert_eq!(url("./assets/app.css"), None);
        assert_eq!(url("assets/logo.png"), None);
    }

    #[test]
    fn a_port_that_is_not_digits_does_not_count() {
        assert_eq!(url("https://example.invalid:port/"), None);
    }

    #[test]
    fn a_host_without_a_dot_does_not_count() {
        assert_eq!(url("http://localhost:8080/"), None);
    }

    #[test]
    fn w3c_namespace_uris_are_dropped() {
        assert_eq!(url("http://www.w3.org/2000/svg"), None);
        assert_eq!(url("http://www.w3.org/1999/xlink"), None);
        assert_eq!(
            url("http://www.w3.org/2000/svg/other"),
            Some("http://www.w3.org/2000/svg/other".into())
        );
    }

    #[test]
    fn a_template_with_a_whole_host_records_its_start() {
        let start = ExternalUrl {
            url: "https://cdn.example.invalid/a/".into(),
            host: "cdn.example.invalid".into(),
        };
        assert_eq!(
            template_start("https://cdn.example.invalid/a/"),
            TemplateStart::Url(start)
        );
    }

    #[test]
    fn a_template_that_builds_the_host_is_a_dynamic_host_site() {
        assert_eq!(template_start("https://"), TemplateStart::DynamicHost);
        assert_eq!(template_start("https://api."), TemplateStart::DynamicHost);
        assert_eq!(
            template_start("https://api.example.invalid"),
            TemplateStart::DynamicHost
        );
    }

    #[test]
    fn a_protocol_relative_template_is_ignored() {
        assert_eq!(template_start("//----"), TemplateStart::NotAUrl);
    }

    #[test]
    fn a_template_without_a_scheme_is_not_a_url() {
        assert_eq!(template_start("PixiJS "), TemplateStart::NotAUrl);
    }

    #[test]
    fn the_book_sorts_urls_hosts_and_sources() {
        let path = |raw: &str| RelPath::new(raw).unwrap();
        let mut book = UrlBook::default();
        book.add(whole_url("https://b.invalid/x").unwrap(), &path("main.js"));
        book.add(whole_url("https://A.invalid/y").unwrap(), &path("main.js"));
        book.add(
            whole_url("https://b.invalid/x").unwrap(),
            &path("index.html"),
        );
        book.add_dynamic_host_sites(2);
        let external = book.finish();
        assert_eq!(external.hosts, ["a.invalid", "b.invalid"]);
        let listed: Vec<&str> = external.urls.iter().map(|url| url.url.as_str()).collect();
        assert_eq!(listed, ["https://a.invalid/y", "https://b.invalid/x"]);
        assert_eq!(
            external.urls[1].sources,
            [path("index.html"), path("main.js")]
        );
        assert_eq!(external.dynamic_host_sites, 2);
    }
}
