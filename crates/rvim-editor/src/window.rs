//! Windows, tabpages and the split layout tree (reference: `window.c`).
//!
//! Vim keeps one frame tree per tabpage. A frame is either a leaf (one window)
//! or a row/column of child frames:
//!
//! * [`Layout::Row`] holds windows side by side, which is what `:vsplit` and
//!   `<C-w>v` create; the frames are drawn left to right separated by a single
//!   column of `&#124;`.
//! * [`Layout::Col`] holds windows stacked on top of each other, which is what
//!   `:split` and `<C-w>s` create; every window carries its own status line, so
//!   each frame is one row taller than its buffer area.
//!
//! The manager here is deliberately UI free: it owns window objects (cursor,
//! viewport, window-local options) and computes screen rectangles for a given
//! terminal size, but never touches a terminal.

use std::collections::HashMap;

use rvim_core::{Pos, VimError, VimResult};

use crate::options::OptValue;

/// Window ids follow vim's numbering: real windows are >= 1000, and `0` means
/// "the current window" wherever a window argument is accepted.
pub type WindowId = i32;

/// Tabpage ids are 1-based, like vim's `:tabnext` numbering.
pub type TabpageId = i32;

/// First id handed out by [`Windows::new`].
pub const FIRST_WIN_ID: WindowId = 1000;

/// First tabpage id.
pub const FIRST_TAB_ID: TabpageId = 1;

/// One rectangle on the screen, in cells (zero-based row/col, half-open).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub row: usize,
    pub col: usize,
    pub rows: usize,
    pub cols: usize,
}

impl Rect {
    pub const fn new(row: usize, col: usize, rows: usize, cols: usize) -> Self {
        Self {
            row,
            col,
            rows,
            cols,
        }
    }

    pub fn contains(&self, row: usize, col: usize) -> bool {
        row >= self.row
            && row < self.row + self.rows
            && col >= self.col
            && col < self.col + self.cols
    }

    /// Rows that show buffer text (everything above this window's status line).
    pub fn text_rows(&self) -> usize {
        self.rows.saturating_sub(1)
    }
}

/// A window tabpage: the frame tree plus the two windows that `<C-w>p` cycles.
#[derive(Clone, Debug)]
pub struct TabPage {
    pub id: TabpageId,
    pub layout: Layout,
    /// The current window of this tabpage (`curwin`).
    pub current: WindowId,
    /// The previously current window (`prevwin`, `<C-w>p`).
    pub prev: WindowId,
}

/// One split frame: a leaf window or a group of children.
#[derive(Clone, Debug)]
pub enum Layout {
    Leaf(WindowId),
    /// Children side by side (`:vsplit`), separated by one screen column.
    Row(Vec<Layout>),
    /// Children stacked (`:split`), one status line per child.
    Col(Vec<Layout>),
}

impl Layout {
    pub fn leaf(&self) -> Option<WindowId> {
        match self {
            Layout::Leaf(id) => Some(*id),
            _ => None,
        }
    }

    /// Windows in tree order (top to bottom, left to right).
    pub fn leaves(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<WindowId>) {
        match self {
            Layout::Leaf(id) => out.push(*id),
            Layout::Row(children) | Layout::Col(children) => {
                for child in children {
                    child.collect(out);
                }
            }
        }
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.leaves().contains(&id)
    }

    /// Replace `target` by a two-child group so `new_id` becomes a sibling.
    ///
    /// `vertical` selects a [`Layout::Row`] (`:vsplit`); otherwise a
    /// [`Layout::Col`] (`:split`). `new_first` puts the new window before the
    /// old one, which is where vim puts it when 'splitbelow'/'splitright' are
    /// off (the defaults).
    pub fn split_leaf(
        &mut self,
        target: WindowId,
        new_id: WindowId,
        vertical: bool,
        new_first: bool,
    ) -> bool {
        if !self.contains(target) {
            return false;
        }
        self.split_leaf_inner(target, new_id, vertical, new_first)
    }

