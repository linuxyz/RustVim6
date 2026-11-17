//! Quickfix and location-list subsystem (reference: vim src/quickfix.c).
//!
//! This module owns the list model, the ten-entry list stacks, navigation and
//! the quickfix window. The Ex commands, the 'errorformat' parsing and the
//! setqflist()/getqflist() builtins live in rvim-ex; this crate only stores the
//! data so that the editor aggregate stays self-contained.
//!
//! # Model
//!
//! * QfEntry is a reduced qfline_T. file names are kept as strings (an empty
//!   name means the buffer that was current when the entry was created). The
//!   end column, error number, module and search pattern of vim's qfline_T are
//!   not represented; getqflist() reports them as zero/empty strings.
//! * QfList matches vim's qf_list_T: title, entries, current entry index, list
//!   number and context. The index is 1-based, exactly like vim's qf_index and
//!   the idx item of getqflist(); 0 means "no valid entry selected".
//! * QuickfixState owns the global stack and one stack per window for location
//!   lists. Each stack keeps at most QF_STACK_LEN (10) lists. Creating a list
//!   drops the lists newer than the current one (vim's qf_new_list()); when the
//!   stack is full the oldest list is dropped.
//!
//! # Supported semantics
//!
//! * qf_set() creates a new current list (like :cexpr/:cgetexpr/:cfile).
//! * qf_push() appends entries to the current list, creating a list when the
//!   stack is empty (like :caddexpr and setqflist() with action 'a').
//! * qf_next()/qf_prev() skip entries with valid == false. They wrap only when
//!   'wrapscan' is on (the caller passes the flag); otherwise they return
//!   false and the Ex layer reports E553.
//! * qf_first()/qf_last() select the first/last valid entry.
//! * qf_jump_to() opens the entry file/buffer with Editor::open_file (reusing a
//!   buffer with the same name), updates the list index and positions the
//!   cursor. It returns false for an empty list or an invalid entry.
//! * :colder/:cnewer move through the stack; E380/E381 are returned when the
//!   stack is already at the bottom/top. A count that crosses the boundary
//!   stops at the boundary instead of emitting one error per step.
//!
//! # Quickfix window
//!
//! qf_open_window() splits the current window, switches the new window to a
//! buffer named "quickfix", marks the window with Window::quickfix and, when
//! the option exists, sets 'buftype' to "quickfix" and 'winfixheight'. Lines
//! are formatted as:
//!
//! ```text
//!  1 file.txt|12 col 3| error text
//! ```
//!
//! with the entry number right-aligned in two columns (vim displays the number
//! through the 'number' option; this rewrite writes it into the text instead).
//! A list title, when present, occupies the first buffer line; the task-level
//! format asks for this, while vim keeps the title in w:quickfix_title. The
//! buffer is refreshed whenever the displayed list changes or the stack moves.
//!
//! # Deviations from Vim
//!
//! * Only one quickfix/location-list window is modelled. Opening a location
//!   list window while a quickfix window is open reuses that window and changes
//!   what it displays; vim allows several location-list windows.
//! * getqflist()/setqflist() identify a list by its stack position; the unique
//!   vim qf_id is therefore reported as the list number.
//! * Jumping out of the quickfix window moves to another window of the current
//!   tab page (splitting one when the quickfix window is the only one).
//! * 'buftype' is not in this build's option table, so setting it is a
//!   best-effort no-op; 'winfixheight' is set when present.
//! * The quickfix buffer is made read-only only indirectly; the option table
//!   has no 'nomodifiable' entry.
//! * The 'errorformat' option is not in the option table. QuickfixState stores
//!   an optional user supplied value and the Ex layer falls back to vim's
//!   default when it is None.

use std::collections::HashMap;
use std::path::PathBuf;

use rvim_core::{Pos, VimError, VimResult};

use crate::editor::{Editor, SetScope};
use crate::options::OptValue;
use crate::window::WindowId;

/// Maximum number of lists kept in one quickfix or location-list stack.
pub const QF_STACK_LEN: usize = 10;

/// One quickfix/location-list entry, modelled on Vim's qfline_T.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QfEntry {
    /// File name as typed by the compiler; empty means "the current buffer".
    pub filename: String,
    /// Buffer number when the entry is attached to an existing buffer.
    pub bufnr: Option<i32>,
    /// 1-based line number; 0 means the entry cannot be jumped to.
    pub lnum: usize,
    /// 1-based end line of a range, 0 when the entry is a single line.
    pub end_lnum: usize,
    /// 1-based byte column, 0 when unknown.
    pub col: usize,
    /// Message text shown in the quickfix window and by :clist.
    pub text: String,
    /// Type character: E, W, I, N, or NUL when unknown.
    pub entry_type: char,
    /// False for lines that no 'errorformat' pattern recognised.
    pub valid: bool,
}

/// One quickfix/location list (vim's qf_list_T).
#[derive(Clone, Debug, Default)]
pub struct QfList {
    /// Title shown in the quickfix window and returned by getqflist().
    pub title: String,
    /// Entries in list order.
    pub entries: Vec<QfEntry>,
    /// 1-based index of the current entry; 0 when none is selected.
    pub idx: usize,
    /// 1-based position of the list in its stack.
    pub nr: i64,
    /// User supplied context strings (vim stores an arbitrary value; here only
    /// string items are kept).
    pub context: Vec<String>,
}

impl QfList {
    /// 1-based index of the first valid entry, or 0 when none is valid.
    fn first_valid_index(&self) -> usize {
        self.entries
            .iter()
            .position(|entry| entry.valid)
            .map(|index| index + 1)
            .unwrap_or(0)
    }
}

/// What a quickfix window currently displays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum QfTarget {
    /// The global quickfix stack.
    #[default]
    Quickfix,
    /// The location-list stack of the given source window.
    Location(WindowId),
}

