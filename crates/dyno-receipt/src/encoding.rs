//! The two encodings of a receipt: base64url with no padding for bytes, and lower-case hex for
//! hashes.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The bytes as base64url with no padding.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The `N` bytes that a base64url text holds. None when the text has padding, a character of
/// another alphabet or bits after its last byte, or holds another number of bytes. So each key
/// and each signature has one spelling only.
pub(crate) fn bytes_of_base64url<const N: usize>(text: &str) -> Option<[u8; N]> {
    URL_SAFE_NO_PAD.decode(text).ok()?.try_into().ok()
}

/// Whether the text is `length` lower-case hex characters, as dyno writes a hash.
pub(crate) fn is_lower_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_survive_a_round_trip_with_the_url_alphabet_and_no_padding() {
        // 0xfb 0xff gives "-_8" in the URL alphabet, and "+/8=" in the standard one.
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(bytes_of_base64url::<2>("-_8"), Some([0xfb, 0xff]));
    }

    #[test]
    fn text_with_padding_another_alphabet_or_another_length_is_not_taken() {
        for text in ["-_8=", "+/8", "-_8 ", "-_", "-_8A", ""] {
            assert_eq!(bytes_of_base64url::<2>(text), None, "{text:?}");
        }
    }

    #[test]
    fn text_with_bits_after_its_last_byte_is_not_taken() {
        // "-_8" ends on a byte. "-_9" holds the same two bytes and one more bit.
        assert_eq!(bytes_of_base64url::<2>("-_9"), None);
    }

    #[test]
    fn a_hash_is_lower_case_hex_of_its_own_length() {
        assert!(is_lower_hex("0123456789abcdef", 16));
        for text in ["0123456789ABCDEF", "0123456789abcde", "0123456789abcdeg"] {
            assert!(!is_lower_hex(text, 16), "{text}");
        }
    }
}
