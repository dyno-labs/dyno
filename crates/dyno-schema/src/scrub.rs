//! Free text from outside dyno, such as a driver's error, can repeat a launch URL with its
//! password or session id. Such text must go through [`scrub`] before it enters a result.

use url::Url;

use crate::launch_url::recorded_form;

/// What replaces a URL that does not parse, after its scheme.
const REDACTED: &str = "[redacted]";

/// The text with every URL in its recorded form: scheme, host, port and path, with no user info,
/// query string or fragment. A URL in another URL's path gets the same treatment. A URL that does
/// not parse keeps only its scheme. A part of the text with no `://`, such as a bare path and
/// query string, is left as it is.
///
/// A URL starts at the scheme before a `://` and ends at the next whitespace, control character,
/// `"`, `<` or `>`. A serialized URL, such as the launch URL a driver gets, escapes all of those. A
/// comma or a bracket does not end a URL.
pub fn scrub(text: &str) -> String {
    let mut scrubbed = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(separator) = rest.find("://") {
        let url_start = start_of_scheme(&rest[..separator]);
        let url_end = separator + end_of_url(&rest[separator..]);
        scrubbed.push_str(&rest[..url_start]);
        scrubbed.push_str(&scrub_url(
            &rest[url_start..url_end],
            &rest[url_start..separator],
        ));
        rest = &rest[url_end..];
    }
    scrubbed.push_str(rest);
    scrubbed
}

/// Where the scheme starts: after the last character that cannot be in a scheme.
fn start_of_scheme(before_separator: &str) -> usize {
    before_separator
        .char_indices()
        .rev()
        .take_while(|&(_, character)| is_scheme_character(character))
        .last()
        .map_or(before_separator.len(), |(index, _)| index)
}

/// A letter, digit, `+`, `-` or `.`, as RFC 3986 allows after a scheme's first letter.
fn is_scheme_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
}

/// Where the URL ends: at the next whitespace, control character, `"`, `<` or `>`, or at the end
/// of the text.
fn end_of_url(from_separator: &str) -> usize {
    from_separator
        .find(|character: char| {
            character.is_whitespace()
                || character.is_control()
                || matches!(character, '"' | '<' | '>')
        })
        .unwrap_or(from_separator.len())
}

/// The URL's recorded form. The caller passes the scheme, because it has already found where the
/// scheme starts and ends.
fn scrub_url(url: &str, scheme: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => recorded_form(&parsed),
        Err(_) => format!("{scheme}://{REDACTED}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_loses_its_user_info_query_and_fragment() {
        assert_eq!(
            scrub("could not load https://user:pw@games.example/g?token=SECRET#frag in time"),
            "could not load https://games.example/g in time"
        );
    }

    #[test]
    fn every_url_in_the_text_is_scrubbed() {
        assert_eq!(
            scrub("https://a.example/?t=1 then wss://u:p@b.example:8443/socket?t=2"),
            "https://a.example/ then wss://b.example:8443/socket"
        );
    }

    #[test]
    fn text_with_no_url_is_unchanged() {
        let text = "no browser: exit code 1, see stderr";
        assert_eq!(scrub(text), text);
    }

    #[test]
    fn a_bare_path_and_query_is_unchanged() {
        let text = "GET /g?token=SECRET 404";
        assert_eq!(scrub(text), text);
    }

    #[test]
    fn a_url_that_does_not_parse_keeps_only_its_scheme() {
        assert_eq!(
            scrub("bad https://u:abc/def@h/g?token=SECRET end"),
            "bad https://[redacted] end"
        );
    }

    #[test]
    fn a_url_with_no_scheme_is_redacted_after_the_separator() {
        assert_eq!(scrub("see ://h/?token=SECRET"), "see ://[redacted]");
    }

    #[test]
    fn a_url_ends_at_a_space_quote_or_angle_bracket() {
        assert_eq!(
            scrub(r#"url "https://h/g?t=SECRET" <https://h/x?t=SECRET>"#),
            r#"url "https://h/g" <https://h/x>"#
        );
    }

    #[test]
    fn a_url_runs_past_a_comma_or_bracket_to_the_next_whitespace() {
        // The closing bracket is part of the query string, so it goes with it.
        assert_eq!(
            scrub("(https://h/g?a=1,token=SECRET) end"),
            "(https://h/g end"
        );
    }

    #[test]
    fn a_url_in_another_urls_path_is_scrubbed_too() {
        assert_eq!(
            scrub("via https://proxy.example/fetch/https://u:pw@games.example/g?t=1 failed"),
            "via https://proxy.example/fetch/https://games.example/g failed"
        );
    }

    #[test]
    fn scrubbed_text_scrubs_to_itself() {
        let once = scrub("https://u:p@h/g?t=1 and https://u:abc/def@h/ and ://x");
        assert_eq!(scrub(&once), once);
    }
}