/// Per-window location-list stack.
#[derive(Debug, Default)]
struct LocListStack {
    lists: Vec<QfList>,
    cur: usize,
}

/// Global quickfix plus per-window location-list state.
///
/// The struct is owned by Editor; use Editor::qf_state()/qf_state_mut() and the
/// accessors below to read or update it.
#[derive(Debug, Default)]
pub struct QuickfixState {
    lists: Vec<QfList>,
    cur: usize,
    loclists: HashMap<WindowId, LocListStack>,
    /// Cached number of the synthetic quickfix buffer.
    qf_bufnr: Option<i32>,
    /// Window currently displaying a quickfix/location list.
    qf_win: Option<WindowId>,
    /// What that window displays.
    target: QfTarget,
    /// User supplied 'errorformat' value; None means vim's default.
    errorformat: Option<String>,
}

impl QuickfixState {
    /// All lists of the global stack, oldest first.
    pub fn lists(&self) -> &[QfList] {
        &self.lists
    }

    /// Index (0-based) of the current global list.
    pub fn current_index(&self) -> usize {
        self.cur
    }

    /// The current global list, if any.
    pub fn current_list(&self) -> Option<&QfList> {
        self.lists.get(self.cur)
    }

    /// Mutable access to the current global list.
    pub fn current_list_mut(&mut self) -> Option<&mut QfList> {
        self.lists.get_mut(self.cur)
    }

    /// The global list at a 0-based stack position.
    pub fn list(&self, index: usize) -> Option<&QfList> {
        self.lists.get(index)
    }

    /// Mutable access to a global list at a 0-based stack position.
    pub fn list_mut(&mut self, index: usize) -> Option<&mut QfList> {
        self.lists.get_mut(index)
    }

    /// Make the list at a 0-based stack position current.
    pub fn select_index(&mut self, index: usize) -> bool {
        if index < self.lists.len() {
            self.cur = index;
            true
        } else {
            false
        }
    }

    /// The user supplied 'errorformat', or None when the default applies.
    pub fn errorformat(&self) -> Option<&str> {
        self.errorformat.as_deref()
    }

    /// Replace the user supplied 'errorformat' value.
    pub fn set_errorformat(&mut self, spec: Option<String>) {
        self.errorformat = spec;
    }

    /// Number of lists in a window's location-list stack.
    pub fn loclist_count(&self, win: WindowId) -> usize {
        self.loclists.get(&win).map_or(0, |stack| stack.lists.len())
    }

    /// Index (0-based) of the current list in a window's location-list stack.
    pub fn loclist_index(&self, win: WindowId) -> usize {
        self.loclists.get(&win).map_or(0, |stack| stack.cur)
    }

    /// The current location list of a window, if the stack exists.
    pub fn loclist_current(&self, win: WindowId) -> Option<&QfList> {
        self.loclists
            .get(&win)
            .and_then(|stack| stack.lists.get(stack.cur))
    }

    /// Mutable access to the current location list of a window.
    pub fn loclist_current_mut(&mut self, win: WindowId) -> Option<&mut QfList> {
        let stack = self.loclists.get_mut(&win)?;
        stack.lists.get_mut(stack.cur)
    }

    /// A location list at a 0-based stack position.
    pub fn loclist_at(&self, win: WindowId, index: usize) -> Option<&QfList> {
        self.loclists
            .get(&win)
            .and_then(|stack| stack.lists.get(index))
    }

    /// Mutable access to a location list at a 0-based stack position.
    pub fn loclist_at_mut(&mut self, win: WindowId, index: usize) -> Option<&mut QfList> {
        self.loclists
            .get_mut(&win)
            .and_then(|stack| stack.lists.get_mut(index))
    }

    /// Make a location list current.
    pub fn loclist_select(&mut self, win: WindowId, index: usize) -> bool {
        let stack = self.ensure_loclist(win);
        if index < stack.lists.len() {
            stack.cur = index;
            true
        } else {
            false
        }
    }

    /// Drop every global list (:setqflist([], 'f') support).
    pub fn clear(&mut self) {
        self.lists.clear();
        self.cur = 0;
    }

    /// Drop a window's location-list stack.
    pub fn clear_loclist(&mut self, win: WindowId) {
        self.loclists.remove(&win);
    }

    /// Get or create the location-list stack of a window.
    fn ensure_loclist(&mut self, win: WindowId) -> &mut LocListStack {
        self.loclists.entry(win).or_default()
    }
}

/// Build a list with the first valid entry selected.
fn new_list(title: &str, entries: Vec<QfEntry>) -> QfList {
    let mut list = QfList {
        title: title.to_string(),
        entries,
        idx: 0,
        nr: 0,
        context: Vec::new(),
    };
    list.idx = list.first_valid_index();
    list
}

/// Renumber a stack after a push/drop so nr mirrors the 1-based position.
fn renumber(lists: &mut [QfList]) {
    for (index, list) in lists.iter_mut().enumerate() {
        list.nr = index as i64 + 1;
    }
}

/// Push a brand new list, truncating newer lists and dropping the oldest when
/// the stack is full (qf_new_list() in quickfix.c).
fn push_new_list(lists: &mut Vec<QfList>, cur: &mut usize, title: &str, entries: Vec<QfEntry>) {
    while lists.len() > *cur + 1 {
        lists.pop();
    }
    if lists.is_empty() {
        lists.push(new_list(title, entries));
        *cur = 0;
    } else if lists.len() >= QF_STACK_LEN {
        lists.remove(0);
        *cur = lists.len();
        lists.push(new_list(title, entries));
    } else {
        *cur += 1;
        lists.push(new_list(title, entries));
    }
    if *cur >= lists.len() {
        *cur = lists.len().saturating_sub(1);
    }
    renumber(lists);
}

