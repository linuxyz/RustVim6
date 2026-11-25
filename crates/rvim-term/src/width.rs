//! Character display width shared by the grid, renderer and GUI.

use unicode_width::UnicodeWidthChar;

/// Width of one Unicode scalar in terminal cells (0 for combining marks, 2 for
/// East Asian wide characters and most emoji).
pub fn char_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(1)
}
