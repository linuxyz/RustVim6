//! Diff mode: per-window diff state, filler lines and changed-line flags
//! (reference: `src/diff.c`, plus `foldlevelDiff()` in `src/fold.c`).
//!
//! `:diffthis` puts a window's buffer in diff mode by setting the window-local
//! `'diff'` option. Once at least two windows in a tabpage are diffing, the
//! buffers they show are compared with [`rvim_diff::change_blocks`] and every
//! diff window reports two things for each of its lines:
//!
//! * how many **filler** rows the TUI has to draw *above* the line, so that two
//!   views of equal-length screens show the same lines at the same height
//!   (`diff_check_fill()` in vim);
//! * whether the line belongs to a changed block and therefore gets the
//!   `DiffChange` highlighting (`diff_check()` in vim).
//!
//! When a changed block deletes `N` lines from A and inserts `M` lines into B,
//! the window showing the side with fewer lines gets `M - N` filler rows above
//! the line *below* the block; the side with more lines gets none. A block that
//! is a pure insertion on one side therefore shows its "deleted" marking on the
//! line under the insertion point, which is what makes `]c` and `do` land
//! somewhere useful.
//!
//! # Which buffers are compared
//!
//! Vim keeps an explicit per-tabpage registration list (`tp_diffbuf`,
//! `diff_buf_add()`) and diffs the first registered buffer against all others.
//! This module derives that list from the windows of the tabpage instead:
//! walking the split tree in order, every window whose local `'diff'` value is
//! on contributes its buffer, and the first distinct buffer found is the base
//! ("original") one. Windows showing the base are compared against each of the
//! other buffers; the other windows are compared against the base only.
//!
//! # Caching
//!
//! [`DiffState`] memoises the changed blocks together with the
//! `b:changedtick` and line count of every participating buffer. A cached
//! state whose ticks still match is reused, anything else is recomputed from the
//! buffer text, so a stale cache is never *wrong* - it only costs one diff.
//! [`Editor::diff_update`] rewrites the cache of every diff window and is what
//! the editor calls after an edit (vim's `tp_diff_invalid` path).
//!
//! # Deliberate deviations from Vim 8.2
//!
//! * The diff order is the window-tree order, not vim's `diff_buf_add()` order.
//!   With exactly two buffers (the normal case) this makes no difference; with
//!   three or four the changed blocks of the base are the union of the pairwise
//!   diffs against every other buffer instead of vim's merged `diff_T` list, so
//!   fillers can differ where several buffers changed the same region.
//! * `'diffopt'` does not exist yet: fillers are always on and `icase`,
//!   `iwhite*`, `context`, `foldcolumn`, `horizontal` and `vertical` are
//!   not supported. `:diffsplit` is horizontal unless it is prefixed with
//!   `:vertical`.
//! * `diff_check()` returns -1 for "changed" and -2 for "added/deleted"; the
//!   flag returned by [`Editor::diff_filler`] merges both into `true`.
//! * `:diffoff` restores `'foldmethod'` to "manual" and turns `'foldenable'`
//!   off, but the previous values of `'scrollbind'`, `'cursorbind'`,
//!   `'wrap'`, `'foldcolumn'` and `'foldlevel'` are not saved and therefore
//!   not restored (vim keeps them in `w_..._save` fields).
//! * Diff mode lives in the window-local `'diff'` value, which a split copies,
//!   so `:split` inside a diff window also diffs the new window's buffer. Vim
//!   copies `'diff'` as well but only shows diffs for registered buffers.
//! * No `diff_hlID()`/syntax-driven highlighting is produced; the TUI decides
//!   how to paint a changed line from the boolean [`Editor::diff_filler`]
//!   returns.
//! * The changed blocks are cached inside the window's local option map under a
//!   reserved key (see [`STATE_KEY`]) because [`crate::editor::Editor`] has no
//!   field for diff state yet. It is not a real option: `:set all` only
//!   enumerates the option table, so the value stays invisible. Moving it into an
//!   `Editor` field later only touches [`cached_state`] and [`store_state`].
//! * `'foldmethod=diff'` is set by `:diffthis` and the diff folds are computed
//!   here from vim's `diff_infold()` rule: every line further than `'diffopt'`
//!   "context" (6, its default) from a change is folded, so the unchanged
//!   parts of both views collapse. `:diffoff` drops those folds instead of
//!   keeping them as manual folds the way vim does.
//! * `:diffpatch` (external patch tooling) and `diff_lnum_win()` cursor
//!   correspondence are not implemented.

use rvim_core::{VimError, VimResult};

use crate::editor::Editor;
use crate::fold::Fold;
use crate::options::OptValue;
use crate::register::{RegType, Register};
use crate::window::WindowId;

pub use rvim_diff::ChangeBlock;

/// Reserved key of the memoised diff state in a window's local option map.
///
/// The value is the encoded [`DiffState`] (see [`encode`]); a window without
/// it simply has no cached diff.
pub const STATE_KEY: &str = "rvim_diff_state";

/// Number of buffers vim allows in one diff (`DB_COUNT` in `diff.c`).
pub const MAX_DIFF_BUFFERS: usize = 4;

/// One buffer taking part in a window's diff, as it was when the changed blocks
/// were computed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffBuffer {
    /// Buffer number.
    pub bufnr: i32,
    /// `b:changedtick` at computation time; a different tick invalidates the
    /// cached blocks.
    pub changedtick: u64,
    /// Line count at computation time. Needed on top of the tick because
    /// reloading a file (`:edit!`) replaces the buffer and restarts the tick.
    pub lines: usize,
}

/// The changed blocks between one window's buffer and one other diff buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffPair {
    /// Buffer number of the other side of the comparison.
    pub bufnr: i32,
    /// Changed blocks whose `a_*` range covers **this** window's buffer and
    /// whose `b_*` range covers `bufnr`.
    pub blocks: Vec<ChangeBlock>,
}

