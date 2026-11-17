//! Per-window folding (reference: fold.c).
//!
//! Vim keeps a separate fold list for every window, so this module stores one
//! [FoldStore] entry per [WindowId] on the [Editor]. A fold covers the
//! inclusive, zero-based line range start..=end and is keyed by its first
//! line; a closed fold hides start + 1..=end from the screen while the first
//! line doubles as the fold text.
//!
//! # Deviations from Vim
//!
//! * 'foldmarker' is not part of the M5 option table, so the marker method
//!   always uses Vim's default "{{{,}}}" pair.
//! * 'foldignore', 'foldnestmax' and 'diffopt' are not in the option table
//!   either. Blank lines are treated as "undefined level" lines (they belong
//!   to the fold above them, like Vim's indent method) and the computed depth
//!   is capped at [MAX_FOLD_LEVEL] (Vim's MAX_LEVEL).
//! * 'foldmethod' syntax, expr and diff are accepted by the option table but
//!   there is no syntax engine, expression evaluator or diff engine at this
//!   layer. Those methods keep the existing (manual) folds and are reported by
//!   [Editor::unsupported_foldmethod] instead of being faked.
//! * A custom 'foldtext' expression is not evaluated; [Editor::fold_text]
//!   always renders Vim's default foldtext() layout.
//! * 'foldminlines' is compared against the number of buffer lines, not
//!   against the number of screen lines a wrapped fold would occupy.
//! * There are no automatic invalidation hooks yet: the TUI (or any caller)
//!   must call [Editor::refresh_folds_for_current_window] after edits, after a
//!   change to 'foldmethod'/'shiftwidth'/'tabstop'/'foldminlines'/'foldlevel'
//!   and after switching windows. That method is O(lines) for the indent and
//!   marker methods and O(1) otherwise.
//! * Creation is forgiving: a new manual fold that partially overlaps an
//!   existing fold is widened to their union, while a fold fully inside (or
//!   fully containing) another one simply nests (Vim moves contained folds one
//!   level deeper). Folds whose range runs past the end of the buffer are
//!   clamped.

use std::collections::HashMap;

use rvim_core::{VimError, VimResult};

use crate::editor::Editor;
use crate::options::OptValue;
use crate::window::WindowId;

/// Maximum fold depth (Vim's fold.c MAX_LEVEL).
pub const MAX_FOLD_LEVEL: usize = 20;

/// One fold: an inclusive, zero-based line range and its nesting level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    /// First line of the fold (zero-based, inclusive).
    pub start: usize,
    /// Last line of the fold (zero-based, inclusive).
    pub end: usize,
    /// Nesting level, 1 for a top-level fold (Vim's fd_level).
    pub level: usize,
    /// True when the fold is closed; a closed fold hides start + 1..=end.
    pub closed: bool,
}

impl Fold {
    /// Build a fold from its already-clamped line range.
    pub const fn new(start: usize, end: usize, level: usize, closed: bool) -> Self {
        Self {
            start,
            end,
            level,
            closed,
        }
    }

    /// Number of buffer lines covered by the fold (at least one).
    pub fn line_count(&self) -> usize {
        self.end.saturating_sub(self.start) + 1
    }

    /// Is line inside this fold (inclusive)?
    pub fn contains(&self, line: usize) -> bool {
        line >= self.start && line <= self.end
    }
}

/// The fold lists of every window, keyed by [WindowId].
///
/// Lists are kept sorted in line order: by first line, then outer before inner
/// (larger end first) and finally by level. Missing windows have no folds.
#[derive(Clone, Debug, Default)]
pub struct FoldStore {
    per_window: HashMap<WindowId, Vec<Fold>>,
}

impl FoldStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of windows that have at least one fold.
    pub fn window_count(&self) -> usize {
        self.per_window.len()
    }

    /// True when no window has any fold.
    pub fn is_empty(&self) -> bool {
        self.per_window.is_empty()
    }

    /// The folds of win, in line order (an empty slice when it has none).
    pub fn get(&self, win: WindowId) -> &[Fold] {
        self.per_window.get(&win).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Replace the folds of win; the list is sorted before it is stored.
    pub fn set(&mut self, win: WindowId, mut folds: Vec<Fold>) {
        sort_folds(&mut folds);
        if folds.is_empty() {
            self.per_window.remove(&win);
        } else {
            self.per_window.insert(win, folds);
        }
    }

    /// Drop the folds of win.
    pub fn clear(&mut self, win: WindowId) {
        self.per_window.remove(&win);
    }

    /// Drop the folds of windows for which keep returns false (used when
    /// windows are closed between refreshes).
    pub fn retain_windows(&mut self, keep: impl Fn(WindowId) -> bool) {
        self.per_window.retain(|id, _| keep(*id));
    }

    fn get_mut(&mut self, win: WindowId) -> Option<&mut Vec<Fold>> {
        self.per_window.get_mut(&win)
    }
}

/// Sort folds in line order, outer folds before the folds nested in them.
fn sort_folds(folds: &mut [Fold]) {
    folds.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| b.end.cmp(&a.end))
            .then_with(|| a.level.cmp(&b.level))
    });
}

