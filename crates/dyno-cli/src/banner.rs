//! The dyno wordmark in block characters, printed above `dyno --version` in a terminal. The two
//! files are copies of `brand/terminal/banner-small.*` (brand/README.md, "Terminal").

use std::ffi::OsStr;
use std::io::{IsTerminal, Write};

/// 24-bit colour, cool to hot.
const COLOUR: &str = include_str!("banner-small.ansi");

/// The same characters with no escape codes.
const PLAIN: &str = include_str!("banner-small.txt");

/// Prints the banner when stdout is a terminal. Piped or redirected, `--version` stays the one
/// line that scripts read.
pub fn print() {
    let stdout = std::io::stdout();
    if !stdout.is_terminal() {
        return;
    }
    let no_color = std::env::var_os("NO_COLOR");
    let colorterm = std::env::var_os("COLORTERM");
    let banner = choose(no_color.as_deref(), colorterm.as_deref());
    let _ = writeln!(stdout.lock(), "{banner}");
}

/// The colour banner only when the terminal says it has truecolor and `NO_COLOR` is unset or
/// empty (no-color.org).
fn choose(no_color: Option<&OsStr>, colorterm: Option<&OsStr>) -> &'static str {
    let colour_off = no_color.is_some_and(|value| !value.is_empty());
    let truecolor = colorterm.is_some_and(|value| value == "truecolor" || value == "24bit");
    if truecolor && !colour_off {
        COLOUR
    } else {
        PLAIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> Option<&OsStr> {
        Some(OsStr::new(value))
    }

    #[test]
    fn a_truecolor_terminal_gets_the_colour_banner() {
        assert_eq!(choose(None, os("truecolor")), COLOUR);
        assert_eq!(choose(None, os("24bit")), COLOUR);
        assert_eq!(choose(os(""), os("truecolor")), COLOUR);
    }

    #[test]
    fn no_color_or_no_truecolor_gets_the_plain_banner() {
        assert_eq!(choose(os("1"), os("truecolor")), PLAIN);
        assert_eq!(choose(None, None), PLAIN);
        assert_eq!(choose(None, os("256color")), PLAIN);
    }

    #[test]
    fn the_two_banners_draw_the_same_characters() {
        let mut stripped = String::new();
        let mut rest = COLOUR;
        while let Some(start) = rest.find('\x1b') {
            stripped.push_str(&rest[..start]);
            let end = rest[start..].find('m').expect("an escape code ends in m");
            rest = &rest[start + end + 1..];
        }
        stripped.push_str(rest);
        assert_eq!(stripped, PLAIN);
        assert!(!PLAIN.contains('\x1b'));
    }
}