/// Everything one diff window knows: its buffer, the buffers it is diffed
/// against and the cached changed blocks.
///
/// The state is a pure function of the buffer texts plus the set of diff
/// windows, so it can always be recomputed; the cached copy only avoids running
/// the diff engine for every line the TUI asks about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffState {
    /// The buffer of the window this state belongs to.
    pub bufnr: i32,
    /// Every buffer in the diff, in window-tree order; the first entry is the
    /// base ("original") buffer.
    pub bufs: Vec<DiffBuffer>,
    /// The changed blocks against the buffers this window is compared with.
    pub pairs: Vec<DiffPair>,
}

impl DiffState {
    /// Line count of this window's buffer when the state was computed.
    pub fn line_count(&self) -> usize {
        self.bufs
            .iter()
            .find(|buffer| buffer.bufnr == self.bufnr)
            .map(|buffer| buffer.lines)
            .unwrap_or(0)
    }

    /// Filler rows to draw above zero-based `line`, and whether that line is
    /// part of a change.
    ///
    /// Out-of-range lines report no filler, matching vim's `diff_check()`
    /// safety check (`lnum < 1 || lnum > line_count + 1`).
    pub fn status(&self, line: usize) -> (usize, bool) {
        if line > self.line_count() {
            return (0, false);
        }
        let mut filler = 0;
        let mut changed = false;
        for pair in &self.pairs {
            let (pair_filler, pair_changed) = pair_status(&pair.blocks, line);
            filler = filler.max(pair_filler);
            changed |= pair_changed;
        }
        (filler, changed)
    }
}

impl Editor {
    /// `:diffthis` - put the current window and its buffer in diff mode.
    ///
    /// Sets `'diff'`, `'foldmethod=diff'`, `'foldenable'`,
    /// `'foldlevel'`=0, `'scrollbind'`, `'cursorbind'` and `'wrap'` off the
    /// way vim's `diff_win_options()` does, then recomputes the diffs.
    ///
    /// # Errors
    ///
    /// `E96` when the tabpage already diffs [`MAX_DIFF_BUFFERS`] buffers and
    /// this window shows yet another one.
    pub fn diff_this(&mut self) -> VimResult<()> {
        let win = self.current_win_id();
        let Some(own) = bufnr_of(self, win) else {
            return Err(VimError::internal("current window has no buffer"));
        };
        let parts = participants(self, win);
        if !parts.contains(&own) && parts.len() >= MAX_DIFF_BUFFERS {
            return Err(VimError::new(
                "E96",
                format!("Cannot diff more than {MAX_DIFF_BUFFERS} buffers"),
            ));
        }
        self.diff_enable_window(win)?;
        self.diff_update()
    }

    /// `:diffoff` - leave diff mode for the current window.
    ///
    /// With `all` (`:diffoff!`) every window of every tabpage leaves diff
    /// mode. Windows that are not diffing are left alone, so `:diffoff` in an
    /// ordinary window is a no-op, like in vim.
    pub fn diff_off(&mut self, all: bool) -> VimResult<()> {
        let mut targets = Vec::new();
        if all {
            targets.extend(all_window_ids(self));
        } else {
            targets.push(self.current_win_id());
        }
        for win in targets {
            if !window_diff_flag(self, win) {
                continue;
            }
            self.set_window_option(win, "diff", OptValue::Bool(false))?;
            let method = self
                .wopt_for(win, "foldmethod")
                .ok()
                .and_then(|value| value.as_str().map(str::to_string));
            if method.as_deref() == Some("diff") {
                self.set_window_option(win, "foldmethod", OptValue::Str("manual".to_string()))?;
                // vim: with 'foldmethod' back to "manual" the diff folds stay
                // closed, so 'foldenable' is switched off (diff.c ex_diffoff).
                self.set_window_option(win, "foldenable", OptValue::Bool(false))?;
                // vim turns the diff folds into manual folds here; we drop them
                // so a later edit cannot leave stale ranges behind.
                self.fold_store.clear(win);
            }
            clear_state(self, win);
        }
        Ok(())
    }

    /// `:diffupdate` - recompute the changed lines of every diff window.
    ///
    /// Windows whose cache is still valid (same `b:changedtick` and line count
    /// for every participating buffer) are kept as they are, so repeating
    /// `:diffupdate` is cheap.
    pub fn diff_update(&mut self) -> VimResult<()> {
        let mut diff_windows = Vec::new();
        for win in all_window_ids(self) {
            if !window_diff_flag(self, win) {
                clear_state(self, win);
                continue;
            }
            let fresh = cached_state(self, win).is_some_and(|state| is_current(&state, self, win));
            if !fresh {
                match compute_state(self, win) {
                    Some(state) => store_state(self, win, &state),
                    None => clear_state(self, win),
                }
            }
            diff_windows.push(win);
        }
        for win in diff_windows {
            refresh_diff_folds(self, win);
        }
        Ok(())
    }

    /// True when the window shows a buffer in diff mode.
    ///
    /// This is the window-local `'diff'` value, the same flag `:diffthis` sets
    /// and `:diffoff` clears (vim's `w_p_diff`).
    pub fn in_diff_mode(&self, win: WindowId) -> bool {
        window_diff_flag(self, win)
    }

    /// Number of filler lines to show above `line` in window `win`, and the
    /// 'changed line' flag for `line` (vim's `diff_check()` /
    /// `diff_check_fill()`).
    ///
    /// `line` is a **zero-based** buffer line index, like [`Editor::cursor`]
    /// and [`crate::window::Window::top_line`]; vim's one-based `lnum` is
    /// `line + 1`. Lines of a window that is not in diff mode, and lines past
    /// the end of the buffer, report `(0, false)`.
    ///
    /// This only borrows the editor: a stale cache is recomputed on the fly
    /// instead of being written back, so an editor that wants the cache refreshed
    /// after an edit calls [`Editor::diff_update`].
    pub fn diff_filler(&self, win: WindowId, line: usize) -> (usize, bool) {
        match resolved_state(self, win) {
            Some(state) => state.status(line),
            None => (0, false),
        }
    }

