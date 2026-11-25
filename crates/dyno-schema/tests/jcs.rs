//! `jcs`: RFC 8785 canonical JSON of JSON that another program wrote.
//!
//! A receipt's signature and `result_sha256` are over these bytes, and a signer in JavaScript gets
//! them from `JSON.parse` and `JSON.stringify`. So each test gives the bytes that those two give,
//! from the RFC's own examples or from Node.

use dyno_schema::canonical::{jcs, sha256_hex, sha256_of_jcs};
use serde_json::Value;

/// The canonical JSON of a JSON text.
fn canonical(text: &str) -> String {
    let json: Value = serde_json::from_str(text).unwrap_or_else(|error| panic!("{text}: {error}"));
    jcs(&json).unwrap()
}

/// The double with these bits, in hex, as RFC 8785 appendix B gives each of its numbers.
fn double(bits: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(bits, 16).unwrap())
}

#[test]
fn keys_are_sorted_lists_keep_their_order_and_whitespace_goes() {
    assert_eq!(
        canonical(r#"{ "b": [3, 1], "a": { "d": null, "c": true } }"#),
        r#"{"a":{"c":true,"d":null},"b":[3,1]}"#
    );
}

#[test]
fn the_example_of_rfc_8785_section_3_2_2_gives_its_output() {
    let input = r#"{
        "numbers": [333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
        "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
        "literals": [null, true, false]
    }"#;
    assert_eq!(
        canonical(input),
        "{\"literals\":[null,true,false],\
         \"numbers\":[333333333.3333333,1e+30,4.5,0.002,1e-27],\
         \"string\":\"€$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}"
    );
}

#[test]
fn keys_sort_by_utf_16_code_units_as_rfc_8785_section_3_2_3_does() {
    // The emoji is two surrogates, D83D and DE00, so it sorts before U+FB33. By code point, or
    // by UTF-8 bytes, it would sort after it.
    let input = r#"{
        "\u20ac": "Euro Sign",
        "\r": "Carriage Return",
        "\ufb33": "Hebrew Letter Dalet With Dagesh",
        "1": "One",
        "\ud83d\ude00": "Emoji: Grinning Face",
        "\u0080": "Control",
        "\u00f6": "Latin Small Letter O With Diaeresis"
    }"#;
    assert_eq!(
        canonical(input),
        "{\"\\r\":\"Carriage Return\",\
         \"1\":\"One\",\
         \"\u{80}\":\"Control\",\
         \"ö\":\"Latin Small Letter O With Diaeresis\",\
         \"€\":\"Euro Sign\",\
         \"😀\":\"Emoji: Grinning Face\",\
         \"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
    );
}

