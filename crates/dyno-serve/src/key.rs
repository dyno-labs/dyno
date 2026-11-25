//! The run key: only the page that dyno asked for can drive a served run.
//!
//! The harness server takes a run's events over HTTP, from any browser that reaches its port.
//! On a shared network, such as a lab where several phones run checks on one host, any web page
//! can send it requests, the game of another check too. So each run has a key of 128 random bits
//! from the system's random source, which dyno makes and puts in the harness page's URL, the one
//! it opens in the browser or prints. Every request to the harness server must carry it, or it is
//! refused and changes nothing. The page's own requests carry it in a header.
//!
//! The game cannot learn the key. It is on another origin, so it cannot read the page's URL, and
//! the page never sets `document.domain`, which would make the two ports one origin. The page and
//! its frame send no referrer, so the frame's request does not name the page's URL. The page gives
//! the frame nothing but the game's URL, and its messages to the frame hold no key. The probe in
//! the frame has no key either: it posts its events to the page around it, which takes messages
//! from its own frame only, as one of three kinds of event of that frame (a probe event, its
//! document, a message to the host), so it cannot say hello, answer the page's commands or end a
//! run.
//!
//! A key is no secret from whoever reads the network: the page is plain HTTP. It keeps out web
//! content, not a party that sees the traffic.

use std::fmt;

use crate::ServeError;

/// The query string parameter that carries the key in the harness page's URL.
pub const KEY_PARAMETER: &str = "key";

/// The header that carries the key in the harness page's own requests.
pub const KEY_HEADER: &str = "X-Dyno-Key";

/// The header that marks dyno's own refusal of a request without the run's key, so the page can
/// tell it from a 403 that something between it and dyno made, such as a proxy, which must not
/// stop a run.
pub const REFUSED_HEADER: (&str, &str) = ("X-Dyno-Refused", "key");

/// How many random bytes a key has: 128 bits.
const KEY_BYTES: usize = 16;

/// One run's key, as lower-case hex. Its debug form leaves the key out, so no log can hold it.
#[derive(Clone, PartialEq, Eq)]
pub struct RunKey(String);

impl RunKey {
    /// A new key from the system's random source.
    pub fn new() -> Result<RunKey, ServeError> {
        let mut bytes = [0_u8; KEY_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|error| ServeError::Key(format!("no random bytes: {error}")))?;
        Ok(RunKey(
            bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        ))
    }

    /// The key as it goes in a URL or a header.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `given` is this key. The time it takes depends on the lengths alone, and a key's
    /// length is no secret, so it tells a guesser nothing about how close a guess came.
    pub fn matches(&self, given: &str) -> bool {
        let (key, given) = (self.0.as_bytes(), given.as_bytes());
        if key.len() != given.len() {
            return false;
        }
        // Every byte is compared, with no early end, and black_box keeps the compiler from
        // making one.
        let differ = key.iter().zip(given).fold(0_u8, |differ, (a, b)| {
            std::hint::black_box(differ | (a ^ b))
        });
        differ == 0
    }
}

impl fmt::Debug for RunKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RunKey(..)")
    }
}

/// A URL with a run's key in it: the harness page's, as dyno opens or prints it. Its debug form
/// leaves the key out.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyedUrl {
    url: String,
    /// Where the key starts in `url`.
    key_at: usize,
}

impl KeyedUrl {
    /// The URL `base`, which ends with `/`, with `key` in its query string.
    pub fn new(base: &str, key: &RunKey) -> KeyedUrl {
        let without_key = format!("{base}?{KEY_PARAMETER}=");
        KeyedUrl {
            key_at: without_key.len(),
            url: without_key + key.as_str(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.url
    }
}

impl fmt::Debug for KeyedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", format!("{}..", &self.url[..self.key_at]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_128_bits_of_hex_and_each_is_new() {
        let key = RunKey::new().unwrap();
        assert_eq!(key.as_str().len(), 32);
        assert!(
            key.as_str()
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{}",
            key.as_str()
        );
        let others: Vec<RunKey> = (0..100).map(|_| RunKey::new().unwrap()).collect();
        assert!(others.iter().all(|other| *other != key));
    }

    #[test]
    fn only_the_very_key_matches() {
        let key = RunKey::new().unwrap();
        assert!(key.matches(key.as_str()));
        assert!(!key.matches(""));
        assert!(!key.matches(&key.as_str()[..31]));
        assert!(!key.matches(&format!("{}0", key.as_str())));
        assert!(!key.matches(&key.as_str().to_uppercase()));
        // One byte off, at the start, in the middle and at the end.
        for at in [0, 16, 31] {
            let mut other = key.as_str().as_bytes().to_vec();
            other[at] = if other[at] == b'0' { b'1' } else { b'0' };
            assert!(!key.matches(std::str::from_utf8(&other).unwrap()), "{at}");
        }
    }

    #[test]
    fn no_debug_form_holds_the_key() {
        let key = RunKey::new().unwrap();
        let url = KeyedUrl::new("http://127.0.0.1:7370/", &key);
        assert_eq!(
            url.as_str(),
            format!("http://127.0.0.1:7370/?key={}", key.as_str())
        );
        assert_eq!(format!("{key:?}"), "RunKey(..)");
        assert_eq!(format!("{url:?}"), r#""http://127.0.0.1:7370/?key=..""#);
    }
}