    /// The other diff windows for the buffer shown in `win`.
    ///
    /// These are the windows `:diffget`/`:diffput` and `]c` work against,
    /// limited to the tabpage of `win` (vim keeps diffs per tabpage). Windows
    /// showing the same buffer as `win` are not counterparts - they already show
    /// the other side of the comparison.
    pub fn diff_counterparts(&self, win: WindowId) -> Vec<WindowId> {
        let Some(own) = bufnr_of(self, win) else {
            return Vec::new();
        };
        if !window_diff_flag(self, win) {
            return Vec::new();
        }
        tab_windows(self, win)
            .into_iter()
            .filter(|id| {
                *id != win
                    && window_diff_flag(self, *id)
                    && bufnr_of(self, *id).is_some_and(|bufnr| bufnr != own)
            })
            .collect()
    }

    /// First changed line after (`forward`) or before (`!forward`) `line`.
    ///
    /// This is the `]c`/`[c` motion: vim's `diff_move_to()` with a count of
    /// one. `line` is zero-based; the result is the start of the next changed
    /// block, clamped to the last line of the buffer. `None` means the cursor is
    /// already past the last change (forward) or before the first one (backward).
    pub fn diff_next_change(&self, win: WindowId, line: usize, forward: bool) -> Option<usize> {
        let state = resolved_state(self, win)?;
        let mut starts: Vec<usize> = state
            .pairs
            .iter()
            .flat_map(|pair| pair.blocks.iter().map(|block| block.a_start))
            .collect();
        starts.sort_unstable();
        starts.dedup();
        if forward {
            let last = state.line_count().saturating_sub(1);
            let target = starts
                .iter()
                .copied()
                .find(|start| *start > line)?
                .min(last);
            // Do not report a "move" to the line the cursor is already on.
            (target > line).then_some(target)
        } else {
            starts.iter().rev().copied().find(|start| *start < line)
        }
    }

    /// `:diffget` - copy the changed block under the cursor from the buffer in
    /// `target` into the current buffer.
    ///
    /// The block is inserted (or deleted, or replaced) as one undo step with
    /// marks adjusted, and every diff cache is refreshed afterwards. When `reg`
    /// is given the copied lines are also yanked into that register.
    ///
    /// # Errors
    ///
    /// `E99` when the current window is not in diff mode, `E100` when `target`
    /// is not a diff window, and whatever the buffer edit reports.
    pub fn diff_get(&mut self, target: WindowId, reg: Option<char>) -> VimResult<()> {
        self.copy_change_block(target, reg, true)
    }

    /// `:diffput` - copy the changed block under the cursor from the current
    /// buffer into the buffer shown in `target`.
    ///
    /// The target buffer gets the edit (and the undo step); the current window
    /// and cursor are left where they were. `reg` yanks the copied lines into
    /// that register, as in [`Editor::diff_get`].
    ///
    /// # Errors
    ///
    /// `E99`/`E100` as for [`Editor::diff_get`].
    pub fn diff_put(&mut self, target: WindowId, reg: Option<char>) -> VimResult<()> {
        self.copy_change_block(target, reg, false)
    }

    /// Set every option `diff_win_options()` sets for `win` (without the diff
    /// recomputation, which [`Editor::diff_this`] does itself).
    fn diff_enable_window(&mut self, win: WindowId) -> VimResult<()> {
        self.set_window_option(win, "diff", OptValue::Bool(true))?;
        self.set_window_option(win, "foldmethod", OptValue::Str("diff".to_string()))?;
        self.set_window_option(win, "foldenable", OptValue::Bool(true))?;
        self.set_window_option(win, "foldlevel", OptValue::Num(0))?;
        self.set_window_option(win, "scrollbind", OptValue::Bool(true))?;
        self.set_window_option(win, "cursorbind", OptValue::Bool(true))?;
        self.set_window_option(win, "wrap", OptValue::Bool(false))?;
        Ok(())
    }

    /// Shared body of `:diffget` (`get == true`) and `:diffput`.
    fn copy_change_block(
        &mut self,
        target: WindowId,
        reg: Option<char>,
        get: bool,
    ) -> VimResult<()> {
        let cur = self.current_win_id();
        let Some(own_buf) = bufnr_of(self, cur) else {
            return Err(VimError::new("E99", "Current buffer is not in diff mode"));
        };
        if !window_diff_flag(self, cur) {
            return Err(VimError::new("E99", "Current buffer is not in diff mode"));
        }
        if !window_diff_flag(self, target) {
            return Err(VimError::new("E100", "No other buffer in diff mode"));
        }
        let Some(other_buf) = bufnr_of(self, target) else {
            return Err(VimError::new("E100", "No other buffer in diff mode"));
        };
        if other_buf == own_buf {
            return Ok(());
        }
        let own_text = buffer_text(self, own_buf)
            .ok_or_else(|| VimError::new("E99", "Current buffer is not in diff mode"))?;
        let other_text = buffer_text(self, other_buf)
            .ok_or_else(|| VimError::new("E100", "No other buffer in diff mode"))?;
        let blocks = rvim_diff::change_blocks(&own_text, &other_text);
        let line = self.cursor().line;
        let Some(block) = block_at_line(&blocks, line) else {
            // No change under (or right next to) the cursor: vim does nothing.
            return Ok(());
        };
        let (source_start, source_len, dest_start, dest_len) = if get {
            (block.b_start, block.b_len, block.a_start, block.a_len)
        } else {
            (block.a_start, block.a_len, block.b_start, block.b_len)
        };
        let source = if get { &other_text } else { &own_text };
        let mut new_lines: Vec<String> = source
            .get(source_start..source_start + source_len)
            .unwrap_or_default()
            .to_vec();
        if let Some(name) = reg {
            let text = new_lines.join("\n") + "\n";
            self.registers.yank(
                Some(name),
                Register {
                    text,
                    reg_type: RegType::LineWise,
                },
            );
        }
        let dest_buf = if get { own_buf } else { other_buf };
        let dest_total = self
            .buffer(dest_buf)
            .map(|buf| buf.line_count())
            .unwrap_or(0);
        if new_lines.is_empty() && dest_start == 0 && dest_len >= dest_total {
            // Never leave a buffer without lines: vim keeps one empty line.
            new_lines.push(String::new());
        }
        if get {
            self.replace_lines(dest_start, dest_start + dest_len, new_lines)?;
        } else {
            let saved_cursor = self.windows.get(target).map(|window| window.cursor);
            self.goto_window(target)?;
            let result = self.replace_lines(dest_start, dest_start + dest_len, new_lines);
            if let Some(pos) = saved_cursor {
                self.set_cursor(pos);
            }
            let back = self.goto_window(cur);
            result?;
            back?;
        }
        self.diff_update()
    }
}

