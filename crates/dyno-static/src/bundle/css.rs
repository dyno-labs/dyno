//! CSS: the values of `url(...)` and `@import` (design section 8.6).
//!
//! A scanner, not a parser: one pass that skips comments and strings, and reads each reference
//! as written, so `urls::whole_url` decides which ones leave the build. In a quoted value, a
//! backslash keeps the next character. Hex escapes and backslashes in unquoted values are not
//! decoded, and the strings in `image-set()` are not read.

const URL_OPEN: &str = "url(";
const URL_CLOSE: char = ')';
const IMPORT: &str = "@import";
const COMMENT_OPEN: &str = "/*";
const COMMENT_CLOSE: &str = "*/";
const QUOTES: [char; 2] = ['"', '\''];
const ESCAPE: char = '\\';
/// A raw newline ends a string as a bad string. CSS reads a carriage return and a form feed as
/// newlines too.
const NEWLINES: [char; 3] = ['\n', '\r', '\x0c'];
const CARRIAGE_RETURN: char = '\r';
const LINE_FEED: char = '\n';

/// The references in a stylesheet, a `<style>` element or a `style` attribute, in order.
pub fn references(css: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(next) = css[at..].chars().next() {
        let rest = &css[at..];
        if rest.starts_with(COMMENT_OPEN) {
            at = comment_end(css, at);
        } else if starts_with_keyword(rest, URL_OPEN) {
            let (reference, end) = url_argument(css, at + URL_OPEN.len());
            found.extend(reference);
            at = end;
        } else if starts_with_keyword(rest, IMPORT) {
            let (reference, end) = import_target(css, at + IMPORT.len());
            found.extend(reference);
            at = end;
        } else if QUOTES.contains(&next) {
            let (_, end) = quoted(css, at);
            at = end;
        } else {
            at += next.len_utf8();
        }
    }
    found
}

/// The byte after the comment that starts at `start`, or the end of the text when it is not
/// closed.
fn comment_end(css: &str, start: usize) -> usize {
    let body = start + COMMENT_OPEN.len();
    match css[body..].find(COMMENT_CLOSE) {
        Some(close) => body + close + COMMENT_CLOSE.len(),
        None => css.len(),
    }
}

/// Whether `text` starts with `keyword`, in any case.
fn starts_with_keyword(text: &str, keyword: &str) -> bool {
    text.as_bytes()
        .get(..keyword.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(keyword.as_bytes()))
}

/// The argument of the `url(` that ends at `after`, and the byte after the argument.
fn url_argument(css: &str, after: usize) -> (Option<String>, usize) {
    let rest = css[after..].trim_start();
    let start = css.len() - rest.len();
    if rest.starts_with(QUOTES) {
        return quoted(css, start);
    }
    match rest.find(URL_CLOSE) {
        Some(close) => {
            let value = rest[..close].trim().to_owned();
            (Some(value), start + close + URL_CLOSE.len_utf8())
        }
        // Like a browser, an unclosed `url(` runs to the end of the text.
        None => (None, css.len()),
    }
}

/// The string that starts with a quote at `start`, and the byte after it. None when a newline
/// or the end of the text comes before the closing quote.
fn quoted(css: &str, start: usize) -> (Option<String>, usize) {
    let mut chars = css[start..].char_indices().peekable();
    let Some((_, quote)) = chars.next() else {
        return (None, start);
    };
    let mut value = String::new();
    while let Some((index, c)) = chars.next() {
        if c == quote {
            return (Some(value), start + index + c.len_utf8());
        }
        if NEWLINES.contains(&c) {
            return (None, start + index);
        }
        if c != ESCAPE {
            value.push(c);
            continue;
        }
        let Some((_, escaped)) = chars.next() else {
            break;
        };
        // An escaped newline joins two lines of one string, and CR LF is one newline.
        if escaped == CARRIAGE_RETURN {
            chars.next_if(|&(_, next)| next == LINE_FEED);
        } else if !NEWLINES.contains(&escaped) {
            value.push(escaped);
        }
    }
    (None, css.len())
}

/// The target of the `@import "..."` that ends at `after`, and the byte after it.
fn import_target(css: &str, after: usize) -> (Option<String>, usize) {
    let rest = css[after..].trim_start();
    let start = css.len() - rest.len();
    if rest.starts_with(QUOTES) {
        return quoted(css, start);
    }
    // `@import url(...)` is read when the scan reaches its `url(`.
    (None, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_values_are_read_with_or_without_quotes() {
        let css = r#"a { background: url("x.png") } b { src: url('y.woff2') } c { cursor: url( z.cur ) }"#;
        assert_eq!(references(css), ["x.png", "y.woff2", "z.cur"]);
    }

    #[test]
    fn a_quoted_value_keeps_an_escaped_quote() {
        assert_eq!(references(r#"a { b: url("x\"y.png") }"#), [r#"x"y.png"#]);
    }

    #[test]
    fn imports_are_read_as_strings_or_urls() {
        let css = r#"@import "a.css"; @import url(b.css);"#;
        assert_eq!(references(css), ["a.css", "b.css"]);
    }

    #[test]
    fn a_media_query_after_an_import_is_not_a_reference() {
        assert_eq!(references("@import 'c.css' screen;"), ["c.css"]);
    }

    #[test]
    fn keywords_are_matched_in_any_case() {
        let css = "@IMPORT 'a.css'; b { background: URL(x.png) }";
        assert_eq!(references(css), ["a.css", "x.png"]);
    }

    #[test]
    fn a_comment_is_skipped() {
        assert_eq!(
            references("/* url(old.png) */ a { background: url(new.png) }"),
            ["new.png"]
        );
    }

    #[test]
    fn an_unclosed_comment_runs_to_the_end() {
        assert!(references("/* unclosed url(x.png)").is_empty());
    }

    #[test]
    fn a_url_inside_a_string_is_not_a_reference() {
        let css = r#"a::before { content: "url(x.png)" } b { background: url(y.png) }"#;
        assert_eq!(references(css), ["y.png"]);
    }

    #[test]
    fn a_comment_start_inside_a_string_is_not_a_comment() {
        let css = r#"a { content: "/*" } b { background: url(y.png) }"#;
        assert_eq!(references(css), ["y.png"]);
    }

    #[test]
    fn a_quoted_url_that_meets_a_newline_is_not_a_reference() {
        let css = "a { background: url(\"x.png\n) } b { background: url(y.png) }";
        assert_eq!(references(css), ["y.png"]);
    }

    #[test]
    fn an_unclosed_quoted_url_is_not_a_reference() {
        assert!(references("a { background: url('x.png").is_empty());
    }

    #[test]
    fn an_escaped_newline_joins_a_string() {
        assert_eq!(references("@import 'a\\\nb.css';"), ["ab.css"]);
    }

    #[test]
    fn an_escaped_carriage_return_and_line_feed_join_a_string_once() {
        assert_eq!(references("@import 'a\\\r\nb.css';"), ["ab.css"]);
    }

    #[test]
    fn an_escaped_form_feed_joins_a_string() {
        assert_eq!(references("@import 'a\\\x0cb.css';"), ["ab.css"]);
    }

    #[test]
    fn a_carriage_return_or_form_feed_ends_a_quoted_url() {
        let css = "a { background: url('x.png\r) } b { background: url('w.png\x0c) } c { background: url(y.png) }";
        assert_eq!(references(css), ["y.png"]);
    }

    #[test]
    fn an_unclosed_url_ends_the_scan() {
        assert!(references("a { background: url(x.png; } @import 'b.css';").is_empty());
    }
}
