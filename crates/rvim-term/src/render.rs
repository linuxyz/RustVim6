//! Screen layout: viewport of buffer lines, statusline, command line, message
//! line and wildmenu (reference: `screen.c` `update_screen()`, `draw_status_line()`,
//! `showmode()`, and `cmdline.c` for the prompt line).

use crate::grid::Grid;
use crate::style::{Color, Style};
use crate::width::char_width;

/// Command-line row contents (`:` or `/` prompt).
#[derive(Clone, Copy, Debug)]
pub struct CmdlineView<'a> {
    pub prompt: char,
    pub text: &'a str,
    /// Cursor position inside `text`, as a char index.
    pub cursor: usize,
}

/// Wildmenu row contents: tab-completion candidates above the command line.
#[derive(Clone, Copy, Debug)]
pub struct WildmenuView<'a> {
    pub items: &'a [String],
    pub selected: usize,
}

/// Everything the renderer needs for one frame.
pub struct ScreenModel<'a> {
    /// Buffer text, one entry per line.
    pub lines: &'a [String],
    /// Cursor as (line index, byte column).
    pub cursor: (usize, usize),
    /// First visible line.
    pub top: usize,
    /// Horizontal scroll offset in display cells.
    pub left: usize,
    /// Statusline text (already formatted by the caller).
    pub status: &'a str,
    /// Message to show on the last row (empty for none).
    pub message: &'a str,
    /// Command line, when one is being edited.
    pub cmdline: Option<CmdlineView<'a>>,
    /// Wildmenu, when completion is cycling.
    pub wildmenu: Option<WildmenuView<'a>>,
    /// Show `~` for lines past the end of the buffer (vim's default).
    pub show_eob: bool,
    /// 'number': draw a line-number gutter.
    pub number: bool,
    /// 'tabstop': cells a <Tab> advances to (vim default 8).
    pub tabstop: usize,
    /// More output is queued: append vim's `-- More --` prompt.
    pub more: bool,
}

/// Styles used by the layout; a later milestone maps them to highlight groups.
pub struct Theme {
    pub text: Style,
    pub status: Style,
    pub cmdline: Style,
    pub message: Style,
    pub wildmenu: Style,
    pub wildmenu_selected: Style,
    pub eob: Style,
    /// Inactive window status line (M5: highlight group StatusLineNC).
    pub status_nc: Style,
    /// Window separator column (VertSplit).
    pub separator: Style,
    /// Text of a closed fold.
    pub fold: Style,
    /// Diff filler line.
    pub filler: Style,
    /// Visual selection (Visual).
    pub selection: Style,
    /// Popup window background (Pmenu).
    pub popup: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Style::NORMAL,
            status: Style::REVERSE,
            cmdline: Style::NORMAL,
            message: Style::NORMAL,
            wildmenu: Style::NORMAL,
            wildmenu_selected: Style {
                fg: Color::Ansi(0),
                bg: Color::Ansi(6),
                ..Style::NORMAL
            },
            eob: Style {
                fg: Color::Ansi(4),
                ..Style::NORMAL
            },
            status_nc: Style {
                fg: Color::Ansi(0),
                bg: Color::Ansi(7),
                ..Style::NORMAL
            },
            separator: Style {
                fg: Color::Ansi(4),
                ..Style::NORMAL
            },
            fold: Style {
                fg: Color::Ansi(6),
                ..Style::NORMAL
            },
            filler: Style {
                fg: Color::Ansi(4),
                ..Style::NORMAL
            },
            selection: Style::REVERSE,
            popup: Style {
                bg: Color::Ansi(7),
                ..Style::NORMAL
            },
        }
    }
}

/// Width of the 'number' gutter for a buffer of `line_count` lines.
pub fn gutter_width(number: bool, line_count: usize) -> usize {
    if number {
        line_count.max(1).to_string().len() + 1
    } else {
        0
    }
}