/// `'diffopt'` "context" default: a line this close to a change stays unfolded.
const DIFF_CONTEXT: usize = 6;

/// Recompute the `'foldmethod=diff'` folds of `win` (vim's `diff_fold_update()`
/// and `foldlevelDiff()`).
///
/// Every run of lines that is further than [DIFF_CONTEXT] lines away from any
/// change becomes one closed level-1 fold, which is what makes `vimdiff` collapse
/// the text both views agree on. `'foldminlines'` is honoured. Windows that are
/// not in diff mode, or whose `'foldmethod'` is no longer "diff", keep whatever
/// folds they have.
fn refresh_diff_folds(ed: &mut Editor, win: WindowId) {
    if !window_diff_flag(ed, win) {
        return;
    }
    let method = ed
        .wopt_for(win, "foldmethod")
        .ok()
        .and_then(|value| value.as_str().map(str::to_string));
    if method.as_deref() != Some("diff") {
        return;
    }
    let folds = diff_folds(ed, win);
    ed.fold_store.set(win, folds);
}

/// The `'foldmethod=diff'` folds of `win`.
///
/// A line stays unfolded when it lies inside a changed block or within
/// [DIFF_CONTEXT] lines of one (vim's `diff_infold()`); all other lines are
/// grouped into closed level-1 folds.
fn diff_folds(ed: &Editor, win: WindowId) -> Vec<Fold> {
    let Some(state) = resolved_state(ed, win) else {
        return Vec::new();
    };
    if state.bufs.len() < 2 || state.pairs.is_empty() {
        // vim's diff_infold(): without another diff buffer there is nothing to
        // fold against, so every line keeps foldlevel 0.
        return Vec::new();
    }
    let total = state.line_count();
    if total == 0 {
        return Vec::new();
    }
    let mut near = vec![false; total];
    for pair in &state.pairs {
        for block in &pair.blocks {
            let first = block.a_start.saturating_sub(DIFF_CONTEXT);
            let last = (block.a_start + block.a_len + DIFF_CONTEXT).min(total - 1);
            for flag in near.iter_mut().take(last + 1).skip(first) {
                *flag = true;
            }
        }
    }
    let min_lines = ed.wopt_for_num(win, "foldminlines", 1).max(1) as usize;
    let mut folds = Vec::new();
    let mut line = 0;
    while line < total {
        if near[line] {
            line += 1;
            continue;
        }
        let start = line;
        while line < total && !near[line] {
            line += 1;
        }
        let end = line - 1;
        if end - start + 1 >= min_lines {
            folds.push(Fold::new(start, end, 1, true));
        }
    }
    folds
}

/// The window-local `'diff'` value of `win` (vim's `w_p_diff`).
///
/// Only the window-local value counts: `'diff'` has no global copy in vim, and
/// reading the global fallback would report every window as diffing after a
/// `:setglobal diff`.
fn window_diff_flag(ed: &Editor, win: WindowId) -> bool {
    ed.windows
        .get(win)
        .and_then(|window| window.get_local("diff"))
        .and_then(OptValue::as_bool)
        .unwrap_or(false)
}

/// Buffer shown by `win`, `None` for a window that no longer exists.
fn bufnr_of(ed: &Editor, win: WindowId) -> Option<i32> {
    ed.windows.get(win).map(|window| window.bufnr)
}

/// Windows of the tabpage containing `win`, in split-tree order.
fn tab_windows(ed: &Editor, win: WindowId) -> Vec<WindowId> {
    ed.windows
        .tab_of(win)
        .and_then(|index| ed.windows.tabs.get(index))
        .map(|tab| tab.layout.leaves())
        .unwrap_or_default()
}

/// Windows of every tabpage, in tabpage and split-tree order.
fn all_window_ids(ed: &Editor) -> Vec<WindowId> {
    let mut ids = Vec::new();
    for tab in &ed.windows.tabs {
        ids.extend(tab.layout.leaves());
    }
    ids
}

/// The distinct buffers diffed in `win`'s tabpage, in window-tree order.
///
/// The first entry is the base buffer that every other buffer is compared with.
fn participants(ed: &Editor, win: WindowId) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    for id in tab_windows(ed, win) {
        if !window_diff_flag(ed, id) {
            continue;
        }
        let Some(bufnr) = bufnr_of(ed, id) else {
            continue;
        };
        if !out.contains(&bufnr) {
            out.push(bufnr);
        }
    }
    out
}

/// All lines of `bufnr` as owned strings (the input of the diff engine).
fn buffer_text(ed: &Editor, bufnr: i32) -> Option<Vec<String>> {
    let buffer = ed.buffer(bufnr).ok()?;
    Some(
        buffer
            .lines()
            .iter()
            .map(|line| line.as_str().to_string())
            .collect(),
    )
}

