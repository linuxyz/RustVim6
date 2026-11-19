//! CTRL-A / CTRL-X: add to or subtract from the number under the cursor
//! (reference: ops.c `do_addsub()`, normal.c `nv_addsub()`).
//!
//! Supported exactly like vim's normal-mode form:
//!
//! * decimal, plus hexadecimal (`0x1f`), binary (`0b101`) and octal (`077`)
//!   when `'nrformats'` enables them (default `bin,hex`);
//! * the leading `-` sign is part of the number (`-5` + 1 is `-4`);
//! * a number written with leading zeros keeps its width (`007` + 1 is `008`);
//!   hexadecimal keeps its case (`0xFF` + 1 is `0x100`, `0XFF` + 1 is `0X100`);
//! * when the cursor is not on a number, the first number later on the line is
//!   used (`abc 12` increments 12 from anywhere before it);
//! * `'nrformats'` `alpha` increments letters; `unsigned` never goes negative.
//!
//! Not implemented: the Visual-mode form (`g<C-a>` sequential numbering) and
//! vim's 64-bit wraparound on overflow (the value is clamped instead).

use rvim_core::{Pos, VimResult};
use rvim_editor::Editor;

/// One parsed number: the prefix style, its byte length and its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedNumber {
    /// 0 for decimal, '0' for octal, 'x'/'X' for hex, 'b'/'B' for binary.
    prefix: char,
    /// Byte length of the whole number, prefix included.
    len: usize,
    /// Absolute value.
    value: u64,
    /// True when the number contains an upper-case hex digit or `0X`.
    upper: bool,
}

fn is_bdigit(ch: u8) -> bool {
    ch == b'0' || ch == b'1'
}

fn is_xdigit(ch: u8) -> bool {
    ch.is_ascii_hexdigit()
}

fn parse_number(text: &str, do_bin: bool, do_oct: bool, do_hex: bool) -> ParsedNumber {
    let bytes = text.as_bytes();
    let mut out = ParsedNumber {
        prefix: '\0',
        len: 0,
        value: 0,
        upper: false,
    };
    if bytes.is_empty() {
        return out;
    }
    // 0x / 0X
    if do_hex
        && bytes.len() >= 3
        && bytes[0] == b'0'
        && (bytes[1] == b'x' || bytes[1] == b'X')
        && is_xdigit(bytes[2])
    {
        out.prefix = bytes[1] as char;
        out.upper = bytes[1] == b'X';
        let mut i = 2;
        let mut value: u64 = 0;
        while i < bytes.len() && is_xdigit(bytes[i]) {
            if bytes[i].is_ascii_uppercase() {
                out.upper = true;
            }
            value = value
                .wrapping_mul(16)
                .wrapping_add((bytes[i] as char).to_digit(16).unwrap_or(0) as u64);
            i += 1;
        }
        out.len = i;
        out.value = value;
        return out;
    }
    // 0b / 0B
    if do_bin
        && bytes.len() >= 3
        && bytes[0] == b'0'
        && (bytes[1] == b'b' || bytes[1] == b'B')
        && is_bdigit(bytes[2])
    {
        out.prefix = bytes[1] as char;
        let mut i = 2;
        let mut value: u64 = 0;
        while i < bytes.len() && is_bdigit(bytes[i]) {
            value = value.wrapping_mul(2).wrapping_add((bytes[i] - b'0') as u64);
            i += 1;
        }
        out.len = i;
        out.value = value;
        return out;
    }
    // 0NNN octal, when enabled.
    if do_oct && bytes[0] == b'0' {
        let mut i = 1;
        let mut value: u64 = 0;
        while i < bytes.len() && (b'0'..=b'7').contains(&bytes[i]) {
            value = value.wrapping_mul(8).wrapping_add((bytes[i] - b'0') as u64);
            i += 1;
        }
        if i > 1 {
            out.prefix = '0';
            out.len = i;
            out.value = value;
            return out;
        }
    }
    // Decimal.
    let mut i = 0;
    let mut value: u64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        value = value
            .wrapping_mul(10)
            .wrapping_add((bytes[i] - b'0') as u64);
        i += 1;
    }
    out.len = i;
    out.value = value;
    out
}

/// Where the number is and how it is written.
#[derive(Clone, Copy, Debug)]
struct Found {
    /// Byte column where the replacement starts (the `-` when there is one).
    start: usize,
    /// Byte column of the first digit (after the sign).
    digits: usize,
    negative: bool,
    parsed: ParsedNumber,
}