/// Append entries to the current list, creating one when the stack is empty.
fn append_entries(lists: &mut Vec<QfList>, cur: &mut usize, title: &str, entries: Vec<QfEntry>) {
    if lists.is_empty() {
        push_new_list(lists, cur, title, entries);
        return;
    }
    let index = (*cur).min(lists.len().saturating_sub(1));
    let list = &mut lists[index];
    if !title.is_empty() {
        list.title = title.to_string();
    }
    let was_empty = list.entries.is_empty();
    list.entries.extend(entries);
    if was_empty || list.idx == 0 {
        list.idx = list.first_valid_index();
    }
}

/// Replace the entries of the current list in place, creating it when needed.
fn replace_entries(
    lists: &mut Vec<QfList>,
    cur: &mut usize,
    title: Option<&str>,
    entries: Vec<QfEntry>,
) {
    if lists.is_empty() {
        push_new_list(lists, cur, title.unwrap_or(""), entries);
        return;
    }
    let index = (*cur).min(lists.len().saturating_sub(1));
    let list = &mut lists[index];
    list.entries = entries;
    if let Some(title) = title {
        list.title = title.to_string();
    }
    list.idx = list.first_valid_index();
}

/// Compute the 1-based index reached after count valid steps from the current
/// entry. Wrapping only happens when wrap is true, and at most one full cycle
/// is examined so a count larger than the number of valid entries fails instead
/// of looping forever.
fn navigation_target(list: &QfList, count: usize, forward: bool, wrap: bool) -> Option<usize> {
    let total = list.entries.len();
    if total == 0 {
        return None;
    }
    let count = count.max(1);
    let current = if list.idx == 0 {
        None
    } else {
        Some(list.idx.saturating_sub(1).min(total - 1))
    };
    let start = if forward {
        match current {
            Some(index) if index + 1 < total => index + 1,
            Some(_) if wrap => 0,
            Some(_) => return None,
            None => 0,
        }
    } else {
        match current {
            Some(index) if index > 0 => index - 1,
            Some(_) if wrap => total - 1,
            Some(_) => return None,
            None => total - 1,
        }
    };
    let limit = if wrap {
        total
    } else if forward {
        total - start
    } else {
        start + 1
    };
    let mut index = start;
    let mut found = 0usize;
    for _ in 0..limit {
        if list.entries[index].valid {
            found += 1;
            if found == count {
                return Some(index + 1);
            }
        }
        index = if forward {
            (index + 1) % total
        } else {
            (index + total - 1) % total
        };
    }
    None
}

/// First 1-based valid entry of a list.
fn first_valid_target(list: &QfList) -> Option<usize> {
    let index = list.first_valid_index();
    if index == 0 { None } else { Some(index) }
}

/// Last 1-based valid entry of a list.
fn last_valid_target(list: &QfList) -> Option<usize> {
    list.entries
        .iter()
        .rposition(|entry| entry.valid)
        .map(|index| index + 1)
}

impl Editor {
    /// The quickfix/location-list state of this editor.
    pub fn qf_state(&self) -> &QuickfixState {
        &self.quickfix
    }

    /// Mutable quickfix/location-list state.
    pub fn qf_state_mut(&mut self) -> &mut QuickfixState {
        &mut self.quickfix
    }

    /// Replace the current quickfix list with a new list.
    ///
    /// Lists newer than the current one are dropped and the ten-list limit is
    /// enforced, like vim's qf_new_list(); the method is used by :cexpr,
    /// :cgetexpr, :cfile and setqflist() without an explicit action.
    pub fn qf_set(&mut self, title: &str, entries: Vec<QfEntry>) {
        push_new_list(
            &mut self.quickfix.lists,
            &mut self.quickfix.cur,
            title,
            entries,
        );
        self.refresh_qf_window();
    }

    /// Append entries to the current quickfix list.
    ///
    /// A list is created when the stack is empty (vim's :caddexpr and the 'a'
    /// action of setqflist()).
    pub fn qf_push(&mut self, title: &str, entries: Vec<QfEntry>) {
        append_entries(
            &mut self.quickfix.lists,
            &mut self.quickfix.cur,
            title,
            entries,
        );
        self.refresh_qf_window();
    }

    /// Replace the entries of the current quickfix list in place.
    ///
    /// Unlike qf_set() the stack does not grow; this is the 'r' action of
    /// setqflist(). A list is created when the stack is empty.
    pub fn qf_replace_entries(&mut self, title: Option<&str>, entries: Vec<QfEntry>) {
        replace_entries(
            &mut self.quickfix.lists,
            &mut self.quickfix.cur,
            title,
            entries,
        );
        self.refresh_qf_window();
    }

    /// 1-based index of the current entry in the current quickfix list.
    pub fn qf_current(&self) -> Option<usize> {
        self.quickfix
            .current_list()
            .and_then(|list| (list.idx > 0).then_some(list.idx))
    }

    /// 1-based index of the current entry in a window's location list.
    pub fn loclist_current_index(&self, win: WindowId) -> Option<usize> {
        self.quickfix
            .loclist_current(win)
            .and_then(|list| (list.idx > 0).then_some(list.idx))
    }

    /// Jump to entry n of the current quickfix list.
    ///
    /// n is 1-based, like :cc and vim's errornr; 0 re-displays the current
    /// entry. The file is opened through open_file (reusing a buffer with the
    /// same name) and the cursor is placed on lnum/col. Returns false for an
    /// empty list or an invalid entry.
    pub fn qf_jump_to(&mut self, n: usize) -> VimResult<bool> {
        self.jump_to_index(None, n)
    }