/// Compute the diff state of `win` from the current buffer texts.
///
/// Returns `None` when the window is not in diff mode, is gone, or shows a
/// buffer that cannot be read.
fn compute_state(ed: &Editor, win: WindowId) -> Option<DiffState> {
    let own = bufnr_of(ed, win)?;
    if !window_diff_flag(ed, win) {
        return None;
    }
    let parts = participants(ed, win);
    let mut bufs = Vec::with_capacity(parts.len());
    for bufnr in &parts {
        let buffer = ed.buffer(*bufnr).ok()?;
        bufs.push(DiffBuffer {
            bufnr: *bufnr,
            changedtick: buffer.changedtick(),
            lines: buffer.line_count(),
        });
    }
    let own_text = buffer_text(ed, own)?;
    let base = parts.first().copied().unwrap_or(own);
    // Vim diffs the base buffer against every other buffer; a window that does
    // not show the base itself only has to be compared with the base.
    let others: Vec<i32> = if own == base {
        parts.get(1..).unwrap_or_default().to_vec()
    } else {
        vec![base]
    };
    let mut pairs = Vec::with_capacity(others.len());
    for other in others {
        if other == own {
            continue;
        }
        let other_text = buffer_text(ed, other)?;
        pairs.push(DiffPair {
            bufnr: other,
            blocks: rvim_diff::change_blocks(&own_text, &other_text),
        });
    }
    Some(DiffState {
        bufnr: own,
        bufs,
        pairs,
    })
}

/// Whether `state` still describes the buffers of `win`.
fn is_current(state: &DiffState, ed: &Editor, win: WindowId) -> bool {
    if bufnr_of(ed, win) != Some(state.bufnr) {
        return false;
    }
    let parts = participants(ed, win);
    if parts.len() != state.bufs.len() {
        return false;
    }
    for (part, cached) in parts.iter().zip(state.bufs.iter()) {
        if *part != cached.bufnr {
            return false;
        }
        match ed.buffer(*part) {
            Ok(buffer)
                if buffer.changedtick() == cached.changedtick
                    && buffer.line_count() == cached.lines => {}
            _ => return false,
        }
    }
    true
}

/// The cached state of `win`, whether stale or not.
fn cached_state(ed: &Editor, win: WindowId) -> Option<DiffState> {
    let text = ed.windows.get(win)?.get_local(STATE_KEY)?.as_str()?;
    decode(text)
}

/// Memoise `state` in the window's local option map.
fn store_state(ed: &mut Editor, win: WindowId, state: &DiffState) {
    if let Some(window) = ed.windows.get_mut(win) {
        window.set_local(STATE_KEY, OptValue::Str(encode(state)));
    }
}

/// Drop the cached state of `win`.
fn clear_state(ed: &mut Editor, win: WindowId) {
    if let Some(window) = ed.windows.get_mut(win) {
        window.local.remove(STATE_KEY);
    }
}

/// The state to answer a `&self` query with: the cache when it is still valid,
/// a freshly computed state otherwise.
fn resolved_state(ed: &Editor, win: WindowId) -> Option<DiffState> {
    if !window_diff_flag(ed, win) {
        return None;
    }
    if let Some(state) = cached_state(ed, win) {
        if is_current(&state, ed, win) {
            return Some(state);
        }
    }
    compute_state(ed, win)
}

/// Filler rows above `line` and the changed flag for one pairwise diff.
fn pair_status(blocks: &[ChangeBlock], line: usize) -> (usize, bool) {
    for block in blocks {
        if block.a_len == 0 {
            // A pure insertion: the line under the insertion point carries the
            // "added/deleted" mark and the filler rows (vim's -2 return value).
            if line == block.a_start {
                return (block.b_len, true);
            }
            continue;
        }
        let end = block.a_start + block.a_len;
        if line >= block.a_start && line < end {
            return (0, true);
        }
        if line == end {
            // The line below the block gets the fillers, highlighted or not.
            return (block.b_len.saturating_sub(block.a_len), false);
        }
    }
    (0, false)
}

/// The changed block that contains (or is adjacent to) `line`.
///
/// `:diffget`/`:diffput` without a range work on the cursor line plus the
/// line above it, and on an insertion below the cursor on the last line, exactly
/// like vim's `ex_diffgetput()` widening of the range.
fn block_at_line(blocks: &[ChangeBlock], line: usize) -> Option<ChangeBlock> {
    block_containing(blocks, line)
        .or_else(|| {
            line.checked_sub(1)
                .and_then(|above| block_containing(blocks, above))
        })
        .or_else(|| {
            blocks
                .iter()
                .copied()
                .find(|block| block.a_len == 0 && block.a_start == line + 1)
        })
}

/// The changed block whose own-side range contains `line`.
fn block_containing(blocks: &[ChangeBlock], line: usize) -> Option<ChangeBlock> {
    blocks.iter().copied().find(|block| {
        if block.a_len == 0 {
            line == block.a_start
        } else {
            line >= block.a_start && line < block.a_start + block.a_len
        }
    })
}

/// Serialise a [`DiffState`] for the window-local cache.
///
/// The format is a `;`-separated token list: `v1`, one `o,<bufnr>`, one
/// `b,<bufnr>,<tick>,<lines>` per participating buffer, and per pair a
/// `p,<bufnr>` header followed by one `k,<a_start>,<a_len>,<b_start>,<b_len>`
/// per changed block. Everything is decimal, so decoding is trivial and cannot
/// depend on the locale.
fn encode(state: &DiffState) -> String {
    let mut out = String::from("v1");
    out.push_str(&format!(";o,{}", state.bufnr));
    for buffer in &state.bufs {
        out.push_str(&format!(
            ";b,{},{},{}",
            buffer.bufnr, buffer.changedtick, buffer.lines
        ));
    }
    for pair in &state.pairs {
        out.push_str(&format!(";p,{}", pair.bufnr));
        for block in &pair.blocks {
            out.push_str(&format!(
                ";k,{},{},{},{}",
                block.a_start, block.a_len, block.b_start, block.b_len
            ));
        }
    }
    out
}