/// Expand one buffer line into screen cells (one `char` per cell).
///
/// Vim never writes a buffer byte to the terminal unchanged: a <Tab> advances to
/// the next multiple of 'tabstop' (screen.c), and unprintable characters are
/// shown as `^X` (`transchar()`, so <C-A> prints as `^A` and <Nul> as `^@`).
/// Without this a tab-indented file would emit raw tab bytes and shift every
/// following row out of the grid.
pub fn expand_cells(text: &str, tabstop: usize) -> Vec<char> {
    // Clamp the stride: a bogus value must not make one tab allocate unbounded
    // cells ('tabstop' is validated to 1..=9999 by :set).
    let tabstop = tabstop.clamp(1, 1000);
    let mut cells = Vec::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' => {
                let pad = tabstop - (cells.len() % tabstop);
                cells.resize(cells.len() + pad, ' ');
            }
            '\u{7f}' => {
                cells.push('^');
                cells.push('?');
            }
            c if (c as u32) < 0x20 => {
                cells.push('^');
                cells.push(char::from(b'@' + c as u8));
            }
            c => {
                let width = char_width(c);
                if width == 0 {
                    // Combining marks have no cell of their own; the simple
                    // Grid cell model cannot attach them to the previous glyph.
                    continue;
                }
                cells.push(c);
                if width > 1 {
                    cells.resize(cells.len() + width - 1, '\0');
                }
            }
        }
    }
    cells
}

/// Byte column -> display cell column for line `text`.
///
/// Tabs and control characters occupy more than one cell, so this expands the
/// prefix exactly like [`expand_cells`] does.
pub fn display_col(text: &str, byte_col: usize, tabstop: usize) -> usize {
    let prefix: String = text
        .char_indices()
        .take_while(|(i, _)| *i < byte_col)
        .map(|(_, c)| c)
        .collect();
    expand_cells(&prefix, tabstop).len()
}

/// Render one frame.
///
/// Layout, bottom-up (as in vim): last row is the command line or the message
/// line, the row above is the statusline (or the wildmenu while completing), and
/// everything above shows the buffer.
pub fn render(model: &ScreenModel, cols: usize, rows: usize, theme: &Theme) -> Grid {
    let mut grid = Grid::new(cols, rows);
    if rows == 0 || cols == 0 {
        return grid;
    }
    let bottom = rows - 1;
    let status_row = rows.saturating_sub(2);

    // ---- buffer area ----------------------------------------------------
    let text_rows = rows.saturating_sub(2).max(1);
    let gutter = gutter_width(model.number, model.lines.len());
    let text_cols = cols.saturating_sub(gutter);
    for screen_row in 0..text_rows {
        let line_index = model.top + screen_row;
        if model.number {
            let number = if line_index < model.lines.len() {
                format!("{:>width$} ", line_index + 1, width = gutter - 1)
            } else {
                " ".repeat(gutter)
            };
            grid.put(screen_row, 0, &number, theme.eob);
        }
        if line_index < model.lines.len() {
            // Screen cells, not buffer bytes: tabs become spaces up to
            // 'tabstop' and control characters become ^X.
            let cells = expand_cells(&model.lines[line_index], model.tabstop);
            let visible: String = cells.iter().skip(model.left).take(text_cols).collect();
            grid.put(screen_row, gutter, &visible, theme.text);
        } else if model.show_eob && line_index >= model.lines.len() {
            grid.put(screen_row, gutter, "~", theme.eob);
        }
    }

    // ---- statusline / wildmenu -----------------------------------------
    if rows >= 2 {
        if let Some(menu) = model.wildmenu {
            draw_wildmenu(&mut grid, status_row, cols, &menu, theme);
        } else {
            grid.fill_row(status_row, ' ', theme.status);
            grid.put(status_row, 0, model.status, theme.status);
        }
    }

    // ---- command line or message ---------------------------------------
    match model.cmdline {
        Some(cmdline) => {
            grid.fill_row(bottom, ' ', theme.cmdline);
            let mut text = String::new();
            text.push(cmdline.prompt);
            text.push_str(cmdline.text);
            grid.put(bottom, 0, &text, theme.cmdline);
            // Keep the cursor visible: scroll the text when it overflows.
            // The command-line cursor is a character index; convert the
            // prefix through Unicode display width (CJK is two cells).
            let byte = cmdline
                .text
                .char_indices()
                .nth(cmdline.cursor)
                .map(|(byte, _)| byte)
                .unwrap_or(cmdline.text.len());
            let prefix = format!("{}{}", cmdline.prompt, &cmdline.text[..byte]);
            let cursor_display = expand_cells(&prefix, model.tabstop).len();
            let cursor_col = if cursor_display >= cols {
                cols - 1
            } else {
                cursor_display
            };
            grid.set_cursor(Some((bottom, cursor_col)));
        }
        None => {
            if !model.message.is_empty() {
                let style = if is_error_message(model.message) {
                    Style {
                        fg: Color::Ansi(1),
                        ..Style::NORMAL
                    }
                } else {
                    theme.message
                };
                // Messages can contain control bytes, so they go through the
                // same cell expansion as buffer text.
                let mut text: String = expand_cells(model.message, model.tabstop)
                    .into_iter()
                    .collect();
                if model.more {
                    // vim always marks pending output. When the message fills the
                    // row the marker is right-aligned over its tail, so it stays
                    // visible in a narrow window.
                    let prompt = " -- More --";
                    let prompt_len = prompt.chars().count();
                    if text.chars().count() + prompt_len <= cols {
                        text.push_str(prompt);
                    } else if cols > prompt_len {
                        text = text
                            .chars()
                            .take(cols - prompt_len)
                            .chain(prompt.chars())
                            .collect();
                    }
                }
                grid.put(bottom, 0, &text, style);
            }
            // Cursor inside the buffer area.
            let (line, byte_col) = model.cursor;
            let screen_row = line.saturating_sub(model.top);
            if screen_row < text_rows {
                let text = model.lines.get(line).map(String::as_str).unwrap_or("");
                let col = display_col(text, byte_col, model.tabstop)
                    .saturating_sub(model.left)
                    .saturating_add(gutter);
                grid.set_cursor(Some((screen_row, col.min(cols.saturating_sub(1)))));
            } else {
                grid.set_cursor(None);
            }
        }
    }

    grid
}

