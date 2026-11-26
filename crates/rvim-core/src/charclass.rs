//! Character classification with Vim semantics (reference: charset.c).
//!
//! Default keyword rules and Unicode motion classes from the Vim baseline.
//! These helpers use `@,48-57,_,192-255`; custom `iskeyword` values are not
//! consulted here.

/// Whether a character belongs to a default keyword/script class.
pub fn is_word_char(ch: char) -> bool {
    word_class(ch) >= 2
}

/// Vim's default word-motion class: whitespace, punctuation, or a script class.
/// CJK text and Latin text are separate runs; punctuation is never whitespace.
pub fn word_class(ch: char) -> u32 {
    let code = ch as u32;
    if code < 0x100 {
        return if matches!(ch, '\0' | ' ' | '\t' | '\u{a0}') {
            0
        } else if ch.is_ascii_alphanumeric() || ch == '_' || code >= 192 {
            2
        } else {
            1
        };
    }
    use crate::unicode_wordclass::{CLASSES, EMOJI};
    if EMOJI
        .binary_search_by(|&(first, last)| {
            if code < first {
                std::cmp::Ordering::Greater
            } else if code > last {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
    {
        return 3;
    }
    // Use the baseline's inclusive search bounds, including its historical
    // overlapping symbol/superscript intervals.
    let (mut low, mut high) = (0isize, CLASSES.len() as isize - 1);
    while low <= high {
        let mid = (low + high) / 2;
        let (first, last, class) = CLASSES[mid as usize];
        if code < first {
            high = mid - 1;
        } else if code > last {
            low = mid + 1;
        } else {
            return class;
        }
    }
    2
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
