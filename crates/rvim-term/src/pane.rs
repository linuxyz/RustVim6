//! Multi-window screen composition (reference: screen.c draw_tabline /
//! win_draw_end, drawline.c).
//!
//! The single-window layout in [`crate::render`] covers the common case; this
//! module composes an arbitrary set of window panes into one [`Grid`]:
//!
//! * every pane owns a rectangle, a status row and a list of [`RowView`] rows
//!   (buffer text, a folded summary, a diff filler or the end-of-buffer `~`);
//! * panes side by side are separated by a `|` column (vim`s VertSplit), panes
//!   stacked by their own status lines;
//! * the tabline (`showtabline`) occupies the first row when several tabpages
//!   exist, and the command line / message row are the last one.

use crate::grid::Grid;
use crate::render::{Theme, expand_cells};
use crate::style::Style;

/// One content row of a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowView {
    /// Buffer line `line` (zero-based) with its text.
    Text { line: usize, text: String },
    /// A closed fold covering `start..=end` with its rendered summary line.
    Fold {
        start: usize,
        end: usize,
        label: String,
    },
    /// A diff filler row (`diffopt` filler).
    Filler(String),
    /// Past the end of the buffer: vim draws `~`.
    Eob,
}

impl RowView {
    /// Buffer line this row shows, if any.
    pub fn line(&self) -> Option<usize> {
        match self {
            RowView::Text { line, .. } | RowView::Fold { start: line, .. } => Some(*line),
            RowView::Filler(_) | RowView::Eob => None,
        }
    }
}

/// The visual selection to highlight, in *pane* row/column cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionView {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
    /// Blockwise selection: the highlighted cells form a rectangle.
    pub block: bool,
}

/// One window on the screen.
#[derive(Clone, Debug, Default)]
pub struct PaneView {
    /// Top row of the frame (the status line is the last row of the frame).
    pub row: usize,
    pub col: usize,
    /// Total rows including the status line.
    pub rows: usize,
    pub cols: usize,
    /// Content rows; at most `rows - 1` entries are drawn.
    pub content: Vec<RowView>,
    /// Cursor as (content row, display cell column).
    pub cursor: Option<(usize, usize)>,
    /// Horizontal scroll in display cells.
    pub left: usize,
    /// Status line text.
    pub status: String,
    /// Width of the line-number field including its trailing space (0 = off).
    pub number_width: usize,
    /// `'foldcolumn'` width in cells (0 when off).
    pub foldcolumn: usize,
    /// Sign column width in cells (0 when off).
    pub signcolumn: usize,
    /// Sign text per content row.
    pub signs: Vec<String>,
    /// Fold column text per content row.
    pub foldcol: Vec<String>,
    pub tabstop: usize,
    pub selection: Option<SelectionView>,
    /// True for the current window: reverse-video status line and the cursor.
    pub active: bool,
}

impl PaneView {
    fn text_rows(&self) -> usize {
        self.rows.saturating_sub(1)
    }

    /// Cells between the frame edge and the buffer text.
    fn gutter(&self) -> usize {
        self.number_width + self.foldcolumn + self.signcolumn
    }
}

/// A popup overlay rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PopupRect {
    pub row: usize,
    pub col: usize,
    pub rows: usize,
    pub cols: usize,
}

/// Command-line row contents.
#[derive(Clone, Debug, Default)]
pub struct CmdlineText {
    pub prompt: char,
    pub text: String,
    /// Cursor position inside `text`, as a char index.
    pub cursor: usize,
}

/// Wildmenu row contents.
#[derive(Clone, Debug, Default)]
pub struct WildmenuItems {
    pub items: Vec<String>,
    pub selected: usize,
}