    /// Jump to entry n of a window's current location list.
    pub fn loclist_jump_to(&mut self, win: WindowId, n: usize) -> VimResult<bool> {
        self.jump_to_index(Some(win), n)
    }

    /// Move count valid entries forward in the current quickfix list.
    pub fn qf_next(&mut self, count: usize, wrap: bool) -> VimResult<bool> {
        let target = {
            let Some(list) = self.quickfix.current_list() else {
                return Ok(false);
            };
            navigation_target(list, count, true, wrap)
        };
        match target {
            Some(index) => self.qf_jump_to(index),
            None => Ok(false),
        }
    }

    /// Move count valid entries backward in the current quickfix list.
    pub fn qf_prev(&mut self, count: usize, wrap: bool) -> VimResult<bool> {
        let target = {
            let Some(list) = self.quickfix.current_list() else {
                return Ok(false);
            };
            navigation_target(list, count, false, wrap)
        };
        match target {
            Some(index) => self.qf_jump_to(index),
            None => Ok(false),
        }
    }

    /// Move count valid entries forward in a window's location list.
    pub fn loclist_next(&mut self, win: WindowId, count: usize, wrap: bool) -> VimResult<bool> {
        let target = {
            let Some(list) = self.quickfix.loclist_current(win) else {
                return Ok(false);
            };
            navigation_target(list, count, true, wrap)
        };
        match target {
            Some(index) => self.loclist_jump_to(win, index),
            None => Ok(false),
        }
    }

    /// Move count valid entries backward in a window's location list.
    pub fn loclist_prev(&mut self, win: WindowId, count: usize, wrap: bool) -> VimResult<bool> {
        let target = {
            let Some(list) = self.quickfix.loclist_current(win) else {
                return Ok(false);
            };
            navigation_target(list, count, false, wrap)
        };
        match target {
            Some(index) => self.loclist_jump_to(win, index),
            None => Ok(false),
        }
    }

    /// Jump to the first valid entry of the current quickfix list.
    pub fn qf_first(&mut self) -> VimResult<bool> {
        let target = self.quickfix.current_list().and_then(first_valid_target);
        match target {
            Some(index) => self.qf_jump_to(index),
            None => Ok(false),
        }
    }

    /// Jump to the last valid entry of the current quickfix list.
    pub fn qf_last(&mut self) -> VimResult<bool> {
        let target = self.quickfix.current_list().and_then(last_valid_target);
        match target {
            Some(index) => self.qf_jump_to(index),
            None => Ok(false),
        }
    }

    /// Jump to the first valid entry of a window's location list.
    pub fn loclist_first(&mut self, win: WindowId) -> VimResult<bool> {
        let target = self
            .quickfix
            .loclist_current(win)
            .and_then(first_valid_target);
        match target {
            Some(index) => self.loclist_jump_to(win, index),
            None => Ok(false),
        }
    }

    /// Jump to the last valid entry of a window's location list.
    pub fn loclist_last(&mut self, win: WindowId) -> VimResult<bool> {
        let target = self
            .quickfix
            .loclist_current(win)
            .and_then(last_valid_target);
        match target {
            Some(index) => self.loclist_jump_to(win, index),
            None => Ok(false),
        }
    }

    /// Go count lists back in the global quickfix stack (:colder).
    pub fn qf_older(&mut self, count: usize) -> VimResult<()> {
        if count > 0 && self.quickfix.cur == 0 {
            return Err(VimError::new("E380", "At bottom of quickfix stack"));
        }
        self.quickfix.cur = self.quickfix.cur.saturating_sub(count);
        self.refresh_qf_window();
        Ok(())
    }

    /// Go count lists forward in the global quickfix stack (:cnewer).
    pub fn qf_newer(&mut self, count: usize) -> VimResult<()> {
        if count > 0 && self.quickfix.cur + 1 >= self.quickfix.lists.len().max(1) {
            return Err(VimError::new("E381", "At top of quickfix stack"));
        }
        let top = self.quickfix.lists.len().saturating_sub(1);
        self.quickfix.cur = (self.quickfix.cur + count).min(top);
        self.refresh_qf_window();
        Ok(())
    }

    /// Go count lists back in a window's location-list stack (:lolder).
    pub fn loclist_older(&mut self, win: WindowId, count: usize) -> VimResult<()> {
        let cur = self.quickfix.loclist_index(win);
        if count > 0 && cur == 0 {
            return Err(VimError::new("E380", "At bottom of quickfix stack"));
        }
        let stack = self.quickfix.ensure_loclist(win);
        stack.cur = stack.cur.saturating_sub(count);
        self.refresh_qf_window();
        Ok(())
    }

    /// Go count lists forward in a window's location-list stack (:lnewer).
    pub fn loclist_newer(&mut self, win: WindowId, count: usize) -> VimResult<()> {
        let stack = self.quickfix.ensure_loclist(win);
        let top = stack.lists.len().saturating_sub(1);
        if count > 0 && stack.cur >= top {
            return Err(VimError::new("E381", "At top of quickfix stack"));
        }
        stack.cur = (stack.cur + count).min(top);
        self.refresh_qf_window();
        Ok(())
    }

    /// The current location list of a window, if the stack exists.
    pub fn loclist(&self, win: WindowId) -> Option<&QfList> {
        self.quickfix.loclist_current(win)
    }

    /// The current location list of a window, created on demand.
    pub fn loclist_mut(&mut self, win: WindowId) -> &mut QfList {
        let stack = self.quickfix.ensure_loclist(win);
        if stack.lists.is_empty() {
            stack.lists.push(new_list("", Vec::new()));
            stack.cur = 0;
        }
        let index = stack.cur.min(stack.lists.len().saturating_sub(1));
        &mut stack.lists[index]
    }

