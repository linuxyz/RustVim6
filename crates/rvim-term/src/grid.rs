//! Logical screen: a grid of styled cells plus ANSI painting (reference: `Grid`
//! in vim's `screen.c`, `update_screen()`/`screen_line()`).

use crate::style::Style;
use crate::width::char_width;

/// One character cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
    /// Second cell of a double-width character; skipped when painting.
    pub continuation: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::NORMAL,
            continuation: false,
        }
    }
}

/// A full screen of cells, independent of any terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    /// Cursor position as (row, col), 0-based; `None` hides the cursor.
    cursor: Option<(usize, usize)>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        assert!(cols > 0 && rows > 0, "a screen needs at least one cell");
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            cursor: Some((0, 0)),
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.cursor
    }

    pub fn set_cursor(&mut self, pos: Option<(usize, usize)>) {
        self.cursor = pos;
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if row < self.rows && col < self.cols {
            Some(&self.cells[row * self.cols + col])
        } else {
            None
        }
    }

    /// Write `text` starting at (row, col); clipped at the right edge.
    pub fn put(&mut self, row: usize, col: usize, text: &str, style: Style) {
        if row >= self.rows {
            return;
        }
        let mut x = col;
        let mut first = true;
        for ch in text.chars() {
            if ch == '\0' {
                // Continuation cell already placed by the preceding wide glyph.
                // If a horizontal slice starts on one, leave its first half blank.
                if first && x < self.cols {
                    x += 1;
                }
                first = false;
                continue;
            }
            let width = char_width(ch);
            if width == 0 {
                continue;
            }
            if x >= self.cols {
                break;
            }
            if x + width > self.cols {
                self.cells[row * self.cols + x] = Cell {
                    ch: ' ',
                    style,
                    continuation: false,
                };
                x += width;
                first = false;
                continue;
            }
            self.cells[row * self.cols + x] = Cell {
                ch,
                style,
                continuation: false,
            };
            for offset in 1..width {
                self.cells[row * self.cols + x + offset] = Cell {
                    ch: '\0',
                    style,
                    continuation: true,
                };
            }
            x += width;
            first = false;
        }
    }

    /// Re-style the cells [from, to) of a row, leaving their characters alone
    /// (used by syntax highlighting, which paints over the text already drawn).
    pub fn set_style(&mut self, row: usize, from: usize, to: usize, style: Style) {
        if row >= self.rows {
            return;
        }
        let start = from.min(self.cols);
        let end = to.min(self.cols);
        for col in start..end {
            self.cells[row * self.cols + col].style = style;
        }
    }

    /// Fill a whole row with `ch`.
    pub fn fill_row(&mut self, row: usize, ch: char, style: Style) {
        if row >= self.rows {
            return;
        }
        for col in 0..self.cols {
            self.cells[row * self.cols + col] = Cell {
                ch,
                style,
                continuation: false,
            };
        }
    }

    /// Row contents with trailing blanks trimmed (the form tests compare).
    pub fn row_text(&self, row: usize) -> String {
        if row >= self.rows {
            return String::new();
        }
        let text: String = self.cells[row * self.cols..(row + 1) * self.cols]
            .iter()
            .filter(|cell| !cell.continuation)
            .map(|cell| cell.ch)
            .collect();
        text.trim_end().to_string()
    }

    /// Whole screen as plain text, one line per row.
    pub fn text(&self) -> String {
        (0..self.rows)
            .map(|r| self.row_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Full repaint: home the cursor and emit every row's characters.
    pub fn to_ansi(&self) -> String {
        let mut out = String::from("\u{1b}[H");
        for row in 0..self.rows {
            out.push_str(&self.row_ansi(row));
            if row + 1 < self.rows {
                out.push_str("\r\n");
            }
        }
        out.push_str("\u{1b}[0m");
        if let Some((row, col)) = self.cursor {
            out.push_str(&format!("\u{1b}[{};{}H", row + 1, col + 1));
        } else {
            out.push_str("\u{1b}[?25l");
        }
        out
    }

    /// Repaint of only the rows that differ from `previous` (dirty regions).
    pub fn diff_ansi(&self, previous: &Grid) -> String {
        let mut out = String::new();
        if self.cols != previous.cols || self.rows != previous.rows {
            return self.to_ansi();
        }
        for row in 0..self.rows {
            let same = (0..self.cols).all(|col| self.cell(row, col) == previous.cell(row, col));
            if same {
                continue;
            }
            out.push_str(&format!("\u{1b}[{};1H", row + 1));
            out.push_str(&self.row_ansi(row));
        }
        out.push_str("\u{1b}[0m");
        if let Some((row, col)) = self.cursor {
            out.push_str(&format!("\u{1b}[{};{}H", row + 1, col + 1));
        }
        out
    }

    fn row_ansi(&self, row: usize) -> String {
        let mut out = String::new();
        let mut style = Style::NORMAL;
        let mut painted = false;
        for col in 0..self.cols {
            let Some(cell) = self.cell(row, col) else {
                break;
            };
            if cell.continuation {
                continue;
            }
            if cell.style != style {
                out.push_str(&cell.style.sgr());
                style = cell.style;
                painted = true;
            }
            out.push(cell.ch);
        }
        if painted {
            out.push_str("\u{1b}[0m");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Color;

    #[test]
    fn writing_text_clips_at_the_right_edge() {
        let mut grid = Grid::new(5, 2);
        grid.put(0, 0, "abcdef", Style::NORMAL);
        assert_eq!(grid.row_text(0), "abcde");
        assert_eq!(grid.row_text(1), "");
    }

    #[test]
    fn wide_glyphs_use_a_continuation_cell() {
        let mut grid = Grid::new(6, 1);
        grid.put(0, 0, "漢字x", Style::NORMAL);
        assert_eq!(grid.row_text(0), "漢字x");
        assert_eq!(grid.cell(0, 0).unwrap().ch, '漢');
        assert!(grid.cell(0, 1).unwrap().continuation);
        assert_eq!(grid.cell(0, 2).unwrap().ch, '字');
        assert!(grid.cell(0, 3).unwrap().continuation);
        assert_eq!(grid.cell(0, 4).unwrap().ch, 'x');
        assert!(!grid.row_ansi(0).contains('\0'));
    }

    #[test]
    fn a_wide_glyph_at_the_right_edge_becomes_a_space() {
        let mut grid = Grid::new(2, 1);
        grid.put(0, 1, "漢", Style::NORMAL);
        assert_eq!(grid.cell(0, 1).unwrap().ch, ' ');
        assert_eq!(grid.row_text(0), "");
    }

    #[test]
    fn text_dump_has_one_line_per_row() {
        let mut grid = Grid::new(6, 3);
        grid.put(0, 0, "hi", Style::NORMAL);
        grid.put(2, 0, "bye", Style::NORMAL);
        assert_eq!(grid.text(), "hi\n\nbye");
    }

    #[test]
    fn ansi_repaint_positions_cursor_and_resets_style() {
        let mut grid = Grid::new(3, 1);
        grid.put(0, 0, "abc", Style::NORMAL);
        grid.set_cursor(Some((0, 1)));
        let ansi = grid.to_ansi();
        assert!(ansi.starts_with("\u{1b}[H"));
        assert!(ansi.contains("abc"));
        assert!(ansi.ends_with("\u{1b}[1;2H"));
    }

    #[test]
    fn diff_only_touches_changed_rows() {
        let mut before = Grid::new(4, 3);
        before.put(0, 0, "one", Style::NORMAL);
        before.put(1, 0, "two", Style::NORMAL);
        let mut after = before.clone();
        after.put(1, 0, "TWO", Style::NORMAL);
        let diff = after.diff_ansi(&before);
        assert!(diff.contains("TWO"), "changed row repainted");
        assert!(!diff.contains("one"), "unchanged row untouched");

        let identical = after.diff_ansi(&after);
        assert!(!identical.contains("TWO"), "nothing to repaint");
    }

    #[test]
    fn styles_are_encoded_per_run() {
        let mut grid = Grid::new(4, 1);
        grid.put(0, 0, "a", Style::NORMAL);
        grid.put(
            0,
            1,
            "b",
            Style {
                fg: Color::Ansi(1),
                ..Style::NORMAL
            },
        );
        let ansi = grid.row_ansi(0);
        assert!(ansi.contains("\u{1b}[0;31m"), "red run: {ansi:?}");
    }
}
