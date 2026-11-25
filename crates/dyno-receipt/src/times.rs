//! The times of a receipt and of a lab's keys.

use std::time::SystemTime;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The shape of a receipt's time, with `0` for each digit.
const RECEIPT_TIME_SHAPE: &[u8] = b"0000-00-00T00:00:00Z";

/// An RFC 3339 time, with any offset and any fraction of a second. None when the text is not
/// one. A lab's keys document states its times this way.
pub(crate) fn parse(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339).ok()
}

/// Whether the text is a time as a receipt states it: RFC 3339, in UTC, to the second, ending
/// in `Z`, such as `2026-11-25T10:00:00Z`. The time is signed as text, so it has one spelling.
pub fn is_receipt_time(text: &str) -> bool {
    // An RFC 3339 parser also takes a space or a small letter where the shape has `T` and `Z`.
    let has_the_shape = text.len() == RECEIPT_TIME_SHAPE.len()
        && text
            .bytes()
            .zip(RECEIPT_TIME_SHAPE)
            .all(|(byte, shape)| match shape {
                b'0' => byte.is_ascii_digit(),
                _ => byte == *shape,
            });
    has_the_shape && parse(text).is_some()
}

/// A time as a receipt states it: in UTC, with the part of a second dropped.
pub fn receipt_time(at: SystemTime) -> String {
    OffsetDateTime::from(at)
        .replace_nanosecond(0)
        .expect("0 is a nanosecond of every second")
        .format(&Rfc3339)
        .expect("a time in UTC has an RFC 3339 form")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_time_is_written_in_utc_to_the_second() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_millis(1_795_608_000_999);
        assert_eq!(receipt_time(at), "2026-11-25T12:00:00Z");
        assert!(is_receipt_time(&receipt_time(at)));
        assert!(is_receipt_time(&receipt_time(SystemTime::now())));
    }

    #[test]
    fn a_receipt_time_is_utc_to_the_second_and_ends_in_z() {
        assert!(is_receipt_time("2026-11-25T10:00:00Z"));
        for text in [
            "2026-11-25T10:00:00.000Z",
            "2026-11-25T10:00:00+00:00",
            "2026-11-25T10:00:00z",
            "2026-11-25 10:00:00Z",
            "2026-11-25t10:00:00Z",
            "2026-11-25T10:00Z",
            "2026-13-25T10:00:00Z",
            "2026-11-25",
            "",
        ] {
            assert!(!is_receipt_time(text), "{text}");
        }
    }

    #[test]
    fn a_keys_document_time_can_have_an_offset_and_a_fraction() {
        let utc = parse("2026-11-25T10:00:00Z").unwrap();
        assert_eq!(parse("2026-11-25T10:00:00.000Z"), Some(utc));
        assert_eq!(parse("2026-11-25T12:00:00+02:00"), Some(utc));
        assert_eq!(parse("2026-11-25T10:00:00.000000+00:00"), Some(utc));
        assert!(parse("2026-11-25T10:00:00.5Z").unwrap() > utc);
        assert_eq!(parse("yesterday"), None);
    }
}
