//! UTF-8 text helpers. Vim counts columns by character, stores byte offsets;
//! these utilities bridge the two without pulling in any third-party crate.

/// Number of Unicode scalar values (characters) in a UTF-8 string.
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Convert a zero-based *character* column to a zero-based byte offset.
/// Clamps to the line length when `char_col` exceeds the character count.
pub fn char_col_to_byte(s: &str, char_col: usize) -> usize {
    let mut remaining = char_col;
    for (idx, ch) in s.char_indices() {
        if remaining == 0 {
            return idx;
        }
        remaining -= 1;
        let _ = ch;
    }
    s.len()
}

/// Snap a byte offset to the start of the character containing it.
///
/// Buffer positions (`Pos.col`) are byte offsets (`position.rs`), but a byte
/// offset may point inside a multi-byte UTF-8 sequence when it was produced by
/// an invalid motion or a stale value. Rust strings cannot be indexed at such
/// an offset, so every text mutation snaps back to a char boundary first
/// (Vim's own byte columns are always on a character start because its
/// `inc_cursor`/`dec_cursor` use `mb_ptr2len`).
pub fn snap_byte(s: &str, byte: usize) -> usize {
    let mut b = byte.min(s.len());
    while b > 0 && !s.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// Byte offset of the first byte of the last character in `s`, or 0 when the
/// string is empty. Used to leave the cursor on the last inserted character.
pub fn last_char_offset(s: &str) -> usize {
    match s.char_indices().last() {
        Some((idx, _)) => idx,
        None => 0,
    }
}

/// Convert a zero-based byte offset to a zero-based character column.
/// An offset inside a multi-byte sequence snaps back to the char boundary.
pub fn byte_to_char_col(s: &str, byte: usize) -> usize {
    let mut b = byte.min(s.len());
    // Snap an offset inside a multi-byte sequence back to its start.
    while b > 0 && !s.is_char_boundary(b) {
        b -= 1;
    }
    s[..b].chars().count()
}

/// Return the byte offset of the character `n` positions to the right,
/// clamped at end-of-line.
pub fn move_chars(s: &str, byte: usize, n: isize) -> usize {
    let chars: Vec<usize> = s.char_indices().map(|(i, _)| i).chain([s.len()]).collect();
    let cur = byte_to_char_col(s, byte);
    let target = (cur as isize + n).clamp(0, (chars.len() - 1) as isize) as usize;
    chars[target]
}

/// Split text into logical lines, remembering whether each line had a newline.
/// Mirrors Vim's file reading: a trailing newline does not create an empty
/// extra line, while `"a\nb"` yields two lines.
pub fn split_lines(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            out.push((text[start..i].to_string(), true));
            start = i + 1;
        }
        i += 1;
    }
    if start < text.len() || out.is_empty() {
        out.push((text[start..].to_string(), false));
    }
    out
}

/// Inverse of [split_lines]: join lines honouring their per-line newline flag.
pub fn join_lines(lines: &[(String, bool)]) -> String {
    let mut out = String::new();
    for (text, has_nl) in lines {
        out.push_str(text);
        if *has_nl {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_and_byte_conversion() {
        let s = "abéx"; // a,b,é(2 bytes),x
        assert_eq!(char_len(s), 4);
        assert_eq!(char_col_to_byte(s, 2), 2); // e-acute begins at byte 2
        assert_eq!(byte_to_char_col(s, 3), 2); // byte 3 inside e-acute -> col 2
        assert_eq!(move_chars(s, 0, 2), 2);
        assert_eq!(move_chars(s, s.len(), -1), 4);
    }

    #[test]
    fn byte_snapping_and_last_char() {
        let s = "あb"; // あ = 3 bytes at offsets 0..3, b at 3
        assert_eq!(snap_byte(s, 1), 0); // inside あ -> back to its start
        assert_eq!(snap_byte(s, 3), 3); // already on the boundary
        assert!(snap_byte(s, 99) <= s.len());
        assert_eq!(last_char_offset("abc"), 2);
        assert_eq!(last_char_offset("あb"), 3);
        assert_eq!(last_char_offset(""), 0);
    }

    #[test]
    fn line_splitting() {
        assert_eq!(
            split_lines(
                "a
b
c"
            ),
            vec![("a".into(), true), ("b".into(), true), ("c".into(), false)]
        );
        assert_eq!(
            split_lines(
                "a
b
"
            ),
            vec![("a".into(), true), ("b".into(), true)]
        );
        assert_eq!(split_lines(""), vec![("".into(), false)]);
        // A lone newline is one empty line in Vim's model.
        assert_eq!(split_lines("\n"), vec![("".into(), true)]);
    }

    #[test]
    fn join_roundtrip() {
        let l = split_lines(
            "one
two
three",
        );
        assert_eq!(
            join_lines(&l),
            "one
two
three"
        );
    }
}