    fn split_leaf_inner(
        &mut self,
        target: WindowId,
        new_id: WindowId,
        vertical: bool,
        new_first: bool,
    ) -> bool {
        match self {
            Layout::Leaf(id) if *id == target => {
                let old = Layout::Leaf(*id);
                let fresh = Layout::Leaf(new_id);
                let children = if new_first {
                    vec![fresh, old]
                } else {
                    vec![old, fresh]
                };
                *self = if vertical {
                    Layout::Row(children)
                } else {
                    Layout::Col(children)
                };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Row(children) | Layout::Col(children) => children
                .iter_mut()
                .any(|c| c.split_leaf_inner(target, new_id, vertical, new_first)),
        }
    }

    /// Remove one leaf; a group left with a single child collapses to it.
    pub fn remove_leaf(&mut self, id: WindowId) -> bool {
        let (removed, replacement) = match self {
            Layout::Leaf(leaf) if *leaf == id => return true,
            Layout::Leaf(_) => (false, None),
            Layout::Row(children) | Layout::Col(children) => {
                let mut removed = false;
                children.retain_mut(|child| {
                    if child.remove_leaf(id) && child.leaf() == Some(id) {
                        removed = true;
                        false
                    } else {
                        true
                    }
                });
                let replacement = if children.len() == 1 {
                    Some(children.remove(0))
                } else {
                    None
                };
                (removed, replacement)
            }
        };
        if let Some(only) = replacement {
            *self = only;
        }
        removed
    }

    /// Depth-first search for the window in the given direction from `from`.
    fn neighbours(
        &self,
        from: Rect,
        dir: Dir,
        out: &mut Vec<(WindowId, Rect)>,
        rects: &HashMap<WindowId, Rect>,
    ) {
        match self {
            Layout::Leaf(id) => {
                if let Some(rect) = rects.get(id) {
                    if dir.neighbour(from, *rect) {
                        out.push((*id, *rect));
                    }
                }
            }
            Layout::Row(children) | Layout::Col(children) => {
                for child in children {
                    child.neighbours(from, dir, out, rects);
                }
            }
        }
    }
}

/// A window direction (`<C-w>h`, `j`, `k`, `l`, and `:wincmd` equivalents).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

impl Dir {
    pub fn from_key(key: char) -> Option<Dir> {
        match key {
            'h' => Some(Dir::Left),
            'j' => Some(Dir::Down),
            'k' => Some(Dir::Up),
            'l' => Some(Dir::Right),
            _ => None,
        }
    }

    /// Is `other` a neighbour of `from` in this direction? Mirrors
    /// `win_vert_neighbor()`/`win_horz_neighbor()` (window.c): the candidate
    /// must overlap on the perpendicular axis and start beyond the edge.
    fn neighbour(self, from: Rect, other: Rect) -> bool {
        if other.eq(&from) {
            return false;
        }
        // Moving left/right needs the windows to share rows; moving up/down
        // needs them to share columns. Touching edges count as sharing.
        let rows_overlap = other.row <= from.row + from.rows && from.row <= other.row + other.rows;
        let cols_overlap = other.col <= from.col + from.cols && from.col <= other.col + other.cols;
        match self {
            Dir::Left => rows_overlap && other.col + other.cols <= from.col,
            Dir::Right => rows_overlap && other.col >= from.col + from.cols,
            Dir::Up => cols_overlap && other.row + other.rows <= from.row,
            Dir::Down => cols_overlap && other.row >= from.row + from.rows,
        }
    }

    /// Distance between the two rectangles along the direction of travel.
    fn distance(self, from: Rect, other: Rect) -> usize {
        match self {
            Dir::Left => from.col.saturating_sub(other.col + other.cols),
            Dir::Right => other.col.saturating_sub(from.col + from.cols),
            Dir::Up => from.row.saturating_sub(other.row + other.rows),
            Dir::Down => other.row.saturating_sub(from.row + from.rows),
        }
    }
}

/// One window: a viewport onto a buffer plus its window-local options.
#[derive(Clone, Debug)]
pub struct Window {
    pub id: WindowId,
    pub tabpage: TabpageId,
    pub bufnr: i32,
    pub cursor: Pos,
    /// First displayed line (zero-based).
    pub top_line: usize,
    /// Horizontal scroll in display cells (`w_leftcol`).
    pub left: usize,
    /// Desired column for `j`/`k` (`w_curswant`), a byte column.
    pub curswant: usize,
    /// Window-local option values; absent means "use the global value".
    pub local: HashMap<String, OptValue>,
    /// Requested buffer-area height in rows (without the status line).
    pub height: usize,
    /// Requested width in columns.
    pub width: usize,
    /// Rectangle from the last layout pass.
    pub rect: Rect,
    /// True for the quickfix window created by `:copen` (special buffer).
    pub quickfix: bool,
}

impl Window {
    pub fn new(id: WindowId, tabpage: TabpageId, bufnr: i32, height: usize, width: usize) -> Self {
        Self {
            id,
            tabpage,
            bufnr,
            cursor: Pos::new(0, 0),
            top_line: 0,
            left: 0,
            curswant: 0,
            local: HashMap::new(),
            height,
            width,
            rect: Rect::default(),
            quickfix: false,
        }
    }

    pub fn get_local(&self, name: &str) -> Option<&OptValue> {
        self.local.get(name)
    }

    pub fn set_local(&mut self, name: &str, value: OptValue) {
        self.local.insert(name.to_string(), value);
    }

