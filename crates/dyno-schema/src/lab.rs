//! The name of the kind of lab that ran a check, which `--lab` gives and `run.lab` records.
//!
//! A phone on a desk, a rig of phones and a cloud device lab can give different numbers for the
//! same build, so a result says which kind measured it. The name is a label: dyno does not check
//! that the lab is what the name says.

/// The most characters a lab name may have.
pub const MAX_LAB_NAME_CHARS: usize = 32;

/// A lab name, as it is recorded: a short word in lower case, such as "desk", "rig" or
/// "devicefarm". It starts with a letter, and holds only the letters a to z, digits and `-`, so
/// two people who mean the same lab write the same name. An error for any other text.
pub fn lab_name(text: &str) -> Result<String, String> {
    let starts_with_a_letter = text.starts_with(|first: char| first.is_ascii_lowercase());
    let only_word_characters = text
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if starts_with_a_letter && only_word_characters && text.len() <= MAX_LAB_NAME_CHARS {
        return Ok(text.to_owned());
    }
    Err(format!(
        "lab name {text:?} must be a short word in lower case, such as desk, rig or devicefarm: \
         it starts with a letter, and has at most {MAX_LAB_NAME_CHARS} letters a to z, digits \
         and -"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_word_in_lower_case_is_a_lab_name() {
        for name in ["desk", "rig", "devicefarm", "rig-2", "a"] {
            assert_eq!(lab_name(name).as_deref(), Ok(name));
        }
    }

    #[test]
    fn a_name_of_the_most_characters_is_taken_and_one_more_is_not() {
        let longest = "a".repeat(MAX_LAB_NAME_CHARS);
        assert_eq!(lab_name(&longest), Ok(longest.clone()));
        assert!(lab_name(&format!("{longest}a")).is_err());
    }

    #[test]
    fn upper_case_spaces_and_other_characters_are_refused() {
        for name in ["Desk", "phone lab", "rig_2", "désk", "rig.2", " desk"] {
            assert!(lab_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_name_starts_with_a_letter() {
        for name in ["", "2rig", "-rig"] {
            assert!(lab_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn the_error_names_the_text_and_the_rule() {
        let error = lab_name("Phone Lab").unwrap_err();
        assert!(error.contains(r#""Phone Lab""#), "{error}");
        assert!(error.contains("lower case"), "{error}");
    }
}