    /// Replace the current location list with a new list (:lexpr, :lgetexpr,
    /// setloclist()).
    pub fn loclist_set(&mut self, win: WindowId, entries: Vec<QfEntry>) {
        {
            let stack = self.quickfix.ensure_loclist(win);
            push_new_list(&mut stack.lists, &mut stack.cur, "", entries);
        }
        self.refresh_qf_window();
    }

    /// Append entries to a window's current location list.
    pub fn loclist_push(&mut self, win: WindowId, entries: Vec<QfEntry>) {
        {
            let stack = self.quickfix.ensure_loclist(win);
            append_entries(&mut stack.lists, &mut stack.cur, "", entries);
        }
        self.refresh_qf_window();
    }

    /// Replace the entries of a window's current location list in place.
    pub fn loclist_replace_entries(
        &mut self,
        win: WindowId,
        title: Option<&str>,
        entries: Vec<QfEntry>,
    ) {
        {
            let stack = self.quickfix.ensure_loclist(win);
            replace_entries(&mut stack.lists, &mut stack.cur, title, entries);
        }
        self.refresh_qf_window();
    }

    /// Open a quickfix window showing the current global list.
    ///
    /// An existing quickfix window is reused (and made current); otherwise the
    /// current window is split with Editor::split_window(false), the new window
    /// is switched to the quickfix buffer and marked with Window::quickfix.
    /// Height 0 means "keep the existing size, use ten lines for a new
    /// window"; a positive value resizes the window. Returns the window id.
    pub fn qf_open_window(&mut self, height: usize) -> VimResult<WindowId> {
        self.open_qf_window(QfTarget::Quickfix, height)
    }

    /// Open a window showing a window's location list (:lopen).
    ///
    /// The same single-window policy as qf_open_window() applies.
    pub fn loclist_open_window(&mut self, source: WindowId, height: usize) -> VimResult<WindowId> {
        let _ = self.loclist_mut(source);
        self.open_qf_window(QfTarget::Location(source), height)
    }

    /// Close the quickfix window, when one is open.
    pub fn qf_close_window(&mut self) -> VimResult<()> {
        if let Some(id) = self.live_qf_window() {
            self.windows.close(id)?;
            self.quickfix.qf_win = None;
            self.quickfix.target = QfTarget::Quickfix;
        }
        Ok(())
    }

    /// Close the location-list window belonging to source, when open.
    pub fn loclist_close_window(&mut self, source: WindowId) -> VimResult<()> {
        if self.quickfix.target == QfTarget::Location(source) {
            return self.qf_close_window();
        }
        Ok(())
    }

    /// True when the current window is a quickfix/location-list window.
    pub fn in_quickfix_window(&self) -> bool {
        self.windows.current().quickfix
    }

    /// Number of the quickfix buffer, created lazily.
    pub fn qf_bufnr(&mut self) -> i32 {
        if let Some(id) = self.quickfix.qf_bufnr {
            if self.buffer(id).is_ok() {
                return id;
            }
        }
        let id = self.new_buffer();
        if let Ok(buffer) = self.buffer_mut(id) {
            buffer.name = Some(PathBuf::from("quickfix"));
        }
        self.quickfix.qf_bufnr = Some(id);
        id
    }

    /// Number of the quickfix buffer, or 0 when it has not been created.
    pub fn qf_buffer_number(&self) -> i32 {
        self.quickfix
            .qf_bufnr
            .filter(|id| self.buffer(*id).is_ok())
            .unwrap_or(0)
    }

    /// Id of the window showing a quickfix/location list, if any.
    pub fn qf_window_id(&self) -> Option<WindowId> {
        if let Some(id) = self.quickfix.qf_win {
            if self.windows.get(id).is_some_and(|window| window.quickfix) {
                return Some(id);
            }
        }
        self.windows
            .ids()
            .into_iter()
            .find(|id| self.windows.get(*id).is_some_and(|window| window.quickfix))
    }

    /// Source window whose location list is displayed, when the quickfix window
    /// shows a location list.
    pub fn qf_location_source(&self) -> Option<WindowId> {
        match self.quickfix.target {
            QfTarget::Location(source) => Some(source),
            QfTarget::Quickfix => None,
        }
    }

    /// Refresh the quickfix window after callers edited a QfList directly.
    pub fn qf_refresh(&mut self) {
        self.refresh_qf_window();
    }

    /// Jump to a 1-based entry of the quickfix (None) or location (Some) list.
    fn jump_to_index(&mut self, location: Option<WindowId>, n: usize) -> VimResult<bool> {
        let (target, entry) = {
            let list = match location {
                Some(win) => self.quickfix.loclist_current(win),
                None => self.quickfix.current_list(),
            };
            let Some(list) = list else {
                return Ok(false);
            };
            if list.entries.is_empty() {
                return Ok(false);
            }
            let target = if n == 0 {
                if list.idx > 0 {
                    list.idx
                } else {
                    list.first_valid_index()
                }
            } else {
                n.min(list.entries.len())
            };
            if target == 0 {
                return Ok(false);
            }
            let entry = list.entries[target - 1].clone();
            if !entry.valid {
                return Ok(false);
            }
            (target, entry)
        };
        self.leave_qf_window()?;
        let bufnr = self.open_entry_target(&entry)?;
        {
            let list = match location {
                Some(win) => self.quickfix.loclist_current_mut(win),
                None => self.quickfix.current_list_mut(),
            };
            if let Some(list) = list {
                list.idx = target;
                if let Some(stored) = list.entries.get_mut(target - 1) {
                    stored.bufnr = Some(bufnr);
                }
            }
        }
        let pos = Pos::new(entry.lnum.saturating_sub(1), entry.col.saturating_sub(1));
        self.set_cursor(pos);
        let cursor = self.cursor();
        let current = self.windows.current_id();
        if let Some(window) = self.windows.get_mut(current) {
            window.top_line = cursor.line.saturating_sub(1);
        }
        self.refresh_qf_window();
        Ok(true)
    }