    /// Buffer area height in rows (never zero, so a degenerate geometry still
    /// has a place for the cursor).
    pub fn text_rows(&self) -> usize {
        self.rect.text_rows().max(1)
    }
}

/// The whole window/tabpage state of an editor.
#[derive(Debug)]
pub struct Windows {
    pub tabs: Vec<TabPage>,
    windows: HashMap<WindowId, Window>,
    /// Index into `tabs`.
    cur_tab: usize,
    next_win: WindowId,
    next_tab: TabpageId,
    /// Rectangles from the most recent [`Windows::layout`] pass.
    last_rects: HashMap<WindowId, Rect>,
}

impl Default for Windows {
    fn default() -> Self {
        Self::new()
    }
}

impl Windows {
    /// Start with one tabpage holding one window on buffer 1.
    pub fn new() -> Self {
        let mut windows = HashMap::new();
        let win = Window::new(FIRST_WIN_ID, FIRST_TAB_ID, 1, 20, 80);
        windows.insert(win.id, win);
        Self {
            tabs: vec![TabPage {
                id: FIRST_TAB_ID,
                layout: Layout::Leaf(FIRST_WIN_ID),
                current: FIRST_WIN_ID,
                prev: FIRST_WIN_ID,
            }],
            windows,
            cur_tab: 0,
            next_win: FIRST_WIN_ID + 1,
            next_tab: FIRST_TAB_ID + 1,
            last_rects: HashMap::new(),
        }
    }

    pub fn current_tab(&self) -> &TabPage {
        &self.tabs[self.cur_tab]
    }

    pub fn current_tab_mut(&mut self) -> &mut TabPage {
        let idx = self.cur_tab;
        &mut self.tabs[idx]
    }