/// Locate the number the cursor refers to, following `do_addsub()`.
fn find_number(line: &str, col: usize, formats: &FormatFlags) -> Option<Found> {
    let bytes = line.as_bytes();
    if bytes.is_empty() || col >= bytes.len() {
        return None;
    }
    let at = |i: usize| bytes.get(i).copied().unwrap_or(0);
    let mut start = col;

    if formats.bin {
        while start > 0 && is_bdigit(at(start)) {
            start -= 1;
        }
    }
    if formats.hex {
        while start > 0 && is_xdigit(at(start)) {
            start -= 1;
        }
    }
    // "0b"/"0x" pattern overlap: rescan as a plain digit run.
    let hex_prefix = start > 0
        && (at(start) == b'x' || at(start) == b'X')
        && at(start.wrapping_sub(1)) == b'0'
        && is_xdigit(at(start + 1));
    if formats.bin && formats.hex && !hex_prefix {
        start = col;
        while start > 0 && at(start).is_ascii_digit() {
            start -= 1;
        }
    }
    let bin_prefix = start > 0
        && (at(start) == b'b' || at(start) == b'B')
        && at(start.wrapping_sub(1)) == b'0'
        && is_bdigit(at(start + 1));
    if (formats.hex && hex_prefix) || (formats.bin && bin_prefix) {
        start -= 1; // point at the leading 0
    } else {
        // Forward to the next digit, then back to the start of that number.
        start = col;
        while start < bytes.len()
            && !at(start).is_ascii_digit()
            && !(formats.alpha && at(start).is_ascii_alphabetic())
        {
            start += 1;
        }
        while start > 0
            && at(start.wrapping_sub(1)).is_ascii_digit()
            && !(formats.alpha && at(start.wrapping_sub(1)).is_ascii_alphabetic())
        {
            start -= 1;
        }
    }
    if start >= bytes.len() {
        return None;
    }
    let first = at(start);
    if !first.is_ascii_digit() && !(formats.alpha && first.is_ascii_alphabetic()) {
        return None;
    }
    // A '-' immediately before the digits is part of the number.
    let mut sign = start;
    let mut negative = false;
    if sign > 0 && at(sign - 1) == b'-' && !formats.unsigned {
        sign -= 1;
        negative = true;
    }
    let digits = if negative { sign + 1 } else { sign };
    let parsed = parse_number(&line[digits..], formats.bin, formats.oct, formats.hex);
    if parsed.len == 0 {
        return None;
    }
    Some(Found {
        start: sign,
        digits,
        negative,
        parsed,
    })
}

/// The `'nrformats'` flags (vim checks for the letters b/o/x/p/u).
#[derive(Clone, Copy, Debug, Default)]
struct FormatFlags {
    bin: bool,
    oct: bool,
    hex: bool,
    alpha: bool,
    unsigned: bool,
}

impl FormatFlags {
    fn from_option(value: &str) -> Self {
        Self {
            bin: value.contains('b'),
            oct: value.contains('o'),
            hex: value.contains('x'),
            alpha: value.contains('p'),
            unsigned: value.contains('u'),
        }
    }
}

/// Render the new number, keeping vim's width and case conventions.
fn render(
    parsed: ParsedNumber,
    negative: bool,
    value: i128,
    first_digit: u8,
    flags: FormatFlags,
) -> String {
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    let magnitude = value.unsigned_abs();
    // Prefix: '0' + style character (octal only needs the '0').
    let prefix_len = match parsed.prefix {
        '\0' => 0,
        '0' => 1,
        _ => 2,
    };
    if prefix_len == 2 {
        out.push('0');
        out.push(parsed.prefix);
    } else if prefix_len == 1 {
        out.push('0');
    }
    let digits = match parsed.prefix {
        'x' | 'X' => {
            if parsed.upper {
                format!("{magnitude:X}")
            } else {
                format!("{magnitude:x}")
            }
        }
        'b' | 'B' => format!("{magnitude:b}"),
        '0' => format!("{magnitude:o}"),
        _ => format!("{magnitude}"),
    };
    // Keep the original width when the number started with a zero, unless the
    // result could be mistaken for an octal number (ops.c do_addsub()).
    let pad = if first_digit == b'0' && !(flags.oct && parsed.prefix == '0') {
        parsed.len.saturating_sub(prefix_len + digits.len())
    } else {
        0
    };
    for _ in 0..pad {
        out.push('0');
    }
    out.push_str(&digits);
    out
}