    /// Move out of a quickfix window before jumping to an entry.
    fn leave_qf_window(&mut self) -> VimResult<()> {
        if !self.in_quickfix_window() {
            return Ok(());
        }
        let current = self.windows.current_id();
        let other = self
            .windows
            .ids()
            .into_iter()
            .find(|id| *id != current && self.windows.get(*id).is_some_and(|w| !w.quickfix));
        match other {
            Some(id) => self.goto_window(id),
            // The quickfix window is alone: create a normal window above it.
            None => self.split_window(false).map(|_| ()),
        }
    }

    /// Open the buffer/file of one entry and make it current.
    fn open_entry_target(&mut self, entry: &QfEntry) -> VimResult<i32> {
        if let Some(bufnr) = entry.bufnr {
            if self.buffer(bufnr).is_ok() {
                self.switch_buffer(bufnr)?;
                return Ok(bufnr);
            }
        }
        if entry.filename.is_empty() {
            return Ok(self.current_bufnr());
        }
        let path = PathBuf::from(&entry.filename);
        self.open_file(&path).map_err(|error| {
            VimError::new(
                "E484",
                format!("Can't open file {}: {}", path.display(), error.message),
            )
        })
    }

    /// Shared implementation of :copen/:lopen.
    fn open_qf_window(&mut self, target: QfTarget, height: usize) -> VimResult<WindowId> {
        if let Some(id) = self.live_qf_window() {
            self.quickfix.target = target;
            self.goto_window(id)?;
            if height > 0 {
                let _ = self.windows.set_size(height, false);
            }
            self.refresh_qf_window();
            return Ok(id);
        }
        let id = self.split_window(false)?;
        let bufnr = self.qf_bufnr();
        self.switch_buffer(bufnr)?;
        if let Some(window) = self.windows.get_mut(id) {
            window.quickfix = true;
        }
        // 'buftype' is not in this build's option table; ignore the failure.
        let _ = self.set_option(
            "buftype",
            OptValue::Str("quickfix".to_string()),
            SetScope::Local,
        );
        let _ = self.set_window_option(id, "winfixheight", OptValue::Bool(true));
        let wanted = if height == 0 { 10 } else { height };
        let _ = self.windows.set_size(wanted, false);
        self.quickfix.qf_win = Some(id);
        self.quickfix.target = target;
        self.refresh_qf_window();
        Ok(id)
    }

    /// Id of the live quickfix window, updating the cached one.
    fn live_qf_window(&mut self) -> Option<WindowId> {
        if let Some(id) = self.quickfix.qf_win {
            if self.windows.get(id).is_some_and(|window| window.quickfix) {
                return Some(id);
            }
        }
        let found = self
            .windows
            .ids()
            .into_iter()
            .find(|id| self.windows.get(*id).is_some_and(|window| window.quickfix));
        self.quickfix.qf_win = found;
        found
    }

    /// Rewrite the quickfix buffer from the list currently displayed.
    fn refresh_qf_window(&mut self) {
        if self.live_qf_window().is_none() {
            return;
        }
        let Some((lines, cursor)) = self.render_qf_target() else {
            return;
        };
        let bufnr = self.qf_bufnr();
        if let Some(win) = self.live_qf_window() {
            if let Some(window) = self.windows.get_mut(win) {
                window.bufnr = bufnr;
            }
        }
        let last = lines.len().saturating_sub(1);
        let cursor = cursor.min(last);
        if let Ok(buffer) = self.buffer_mut(bufnr) {
            let end = buffer.line_count();
            let _ = buffer.replace_line_range(0, end, lines);
            buffer.set_changed(false);
            buffer.last_cursor = Pos::new(cursor, 0);
        }
        if let Some(win) = self.live_qf_window() {
            if let Some(window) = self.windows.get_mut(win) {
                window.cursor = Pos::new(cursor, 0);
                window.curswant = 0;
                window.top_line = cursor.saturating_sub(1);
            }
        }
    }

