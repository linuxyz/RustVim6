//! Character classification with Vim semantics (reference: charset.c).
//!
//! M1 covers ASCII word/whitespace/keyword rules plus UTF-8 Unicode helpers.
//! The `iskeyword` option is threaded through at call sites later; the defaults
//! here match Vim's default `@,48-57,_,192-255`.

/// Vim `iskeyword` default for ASCII letters: [A-Za-z_].
pub fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || (ch as u32) >= 192
}

pub fn is_blank(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

/// Whitespace as understood by motion commands (`\\s`).
pub fn is_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\r' | '\n' | '\u{000b}' | '\u{000c}')
}

/// `isalpha()` equivalent used by option parsing / file names.
pub fn is_alpha(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

pub fn is_lower(ch: char) -> bool {
    ch.is_ascii_lowercase()
}

pub fn is_upper(ch: char) -> bool {
    ch.is_ascii_uppercase()
}

pub fn to_lower(ch: char) -> char {
    ch.to_ascii_lowercase()
}

pub fn to_upper(ch: char) -> char {
    ch.to_ascii_uppercase()
}

/// Case-insensitive ASCII compare used by option/command abbreviation matching.
pub fn eq_ignore_case(a: char, b: char) -> bool {
    to_lower(a) == to_lower(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_and_space() {
        assert!(is_word_char('a') && is_word_char('9') && is_word_char('_'));
        assert!(!is_word_char(' ') && !is_word_char('.'));
        assert!(is_blank(' ') && is_blank('\t') && !is_blank('\n'));
        assert!(is_space('\n'));
        assert!(eq_ignore_case('Q', 'q'));
    }
}