/// Read back an [`encode`]d state; `None` when the text is not ours.
///
/// A malformed value (an old version, a user who poked at the option map) is
/// simply treated as "no cache" and recomputed, never as a panic.
fn decode(text: &str) -> Option<DiffState> {
    let mut tokens = text.split(';');
    if tokens.next()? != "v1" {
        return None;
    }
    let mut state = DiffState {
        bufnr: 0,
        bufs: Vec::new(),
        pairs: Vec::new(),
    };
    let mut own_seen = false;
    for token in tokens {
        let mut fields = token.split(',');
        match fields.next()? {
            "o" => {
                state.bufnr = fields.next()?.parse().ok()?;
                own_seen = true;
            }
            "b" => state.bufs.push(DiffBuffer {
                bufnr: fields.next()?.parse().ok()?,
                changedtick: fields.next()?.parse().ok()?,
                lines: fields.next()?.parse().ok()?,
            }),
            "p" => state.pairs.push(DiffPair {
                bufnr: fields.next()?.parse().ok()?,
                blocks: Vec::new(),
            }),
            "k" => {
                let block = ChangeBlock {
                    a_start: fields.next()?.parse().ok()?,
                    a_len: fields.next()?.parse().ok()?,
                    b_start: fields.next()?.parse().ok()?,
                    b_len: fields.next()?.parse().ok()?,
                };
                state.pairs.last_mut()?.blocks.push(block);
            }
            _ => return None,
        }
    }
    if !own_seen || state.bufs.is_empty() {
        return None;
    }
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_core::Pos;

    /// Replace the whole text of the current buffer.
    fn set_text(ed: &mut Editor, lines: &[&str]) {
        let after: Vec<String> = lines.iter().map(|text| (*text).to_string()).collect();
        let total = ed.current_buffer().line_count();
        ed.replace_lines(0, total, after).unwrap();
    }

    /// Lines of a buffer, for comparing edits.
    fn lines_of(ed: &Editor, bufnr: i32) -> Vec<String> {
        ed.buffer(bufnr)
            .unwrap()
            .lines()
            .iter()
            .map(|line| line.as_str().to_string())
            .collect()
    }

    /// Two windows showing two different buffers, both in diff mode.
    ///
    /// Returns (editor, window showing `a`, window showing `b`). The split puts
    /// the second window on top, so `b` is the base buffer of the comparison.
    fn two_way(a: &[&str], b: &[&str]) -> (Editor, WindowId, WindowId) {
        let mut ed = Editor::new();
        set_text(&mut ed, a);
        let left = ed.current_win_id();
        let right = ed.split_window(false).unwrap();
        let bufnr = ed.new_buffer();
        ed.switch_buffer(bufnr).unwrap();
        set_text(&mut ed, b);
        ed.diff_this().unwrap();
        ed.goto_window(left).unwrap();
        ed.diff_this().unwrap();
        (ed, left, right)
    }

    /// `&[String]` as the `&[&str]` the text helpers take.
    fn refs(lines: &[String]) -> Vec<&str> {
        lines.iter().map(String::as_str).collect()
    }

    #[test]
    fn diff_this_sets_the_window_options() {
        let mut ed = Editor::new();
        let win = ed.current_win_id();
        assert!(!ed.in_diff_mode(win));
        ed.diff_this().unwrap();
        assert!(ed.in_diff_mode(win));
        assert_eq!(
            ed.windows.get(win).unwrap().get_local("diff"),
            Some(&OptValue::Bool(true))
        );
        assert_eq!(
            ed.wopt_for(win, "foldmethod").unwrap().as_str(),
            Some("diff")
        );
        assert_eq!(
            ed.wopt_for(win, "foldenable").unwrap().as_bool(),
            Some(true)
        );
        assert_eq!(ed.wopt_for(win, "foldlevel").unwrap().as_num(), Some(0));
        assert_eq!(
            ed.wopt_for(win, "scrollbind").unwrap().as_bool(),
            Some(true)
        );
        assert_eq!(
            ed.wopt_for(win, "cursorbind").unwrap().as_bool(),
            Some(true)
        );
        assert_eq!(ed.wopt_for(win, "wrap").unwrap().as_bool(), Some(false));
        assert!(ed.diff_counterparts(win).is_empty());
        assert_eq!(ed.diff_filler(win, 0), (0, false));
        assert!(ed.diff_next_change(win, 0, true).is_none());
        assert!(
            ed.folds_for(win).is_empty(),
            "a lone diff window has nothing to fold against"
        );
    }

    #[test]
    fn fillers_align_the_shorter_side() {
        let (ed, left, right) = two_way(&["1", "2", "3"], &["1", "x", "y", "2", "3"]);
        // The insertion sits after line "1" of the left window: two filler rows
        // above its line index 1, which is also the "added/deleted" line.
        assert_eq!(ed.diff_filler(left, 1), (2, true));
        assert_eq!(ed.diff_filler(left, 0), (0, false));
        assert_eq!(ed.diff_filler(left, 3), (0, false));
        // The window with the extra lines needs no fillers; the inserted lines
        // are the changed ones.
        assert_eq!(ed.diff_filler(right, 1), (0, true));
        assert_eq!(ed.diff_filler(right, 2), (0, true));
        assert_eq!(ed.diff_filler(right, 3), (0, false));
        assert_eq!(ed.diff_filler(left, 99), (0, false), "past the end");
        assert_eq!(ed.diff_filler(-1, 0), (0, false), "no such window");
    }

    #[test]
    fn a_replaced_line_is_changed_on_both_sides() {
        let (ed, left, right) = two_way(&["a", "b", "c"], &["a", "B", "c"]);
        assert_eq!(ed.diff_filler(left, 1), (0, true));
        assert_eq!(ed.diff_filler(right, 1), (0, true));
        assert_eq!(ed.diff_filler(left, 0), (0, false));
        assert_eq!(ed.diff_filler(left, 2), (0, false));
        assert_eq!(ed.diff_filler(right, 2), (0, false));
    }

    #[test]
    fn diff_off_clears_the_mode_and_the_fill() {
        let (mut ed, left, right) = two_way(&["1", "2"], &["1", "x", "2"]);
        assert_eq!(ed.diff_filler(left, 1).0, 1);
        ed.goto_window(left).unwrap();
        ed.diff_off(false).unwrap();
        assert!(!ed.in_diff_mode(left));
        assert_eq!(ed.diff_filler(left, 1), (0, false));
        assert_eq!(
            ed.wopt_for(left, "foldmethod").unwrap().as_str(),
            Some("manual")
        );
        assert_eq!(
            ed.wopt_for(left, "foldenable").unwrap().as_bool(),
            Some(false)
        );
        assert!(
            ed.in_diff_mode(right),
            "the other window stays in diff mode"
        );
        assert!(ed.diff_counterparts(right).is_empty());
        ed.diff_off(true).unwrap();
        assert!(!ed.in_diff_mode(right));
        assert_eq!(ed.diff_filler(right, 1), (0, false));
    }

    #[test]
    fn diff_next_change_walks_the_changed_blocks() {
        let a: Vec<String> = (0..10).map(|n| format!("l{n}")).collect();
        let mut b = a.clone();
        b[2] = "two".to_string();
        b[7] = "seven".to_string();
        let (ed, left, _right) = two_way(&refs(&a), &refs(&b));
        assert_eq!(ed.diff_next_change(left, 0, true), Some(2));
        assert_eq!(ed.diff_next_change(left, 2, true), Some(7));
        assert_eq!(ed.diff_next_change(left, 7, true), None);
        assert_eq!(ed.diff_next_change(left, 9, true), None);
        assert_eq!(ed.diff_next_change(left, 9, false), Some(7));
        assert_eq!(ed.diff_next_change(left, 3, false), Some(2));
        assert_eq!(ed.diff_next_change(left, 2, false), None);
    }

    #[test]
    fn the_cache_follows_edits() {
        let (mut ed, left, _right) = two_way(&["1", "2", "3"], &["1", "x", "y", "2", "3"]);
        assert_eq!(ed.diff_filler(left, 1), (2, true));
        // Insert a line into the left buffer without calling diff_update(): the
        // cached ticks no longer match, so the query recomputes from the text.
        ed.goto_window(left).unwrap();
        ed.replace_lines(1, 1, vec!["new".to_string()]).unwrap();
        assert_eq!(ed.diff_filler(left, 1), (0, true));
        assert_eq!(ed.diff_filler(left, 2), (1, false), "one filler above '2'");
        ed.diff_update().unwrap();
        assert_eq!(ed.diff_filler(left, 2), (1, false));
    }

    #[test]
    fn diff_get_copies_the_block_as_one_undo_step() {
        let (mut ed, left, right) = two_way(&["1", "2", "3"], &["1", "x", "y", "2", "3"]);
        let left_buf = ed.windows.get(left).unwrap().bufnr;
        ed.goto_window(left).unwrap();
        ed.set_cursor(Pos::new(1, 0));
        ed.diff_get(right, None).unwrap();
        assert_eq!(lines_of(&ed, left_buf), vec!["1", "x", "y", "2", "3"]);
        assert_eq!(ed.diff_filler(left, 1), (0, false), "the change is gone");
        ed.undo().unwrap();
        assert_eq!(lines_of(&ed, left_buf), vec!["1", "2", "3"]);
    }

    #[test]
    fn diff_get_can_yank_the_copied_lines() {
        let (mut ed, left, right) = two_way(&["1", "2", "3"], &["1", "x", "y", "2", "3"]);
        ed.goto_window(left).unwrap();
        ed.set_cursor(Pos::new(1, 0));
        ed.diff_get(right, Some('a')).unwrap();
        let register = ed.registers.get('a').expect("register a");
        assert_eq!(register.text, "x\ny\n");
    }

    #[test]
    fn diff_put_writes_into_the_other_buffer() {
        let (mut ed, left, right) = two_way(&["1", "2", "3"], &["1", "x", "2", "3"]);
        let left_buf = ed.windows.get(left).unwrap().bufnr;
        ed.goto_window(right).unwrap();
        ed.set_cursor(Pos::new(1, 0));
        ed.diff_put(left, None).unwrap();
        assert_eq!(lines_of(&ed, left_buf), vec!["1", "x", "2", "3"]);
        assert_eq!(
            ed.current_win_id(),
            right,
            "diffput keeps the current window"
        );
        assert_eq!(ed.cursor().line, 1, "and its cursor");
        assert_eq!(ed.diff_filler(left, 1), (0, false), "the change is gone");
    }

    #[test]
    fn diff_get_needs_a_diffed_counterpart() {
        let mut ed = Editor::new();
        let first = ed.current_win_id();
        assert_eq!(
            ed.diff_get(first, None).unwrap_err().code,
            "E99",
            "the current window is not in diff mode"
        );
        ed.diff_this().unwrap();
        // A split inherits 'diff', and both windows show the same buffer, so
        // there is nothing to copy and nothing to report.
        let second = ed.split_window(false).unwrap();
        ed.diff_get(second, None).unwrap();
        // A window that is not diffing is not a valid source.
        let plain = ed.new_buffer();
        ed.switch_buffer(plain).unwrap();
        ed.set_window_option(second, "diff", OptValue::Bool(false))
            .unwrap();
        ed.goto_window(first).unwrap();
        assert_eq!(ed.diff_get(second, None).unwrap_err().code, "E100");
    }

    #[test]
    fn counterparts_exclude_windows_showing_the_same_buffer() {
        let mut ed = Editor::new();
        set_text(&mut ed, &["a", "b"]);
        let first = ed.current_win_id();
        let second = ed.split_window(false).unwrap();
        ed.diff_this().unwrap();
        ed.goto_window(first).unwrap();
        ed.diff_this().unwrap();
        assert!(
            ed.diff_counterparts(first).is_empty(),
            "two windows on one buffer have nothing to compare"
        );
        ed.goto_window(second).unwrap();
        let bufnr = ed.new_buffer();
        ed.switch_buffer(bufnr).unwrap();
        set_text(&mut ed, &["a", "B"]);
        assert_eq!(ed.diff_counterparts(first), vec![second]);
        assert_eq!(ed.diff_counterparts(second), vec![first]);
        assert_eq!(ed.diff_filler(second, 1), (0, true));
    }

    #[test]
    fn three_buffers_compare_against_the_base() {
        let mut ed = Editor::new();
        set_text(&mut ed, &["1", "2", "3"]);
        let base_win = ed.current_win_id();
        ed.diff_this().unwrap();
        let mid = ed.split_window(false).unwrap();
        let mid_buf = ed.new_buffer();
        ed.switch_buffer(mid_buf).unwrap();
        set_text(&mut ed, &["1", "x", "2", "3"]);
        ed.diff_this().unwrap();
        let top = ed.split_window(false).unwrap();
        let top_buf = ed.new_buffer();
        ed.switch_buffer(top_buf).unwrap();
        set_text(&mut ed, &["1", "y", "z", "2", "3"]);
        ed.diff_this().unwrap();
        // The tree order is [top, mid, base_win], so the last-created window
        // shows the base buffer: the other two compare against it.
        for win in [top, mid, base_win] {
            assert_eq!(ed.diff_counterparts(win).len(), 2, "window {win}");
        }
        // The "mid" buffer has one line where the base has two, so it shows one
        // filler above its "2" line; the shortest buffer misses both extra lines
        // and shows two. The base itself needs none.
        assert_eq!(ed.diff_filler(mid, 2), (1, false));
        assert_eq!(ed.diff_filler(base_win, 1), (2, true));
        assert_eq!(ed.diff_filler(top, 1), (0, true));
        assert_eq!(ed.diff_filler(top, 2), (0, true));
        assert_eq!(ed.diff_filler(top, 3), (0, false));
    }

    #[test]
    fn diff_mode_accepts_at_most_four_buffers() {
        let mut ed = Editor::new();
        let mut wins = vec![ed.current_win_id()];
        for _ in 0..MAX_DIFF_BUFFERS {
            let tallest = ed
                .windows
                .ids()
                .into_iter()
                .max_by_key(|id| ed.windows.get(*id).map(|window| window.height))
                .expect("a window");
            ed.goto_window(tallest).unwrap();
            let win = ed.split_window(false).unwrap();
            let bufnr = ed.new_buffer();
            ed.switch_buffer(bufnr).unwrap();
            wins.push(win);
        }
        // Splits inherit the window-local options; start from a clean slate.
        for win in &wins {
            ed.set_window_option(*win, "diff", OptValue::Bool(false))
                .unwrap();
        }
        for (index, win) in wins.iter().enumerate() {
            ed.goto_window(*win).unwrap();
            if index < MAX_DIFF_BUFFERS {
                ed.diff_this().unwrap();
            } else {
                assert_eq!(ed.diff_this().unwrap_err().code, "E96");
            }
        }
    }

    #[test]
    fn diff_mode_folds_the_unchanged_lines() {
        // 20 lines with one change near the top: the first line of a change
        // and 'diffopt' context (6 lines) of it stay unfolded, the rest folds.
        let a: Vec<String> = (0..20).map(|n| format!("l{n}")).collect();
        let mut b = a.clone();
        b[1] = "changed".to_string();
        let (mut ed, left, _right) = two_way(&refs(&a), &refs(&b));
        let folds = ed.folds_for(left).to_vec();
        assert_eq!(folds.len(), 1, "one folded tail: {folds:?}");
        assert_eq!((folds[0].start, folds[0].end), (9, 19));
        assert_eq!(folds[0].level, 1);
        assert!(folds[0].closed, "diff folds start closed (fdl=0)");
        ed.goto_window(left).unwrap();
        ed.diff_off(false).unwrap();
        assert!(
            ed.folds_for(left).is_empty(),
            "diffoff drops the diff folds"
        );
    }

    #[test]
    fn undiffed_windows_report_nothing() {
        let ed = Editor::new();
        let win = ed.current_win_id();
        assert!(!ed.in_diff_mode(win));
        assert_eq!(ed.diff_filler(win, 0), (0, false));
        assert!(ed.diff_counterparts(win).is_empty());
        assert_eq!(ed.diff_next_change(win, 0, true), None);
    }

    #[test]
    fn cached_state_round_trips_and_ignores_garbage() {
        let state = DiffState {
            bufnr: 3,
            bufs: vec![
                DiffBuffer {
                    bufnr: 3,
                    changedtick: 7,
                    lines: 4,
                },
                DiffBuffer {
                    bufnr: 5,
                    changedtick: 9,
                    lines: 6,
                },
            ],
            pairs: vec![DiffPair {
                bufnr: 5,
                blocks: vec![ChangeBlock {
                    a_start: 1,
                    a_len: 1,
                    b_start: 2,
                    b_len: 3,
                }],
            }],
        };
        assert_eq!(decode(&encode(&state)), Some(state));
        assert_eq!(decode(""), None);
        assert_eq!(decode("nonsense"), None);
        assert_eq!(decode("v1;o,1"), None, "no participating buffers");
        assert_eq!(decode("v1;o,x;b,1,0,1"), None, "unparsable number");
        assert_eq!(decode("v1;b,1,0,1"), None, "missing the own buffer");
        assert_eq!(
            decode("v1;o,1;b,1,0,1;k,0,0,0,0"),
            None,
            "block without pair"
        );
    }
}