/// Increment (`add`) or decrement the number under the cursor `count` times.
///
/// Returns false when there is no number at or after the cursor (vim beeps).
pub fn addsub(ed: &mut Editor, add: bool, count: u64) -> VimResult<bool> {
    let formats = FormatFlags::from_option(&ed.options.str_or("nrformats", "bin,hex"));
    let line = ed.current_line();
    let col = ed.cursor().col;
    let Some(found) = find_number(&line, col, &formats) else {
        return Ok(false);
    };
    let first_digit = line.as_bytes()[found.digits];
    let count = count.max(1) as i128;

    // 'nrformats' alpha: bump the letter itself.
    if formats.alpha && first_digit.is_ascii_alphabetic() {
        let lower = first_digit.to_ascii_lowercase();
        let ord = i128::from(lower - b'a');
        let next = if add {
            (ord + count).min(25)
        } else {
            (ord - count).max(0)
        };
        let base = if first_digit.is_ascii_uppercase() {
            b'A'
        } else {
            b'a'
        };
        let ch = (base + next as u8) as char;
        let mut new_line = line.clone();
        new_line.replace_range(found.digits..found.digits + 1, &ch.to_string());
        ed.set_line_text(ed.cursor().line, new_line)?;
        ed.set_cursor(Pos::new(ed.cursor().line, found.digits));
        return Ok(true);
    }

    let mut value = i128::from(found.parsed.value);
    if found.negative {
        value = -value;
    }
    let mut value = if add { value + count } else { value - count };
    let mut negative = value < 0;
    if formats.unsigned && negative {
        value = 0;
        negative = false;
    }
    let text = render(found.parsed, negative, value, first_digit, formats);
    let end = found.digits + found.parsed.len;
    let mut new_line = line.clone();
    new_line.replace_range(found.start..end, &text);
    let line_no = ed.cursor().line;
    ed.set_line_text(line_no, new_line)?;
    // vim leaves the cursor on the last character of the new number.
    let cursor_col = (found.start + text.len()).saturating_sub(1);
    let clamped = ed.current_buffer().clamp_pos(Pos::new(line_no, cursor_col));
    ed.set_cursor(clamped);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with(text: &str, col: usize) -> Editor {
        let mut ed = Editor::new();
        ed.set_line_text(0, text.to_string()).unwrap();
        ed.set_cursor(Pos::new(0, col));
        ed
    }

    fn line(ed: &Editor) -> String {
        ed.current_buffer().line_str(0).unwrap().to_string()
    }

    #[test]
    fn increments_and_decrements_a_decimal_number() {
        let mut ed = editor_with("value 41", 6);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "value 42");
        assert!(addsub(&mut ed, false, 3).unwrap());
        assert_eq!(line(&ed), "value 39");
    }

    #[test]
    fn keeps_the_width_of_a_zero_padded_number() {
        let mut ed = editor_with("007", 0);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "008");
        let mut ed = editor_with("099", 0);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "100");
    }

    #[test]
    fn hexadecimal_binary_and_case_are_preserved() {
        let mut ed = editor_with("0x1f", 0);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "0x20");
        let mut ed = editor_with("0XFF", 0);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "0X100");
        let mut ed = editor_with("0x0f", 0);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "0x10");
        let mut ed = editor_with("0b101", 0);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "0b110");
    }

    #[test]
    fn octal_only_applies_when_nrformats_asks_for_it() {
        let mut ed = editor_with("007", 0);
        ed.options
            .set("nrformats", rvim_editor::OptValue::Str("octal".into()))
            .unwrap();
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "010");
    }

    #[test]
    fn the_sign_is_part_of_the_number() {
        let mut ed = editor_with("-5", 1);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "-4");
        let mut ed = editor_with("-5", 1);
        assert!(addsub(&mut ed, false, 6).unwrap());
        assert_eq!(line(&ed), "-11");
        let mut ed = editor_with("0", 0);
        assert!(addsub(&mut ed, false, 1).unwrap());
        assert_eq!(line(&ed), "-1");
    }

    #[test]
    fn finds_the_next_number_when_the_cursor_is_not_on_one() {
        let mut ed = editor_with("abc 12 def", 0);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "abc 13 def");
    }

    #[test]
    fn a_line_without_numbers_is_unchanged() {
        let mut ed = editor_with("abc", 0);
        assert!(!addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "abc");
    }

    #[test]
    fn the_count_moves_by_that_much() {
        let mut ed = editor_with("1", 0);
        assert!(addsub(&mut ed, true, 9).unwrap());
        assert_eq!(line(&ed), "10");
    }

    #[test]
    fn the_cursor_ends_on_the_last_digit() {
        let mut ed = editor_with("9", 0);
        assert!(addsub(&mut ed, true, 1).unwrap());
        assert_eq!(line(&ed), "10");
        assert_eq!(ed.cursor().col, 1);
    }
}