/// True when a sorts before b (same ordering as [sort_folds]).
fn fold_before(a: &Fold, b: &Fold) -> bool {
    a.start < b.start
        || (a.start == b.start && (a.end > b.end || (a.end == b.end && a.level <= b.level)))
}

/// Close every open fold whose level is at least min_level, ending at end.
fn close_open_folds(
    out: &mut Vec<Fold>,
    open: &mut Vec<(usize, usize)>,
    min_level: usize,
    end: usize,
) {
    while let Some(&(level, start)) = open.last() {
        if level < min_level {
            break;
        }
        open.pop();
        if end >= start {
            out.push(Fold::new(start, end, level, false));
        }
    }
}

/// Read an optional level digit following a fold marker.
fn marker_level(bytes: &[u8], after: usize) -> (Option<usize>, usize) {
    let mut index = after;
    let mut value = 0usize;
    let mut any = false;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        any = true;
        value = value
            .saturating_mul(10)
            .saturating_add((bytes[index] - b'0') as usize)
            .min(MAX_FOLD_LEVEL);
        index += 1;
    }
    if any {
        (Some(value), index - after)
    } else {
        (None, 0)
    }
}

/// Width in columns of the leading white space of line, expanding tabs.
fn indent_cols(line: &str, tabstop: usize) -> usize {
    let tabstop = tabstop.max(1);
    let mut col = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col += tabstop - (col % tabstop),
            _ => break,
        }
    }
    col
}