    /// Render the currently displayed list as quickfix buffer lines plus the
    /// zero-based cursor line.
    fn render_qf_target(&self) -> Option<(Vec<String>, usize)> {
        let list = match self.quickfix.target {
            QfTarget::Quickfix => self.quickfix.current_list()?,
            QfTarget::Location(source) => self.quickfix.loclist_current(source)?,
        };
        let mut lines = Vec::with_capacity(list.entries.len() + 1);
        let offset = if list.title.is_empty() {
            0
        } else {
            lines.push(list.title.clone());
            1
        };
        for (index, entry) in list.entries.iter().enumerate() {
            lines.push(self.format_qf_line(index + 1, entry));
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        let cursor = if list.idx > 0 {
            offset + list.idx - 1
        } else {
            offset
        };
        let last = lines.len().saturating_sub(1);
        Some((lines, cursor.min(last)))
    }

    /// One quickfix buffer line, e.g. " 1 file.txt|12 col 3| error text".
    fn format_qf_line(&self, number: usize, entry: &QfEntry) -> String {
        let file = if !entry.filename.is_empty() {
            entry.filename.clone()
        } else if let Some(bufnr) = entry.bufnr {
            self.buffer(bufnr)
                .map(|buffer| buffer.display_name())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let mut range = String::new();
        if entry.lnum > 0 {
            range.push_str(&entry.lnum.to_string());
            if entry.end_lnum > 0 && entry.end_lnum != entry.lnum {
                range.push('-');
                range.push_str(&entry.end_lnum.to_string());
            }
            if entry.col > 0 {
                range.push_str(" col ");
                range.push_str(&entry.col.to_string());
            }
        }
        let text = if entry.valid {
            entry.text.trim_start()
        } else {
            entry.text.as_str()
        };
        format!("{number:>2} {file}|{range}| {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(file: &str, lnum: usize, col: usize, valid: bool) -> QfEntry {
        QfEntry {
            filename: file.to_string(),
            bufnr: None,
            lnum,
            end_lnum: 0,
            col,
            text: format!("message {lnum}"),
            entry_type: 'E',
            valid,
        }
    }

    fn buffer_lines(ed: &mut Editor) -> Vec<String> {
        let bufnr = ed.qf_bufnr();
        let buffer = ed.buffer(bufnr).unwrap();
        (0..buffer.line_count())
            .map(|index| buffer.line_str(index).unwrap().to_string())
            .collect()
    }

    #[test]
    fn set_creates_a_list_and_selects_the_first_valid_entry() {
        let mut ed = Editor::new();
        ed.qf_set(
            "first",
            vec![entry("a", 1, 0, false), entry("b", 2, 0, true)],
        );
        assert_eq!(ed.qf_state().lists().len(), 1);
        assert_eq!(ed.qf_current(), Some(2));
        assert_eq!(ed.qf_state().current_list().unwrap().nr, 1);
    }

    #[test]
    fn set_truncates_newer_lists_and_caps_the_stack() {
        let mut ed = Editor::new();
        ed.qf_set("one", vec![entry("a", 1, 0, true)]);
        ed.qf_set("two", vec![entry("a", 1, 0, true)]);
        ed.qf_set("three", vec![entry("a", 1, 0, true)]);
        assert_eq!(ed.qf_state().lists().len(), 3);
        ed.qf_older(1).unwrap();
        assert_eq!(ed.qf_state().current_list().unwrap().title, "two");
        ed.qf_set("four", vec![entry("a", 1, 0, true)]);
        let titles: Vec<&str> = ed
            .qf_state()
            .lists()
            .iter()
            .map(|list| list.title.as_str())
            .collect();
        assert_eq!(titles, vec!["one", "two", "four"]);
        for _ in 0..20 {
            ed.qf_set("x", vec![entry("a", 1, 0, true)]);
        }
        assert_eq!(ed.qf_state().lists().len(), QF_STACK_LEN);
    }

    #[test]
    fn older_and_newer_move_through_the_stack() {
        let mut ed = Editor::new();
        ed.qf_set("one", Vec::new());
        ed.qf_set("two", Vec::new());
        ed.qf_newer(1).unwrap_err();
        ed.qf_older(1).unwrap();
        assert_eq!(ed.qf_state().current_list().unwrap().title, "one");
        ed.qf_older(1).unwrap_err();
        ed.qf_newer(1).unwrap();
        assert_eq!(ed.qf_state().current_list().unwrap().title, "two");
    }

    #[test]
    fn push_appends_to_the_current_list_and_creates_when_empty() {
        let mut ed = Editor::new();
        ed.qf_push("caddexpr", vec![entry("a", 1, 0, true)]);
        assert_eq!(ed.qf_state().lists().len(), 1);
        assert_eq!(ed.qf_state().current_list().unwrap().entries.len(), 1);
        ed.qf_push("", vec![entry("b", 2, 0, true)]);
        assert_eq!(ed.qf_state().lists().len(), 1);
        assert_eq!(ed.qf_state().current_list().unwrap().entries.len(), 2);
        assert_eq!(ed.qf_state().current_list().unwrap().title, "caddexpr");
    }

    #[test]
    fn next_and_prev_skip_invalid_entries() {
        let mut ed = Editor::new();
        ed.qf_set(
            "list",
            vec![
                entry("a", 1, 0, true),
                entry("a", 2, 0, false),
                entry("a", 3, 0, true),
            ],
        );
        assert_eq!(ed.qf_current(), Some(1));
        assert!(ed.qf_next(1, false).unwrap());
        assert_eq!(ed.qf_current(), Some(3));
        assert!(
            !ed.qf_next(1, false).unwrap(),
            "no entry after the last one"
        );
        assert!(ed.qf_prev(1, false).unwrap());
        assert_eq!(ed.qf_current(), Some(1));
        assert!(!ed.qf_prev(1, false).unwrap());
    }

    #[test]
    fn wrapping_only_happens_when_requested() {
        let mut ed = Editor::new();
        ed.qf_set("list", vec![entry("a", 1, 0, true), entry("a", 2, 0, true)]);
        assert!(ed.qf_next(1, true).unwrap());
        assert_eq!(ed.qf_current(), Some(2));
        assert!(ed.qf_next(1, true).unwrap());
        assert_eq!(ed.qf_current(), Some(1), "wrap to the first entry");
        assert!(ed.qf_prev(1, true).unwrap());
        assert_eq!(ed.qf_current(), Some(2));
    }

    #[test]
    fn first_and_last_use_valid_entries() {
        let mut ed = Editor::new();
        ed.qf_set(
            "list",
            vec![
                entry("a", 1, 0, false),
                entry("a", 2, 0, true),
                entry("a", 3, 0, true),
                entry("a", 4, 0, false),
            ],
        );
        assert!(ed.qf_last().unwrap());
        assert_eq!(ed.qf_current(), Some(3));
        assert!(ed.qf_first().unwrap());
        assert_eq!(ed.qf_current(), Some(2));
    }

    #[test]
    fn jump_to_opens_a_named_buffer_and_positions_the_cursor() {
        let mut ed = Editor::new();
        let target = ed.new_buffer();
        {
            let buffer = ed.buffer_mut(target).unwrap();
            buffer.name = Some(PathBuf::from("target.txt"));
            buffer
                .replace_line_range(
                    0,
                    1,
                    vec![
                        "one".to_string(),
                        "two".to_string(),
                        "three".to_string(),
                        "four".to_string(),
                    ],
                )
                .unwrap();
        }
        ed.switch_buffer(target).unwrap();
        ed.qf_set(
            "list",
            vec![QfEntry {
                filename: "target.txt".to_string(),
                bufnr: None,
                lnum: 3,
                end_lnum: 0,
                col: 2,
                text: "oops".to_string(),
                entry_type: 'E',
                valid: true,
            }],
        );
        assert!(ed.qf_jump_to(1).unwrap());
        assert_eq!(ed.current_bufnr(), target);
        assert_eq!(ed.cursor(), Pos::new(2, 1));
        assert_eq!(ed.qf_current(), Some(1));
    }

    #[test]
    fn jump_to_rejects_empty_lists_and_invalid_entries() {
        let mut ed = Editor::new();
        assert!(!ed.qf_jump_to(1).unwrap());
        ed.qf_set("list", vec![entry("a", 1, 0, false)]);
        assert!(!ed.qf_jump_to(1).unwrap());
        assert!(!ed.qf_first().unwrap());
        assert!(ed.qf_current().is_none());
    }

    #[test]
    fn quickfix_window_is_marked_and_kept_in_sync() {
        let mut ed = Editor::new();
        ed.qf_set(
            "errors",
            vec![entry("a.txt", 12, 3, true), entry("b.txt", 7, 0, true)],
        );
        let id = ed.qf_open_window(10).unwrap();
        assert!(ed.in_quickfix_window());
        assert_eq!(ed.windows.current_id(), id);
        assert!(ed.windows.get(id).unwrap().quickfix);
        let lines = buffer_lines(&mut ed);
        assert_eq!(lines[0], "errors", "the title occupies the first line");
        assert_eq!(lines[1], " 1 a.txt|12 col 3| message 12");
        assert_eq!(lines[2], " 2 b.txt|7| message 7");

        ed.qf_set("errors", vec![entry("c.txt", 1, 1, true)]);
        let lines = buffer_lines(&mut ed);
        assert_eq!(lines[1], " 1 c.txt|1 col 1| message 1");

        ed.qf_close_window().unwrap();
        assert_eq!(ed.window_count(), 1);
        assert!(!ed.in_quickfix_window());
    }

    #[test]
    fn opening_an_existing_quickfix_window_reuses_it() {
        let mut ed = Editor::new();
        ed.qf_set("errors", vec![entry("a.txt", 1, 0, true)]);
        let first = ed.qf_open_window(0).unwrap();
        let second = ed.qf_open_window(0).unwrap();
        assert_eq!(first, second);
        assert_eq!(ed.window_count(), 2);
    }

    #[test]
    fn closing_without_a_quickfix_window_is_a_no_op() {
        let mut ed = Editor::new();
        ed.qf_close_window().unwrap();
        assert_eq!(ed.window_count(), 1);
    }

    #[test]
    fn location_lists_are_per_window() {
        let mut ed = Editor::new();
        let right = ed.current_win_id();
        ed.split_window(false).unwrap();
        let left = ed.current_win_id();
        assert_ne!(left, right);
        ed.loclist_set(left, vec![entry("left", 1, 0, true)]);
        assert!(ed.loclist(right).is_none());
        ed.loclist_set(right, vec![entry("right", 2, 0, true)]);
        assert_eq!(ed.loclist(left).unwrap().entries[0].filename, "left");
        assert_eq!(ed.loclist(right).unwrap().entries[0].filename, "right");
        ed.loclist_mut(left).title = "left list".to_string();
        assert_eq!(ed.loclist(left).unwrap().title, "left list");
    }

    #[test]
    fn location_list_stack_supports_older_and_newer() {
        let mut ed = Editor::new();
        let win = ed.current_win_id();
        ed.loclist_set(win, vec![entry("one", 1, 0, true)]);
        ed.loclist_set(win, vec![entry("two", 1, 0, true)]);
        assert_eq!(ed.qf_state().loclist_count(win), 2);
        ed.loclist_older(win, 1).unwrap();
        assert_eq!(ed.loclist(win).unwrap().entries[0].filename, "one");
        ed.loclist_newer(win, 1).unwrap();
        assert_eq!(ed.loclist(win).unwrap().entries[0].filename, "two");
        ed.loclist_newer(win, 1).unwrap_err();
    }

    #[test]
    fn location_list_window_shows_its_list() {
        let mut ed = Editor::new();
        let source = ed.current_win_id();
        ed.loclist_set(source, vec![entry("loc.txt", 4, 2, true)]);
        let id = ed.loclist_open_window(source, 5).unwrap();
        assert!(ed.windows.get(id).unwrap().quickfix);
        assert_eq!(ed.qf_location_source(), Some(source));
        let lines = buffer_lines(&mut ed);
        assert_eq!(lines[0], " 1 loc.txt|4 col 2| message 4");
    }

    #[test]
    fn errorformat_survives_in_the_state() {
        let mut ed = Editor::new();
        assert!(ed.qf_state().errorformat().is_none());
        ed.qf_state_mut()
            .set_errorformat(Some("%f:%l:%m".to_string()));
        assert_eq!(ed.qf_state().errorformat(), Some("%f:%l:%m"));
        ed.qf_state_mut().set_errorformat(None);
        assert!(ed.qf_state().errorformat().is_none());
    }

    #[test]
    fn replace_entries_keeps_the_stack_size() {
        let mut ed = Editor::new();
        ed.qf_set("list", vec![entry("a", 1, 0, true)]);
        ed.qf_replace_entries(None, vec![entry("b", 2, 0, true)]);
        assert_eq!(ed.qf_state().lists().len(), 1);
        assert_eq!(
            ed.qf_state().current_list().unwrap().entries[0].filename,
            "b"
        );
    }
}
