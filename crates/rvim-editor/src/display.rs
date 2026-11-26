//! Display-cell geometry shared by cursor motions and screen rendering.

use unicode_width::UnicodeWidthChar;

/// Width of a scalar, before tab and control-character expansion.
pub fn char_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(1)
}

fn cell_width(ch: char, col: usize, tabstop: usize) -> usize {
    match ch {
        '\t' => tabstop.clamp(1, 1000) - col % tabstop.clamp(1, 1000),
        '\u{7f}' | '\u{0}'..='\u{1f}' => 2,
        _ => char_width(ch),
    }
}

/// Convert a byte position into its zero-based screen column.
pub fn display_col(text: &str, byte_col: usize, tabstop: usize) -> usize {
    let mut col = 0;
    for (byte, ch) in text.char_indices() {
        if byte >= byte_col {
            break;
        }
        col += cell_width(ch, col, tabstop);
    }
    col
}

/// Find the character covering a desired screen column. Normal-mode positions
/// stop on the last character of short lines, never on a UTF-8 continuation byte.
pub fn byte_col(text: &str, target: usize, tabstop: usize) -> usize {
    let mut col = 0;
    let mut last = 0;
    for (byte, ch) in text.char_indices() {
        let width = cell_width(ch, col, tabstop);
        if width == 0 {
            continue;
        }
        last = byte;
        if target < col + width {
            return byte;
        }
        col += width;
    }
    last
}