    pub fn current_tab_index(&self) -> usize {
        self.cur_tab
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    pub fn current(&self) -> &Window {
        let id = self.current_tab().current;
        self.windows.get(&id).expect("current window exists")
    }

    pub fn current_mut(&mut self) -> &mut Window {
        let id = self.current_tab().current;
        self.windows.get_mut(&id).expect("current window exists")
    }

    pub fn current_id(&self) -> WindowId {
        self.current_tab().current
    }

    /// Window ids of the current tabpage, in tree order.
    pub fn ids(&self) -> Vec<WindowId> {
        self.current_tab().layout.leaves()
    }

    /// Window ids of a tabpage (1-based index); empty when out of range.
    pub fn ids_of_tab(&self, tabnr: usize) -> Vec<WindowId> {
        self.tabs
            .get(tabnr.saturating_sub(1))
            .map(|t| t.layout.leaves())
            .unwrap_or_default()
    }

    pub fn tab_of(&self, id: WindowId) -> Option<usize> {
        self.tabs.iter().position(|t| t.layout.contains(id))
    }

    /// 1-based window number inside its tabpage (`winnr()`).
    pub fn winnr(&self, id: WindowId) -> usize {
        self.tab_of(id)
            .and_then(|t| self.tabs[t].layout.leaves().iter().position(|w| *w == id))
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    pub fn all_ids(&self) -> Vec<WindowId> {
        self.windows.keys().copied().collect()
    }

    fn alloc_win(&mut self) -> WindowId {
        let id = self.next_win;
        self.next_win += 1;
        id
    }

    /// Split the current window in two, both showing `bufnr`.
    ///
    /// Returns `E36` when the terminal is too small for another window, the way
    /// `win_split()` does.
    pub fn split(&mut self, vertical: bool, bufnr: i32, new_first: bool) -> VimResult<WindowId> {
        let id = self.current_id();
        let (height, width) = {
            let win = self.current();
            (win.height, win.width)
        };
        // Every split keeps at least one content row/column per window, plus the
        // status line for stacked splits and one separator column for rows.
        let (new_height, new_width) = if vertical {
            if width < 2 * MIN_WIDTH + 1 {
                return Err(VimError::new(
                    "E36",
                    "Not enough room: need at least 21 columns",
                ));
            }
            let half = width / 2;
            (height, half)
        } else {
            if height < 2 * MIN_HEIGHT + 2 {
                return Err(VimError::new(
                    "E36",
                    "Not enough room: need at least 6 rows",
                ));
            }
            let half = height.div_ceil(2);
            (half, width)
        };
        let tab = self.current_tab().id;
        let new_id = self.alloc_win();
        let mut win = Window::new(new_id, tab, bufnr, new_height, new_width);
        {
            let cur = self.current();
            win.cursor = cur.cursor;
            win.top_line = cur.top_line;
            win.left = cur.left;
            win.curswant = cur.curswant;
            win.local = cur.local.clone();
        }
        // The old window keeps the other half of the space.
        {
            let cur = self.current_mut();
            cur.height = height.saturating_sub(new_height);
            cur.width = width.saturating_sub(new_width);
        }
        self.windows.insert(new_id, win);
        let tabpage = self.current_tab_mut();
        if !tabpage.layout.split_leaf(id, new_id, vertical, new_first) {
            return Err(VimError::internal("current window missing from the layout"));
        }
        tabpage.prev = id;
        tabpage.current = new_id;
        Ok(new_id)
    }

    /// Close one window. The last window of the last tabpage cannot be closed
    /// (`E444`); closing the last window of a tabpage closes the tabpage.
    pub fn close(&mut self, id: WindowId) -> VimResult<()> {
        let Some(tabix) = self.tab_of(id) else {
            return Err(VimError::new("E16", "Invalid window id"));
        };
        let layout_len = self.tabs[tabix].layout.leaves().len();
        if layout_len <= 1 {
            if self.tabs.len() <= 1 {
                return Err(VimError::new("E444", "Cannot close last window"));
            }
            return self.close_tabpage(tabix + 1);
        }
        let was_current = self.tabs[tabix].current == id;
        self.tabs[tabix].layout.remove_leaf(id);
        self.windows.remove(&id);
        if was_current
            || !self.tabs[tabix]
                .layout
                .leaves()
                .contains(&self.tabs[tabix].current)
        {
            let next = if self.tabs[tabix].layout.contains(self.tabs[tabix].prev) {
                self.tabs[tabix].prev
            } else {
                self.tabs[tabix].layout.leaves()[0]
            };
            self.tabs[tabix].current = next;
        }
        Ok(())
    }

    /// `:only`: close every other window of the current tabpage.
    pub fn only(&mut self, keep: WindowId) -> VimResult<()> {
        let Some(tabix) = self.tab_of(keep) else {
            return Err(VimError::new("E16", "Invalid window id"));
        };
        let others: Vec<WindowId> = self.tabs[tabix]
            .layout
            .leaves()
            .into_iter()
            .filter(|w| *w != keep)
            .collect();
        for id in others {
            self.windows.remove(&id);
        }
        self.tabs[tabix].layout = Layout::Leaf(keep);
        self.tabs[tabix].current = keep;
        self.tabs[tabix].prev = keep;
        Ok(())
    }

    /// Make `id` the current window (switching tabpage when needed).
    pub fn goto(&mut self, id: WindowId) -> VimResult<()> {
        let tabix = self
            .tab_of(id)
            .ok_or_else(|| VimError::new("E16", "Invalid window id"))?;
        self.cur_tab = tabix;
        let tab = &mut self.tabs[tabix];
        if tab.current != id {
            tab.prev = tab.current;
            tab.current = id;
        }
        Ok(())
    }

    /// Move to the window in `dir` from the current one (`<C-w>h/j/k/l`).
    ///
    /// `E11` when there is no window in that direction, which is what vim
    /// reports for `:wincmd h` at the left edge.
    pub fn goto_dir(&mut self, dir: Dir) -> VimResult<()> {
        let from_id = self.current_id();
        let from = self.current().rect;
        let rects: HashMap<WindowId, Rect> = self
            .ids()
            .into_iter()
            .filter_map(|id| self.windows.get(&id).map(|w| (id, w.rect)))
            .collect();
        let mut candidates = Vec::new();
        let layout = self.current_tab().layout.clone();
        layout.neighbours(from, dir, &mut candidates, &rects);
        let best = candidates
            .into_iter()
            .min_by_key(|(_, rect)| dir.distance(from, *rect));
        match best {
            Some((id, _)) => {
                let tab = self.current_tab_mut();
                tab.prev = from_id;
                tab.current = id;
                Ok(())
            }
            None => Err(VimError::new("E11", "No window in that direction")),
        }
    }

    /// Go to the next (`<C-w>w`) or previous (`<C-w>W`) window.
    pub fn cycle(&mut self, forward: bool, count: usize) -> VimResult<()> {
        let ids = self.ids();
        if ids.len() < 2 {
            return Err(VimError::new("E11", "Only one window"));
        }
        let cur = self.current_id();
        let pos = ids.iter().position(|w| *w == cur).unwrap_or(0);
        let step = count.max(1);
        let next = if forward {
            (pos + step) % ids.len()
        } else {
            (pos + ids.len() - step % ids.len()) % ids.len()
        };
        let target = ids[next];
        let tab = self.current_tab_mut();
        tab.prev = cur;
        tab.current = target;
        Ok(())
    }

    /// `<C-w>p`: jump to the previously current window.
    pub fn goto_prev(&mut self) -> VimResult<()> {
        let cur = self.current_id();
        let prev = self.current_tab().prev;
        if prev == cur || !self.current_tab().layout.contains(prev) {
            return Err(VimError::new("E11", "No previous window"));
        }
        let tab = self.current_tab_mut();
        tab.prev = cur;
        tab.current = prev;
        Ok(())
    }

    /// `<C-w>t` / `<C-w>b`: top-left / bottom-right window of the tabpage.
    pub fn goto_edge(&mut self, bottom_right: bool) -> VimResult<()> {
        let ids = self.ids();
        let target = if bottom_right {
            ids.last().copied()
        } else {
            ids.first().copied()
        };
        match target {
            Some(id) => {
                let cur = self.current_id();
                let tab = self.current_tab_mut();
                if id != cur {
                    tab.prev = cur;
                    tab.current = id;
                }
                Ok(())
            }
            None => Err(VimError::new("E11", "No windows")),
        }
    }

    /// Resize the current window: `delta` rows (`vertical == false`) or columns.
    /// A negative delta shrinks; the size never drops below the minimum and the
    /// siblings absorb the difference at layout time.
    pub fn resize(&mut self, delta: i64, vertical: bool) -> VimResult<()> {
        if self.ids().len() < 2 {
            return Err(VimError::new("E11", "Cannot resize the only window"));
        }
        let win = self.current_mut();
        let (value, min) = if vertical {
            (win.width as i64, MIN_WIDTH as i64)
        } else {
            (win.height as i64, MIN_HEIGHT as i64)
        };
        let next = (value + delta).max(min) as usize;
        if vertical {
            win.width = next;
        } else {
            win.height = next;
        }
        Ok(())
    }

    /// `:vertical resize N` with an absolute size.
    pub fn set_size(&mut self, size: usize, vertical: bool) -> VimResult<()> {
        if self.ids().len() < 2 {
            return Err(VimError::new("E11", "Cannot resize the only window"));
        }
        let min = if vertical { MIN_WIDTH } else { MIN_HEIGHT };
        let size = size.max(min);
        let win = self.current_mut();
        if vertical {
            win.width = size;
        } else {
            win.height = size;
        }
        Ok(())
    }

    /// Open a new tabpage holding one window on `bufnr`.
    pub fn new_tabpage(&mut self, bufnr: i32, height: usize, width: usize) -> TabpageId {
        let tab_id = self.next_tab;
        self.next_tab += 1;
        let win_id = self.alloc_win();
        let win = Window::new(win_id, tab_id, bufnr, height, width);
        self.windows.insert(win_id, win);
        self.tabs.push(TabPage {
            id: tab_id,
            layout: Layout::Leaf(win_id),
            current: win_id,
            prev: win_id,
        });
        self.cur_tab = self.tabs.len() - 1;
        tab_id
    }

    /// Close tabpage `tabnr` (1-based). The last tabpage cannot be closed.
    pub fn close_tabpage(&mut self, tabnr: usize) -> VimResult<()> {
        if self.tabs.len() <= 1 {
            return Err(VimError::new("E784", "Cannot close last tab page"));
        }
        let idx = tabnr.saturating_sub(1);
        if idx >= self.tabs.len() {
            return Err(VimError::new("E16", "Invalid window id"));
        }
        let tab = self.tabs.remove(idx);
        for id in tab.layout.leaves() {
            self.windows.remove(&id);
        }
        if self.cur_tab >= self.tabs.len() {
            self.cur_tab = self.tabs.len() - 1;
        } else if self.cur_tab > idx {
            self.cur_tab -= 1;
        }
        Ok(())
    }

    /// Switch to tabpage `tabnr` (1-based).
    pub fn goto_tabpage(&mut self, tabnr: usize) -> VimResult<()> {
        if tabnr == 0 || tabnr > self.tabs.len() {
            return Err(VimError::new("E16", "Invalid window id"));
        }
        self.cur_tab = tabnr - 1;
        Ok(())
    }

    /// `:tabclose!`/`:tabonly`: keep only the given tabpage (1-based).
    pub fn tabonly(&mut self, keep: usize) -> VimResult<()> {
        if keep == 0 || keep > self.tabs.len() {
            return Err(VimError::new("E16", "Invalid window id"));
        }
        let mut number = 1;
        let mut doomed: Vec<WindowId> = Vec::new();
        self.tabs.retain_mut(|tab| {
            let keep_this = number == keep;
            if !keep_this {
                doomed.extend(tab.layout.leaves());
            }
            number += 1;
            keep_this
        });
        for id in doomed {
            self.windows.remove(&id);
        }
        self.cur_tab = 0;
        Ok(())
    }

    /// `:tabmove N`: move the current tabpage to position `N` (1-based; 0 puts
    /// it last).
    pub fn move_tabpage(&mut self, to: usize) -> VimResult<()> {
        let cur = self.cur_tab;
        let target = if to == 0 {
            self.tabs.len().saturating_sub(1)
        } else {
            (to - 1).min(self.tabs.len() - 1)
        };
        if target == cur {
            return Ok(());
        }
        let tab = self.tabs.remove(cur);
        self.tabs.insert(target, tab);
        self.cur_tab = target;
        Ok(())
    }

    /// Assign screen rectangles for every window of every tabpage.
    ///
    /// `rows` excludes the command line and message row; `cols` is the full
    /// terminal width. Tabpages other than the current one keep the same
    /// geometry (they are not drawn, but `win_screenpos()` and `winheight()`
    /// answer for them).
    pub fn layout(&mut self, rows: usize, cols: usize) -> &HashMap<WindowId, Rect> {
        let rows = rows.max(2);
        let cols = cols.max(MIN_WIDTH);
        let mut rects: HashMap<WindowId, Rect> = HashMap::new();
        for tab in &self.tabs {
            assign_frame(
                &tab.layout,
                Rect::new(0, 0, rows, cols),
                &self.windows,
                &mut rects,
            );
        }
        for (id, rect) in &rects {
            if let Some(win) = self.windows.get_mut(id) {
                win.rect = *rect;
            }
        }
        self.last_rects = rects;
        &self.last_rects
    }

    /// Number of windows in the current tabpage.
    pub fn wincount(&self) -> usize {
        self.ids().len()
    }

    /// Rectangles from the last [`Windows::layout`] pass.
    pub fn rects(&self) -> &HashMap<WindowId, Rect> {
        &self.last_rects
    }

    /// The rectangle of the status row of `id` (the last row of its frame).
    pub fn status_row(&self, id: WindowId) -> Option<usize> {
        self.windows
            .get(&id)
            .map(|w| w.rect.row + w.rect.rows.saturating_sub(1))
    }

    /// Window whose rectangle contains the given screen cell.
    pub fn at(&self, row: usize, col: usize) -> Option<WindowId> {
        self.ids().into_iter().find(|id| {
            self.windows
                .get(id)
                .is_some_and(|w| w.rect.contains(row, col))
        })
    }
}

/// Minimum buffer-area height of a window (`'winminheight'` default).
pub const MIN_HEIGHT: usize = 1;
/// Minimum width of a window (`'winminwidth'` default).
pub const MIN_WIDTH: usize = 10;

/// Desired size of a frame subtree: `(rows, cols)`.
///
/// A leaf contributes its buffer height **plus one row for its status line**,
/// so a column of windows sums to the exact screen height it needs.
fn desired_size(layout: &Layout, windows: &HashMap<WindowId, Window>) -> (usize, usize) {
    match layout {
        Layout::Leaf(id) => match windows.get(id) {
            Some(win) => (win.height + 1, win.width.max(MIN_WIDTH)),
            None => (2, MIN_WIDTH),
        },
        Layout::Row(children) => {
            let rows = children
                .iter()
                .map(|c| desired_size(c, windows).0)
                .max()
                .unwrap_or(2);
            let cols: usize = children
                .iter()
                .map(|c| desired_size(c, windows).1)
                .sum::<usize>()
                + children.len().saturating_sub(1);
            (rows, cols)
        }
        Layout::Col(children) => {
            let rows: usize = children.iter().map(|c| desired_size(c, windows).0).sum();
            let cols = children
                .iter()
                .map(|c| desired_size(c, windows).1)
                .max()
                .unwrap_or(MIN_WIDTH);
            (rows, cols)
        }
    }
}

/// Scale `desired` so that it sums to `total`, never going below `min`.
///
/// Surplus space goes to the last child (vim's `win_equal()` grows the window
/// that was created last when the terminal is bigger than the frame tree asks
/// for); deficits are taken proportionally from the children that ask for the
/// most room.
fn distribute(desired: &[usize], total: usize, min: usize) -> Vec<usize> {
    let n = desired.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![total.max(min)];
    }
    let floor = min * n;
    if total <= floor {
        return vec![min; n];
    }
    let sum: usize = desired.iter().copied().sum::<usize>().max(1);
    let mut out = vec![min; n];
    if sum <= total {
        for i in 0..n {
            out[i] = desired[i].max(min);
        }
        let used: usize = out.iter().sum();
        if used <= total {
            out[n - 1] += total - used;
        } else {
            shrink_to(&mut out, total, min);
        }
        return out;
    }
    // The frame tree asks for more room than the screen has: hand every child
    // its minimum plus a share of the remaining space proportional to its wish.
    let slack = total - floor;
    let over = sum.saturating_sub(floor).max(1);
    let mut used = 0usize;
    for i in 0..n {
        let extra = desired[i].saturating_sub(min) * slack / over;
        out[i] = min + extra;
        used += out[i];
    }
    if used < total {
        out[n - 1] += total - used;
    } else if used > total {
        shrink_to(&mut out, total, min);
    }
    out
}

fn shrink_to(out: &mut [usize], total: usize, min: usize) {
    let n = out.len();
    let mut used: usize = out.iter().sum();
    while used > total {
        let Some(i) = (0..n).filter(|i| out[*i] > min).max_by_key(|i| out[*i]) else {
            break;
        };
        out[i] -= 1;
        used -= 1;
    }
}

/// Recursively assign rectangles for one frame tree.
fn assign_frame(
    layout: &Layout,
    area: Rect,
    windows: &HashMap<WindowId, Window>,
    out: &mut HashMap<WindowId, Rect>,
) {
    match layout {
        Layout::Leaf(id) => {
            // A window always keeps one row for its status line, even in a
            // degenerate geometry, so the layout stays drawable.
            let rows = area.rows.max(2);
            out.insert(*id, Rect::new(area.row, area.col, rows, area.cols));
        }
        Layout::Row(children) => {
            let separators = children.len().saturating_sub(1);
            let available = area.cols.saturating_sub(separators);
            let desired: Vec<usize> = children
                .iter()
                .map(|c| desired_size(c, windows).1)
                .collect();
            let widths = distribute(&desired, available, MIN_WIDTH);
            let mut col = area.col;
            for (i, child) in children.iter().enumerate() {
                let width = widths[i];
                assign_frame(
                    child,
                    Rect::new(area.row, col, area.rows, width),
                    windows,
                    out,
                );
                col += width + 1; // the `|` separator column
            }
        }
        Layout::Col(children) => {
            let desired: Vec<usize> = children
                .iter()
                .map(|c| desired_size(c, windows).0)
                .collect();
            // Two rows is the smallest usable frame: one text row + status.
            let heights = distribute(&desired, area.rows, MIN_HEIGHT + 1);
            let mut row = area.row;
            for (i, child) in children.iter().enumerate() {
                let height = heights[i];
                assign_frame(
                    child,
                    Rect::new(row, area.col, height, area.cols),
                    windows,
                    out,
                );
                row += height;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids_of(win: &Windows) -> Vec<WindowId> {
        win.ids()
    }

    #[test]
    fn split_creates_a_window_above_with_the_same_buffer() {
        let mut win = Windows::new();
        let first = win.current_id();
        let new = win.split(false, 1, true).unwrap();
        assert_eq!(ids_of(&win), vec![new, first], "new window goes on top");
        assert_eq!(win.current_id(), new);
        assert_eq!(win.get(new).unwrap().bufnr, 1);
        assert_eq!(win.wincount(), 2);
        win.layout(23, 80);
        let a = win.get(new).unwrap().rect;
        let b = win.get(first).unwrap().rect;
        assert_eq!(a.row, 0);
        assert_eq!(a.row + a.rows, b.row, "stacked windows touch");
        assert_eq!(b.row + b.rows, 23, "the layout fills the screen");
        assert!(a.rows >= 2 && b.rows >= 2);
    }

    #[test]
    fn vsplit_places_a_separator_column_between_the_windows() {
        let mut win = Windows::new();
        let first = win.current_id();
        let new = win.split(true, 1, true).unwrap();
        assert_eq!(ids_of(&win), vec![new, first], "new window goes left");
        win.layout(20, 80);
        let a = win.get(new).unwrap().rect;
        let b = win.get(first).unwrap().rect;
        assert_eq!(a.col, 0);
        assert_eq!(a.col + a.cols + 1, b.col, "one separator column");
        assert_eq!(b.col + b.cols, 80);
        assert!(a.cols >= MIN_WIDTH && b.cols >= MIN_WIDTH);
    }

    #[test]
    fn close_collapses_the_layout_and_keeps_one_window() {
        let mut win = Windows::new();
        let first = win.current_id();
        let new = win.split(false, 1, true).unwrap();
        win.close(new).unwrap();
        assert_eq!(ids_of(&win), vec![first]);
        assert_eq!(win.current_id(), first);
        assert!(matches!(win.current_tab().layout, Layout::Leaf(_)));
        assert_eq!(win.close(first).unwrap_err().code, "E444");
    }

    #[test]
    fn only_closes_every_other_window() {
        let mut win = Windows::new();
        let first = win.current_id();
        win.split(false, 1, true).unwrap();
        win.split(true, 1, true).unwrap();
        assert_eq!(win.wincount(), 3);
        win.only(first).unwrap();
        assert_eq!(ids_of(&win), vec![first]);
        assert_eq!(win.all_ids().len(), 1, "stale windows are dropped");
    }

    #[test]
    fn wincmd_moves_between_windows() {
        let mut win = Windows::new();
        let top = win.current_id();
        let bottom = win.split(false, 1, false).unwrap();
        win.layout(23, 80);
        assert_eq!(win.current_id(), bottom);
        win.goto_dir(Dir::Up).unwrap();
        assert_eq!(win.current_id(), top);
        win.goto_dir(Dir::Down).unwrap();
        assert_eq!(win.current_id(), bottom);
        assert_eq!(win.goto_dir(Dir::Down).unwrap_err().code, "E11");
        assert_eq!(win.goto_dir(Dir::Left).unwrap_err().code, "E11");
        win.cycle(true, 1).unwrap();
        assert_eq!(win.current_id(), top);
        win.goto_prev().unwrap();
        assert_eq!(win.current_id(), bottom);
        win.goto_edge(false).unwrap();
        assert_eq!(win.current_id(), top);
        win.goto_edge(true).unwrap();
        assert_eq!(win.current_id(), bottom);
    }

    #[test]
    fn vertical_neighbour_requires_overlapping_columns() {
        let mut win = Windows::new();
        let left = win.current_id();
        let right = win.split(true, 1, false).unwrap();
        win.layout(20, 80);
        assert_eq!(win.current_id(), right);
        win.goto_dir(Dir::Left).unwrap();
        assert_eq!(win.current_id(), left);
        win.goto_dir(Dir::Right).unwrap();
        assert_eq!(win.current_id(), right);
    }

    #[test]
    fn tabpages_hold_their_own_window_trees() {
        let mut win = Windows::new();
        let first = win.current_id();
        let tab2 = win.new_tabpage(1, 20, 80);
        assert_eq!(win.tab_count(), 2);
        assert_eq!(win.current_tab().id, tab2);
        let second = win.current_id();
        assert_ne!(second, first);
        win.goto_tabpage(1).unwrap();
        assert_eq!(win.current_id(), first);
        assert_eq!(win.ids(), vec![first]);
        win.goto_tabpage(2).unwrap();
        assert_eq!(win.current_id(), second);
        win.close_tabpage(2).unwrap();
        assert_eq!(win.tab_count(), 1);
        assert_eq!(win.close_tabpage(1).unwrap_err().code, "E784");
        assert!(
            win.get(second).is_none(),
            "windows of a closed tab are gone"
        );
    }

    #[test]
    fn tabmove_and_tabonly() {
        let mut win = Windows::new();
        win.new_tabpage(1, 20, 80);
        win.new_tabpage(1, 20, 80);
        assert_eq!(win.current_tab_index(), 2);
        win.move_tabpage(1).unwrap();
        assert_eq!(win.current_tab_index(), 0);
        win.move_tabpage(0).unwrap();
        assert_eq!(win.current_tab_index(), 2, ":tabmove 0 puts it last");
        win.tabonly(3).unwrap();
        assert_eq!(win.tab_count(), 1);
        assert_eq!(win.all_ids().len(), 1);
    }

    #[test]
    fn resize_and_set_size_respect_the_minimum() {
        let mut win = Windows::new();
        win.split(false, 1, true).unwrap();
        let cur = win.current_id();
        win.resize(5, false).unwrap();
        let grown = win.get(cur).unwrap().height;
        win.resize(-1000, false).unwrap();
        assert_eq!(win.get(cur).unwrap().height, MIN_HEIGHT);
        win.set_size(7, false).unwrap();
        assert_eq!(win.get(cur).unwrap().height, 7);
        assert!(grown > MIN_HEIGHT);
    }

    #[test]
    fn layout_never_overlaps_and_stays_inside_the_screen() {
        let mut win = Windows::new();
        win.split(false, 1, false).unwrap();
        win.split(true, 1, true).unwrap();
        let rects = win.layout(24, 80).clone();
        assert_eq!(rects.len(), 3);
        let mut cells: Vec<(usize, usize)> = Vec::new();
        for rect in rects.values() {
            assert!(rect.row + rect.rows <= 24, "{rect:?} fits vertically");
            assert!(rect.col + rect.cols <= 80, "{rect:?} fits horizontally");
            for row in rect.row..rect.row + rect.rows {
                for col in rect.col..rect.col + rect.cols {
                    cells.push((row, col));
                }
            }
        }
        let unique: std::collections::HashSet<_> = cells.iter().collect();
        assert_eq!(unique.len(), cells.len(), "windows must not overlap");
    }

    #[test]
    fn window_local_options_shadow_the_global_value() {
        let mut win = Windows::new();
        let id = win.current_id();
        win.get_mut(id)
            .unwrap()
            .set_local("number", OptValue::Bool(true));
        assert_eq!(
            win.get(id).unwrap().get_local("number"),
            Some(&OptValue::Bool(true))
        );
        win.split(false, 1, true).unwrap();
        assert_eq!(
            win.current().get_local("number"),
            Some(&OptValue::Bool(true)),
            "a split inherits the window-local options"
        );
    }

    #[test]
    fn screen_position_and_hit_testing_use_the_last_layout() {
        let mut win = Windows::new();
        let top = win.current_id();
        let bottom = win.split(false, 1, false).unwrap();
        win.layout(23, 80);
        let top_status = win.status_row(top).unwrap();
        assert!(top_status < win.get(bottom).unwrap().rect.row);
        assert_eq!(win.at(0, 0), Some(top));
        let row = win.get(bottom).unwrap().rect.row;
        assert_eq!(win.at(row, 0), Some(bottom));
        assert_eq!(win.at(999, 0), None);
    }
}