/// Everything needed to paint one frame with several windows.
#[derive(Clone, Debug, Default)]
pub struct ScreenLayout {
    pub panes: Vec<PaneView>,
    /// Tabline text (`None` when only one tabpage exists).
    pub tabline: Option<String>,
    pub message: String,
    pub cmdline: Option<CmdlineText>,
    pub wildmenu: Option<WildmenuItems>,
    /// Queued message output: append vim's `-- More --` marker.
    pub more: bool,
    /// 'showcmd' text: the (partial) command being typed, drawn in the last
    /// right-hand columns of the command line row (normal.c display_showcmd).
    pub showcmd: String,
    pub tabstop: usize,
    /// Overlay popups: (lines, rectangle) drawn on top of the panes.
    pub popups: Vec<(Vec<String>, PopupRect)>,
}

/// Render a full frame.
pub fn render_layout(model: &ScreenLayout, cols: usize, rows: usize, theme: &Theme) -> Grid {
    let mut grid = Grid::new(cols.max(1), rows.max(1));
    if rows == 0 || cols == 0 {
        return grid;
    }

    // ---- tabline --------------------------------------------------------
    let mut top = 0usize;
    if let Some(tabline) = &model.tabline {
        grid.fill_row(0, ' ', theme.status);
        grid.put(0, 0, tabline, theme.status);
        top = 1;
    }

    // ---- panes ----------------------------------------------------------
    for pane in &model.panes {
        draw_pane(&mut grid, pane, top, rows, cols, theme);
    }

    // ---- separators -----------------------------------------------------
    for pane in &model.panes {
        if pane.col == 0 {
            continue;
        }
        let col = pane.col - 1;
        for row in pane.row..(pane.row + pane.rows).min(rows.saturating_sub(top)) {
            grid.put(row + top, col, "|", theme.separator);
        }
    }

    // ---- popups ---------------------------------------------------------
    for (lines, rect) in &model.popups {
        for (i, text) in lines.iter().enumerate() {
            let row = rect.row + i + top;
            if row >= rows.saturating_sub(1) {
                break;
            }
            let cols_here = rect.cols.min(cols.saturating_sub(rect.col));
            fill_segment(&mut grid, row, rect.col, cols_here, ' ', theme.popup);
            let clipped: String = text.chars().take(cols_here).collect();
            grid.put(
                row,
                rect.col.min(cols.saturating_sub(1)),
                &clipped,
                theme.popup,
            );
        }
    }

    // ---- wildmenu / command line / message ------------------------------
    let bottom = rows - 1;
    let status_row = rows.saturating_sub(2);
    if rows >= 2 {
        if let Some(menu) = &model.wildmenu {
            grid.fill_row(status_row, ' ', theme.wildmenu);
            let mut col = 0usize;
            for (index, item) in menu.items.iter().enumerate() {
                let style = if index == menu.selected {
                    theme.wildmenu_selected
                } else {
                    theme.wildmenu
                };
                let text = format!("{item}  ");
                grid.put(status_row, col, &text, style);
                col += text.chars().count();
                if col >= cols {
                    break;
                }
            }
        }
    }

    match &model.cmdline {
        Some(cmdline) => {
            grid.fill_row(bottom, ' ', theme.cmdline);
            let text = format!("{}{}", cmdline.prompt, cmdline.text);
            grid.put(bottom, 0, &text, theme.cmdline);
            let cursor_display = cmdline.cursor + 1;
            grid.set_cursor(Some((bottom, cursor_display.min(cols.saturating_sub(1)))));
        }
        None => {
            if !model.message.is_empty() {
                let style = if is_error_message(&model.message) {
                    Style {
                        fg: crate::style::Color::Ansi(1),
                        ..Style::NORMAL
                    }
                } else {
                    theme.message
                };
                let mut text: String = expand_cells(&model.message, model.tabstop)
                    .into_iter()
                    .collect();
                if model.more {
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
        }
    }

    // ---- showcmd --------------------------------------------------------
    // vim writes it at Columns - (SHOWCMD_COLS + 1) on the last row
    // (screen.c comp_col(), normal.c display_showcmd()): a 10 column field plus
    // one separating column at the right edge, left aligned inside the field.
    if model.cmdline.is_none() && !model.showcmd.is_empty() && cols > SHOWCMD_GAP {
        let start = cols - SHOWCMD_GAP;
        let field = cols - start;
        // A longer command keeps only its tail (normal.c add_to_showcmd()).
        let chars: Vec<char> = model.showcmd.chars().collect();
        let skip = chars.len().saturating_sub(field);
        let text: String = chars[skip..].iter().collect();
        fill_segment(&mut grid, bottom, start, field, ' ', theme.message);
        grid.put(bottom, start, &text, theme.message);
    }

    grid
}

/// Columns reserved for 'showcmd' (vim.h SHOWCMD_COLS).
pub const SHOWCMD_COLS: usize = 10;
/// Width taken from the right edge: the field plus its separating column.
pub const SHOWCMD_GAP: usize = SHOWCMD_COLS + 1;

fn draw_pane(
    grid: &mut Grid,
    pane: &PaneView,
    top: usize,
    rows: usize,
    cols: usize,
    theme: &Theme,
) {
    let row = pane.row + top;
    if row >= rows {
        return;
    }
    let text_rows = pane.text_rows().min(rows.saturating_sub(row + 1));
    let gutter = pane.gutter();
    let text_width = pane.cols.saturating_sub(gutter);

    for index in 0..text_rows {
        let screen_row = row + index;
        let mut gutter_text = String::new();
        if pane.signcolumn > 0 {
            let sign = pane.signs.get(index).map(String::as_str).unwrap_or("");
            gutter_text.push_str(&format!("{:>width$}", sign, width = pane.signcolumn));
        }
        if pane.foldcolumn > 0 {
            let fold = pane.foldcol.get(index).map(String::as_str).unwrap_or("");
            gutter_text.push_str(&format!("{:>width$}", fold, width = pane.foldcolumn));
        }
        if pane.number_width > 0 {
            let label = match pane.content.get(index) {
                Some(RowView::Text { line, .. }) | Some(RowView::Fold { start: line, .. }) => {
                    format!("{:>width$} ", line + 1, width = pane.number_width - 1)
                }
                _ => " ".repeat(pane.number_width),
            };
            gutter_text.push_str(&label);
        }
        if !gutter_text.is_empty() {
            grid.put(screen_row, pane.col, &gutter_text, theme.eob);
        }

        let Some(entry) = pane.content.get(index) else {
            continue;
        };
        let style = match entry {
            RowView::Eob => theme.eob,
            RowView::Fold { .. } => theme.fold,
            RowView::Filler(_) => theme.filler,
            RowView::Text { .. } => theme.text,
        };
        let text = match entry {
            RowView::Text { text, .. } => text.as_str(),
            RowView::Fold { label, .. } => label.as_str(),
            RowView::Filler(text) => text.as_str(),
            RowView::Eob => "~",
        };
        let cells = expand_cells(text, pane.tabstop);
        if pane.left < cells.len() {
            let visible: String = cells.iter().skip(pane.left).take(text_width).collect();
            grid.put(screen_row, pane.col + gutter, &visible, style);
        }
    }

    // ---- visual selection ------------------------------------------------
    if let Some(sel) = pane.selection {
        let style = theme.selection;
        let (first_row, last_row) = if sel.start_row <= sel.end_row {
            (sel.start_row, sel.end_row)
        } else {
            (sel.end_row, sel.start_row)
        };
        for index in first_row..=last_row.min(text_rows.saturating_sub(1)) {
            let screen_row = row + index;
            let (from, to) = if sel.block || first_row == last_row {
                (
                    sel.start_col.min(sel.end_col),
                    sel.start_col.max(sel.end_col),
                )
            } else if index == sel.start_row {
                (sel.start_col, text_width.saturating_sub(1))
            } else if index == sel.end_row {
                (0, sel.end_col)
            } else {
                (0, text_width.saturating_sub(1))
            };
            let start = pane.col + gutter + from.min(text_width);
            let end = pane.col + gutter + to.min(text_width.saturating_sub(1));
            for col in start..=end.min(cols.saturating_sub(1)) {
                if let Some(cell) = grid.cell(screen_row, col).copied() {
                    grid.put(screen_row, col, &cell.ch.to_string(), style);
                }
            }
        }
    }

    // ---- status line -----------------------------------------------------
    let status_row = row + pane.rows.saturating_sub(1);
    if status_row < rows.saturating_sub(1) {
        let style = if pane.active {
            theme.status
        } else {
            theme.status_nc
        };
        fill_segment(grid, status_row, pane.col, pane.cols, ' ', style);
        grid.put(status_row, pane.col, &pane.status, style);
    }

    // ---- cursor ----------------------------------------------------------
    if pane.active {
        match pane.cursor {
            Some((index, col)) if index < text_rows => {
                let screen_row = row + index;
                let screen_col =
                    (pane.col + gutter + col.saturating_sub(pane.left)).min(cols.saturating_sub(1));
                grid.set_cursor(Some((screen_row, screen_col)));
            }
            _ => grid.set_cursor(None),
        }
    }
}

/// Fill one horizontal segment of a row without touching the neighbouring
/// window's cells (a status line or popup only owns its own columns).
fn fill_segment(grid: &mut Grid, row: usize, col: usize, width: usize, ch: char, style: Style) {
    for offset in 0..width {
        grid.put(row, col + offset, &ch.to_string(), style);
    }
}

/// vim marks error messages by their `E123:`-style prefix.
fn is_error_message(message: &str) -> bool {
    let bytes = message.as_bytes();
    bytes.len() > 2
        && bytes[0] == b'E'
        && bytes[1].is_ascii_digit()
        && (bytes[2].is_ascii_digit() || bytes[2] == b':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Theme;

    fn text_pane(row: usize, rows: usize, cols: usize) -> PaneView {
        PaneView {
            row,
            col: 0,
            rows,
            cols,
            content: vec![
                RowView::Text {
                    line: 0,
                    text: "alpha".to_string(),
                },
                RowView::Text {
                    line: 1,
                    text: "beta".to_string(),
                },
            ],
            cursor: Some((0, 0)),
            left: 0,
            status: "file.txt".to_string(),
            number_width: 0,
            foldcolumn: 0,
            signcolumn: 0,
            signs: Vec::new(),
            foldcol: Vec::new(),
            tabstop: 8,
            selection: None,
            active: true,
        }
    }

    #[test]
    fn one_pane_draws_text_status_and_message() {
        let model = ScreenLayout {
            panes: vec![text_pane(0, 4, 10)],
            message: "hi".to_string(),
            tabstop: 8,
            ..Default::default()
        };
        let grid = render_layout(&model, 10, 5, &Theme::default());
        assert_eq!(grid.row_text(0), "alpha");
        assert_eq!(grid.row_text(1), "beta");
        assert_eq!(grid.row_text(3), "file.txt");
        assert_eq!(grid.row_text(4), "hi");
        assert_eq!(grid.cursor(), Some((0, 0)));
    }

    #[test]
    fn two_stacked_panes_each_get_a_status_line() {
        let mut top = text_pane(0, 2, 10);
        top.status = "top".to_string();
        let mut bottom = text_pane(2, 3, 10);
        bottom.status = "bottom".to_string();
        bottom.active = false;
        let model = ScreenLayout {
            panes: vec![top, bottom],
            tabstop: 8,
            ..Default::default()
        };
        let grid = render_layout(&model, 10, 6, &Theme::default());
        assert_eq!(grid.row_text(0), "alpha");
        assert_eq!(grid.row_text(1), "top");
        assert_eq!(grid.row_text(2), "alpha");
        assert_eq!(grid.row_text(4), "bottom");
    }

    #[test]
    fn vertical_panes_are_separated_by_a_column() {
        let mut left = text_pane(0, 4, 9);
        left.status = "left".to_string();
        let mut right = text_pane(0, 4, 10);
        right.col = 10;
        right.status = "right".to_string();
        right.active = true;
        let model = ScreenLayout {
            panes: vec![left, right],
            tabstop: 8,
            ..Default::default()
        };
        let grid = render_layout(&model, 20, 5, &Theme::default());
        assert_eq!(grid.cell(0, 9).unwrap().ch, '|', "separator column");
        assert_eq!(grid.row_text(0), "alpha    |alpha");
        assert_eq!(grid.row_text(3), "left     |right");
    }

    #[test]
    fn selection_is_reversed_over_the_selected_cells() {
        let mut pane = text_pane(0, 4, 12);
        pane.selection = Some(SelectionView {
            start_row: 0,
            start_col: 1,
            end_row: 0,
            end_col: 3,
            block: false,
        });
        let model = ScreenLayout {
            panes: vec![pane],
            tabstop: 8,
            ..Default::default()
        };
        let grid = render_layout(&model, 12, 5, &Theme::default());
        assert_eq!(grid.row_text(0), "alpha");
        assert!(grid.cell(0, 0).unwrap().style != grid.cell(0, 1).unwrap().style);
        assert_eq!(grid.cell(0, 1).unwrap().style, Theme::default().selection);
        assert_eq!(grid.cell(0, 3).unwrap().style, Theme::default().selection);
        assert_ne!(grid.cell(0, 4).unwrap().style, Theme::default().selection);
    }

    #[test]
    fn folded_rows_show_their_label_and_the_number_of_lines() {
        let mut pane = text_pane(0, 4, 20);
        pane.content = vec![
            RowView::Fold {
                start: 0,
                end: 4,
                label: "+--  5 lines: alpha".to_string(),
            },
            RowView::Text {
                line: 5,
                text: "omega".to_string(),
            },
        ];
        let model = ScreenLayout {
            panes: vec![pane],
            tabstop: 8,
            ..Default::default()
        };
        let grid = render_layout(&model, 20, 5, &Theme::default());
        assert_eq!(grid.row_text(0), "+--  5 lines: alpha");
        assert_eq!(grid.row_text(1), "omega");
    }

    #[test]
    fn tabline_pushes_the_panes_down_one_row() {
        let model = ScreenLayout {
            panes: vec![text_pane(0, 4, 10)],
            tabline: Some(" 1:a.txt  2:b.txt".to_string()),
            tabstop: 8,
            ..Default::default()
        };
        let grid = render_layout(&model, 20, 6, &Theme::default());
        assert_eq!(grid.row_text(0), " 1:a.txt  2:b.txt");
        assert_eq!(grid.row_text(1), "alpha");
        assert_eq!(grid.cursor(), Some((1, 0)));
    }

    #[test]
    fn popups_are_drawn_over_the_panes() {
        let model = ScreenLayout {
            panes: vec![text_pane(0, 4, 10)],
            popups: vec![(
                vec!["popup line".to_string()],
                PopupRect {
                    row: 1,
                    col: 0,
                    rows: 1,
                    cols: 10,
                },
            )],
            tabstop: 8,
            ..Default::default()
        };
        let grid = render_layout(&model, 10, 5, &Theme::default());
        assert_eq!(grid.row_text(1), "popup line");
    }

    #[test]
    fn sign_and_fold_columns_precede_the_text() {
        let mut pane = text_pane(0, 4, 14);
        pane.signcolumn = 2;
        pane.signs = vec!["> ".to_string()];
        pane.foldcolumn = 1;
        pane.foldcol = vec!["+".to_string()];
        let model = ScreenLayout {
            panes: vec![pane],
            tabstop: 8,
            ..Default::default()
        };
        let grid = render_layout(&model, 14, 5, &Theme::default());
        assert_eq!(grid.row_text(0), "> +alpha");
    }
}