#[test]
fn keys_that_look_the_same_stay_apart_and_the_last_of_a_repeated_key_wins() {
    // "é" as one character sorts after "é" as an "e" and a combining accent: no key is
    // normalized. JSON.parse keeps the last value of a key that is there twice.
    assert_eq!(
        canonical(r#"{"b":1,"a":2,"a":3,"":4,"A":5,"aa":6,"\u00e9":7,"e\u0301":8}"#),
        "{\"\":4,\"A\":5,\"a\":3,\"aa\":6,\"b\":1,\"e\u{301}\":8,\"é\":7}"
    );
}

#[test]
fn each_number_of_rfc_8785_appendix_b_is_written_as_the_rfc_writes_it() {
    let numbers = [
        ("0000000000000000", "0"),
        ("8000000000000000", "0"),
        ("0000000000000001", "5e-324"),
        ("8000000000000001", "-5e-324"),
        ("7fefffffffffffff", "1.7976931348623157e+308"),
        ("ffefffffffffffff", "-1.7976931348623157e+308"),
        ("4340000000000000", "9007199254740992"),
        ("c340000000000000", "-9007199254740992"),
        ("4430000000000000", "295147905179352830000"),
        ("44b52d02c7e14af5", "9.999999999999997e+22"),
        ("44b52d02c7e14af6", "1e+23"),
        ("44b52d02c7e14af7", "1.0000000000000001e+23"),
        ("444b1ae4d6e2ef4e", "999999999999999700000"),
        ("444b1ae4d6e2ef4f", "999999999999999900000"),
        ("444b1ae4d6e2ef50", "1e+21"),
        ("3eb0c6f7a0b5ed8c", "9.999999999999997e-7"),
        ("3eb0c6f7a0b5ed8d", "0.000001"),
        ("41b3de4355555553", "333333333.3333332"),
        ("41b3de4355555554", "333333333.33333325"),
        ("41b3de4355555555", "333333333.3333333"),
        ("41b3de4355555556", "333333333.3333334"),
        ("41b3de4355555557", "333333333.33333343"),
        ("becbf647612f3696", "-0.0000033333333333333333"),
        ("43143ff3c1cb0959", "1424953923781206.2"),
    ];
    for (bits, text) in numbers {
        assert_eq!(
            jcs(&serde_json::json!(double(bits))).unwrap(),
            text,
            "{bits}"
        );
    }
}

#[test]
fn a_double_that_is_not_a_number_is_refused_as_rfc_8785_appendix_b_says() {
    for bits in ["7fffffffffffffff", "7ff0000000000000", "fff0000000000000"] {
        // `json!` turns such a double into null, so the double is given as it is.
        let result = serde_json_canonicalizer::to_string(&double(bits));
        assert!(result.is_err(), "{bits}");
    }
}

#[test]
fn a_number_in_a_json_text_is_written_as_json_stringify_writes_it() {
    // What Node gives for JSON.stringify(JSON.parse(text)).
    let numbers = [
        ("0", "0"),
        ("-0", "0"),
        ("-0.0", "0"),
        ("1.0", "1"),
        ("4.50", "4.5"),
        ("2e-3", "0.002"),
        ("100e-2", "1"),
        ("0.1e1", "1"),
        ("1e21", "1e+21"),
        ("1e-7", "1e-7"),
        ("0.000001", "0.000001"),
        ("123e-20", "1.23e-18"),
        // Too small for a double: zero, with no sign.
        ("1e-400", "0"),
        ("-1e-400", "0"),
        // The largest safe integer, 2^53 - 1, and the integers above it, which a double rounds.
        ("9007199254740991", "9007199254740991"),
        ("9007199254740992", "9007199254740992"),
        ("9007199254740993", "9007199254740992"),
        ("-9007199254740993", "-9007199254740992"),
        ("123456789012345678", "123456789012345680"),
        ("18446744073709551615", "18446744073709552000"),
        ("-9223372036854775808", "-9223372036854776000"),
        ("295147905179352830000", "295147905179352830000"),
        ("12345678901234567890123", "1.2345678901234568e+22"),
        // A decimal between two doubles goes to the nearer one.
        ("333333333.33333329", "333333333.3333333"),
        ("9.999999999999997e22", "9.999999999999997e+22"),
        ("0.1", "0.1"),
        ("0.30000000000000004", "0.30000000000000004"),
        ("5e-324", "5e-324"),
        ("1.7976931348623157e308", "1.7976931348623157e+308"),
    ];
    for (text, written) in numbers {
        assert_eq!(canonical(text), written, "{text}");
    }
}

#[test]
fn a_string_keeps_every_character_but_the_quote_the_backslash_and_the_controls() {
    // What Node gives for the same text: short escapes where JSON has one, lower-case \u00xx for
    // the other controls, and DEL, the line separators, an accent and an emoji as they are.
    assert_eq!(
        canonical(r#""\u0000\u001f\u007f\u2028\u2029\ud83d\ude00\u00e9\b\f\n\r\t\"\\\/""#),
        "\"\\u0000\\u001f\u{7f}\u{2028}\u{2029}😀é\\b\\f\\n\\r\\t\\\"\\\\/\""
    );
}

#[test]
fn text_that_is_not_ascii_is_written_as_utf_8_and_never_escaped() {
    let written = canonical(r#"{"name":"Zo\u00eb","\u65e5\u672c":"\u8a9e"}"#);
    assert_eq!(written, "{\"name\":\"Zoë\",\"日本\":\"語\"}");
    // The three bytes E2 82 AC.
    assert_eq!(canonical(r#""\u20ac""#).as_bytes(), b"\"\xe2\x82\xac\"");
}

#[test]
fn a_number_too_large_for_a_double_and_half_a_surrogate_pair_do_not_parse() {
    // JavaScript reads the first as Infinity and writes null, and writes the second back as
    // an escape. RFC 8785 refuses both, and so does the parser, before `jcs` sees them.
    for text in ["1E400", "-1E400", r#""\ud800""#, r#""\udc00x""#] {
        assert!(serde_json::from_str::<Value>(text).is_err(), "{text}");
    }
}

#[test]
fn empty_values_and_nesting_are_kept() {
    assert_eq!(
        canonical(r#" { "a" : { } , "b" : [ ] , "c" : [ [ ] , { "d" : "" } ] } "#),
        r#"{"a":{},"b":[],"c":[[],{"d":""}]}"#
    );
}

#[test]
fn the_hash_is_of_the_canonical_bytes_so_key_order_and_number_spelling_do_not_change_it() {
    let one: Value = serde_json::from_str(r#"{"a": 1.0, "b": [2e0, "x"]}"#).unwrap();
    let other: Value = serde_json::from_str(r#"{"b":[2,"x"],"a":1}"#).unwrap();
    assert_eq!(sha256_of_jcs(&one).unwrap(), sha256_of_jcs(&other).unwrap());
    assert_eq!(
        sha256_of_jcs(&one).unwrap(),
        sha256_hex(br#"{"a":1,"b":[2,"x"]}"#)
    );
}