/// Strip the leading comment opener, fold markers and trailing comment closer
/// from the first line of a fold (the useful subset of foldtext_cleanup()).
fn clean_fold_text(first_line: &str) -> String {
    let mut text = first_line.trim_start();
    let mut comment = false;
    if let Some(rest) = text.strip_prefix("/*").or_else(|| text.strip_prefix("//")) {
        text = rest.trim_start();
        comment = true;
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    while at < bytes.len() {
        let marker = bytes[at..].starts_with(b"{{{") || bytes[at..].starts_with(b"}}}");
        if marker {
            at += 3;
            while at < bytes.len() && bytes[at].is_ascii_digit() {
                at += 1;
            }
            while at < bytes.len() && (bytes[at] == b' ' || bytes[at] == b'\t') {
                at += 1;
            }
            continue;
        }
        let ch = text[at..].chars().next().unwrap_or('\u{0}');
        out.push(ch);
        at += ch.len_utf8();
    }
    let mut out = out.trim_end();
    if comment {
        out = out.strip_suffix("*/").unwrap_or(out).trim_end();
    }
    out.to_string()
}

/// Pure helper (also used by the TUI): Vim's default foldtext() text for a
/// fold of count lines that starts with first_line.
///
/// The layout is Vim's foldtext() at level 1:
/// "+--  3 lines: int main()". Fold markers ({{{ / }}}, with optional digits),
/// a leading "/*" or "//" comment opener and a trailing "*/" are removed;
/// trailing white space is trimmed.
pub fn fold_label(first_line: &str, count: usize) -> String {
    let count = count.max(1);
    let noun = if count == 1 { "line" } else { "lines" };
    let text = clean_fold_text(first_line);
    if text.is_empty() {
        format!("+--{count:>3} {noun}:")
    } else {
        format!("+--{count:>3} {noun}: {text}")
    }
}

impl Editor {
    /// The folds of the current window, in line order.
    pub fn folds(&self) -> &[Fold] {
        self.folds_for(self.windows.current_id())
    }

    /// The folds of win, in line order (an empty slice when it has none).
    pub fn folds_for(&self, win: WindowId) -> &[Fold] {
        self.fold_store.get(win)
    }

    /// Recompute the current window's folds from 'foldmethod'.
    ///
    /// manual, syntax, expr and diff leave the stored folds alone; indent and
    /// marker rebuild them in O(lines). Fold state of windows that no longer
    /// exist is dropped.
    pub fn recompute_folds(&mut self) -> VimResult<()> {
        self.prune_fold_windows();
        let folds = match self.wopt_str("foldmethod", "manual").as_str() {
            "indent" => self.compute_indent_folds(),
            "marker" => self.compute_marker_folds(),
            _ => return Ok(()),
        };
        let win = self.windows.current_id();
        self.fold_store.set(win, folds);
        Ok(())
    }

    /// Recompute the current window's folds on demand.
    ///
    /// The TUI calls this after edits, after an option that affects folding
    /// changed and after switching windows (this layer has no change hooks).
    pub fn refresh_folds_for_current_window(&mut self) -> VimResult<()> {
        self.recompute_folds()
    }

    /// The 'foldmethod' name when this layer cannot compute it, else None.
    ///
    /// syntax, expr and diff need engines that do not exist here, so the
    /// stored (manual) folds are kept and callers can report the limitation.
    pub fn unsupported_foldmethod(&self) -> Option<&'static str> {
        match self.wopt_str("foldmethod", "manual").as_str() {
            "syntax" => Some("syntax"),
            "expr" => Some("expr"),
            "diff" => Some("diff"),
            _ => None,
        }
    }

    /// Create a closed level-1 manual fold over start..=end.
    ///
    /// Returns the index of the new fold in the window's fold list. A
    /// backwards range is swapped, an end past the buffer is clamped and a
    /// fold that partially overlaps existing folds is widened to their union
    /// (fully contained folds stay as nested entries). Returns E16 when the
    /// range starts past the end of the buffer.
    pub fn add_fold(&mut self, start: usize, end: usize) -> VimResult<usize> {
        self.add_fold_at_level(start, end, 1)
    }

    /// Like [Editor::add_fold], with an explicit nesting level clamped to
    /// 1..=MAX_FOLD_LEVEL.
    pub fn add_fold_at_level(
        &mut self,
        start: usize,
        end: usize,
        level: usize,
    ) -> VimResult<usize> {
        let total = self.line_count();
        if total == 0 {
            return Err(VimError::new("E16", "Invalid range: empty buffer"));
        }
        let (mut first, mut last) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        if first >= total {
            return Err(VimError::new(
                "E16",
                format!("Invalid range: line {} out of range", first + 1),
            ));
        }
        last = last.min(total - 1);
        let level = level.clamp(1, MAX_FOLD_LEVEL);
        let win = self.windows.current_id();
        let mut folds = self.fold_store.get(win).to_vec();
        // Widen the new fold over every fold it partially overlaps so the
        // result stays a properly nested tree of ranges. Containment either
        // way is left alone and simply nests the ranges.
        loop {
            let mut grown = false;
            for fold in &folds {
                let intersects = fold.start <= last && first <= fold.end;
                let contained = (fold.start <= first && fold.end >= last)
                    || (first <= fold.start && last >= fold.end);
                if intersects && !contained {
                    if fold.start < first {
                        first = fold.start;
                    }
                    if fold.end > last {
                        last = fold.end;
                    }
                    grown = true;
                }
            }
            if !grown {
                break;
            }
        }
        let new_fold = Fold::new(first, last, level, true);
        let index = folds.partition_point(|fold| fold_before(fold, &new_fold));
        folds.insert(index, new_fold);
        self.fold_store.set(win, folds);
        Ok(index)
    }

    /// Open (unfold) the outermost closed fold that contains line (zo).
    ///
    /// Returns false when there is no closed fold to open or 'foldenable' is
    /// off. Small folds (at or below 'foldminlines') behave as open.
    pub fn open_fold_at(&mut self, line: usize) -> bool {
        if !self.wopt_bool("foldenable", true) {
            return false;
        }
        let win = self.windows.current_id();
        let min_lines = self.fold_min_lines();
        let Some(folds) = self.fold_store.get_mut(win) else {
            return false;
        };
        match folds
            .iter_mut()
            .find(|fold| fold.closed && fold.contains(line) && fold.line_count() > min_lines)
        {
            Some(fold) => {
                fold.closed = false;
                true
            }
            None => false,
        }
    }

    /// Close one fold that contains line (zc).
    ///
    /// The deepest open fold above the first already-closed fold is closed,
    /// which is Vim's setManualFold() rule. 'foldenable' is switched on, as
    /// ":help zc" promises.
    pub fn close_fold_at(&mut self, line: usize) -> bool {
        let win = self.windows.current_id();
        let min_lines = self.fold_min_lines();
        let mut target = None;
        if let Some(folds) = self.fold_store.get_mut(win) {
            for (index, fold) in folds.iter().enumerate() {
                if !fold.contains(line) {
                    continue;
                }
                if fold.closed {
                    break;
                }
                if fold.line_count() > min_lines {
                    target = Some(index);
                }
            }
            if let Some(index) = target {
                folds[index].closed = true;
            }
        }
        if target.is_some() {
            self.set_foldenable(true);
            true
        } else {
            false
        }
    }

    /// Close every fold that contains line, even folds nested inside an
    /// already-closed fold (zC and :foldclose!).
    ///
    /// Returns false when line is not inside any open fold. 'foldenable' is
    /// switched on when something was closed.
    pub fn close_folds_recursive(&mut self, line: usize) -> bool {
        let win = self.windows.current_id();
        let mut closed = false;
        if let Some(folds) = self.fold_store.get_mut(win) {
            for fold in folds.iter_mut() {
                if fold.contains(line) && !fold.closed {
                    fold.closed = true;
                    closed = true;
                }
            }
        }
        if closed {
            self.set_foldenable(true);
        }
        closed
    }

    /// za: open a closed fold under line, else close the open one.
    pub fn toggle_fold_at(&mut self, line: usize) -> bool {
        if self.open_fold_at(line) {
            return true;
        }
        self.close_fold_at(line)
    }

    /// zR: open every fold of the current window and raise 'foldlevel'.
    pub fn open_all_folds(&mut self) {
        let win = self.windows.current_id();
        let mut max_level = 0usize;
        if let Some(folds) = self.fold_store.get_mut(win) {
            for fold in folds.iter_mut() {
                fold.closed = false;
                max_level = max_level.max(fold.level);
            }
        }
        self.set_foldlevel(max_level as i64);
    }

    /// zM: close every fold of the current window, set 'foldlevel' to 0 and
    /// switch 'foldenable' on.
    pub fn close_all_folds(&mut self) {
        let win = self.windows.current_id();
        if let Some(folds) = self.fold_store.get_mut(win) {
            for fold in folds.iter_mut() {
                fold.closed = true;
            }
        }
        self.set_foldlevel(0);
        self.set_foldenable(true);
    }

    /// Re-apply 'foldlevel' to every fold of the current window (zr / zm).
    ///
    /// A fold is closed when its level is greater than 'foldlevel' and it is
    /// not small.
    pub fn apply_foldlevel(&mut self) {
        let foldlevel = self.foldlevel_option();
        let min_lines = self.fold_min_lines();
        let win = self.windows.current_id();
        if let Some(folds) = self.fold_store.get_mut(win) {
            for fold in folds.iter_mut() {
                fold.closed = fold.level > foldlevel && fold.line_count() > min_lines;
            }
        }
    }

    /// zd: delete the innermost fold that contains line.
    ///
    /// Folds nested inside it stay in the list and simply move one level up.
    /// Returns false when no fold contains the line.
    pub fn delete_fold_at(&mut self, line: usize) -> bool {
        let win = self.windows.current_id();
        let Some(folds) = self.fold_store.get_mut(win) else {
            return false;
        };
        let Some(index) = folds.iter().rposition(|fold| fold.contains(line)) else {
            return false;
        };
        folds.remove(index);
        true
    }

    /// zE: drop every fold of the current window.
    ///
    /// Fold markers in the text are not removed (this layer never edits the
    /// buffer to maintain marker folds).
    pub fn delete_all_folds(&mut self) {
        let win = self.windows.current_id();
        self.fold_store.clear(win);
    }

    /// The deepest fold level at line (Vim's foldlevel()), 0 outside folds.
    ///
    /// This reports levels regardless of 'foldenable', like Vim.
    pub fn fold_level_at(&self, line: usize) -> usize {
        self.folds()
            .iter()
            .filter(|fold| fold.contains(line))
            .map(|fold| fold.level)
            .max()
            .unwrap_or(0)
    }

    /// The outermost closed fold that hides line, as (first, last) line
    /// numbers (None when the line is visible or 'foldenable' is off).
    pub fn closed_fold_at(&self, line: usize) -> Option<(usize, usize)> {
        if !self.wopt_bool("foldenable", true) {
            return None;
        }
        let min_lines = self.fold_min_lines();
        self.folds()
            .iter()
            .find(|fold| fold.closed && fold.contains(line) && fold.line_count() > min_lines)
            .map(|fold| (fold.start, fold.end))
    }

    /// zj: the first line of the next fold after line.
    ///
    /// A closed fold counts as one fold, so its hidden interior is skipped.
    pub fn next_fold(&self, line: usize) -> Option<usize> {
        let after = match self.closed_fold_at(line) {
            Some((_, end)) => end.saturating_add(1),
            None => line.saturating_add(1),
        };
        self.folds()
            .iter()
            .filter(|fold| fold.start >= after)
            .map(|fold| fold.start)
            .min()
    }

    /// zk: the last line of the previous fold before line.
    ///
    /// When line is inside a fold, the start of the innermost fold that starts
    /// above it is returned instead (Vim's foldMoveTo()).
    pub fn prev_fold(&self, line: usize) -> Option<usize> {
        let mut inside = false;
        let mut start = None;
        for fold in self.folds().iter().filter(|fold| fold.contains(line)) {
            inside = true;
            if fold.start < line {
                start = Some(fold.start);
            }
        }
        if inside {
            return start;
        }
        self.folds()
            .iter()
            .filter(|fold| fold.end < line)
            .map(|fold| fold.end)
            .max()
    }

    /// The fold text shown for a fold that starts with first_line and covers
    /// count lines.
    ///
    /// 'foldtext' expressions are not evaluated at this layer; the default
    /// foldtext() layout is always used (see [fold_label]).
    pub fn fold_text(&self, first_line: &str, count: usize) -> String {
        fold_label(first_line, count)
    }

    // --- internals -------------------------------------------------------

    /// Drop fold lists of windows that no longer exist.
    fn prune_fold_windows(&mut self) {
        let windows = &self.windows;
        self.fold_store
            .retain_windows(|id| windows.get(id).is_some());
    }

    fn fold_min_lines(&self) -> usize {
        self.wopt_num("foldminlines", 1).max(0) as usize
    }

    fn foldlevel_option(&self) -> usize {
        self.wopt_num("foldlevel", 0).max(0) as usize
    }

    fn set_fold_local(&mut self, name: &str, value: OptValue) {
        self.windows.current_mut().set_local(name, value);
    }

    fn set_foldenable(&mut self, enabled: bool) {
        self.set_fold_local("foldenable", OptValue::Bool(enabled));
    }

    fn set_foldlevel(&mut self, level: i64) {
        self.set_fold_local("foldlevel", OptValue::Num(level.max(0)));
    }

    /// Compute folds for 'foldmethod=indent'.
    ///
    /// Levels are indent / 'shiftwidth' (with 'shiftwidth' 0 meaning
    /// 'tabstop'). Blank lines have no level of their own, so they stay in the
    /// fold that is open above them; when the next non-blank line drops below
    /// a level, that level's run ends just above it and keeps the trailing
    /// blanks.
    fn compute_indent_folds(&self) -> Vec<Fold> {
        let total = self.line_count();
        if total == 0 {
            return Vec::new();
        }
        let tabstop = self.wopt_num("tabstop", 8).max(1) as usize;
        let shiftwidth = self.wopt_num("shiftwidth", 8);
        let shiftwidth = if shiftwidth > 0 {
            shiftwidth as usize
        } else {
            tabstop
        }
        .max(1);
        let levels: Vec<Option<usize>> = (0..total)
            .map(|line| {
                let text = self.current_buffer().line_str(line).unwrap_or("");
                if text.trim().is_empty() {
                    None
                } else {
                    Some((indent_cols(text, tabstop) / shiftwidth).min(MAX_FOLD_LEVEL))
                }
            })
            .collect();
        let mut open: Vec<(usize, usize)> = Vec::new();
        let mut folds: Vec<Fold> = Vec::new();
        for (line, level) in levels.iter().enumerate() {
            let Some(level) = *level else {
                continue;
            };
            while let Some(&(open_level, start)) = open.last() {
                if open_level <= level {
                    break;
                }
                open.pop();
                folds.push(Fold::new(start, line - 1, open_level, false));
            }
            let top = open.last().map(|&(open_level, _)| open_level).unwrap_or(0);
            for open_level in (top + 1)..=level {
                open.push((open_level, line));
            }
        }
        for (level, start) in open {
            folds.push(Fold::new(start, total - 1, level, false));
        }
        self.finish_computed(folds)
    }

    /// Compute folds for 'foldmethod=marker' with the default {{{,}}} pair
    /// (see the module documentation).
    fn compute_marker_folds(&self) -> Vec<Fold> {
        let total = self.line_count();
        if total == 0 {
            return Vec::new();
        }
        let mut open: Vec<(usize, usize)> = Vec::new();
        let mut folds: Vec<Fold> = Vec::new();
        let mut level = 0usize;
        for line in 0..total {
            let text = self.current_buffer().line_str(line).unwrap_or("");
            let bytes = text.as_bytes();
            let line_start_level = level;
            let mut at = 0usize;
            while at + 3 <= bytes.len() {
                let is_start = &bytes[at..at + 3] == b"{{{";
                let is_end = &bytes[at..at + 3] == b"}}}";
                if !is_start && !is_end {
                    at += 1;
                    continue;
                }
                let (digit, used) = marker_level(bytes, at + 3);
                at += 3 + used;
                if is_start {
                    match digit {
                        Some(digit) if digit > 0 => {
                            close_open_folds(&mut folds, &mut open, digit, line.saturating_sub(1));
                            let top = open.last().map(|&(open_level, _)| open_level).unwrap_or(0);
                            for open_level in (top + 1)..=digit {
                                open.push((open_level, line));
                            }
                            level = digit;
                        }
                        Some(_) => {}
                        None => {
                            if level < MAX_FOLD_LEVEL {
                                level += 1;
                                open.push((level, line));
                            }
                        }
                    }
                } else {
                    match digit {
                        Some(digit) if digit > 0 => {
                            close_open_folds(
                                &mut folds,
                                &mut open,
                                digit + 1,
                                line.saturating_sub(1),
                            );
                            close_open_folds(&mut folds, &mut open, digit, line);
                            level = digit.saturating_sub(1).min(line_start_level);
                        }
                        Some(_) => {}
                        None => {
                            if let Some((open_level, start)) = open.pop() {
                                if line >= start {
                                    folds.push(Fold::new(start, line, open_level, false));
                                }
                            }
                            level = level.saturating_sub(1);
                        }
                    }
                }
            }
        }
        while let Some((level, start)) = open.pop() {
            folds.push(Fold::new(start, total - 1, level, false));
        }
        self.finish_computed(folds)
    }

    /// Apply 'foldminlines' and 'foldlevel' to freshly computed folds.
    ///
    /// Small folds stay in the list (so foldlevel() and the fold column keep
    /// reporting their depth) but are stored open, which is how Vim's fd_small
    /// flag behaves.
    fn finish_computed(&self, mut folds: Vec<Fold>) -> Vec<Fold> {
        let min_lines = self.fold_min_lines();
        let foldlevel = self.foldlevel_option();
        for fold in folds.iter_mut() {
            fold.closed = fold.level > foldlevel && fold.line_count() > min_lines;
        }
        sort_folds(&mut folds);
        folds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::SetScope;

    fn editor_with(lines: &[&str]) -> Editor {
        let mut ed = Editor::new();
        let lines: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
        ed.replace_lines(0, 1, lines).unwrap();
        ed
    }

    fn set(ed: &mut Editor, name: &str, value: OptValue) {
        ed.set_option(name, value, SetScope::Local).unwrap();
    }

    fn set_str(ed: &mut Editor, name: &str, value: &str) {
        set(ed, name, OptValue::Str(value.to_string()));
    }

    fn set_num(ed: &mut Editor, name: &str, value: i64) {
        set(ed, name, OptValue::Num(value));
    }

    #[test]
    fn fold_label_matches_the_default_foldtext_layout() {
        assert_eq!(fold_label("int main()", 3), "+--  3 lines: int main()");
        assert_eq!(fold_label("int main()", 1), "+--  1 line: int main()");
        assert_eq!(
            fold_label("    /* global {{{1 */", 2),
            "+--  2 lines: global"
        );
        assert_eq!(fold_label("# region {{{2", 4), "+--  4 lines: # region");
        assert_eq!(fold_label("", 2), "+--  2 lines:");
        // A zero count is treated as one line instead of producing a weird label.
        assert_eq!(fold_label("x", 0), "+--  1 line: x");
        assert_eq!(
            fold_label("// note }}} text", 2),
            "+--  2 lines: note text",
            "markers are removed with the white space that follows them"
        );
    }

    #[test]
    fn manual_folds_open_close_toggle_and_delete() {
        let mut ed = editor_with(&["one", "two", "three", "four"]);
        let index = ed.add_fold(0, 2).unwrap();
        assert_eq!(index, 0);
        assert_eq!(ed.folds().len(), 1);
        assert_eq!(ed.folds()[0], Fold::new(0, 2, 1, true));
        assert_eq!(ed.closed_fold_at(1), Some((0, 2)));
        assert_eq!(ed.fold_level_at(1), 1);
        assert_eq!(ed.fold_level_at(3), 0);
        assert!(ed.open_fold_at(2));
        assert_eq!(ed.closed_fold_at(1), None);
        assert!(!ed.open_fold_at(1), "already open");
        assert!(ed.close_fold_at(2));
        assert_eq!(ed.closed_fold_at(2), Some((0, 2)));
        assert!(ed.toggle_fold_at(1));
        assert_eq!(ed.closed_fold_at(2), None);
        assert!(ed.toggle_fold_at(1));
        assert_eq!(ed.closed_fold_at(2), Some((0, 2)));
        assert!(ed.delete_fold_at(1));
        assert!(ed.folds().is_empty());
        assert!(!ed.delete_fold_at(1));
    }

    #[test]
    fn manual_fold_ranges_are_swapped_clamped_and_unioned() {
        let mut ed = editor_with(&["a", "b", "c", "d"]);
        // A backwards range is swapped.
        ed.add_fold(2, 1).unwrap();
        assert_eq!(ed.folds()[0], Fold::new(1, 2, 1, true));
        // An end past the buffer is clamped.
        ed.add_fold(3, 99).unwrap();
        assert_eq!(ed.folds()[1], Fold::new(3, 3, 1, true));
        // Overlapping folds are absorbed into one range.
        ed.add_fold(0, 1).unwrap();
        assert_eq!(
            ed.folds()[0],
            Fold::new(0, 2, 1, true),
            "the new fold covers every fold it touches"
        );
        assert_eq!(
            ed.folds().len(),
            3,
            "the contained fold stays as a nested entry"
        );
        // A range that only contains an existing fold nests it instead of
        // widening, so both entries survive.
        ed.add_fold(0, 3).unwrap();
        assert_eq!(ed.folds().len(), 4);
        assert_eq!((ed.folds()[0].start, ed.folds()[0].end), (0, 3));
        // A start past the buffer is an error, not a panic.
        assert_eq!(ed.add_fold(4, 4).unwrap_err().code, "E16");
        assert_eq!(ed.folds().len(), 4);
    }

    #[test]
    fn folds_are_stored_per_window() {
        let mut ed = editor_with(&["a", "b", "c", "d"]);
        let first = ed.windows.current_id();
        ed.add_fold(0, 1).unwrap();
        let second = ed.split_window(false).unwrap();
        assert_ne!(first, second);
        assert!(ed.folds().is_empty(), "a new window starts without folds");
        ed.add_fold(2, 3).unwrap();
        assert_eq!(ed.folds_for(first), &[Fold::new(0, 1, 1, true)]);
        assert_eq!(ed.folds_for(second), &[Fold::new(2, 3, 1, true)]);
        assert_eq!(ed.folds_for(999).len(), 0, "unknown windows have no folds");
    }

    #[test]
    fn foldenable_off_makes_every_fold_open() {
        let mut ed = editor_with(&["a", "b", "c"]);
        ed.add_fold(0, 2).unwrap();
        set(&mut ed, "foldenable", OptValue::Bool(false));
        assert_eq!(ed.closed_fold_at(1), None);
        assert!(!ed.open_fold_at(1));
        assert_eq!(ed.fold_level_at(1), 1, "foldlevel() ignores 'foldenable'");
        set(&mut ed, "foldenable", OptValue::Bool(true));
        assert_eq!(ed.closed_fold_at(1), Some((0, 2)));
    }

    #[test]
    fn small_folds_behave_open() {
        let mut ed = editor_with(&["a", "b", "c"]);
        ed.add_fold(0, 0).unwrap();
        assert_eq!(
            ed.closed_fold_at(0),
            None,
            "a one-line fold is at 'foldminlines'"
        );
        set_num(&mut ed, "foldminlines", 0);
        assert_eq!(ed.closed_fold_at(0), Some((0, 0)));
    }

    #[test]
    fn open_and_close_all_and_apply_foldlevel() {
        let mut ed = editor_with(&["a", "b", "c", "d"]);
        ed.add_fold_at_level(0, 3, 1).unwrap();
        ed.add_fold_at_level(1, 2, 2).unwrap();
        ed.close_all_folds();
        assert_eq!(ed.wopt_num("foldlevel", -1), 0);
        assert!(ed.folds().iter().all(|fold| fold.closed));
        ed.open_all_folds();
        assert_eq!(ed.wopt_num("foldlevel", -1), 2);
        assert!(ed.folds().iter().all(|fold| !fold.closed));
        set_num(&mut ed, "foldlevel", 1);
        ed.apply_foldlevel();
        let closed: Vec<bool> = ed.folds().iter().map(|fold| fold.closed).collect();
        assert_eq!(closed, vec![false, true], "only the level-2 fold closes");
    }

    #[test]
    fn indent_method_groups_by_shiftwidth() {
        let mut ed = editor_with(&[
            "int main()",
            "{",
            "    if (x)",
            "    {",
            "        y;",
            "    }",
            "}",
        ]);
        set_str(&mut ed, "foldmethod", "indent");
        set_num(&mut ed, "shiftwidth", 4);
        ed.recompute_folds().unwrap();
        assert_eq!(
            ed.folds(),
            &[Fold::new(2, 5, 1, true), Fold::new(4, 4, 2, false)],
            "the one-line level-2 fold stays in the list but is open"
        );
        assert_eq!(ed.fold_level_at(4), 2, "foldlevel() still sees level 2");
        // A wider shiftwidth changes the computed levels on refresh.
        set_num(&mut ed, "shiftwidth", 16);
        ed.refresh_folds_for_current_window().unwrap();
        assert!(ed.folds().is_empty());
        assert_eq!(ed.unsupported_foldmethod(), None);
    }

    #[test]
    fn indent_method_keeps_trailing_blank_lines_above() {
        let mut ed = editor_with(&["a", "    b", "", "", "c"]);
        set_str(&mut ed, "foldmethod", "indent");
        set_num(&mut ed, "shiftwidth", 4);
        ed.recompute_folds().unwrap();
        assert_eq!(
            ed.folds(),
            &[Fold::new(1, 3, 1, true)],
            "the blank run belongs to the fold above"
        );
        // Blank lines before the first fold are not part of it.
        let mut ed = editor_with(&["", "    a", "    b", "c"]);
        set_str(&mut ed, "foldmethod", "indent");
        set_num(&mut ed, "shiftwidth", 4);
        ed.recompute_folds().unwrap();
        assert_eq!(ed.folds(), &[Fold::new(1, 2, 1, true)]);
    }

    #[test]
    fn indent_method_expands_tabs_with_tabstop() {
        let mut ed = editor_with(&["a", "\tb", "\tc", "d"]);
        set_str(&mut ed, "foldmethod", "indent");
        set_num(&mut ed, "shiftwidth", 0);
        set_num(&mut ed, "tabstop", 4);
        ed.recompute_folds().unwrap();
        assert_eq!(ed.folds(), &[Fold::new(1, 2, 1, true)]);
    }

    #[test]
    fn indent_method_respects_foldminlines_and_foldlevel() {
        let mut ed = editor_with(&["a", "    b", "c", "    d", "    e"]);
        set_str(&mut ed, "foldmethod", "indent");
        set_num(&mut ed, "shiftwidth", 4);
        ed.recompute_folds().unwrap();
        assert_eq!(
            ed.folds(),
            &[Fold::new(1, 1, 1, false), Fold::new(3, 4, 1, true)],
            "a one-line fold is kept open by 'foldminlines'"
        );
        set_num(&mut ed, "foldminlines", 0);
        set_num(&mut ed, "foldlevel", 1);
        ed.recompute_folds().unwrap();
        assert_eq!(ed.folds().len(), 2);
        assert!(ed.folds().iter().all(|fold| !fold.closed));
    }

    #[test]
    fn marker_method_scans_the_default_markers() {
        let mut ed = editor_with(&[
            "/* global {{{1 */",
            "int a;",
            "/* f {{{2 */",
            "void f() {",
            "}",
            "/* }}}2 */",
            "/* }}}1 */",
            "after",
        ]);
        set_str(&mut ed, "foldmethod", "marker");
        ed.recompute_folds().unwrap();
        assert_eq!(
            ed.folds(),
            &[Fold::new(0, 6, 1, true), Fold::new(2, 5, 2, true)]
        );
        assert_eq!(ed.fold_level_at(3), 2);
        assert_eq!(ed.closed_fold_at(3), Some((0, 6)));
        ed.open_fold_at(3);
        assert_eq!(ed.closed_fold_at(3), Some((2, 5)));
    }

    #[test]
    fn marker_method_supports_level_digits_and_restarts() {
        // Vim's fold-marker.txt example: levels 1 -> 3 -> 2.
        let mut ed = editor_with(&["{{{1", "one", "{{{3", "three", "}}}3", "two"]);
        set_str(&mut ed, "foldmethod", "marker");
        set_num(&mut ed, "foldminlines", 0);
        ed.recompute_folds().unwrap();
        assert_eq!(ed.fold_level_at(3), 3);
        assert_eq!(ed.fold_level_at(5), 2);
        assert_eq!(
            ed.folds(),
            &[
                Fold::new(0, 5, 1, true),
                Fold::new(2, 5, 2, true),
                Fold::new(2, 4, 3, true),
            ]
        );

        // A second level-1 marker closes the first fold and starts a new one.
        let mut ed = editor_with(&["{{{1", "a", "}}}", "{{{", "b", "}}}"]);
        set_str(&mut ed, "foldmethod", "marker");
        ed.recompute_folds().unwrap();
        assert_eq!(
            ed.folds(),
            &[Fold::new(0, 2, 1, true), Fold::new(3, 5, 1, true)]
        );
    }

    #[test]
    fn marker_method_handles_nesting_and_unbalanced_markers() {
        let mut ed = editor_with(&["a {{{", "b {{{", "c", "}}}", "}}}", "tail"]);
        set_str(&mut ed, "foldmethod", "marker");
        ed.recompute_folds().unwrap();
        assert_eq!(
            ed.folds(),
            &[Fold::new(0, 4, 1, true), Fold::new(1, 3, 2, true)]
        );
        // An end marker without a matching start is harmless.
        let mut ed = editor_with(&["a", "}}}", "b"]);
        set_str(&mut ed, "foldmethod", "marker");
        ed.recompute_folds().unwrap();
        assert!(ed.folds().is_empty());
        // A start marker left open at the end of the buffer still closes.
        let mut ed = editor_with(&["a {{{", "b"]);
        set_str(&mut ed, "foldmethod", "marker");
        ed.recompute_folds().unwrap();
        assert_eq!(ed.folds(), &[Fold::new(0, 1, 1, true)]);
    }

    #[test]
    fn next_and_previous_fold_follow_the_cursor() {
        let mut ed = editor_with(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        ed.add_fold(1, 2).unwrap();
        ed.add_fold(4, 6).unwrap();
        assert_eq!(ed.next_fold(0), Some(1));
        assert_eq!(ed.next_fold(1), Some(4), "a closed fold counts as one");
        assert_eq!(ed.next_fold(7), None);
        assert_eq!(ed.prev_fold(3), Some(2), "end of the previous fold");
        assert_eq!(ed.prev_fold(5), Some(4), "start of the containing fold");
        assert_eq!(ed.prev_fold(0), None);
    }

    #[test]
    fn unsupported_methods_keep_the_manual_folds() {
        let mut ed = editor_with(&["a", "b", "c"]);
        ed.add_fold(0, 1).unwrap();
        for method in ["syntax", "expr", "diff"] {
            set_str(&mut ed, "foldmethod", method);
            ed.recompute_folds().unwrap();
            assert_eq!(ed.unsupported_foldmethod(), Some(method));
            assert_eq!(ed.folds(), &[Fold::new(0, 1, 1, true)]);
        }
        set_str(&mut ed, "foldmethod", "manual");
        ed.recompute_folds().unwrap();
        assert_eq!(ed.unsupported_foldmethod(), None);
        assert_eq!(ed.folds().len(), 1);
        ed.delete_all_folds();
        assert!(ed.folds().is_empty());
    }

    #[test]
    fn folds_survive_edge_of_buffer_ranges() {
        // A buffer with a single (empty) line has no levels and no folds.
        let mut ed = Editor::new();
        set_str(&mut ed, "foldmethod", "indent");
        ed.recompute_folds().unwrap();
        assert!(ed.folds().is_empty());
        assert_eq!(ed.fold_level_at(0), 0);
        assert_eq!(ed.closed_fold_at(0), None);
        assert_eq!(ed.next_fold(0), None);
        assert_eq!(ed.prev_fold(0), None);
        // Queries on lines past the end never panic.
        assert_eq!(ed.fold_level_at(usize::MAX), 0);
        assert_eq!(ed.closed_fold_at(usize::MAX), None);
        assert_eq!(ed.next_fold(usize::MAX), None);
        assert_eq!(ed.prev_fold(usize::MAX), None);
        // A one-line fold on the single line is legal.
        ed.add_fold(0, 0).unwrap();
        assert_eq!(ed.fold_level_at(0), 1);
    }

    #[test]
    fn fold_text_uses_the_default_layout() {
        let ed = editor_with(&["int main()", "{"]);
        assert_eq!(ed.fold_text("int main()", 2), "+--  2 lines: int main()");
        assert_eq!(fold_label("# {{{1", 1), "+--  1 line: #");
    }
}