/// vim marks error messages by their `E123:`-style prefix.
fn is_error_message(message: &str) -> bool {
    let bytes = message.as_bytes();
    bytes.len() > 2
        && bytes[0] == b'E'
        && bytes[1].is_ascii_digit()
        && (bytes[2].is_ascii_digit() || bytes[2] == b':')
}

fn draw_wildmenu(grid: &mut Grid, row: usize, cols: usize, menu: &WildmenuView, theme: &Theme) {
    grid.fill_row(row, ' ', theme.wildmenu);
    let mut col = 0usize;
    for (index, item) in menu.items.iter().enumerate() {
        let style = if index == menu.selected {
            theme.wildmenu_selected
        } else {
            theme.wildmenu
        };
        let text = format!("{item}  ");
        grid.put(row, col, &text, style);
        col += text.chars().map(char_width).sum::<usize>();
        if col >= cols {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn model<'a>(lines: &'a [String], status: &'a str) -> ScreenModel<'a> {
        ScreenModel {
            lines,
            cursor: (0, 0),
            top: 0,
            left: 0,
            status,
            message: "",
            cmdline: None,
            wildmenu: None,
            show_eob: true,
            number: false,
            tabstop: 8,
            more: false,
        }
    }

    #[test]
    fn buffer_status_and_message_rows() {
        let text = lines(&["alpha", "beta"]);
        let mut m = model(&text, "file.txt 2L");
        m.message = "hello";
        let grid = render(&m, 20, 5, &Theme::default());
        assert_eq!(grid.row_text(0), "alpha");
        assert_eq!(grid.row_text(1), "beta");
        assert_eq!(grid.row_text(2), "~");
        assert_eq!(grid.row_text(3), "file.txt 2L");
        assert_eq!(grid.row_text(4), "hello");
        assert_eq!(grid.cursor(), Some((0, 0)));
    }

    #[test]
    fn top_line_scrolls_the_viewport() {
        let text = lines(&["one", "two", "three", "four"]);
        let mut m = model(&text, "");
        m.top = 2;
        m.cursor = (3, 1);
        let grid = render(&m, 10, 4, &Theme::default());
        assert_eq!(grid.row_text(0), "three");
        assert_eq!(grid.row_text(1), "four");
        assert_eq!(grid.cursor(), Some((1, 1)));
    }

    #[test]
    fn command_line_replaces_the_message_row_and_places_the_cursor() {
        let text = lines(&["x"]);
        let mut m = model(&text, "status");
        m.cmdline = Some(CmdlineView {
            prompt: ':',
            text: "set nu",
            cursor: 4,
        });
        let grid = render(&m, 20, 3, &Theme::default());
        assert_eq!(grid.row_text(2), ":set nu");
        assert_eq!(grid.cursor(), Some((2, 5)), "prompt shifts the cursor");

        m.cmdline = Some(CmdlineView {
            prompt: ':',
            text: "漢字",
            cursor: 1,
        });
        let grid = render(&m, 20, 3, &Theme::default());
        assert_eq!(grid.row_text(2), ":漢字");
        assert_eq!(grid.cursor(), Some((2, 3)), "CJK counts display cells");
    }

    #[test]
    fn search_prompt_is_shown() {
        let text = lines(&["x"]);
        let mut m = model(&text, "");
        m.cmdline = Some(CmdlineView {
            prompt: '/',
            text: "alp",
            cursor: 3,
        });
        let grid = render(&m, 10, 2, &Theme::default());
        assert_eq!(grid.row_text(1), "/alp");
    }

    #[test]
    fn wildmenu_draws_on_the_status_row() {
        let text = lines(&["x"]);
        let items = vec!["set all".to_string(), "set nu".to_string()];
        let mut m = model(&text, "status");
        m.wildmenu = Some(WildmenuView {
            items: &items,
            selected: 1,
        });
        let grid = render(&m, 30, 3, &Theme::default());
        assert!(grid.row_text(1).contains("set all"));
        assert!(grid.row_text(1).contains("set nu"));
    }

    #[test]
    fn error_messages_are_highlighted() {
        let text = lines(&["x"]);
        let mut m = model(&text, "");
        m.message = "E492: Not an editor command";
        let grid = render(&m, 40, 2, &Theme::default());
        let cell = grid.cell(1, 0).unwrap();
        assert_eq!(cell.style.fg, Color::Ansi(1), "errors are red");
    }

    #[test]
    fn display_column_counts_characters_not_bytes() {
        assert_eq!(display_col("abc", 3, 8), 3);
        assert_eq!(display_col("héllo", 3, 8), 2, "é is two bytes, one column");
        assert_eq!(display_col("漢字", 3, 8), 2, "CJK glyphs occupy two cells");
        assert_eq!(display_col("漢字", 6, 8), 4);
        assert_eq!(display_col("a漢b", 1, 8), 1, "Latin still moves one cell");
        assert_eq!(display_col("a漢b", 4, 8), 3, "a + wide CJK = three cells");
        assert_eq!(
            expand_cells("漢x", 8),
            vec!['漢', '\0', 'x'],
            "wide glyphs reserve a continuation cell"
        );
    }

    #[test]
    fn tabs_expand_to_the_next_tabstop() {
        assert_eq!(
            expand_cells("\tabc", 8),
            vec![' '; 8]
                .into_iter()
                .chain("abc".chars())
                .collect::<Vec<_>>()
        );
        assert_eq!(expand_cells("a\tb", 8).len(), 9, "one char plus a full tab");
        assert_eq!(expand_cells("a\tb", 4).len(), 5, "'tabstop' is honoured");
        assert_eq!(display_col("\tab", 1, 8), 8, "cursor after a tab");
        assert_eq!(display_col("a\tb", 2, 4), 4);
    }

    #[test]
    fn control_characters_are_shown_as_caret_pairs() {
        assert_eq!(expand_cells("\u{1}x", 8), vec!['^', 'A', 'x']);
        assert_eq!(expand_cells("\u{7f}", 8), vec!['^', '?']);
        assert_eq!(expand_cells("\u{0}", 8), vec!['^', '@']);
        // A newline inside a message is a control character too, not a row break.
        assert_eq!(expand_cells("a\nb", 8), vec!['a', '^', 'J', 'b']);
    }

    #[test]
    fn tabbed_lines_render_as_spaces() {
        let text = lines(&["\tabc"]);
        let mut m = model(&text, "");
        m.cursor = (0, 1);
        let grid = render(&m, 20, 3, &Theme::default());
        assert_eq!(grid.row_text(0), "        abc");
        assert_eq!(grid.cursor(), Some((0, 8)), "cursor sits after the tab");
    }

    #[test]
    fn number_gutter_width_follows_the_line_count() {
        assert_eq!(gutter_width(false, 100), 0);
        assert_eq!(gutter_width(true, 9), 2);
        assert_eq!(gutter_width(true, 10), 3);
        assert_eq!(gutter_width(true, 0), 2);
    }

    #[test]
    fn more_prompt_is_appended_to_the_message() {
        let text = lines(&["x"]);
        let mut m = model(&text, "");
        m.message = "one";
        m.more = true;
        let grid = render(&m, 40, 2, &Theme::default());
        assert_eq!(grid.row_text(1), "one -- More --");
    }

    #[test]
    fn more_prompt_stays_visible_on_a_narrow_row() {
        let text = lines(&["x"]);
        let mut m = model(&text, "");
        m.message = "E492: Not an editor command: bogusone";
        m.more = true;
        let grid = render(&m, 24, 2, &Theme::default());
        let row = grid.row_text(1);
        assert_eq!(row.chars().count(), 24);
        assert!(row.ends_with(" -- More --"), "marker survives: {row:?}");
    }
}
