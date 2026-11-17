//! Quickfix and location-list Ex commands and builtins.
//!
//! Reference: quickfix.c (ex_cc, ex_cnext, ex_cbelow, ex_copen, ex_cwindow,
//! ex_cfile, ex_cexpr, qf_age, qf_list, f_getqflist/f_setqflist/f_getloclist/
//! f_setloclist) and runtime/doc/quickfix.txt.
//!
//! The list model itself lives in rvim-editor; this module translates Ex
//! arguments and VimL values into it. The 'errorformat' option is not in this
//! build's option table, so the error format used by :cexpr/:cfile and by the
//! lines item of getqflist() comes from QuickfixState::errorformat(), falling
//! back to vim's default.
//!
//! # Supported subset
//!
//! * :copen/:lopen [height], :cclose/:lclose, :cwindow/:lwindow [height]
//! * :cnext/:cprevious [count], :cfirst/:clast, :cc [nr]
//! * :cbelow/:cabove [count] (line-based, sorted by line and column)
//! * :cexpr/:cgetexpr/:caddexpr and their l* forms
//! * :cfile/:cgetfile and their l* forms ('errorfile' is not in the option
//!   table; an omitted file name means "errors.err", vim's default)
//! * :clist [from][,to], :clist +count and :clist! (all entries)
//! * :colder/:cnewer [count] and their l* forms
//! * getqflist()/setqflist()/getloclist()/setloclist() with the title, nr, id,
//!   items, lines, idx, size, context, qfbufnr and winid properties.
//!
//! The quickfix/location list number ("nr") is also used as the list id because
//! QfList has no separate identifier; see the rvim-editor quickfix module doc.
//!
//! # Deviations from Vim
//!
//! * :cc {nr} accepts the number as an argument and as an Ex address, but only
//!   the address form uses the Quickfix address kind; '.' and '$' are accepted
//!   inside the quickfix window.
//! * :cbelow/:cabove treat entries on the same line as one candidate and do not
//!   implement the optional column-sensitive :cbefore/:cafter commands.
//! * :clist prints the index, file, line/column and text; it has no header line
//!   and no :filter support.
//! * getqflist()/setqflist() report end_col, vcol, nr, module and pattern as
//!   zero/empty strings because QfEntry does not store them.
//! * setqflist() with action 'a' appends to the current list (creating one when
//!   the stack is empty), matching vim's qf_add_entries(); the 'f' action drops
//!   the stack.
//! * getloclist()/setloclist() take the window number as the first argument;
//!   a WindowId passed through the win parameter is used directly.

use std::path::Path;

use rvim_core::{VimError, VimResult};
use rvim_editor::quickfix::{QfEntry, QfList};
use rvim_editor::{Editor, WindowId};
use rvim_viml::Env;
use rvim_viml::value::{Value, VimDict};

use crate::autodispatch;
use crate::errorformat::{ErrorFormat, ErrorItem, default_errorformat};
use crate::exec::ExOutcome;
use crate::parser::{ParsedEx, Range};
use crate::script::ScriptState;

/// Convert one 'errorformat' match into a quickfix entry.
///
/// The conversion lives in the Ex layer because rvim-editor does not depend on
/// rvim-ex. Missing line/column numbers stay 0 and the end column, error number,
/// module and search pattern are not represented by QfEntry.
impl From<ErrorItem> for QfEntry {
    fn from(item: ErrorItem) -> Self {
        Self {
            filename: item.filename.unwrap_or_default(),
            bufnr: None,
            lnum: positive_or_zero(item.lnum),
            end_lnum: positive_or_zero(item.end_lnum),
            col: positive_or_zero(item.col),
            text: item.text,
            entry_type: item.entry_type,
            valid: item.valid,
        }
    }
}

/// Convert a possibly negative i64 from the errorformat matcher to usize.
fn positive_or_zero(value: i64) -> usize {
    usize::try_from(value)
        .ok()
        .filter(|number| *number > 0)
        .unwrap_or(0)
}

/// Convert usize to i64 without panicking on absurd values.
fn to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Which stack a command operates on.
#[derive(Clone, Copy)]
enum StackRef {
    /// Global quickfix stack.
    Qf,
    /// Location-list stack of one window.
    Loc(WindowId),
}

impl StackRef {
    fn current(self, ed: &Editor) -> Option<&QfList> {
        match self {
            StackRef::Qf => ed.qf_state().current_list(),
            StackRef::Loc(win) => ed.loclist(win),
        }
    }

    fn stack_len(self, ed: &Editor) -> usize {
        match self {
            StackRef::Qf => ed.qf_state().lists().len(),
            StackRef::Loc(win) => ed.qf_state().loclist_count(win),
        }
    }

    fn stack_index(self, ed: &Editor) -> usize {
        match self {
            StackRef::Qf => ed.qf_state().current_index(),
            StackRef::Loc(win) => ed.qf_state().loclist_index(win),
        }
    }

    fn at(self, ed: &Editor, index: usize) -> Option<&QfList> {
        match self {
            StackRef::Qf => ed.qf_state().list(index),
            StackRef::Loc(win) => ed.qf_state().loclist_at(win, index),
        }
    }

    fn select(self, ed: &mut Editor, index: usize) -> bool {
        match self {
            StackRef::Qf => ed.qf_state_mut().select_index(index),
            StackRef::Loc(win) => ed.qf_state_mut().loclist_select(win, index),
        }
    }

    fn fill_new(self, ed: &mut Editor, title: &str, entries: Vec<QfEntry>) {
        match self {
            StackRef::Qf => ed.qf_set(title, entries),
            StackRef::Loc(win) => {
                ed.loclist_set(win, entries);
                ed.loclist_mut(win).title = title.to_string();
            }
        }
    }

    fn append(self, ed: &mut Editor, title: &str, entries: Vec<QfEntry>) {
        match self {
            StackRef::Qf => ed.qf_push(title, entries),
            StackRef::Loc(win) => {
                ed.loclist_push(win, entries);
                if !title.is_empty() {
                    ed.loclist_mut(win).title = title.to_string();
                }
            }
        }
    }

    fn replace(self, ed: &mut Editor, title: Option<&str>, entries: Vec<QfEntry>) {
        match self {
            StackRef::Qf => ed.qf_replace_entries(title, entries),
            StackRef::Loc(win) => ed.loclist_replace_entries(win, title, entries),
        }
    }

    fn clear(self, ed: &mut Editor) {
        match self {
            StackRef::Qf => ed.qf_state_mut().clear(),
            StackRef::Loc(win) => ed.qf_state_mut().clear_loclist(win),
        }
        ed.qf_refresh();
    }

    fn refresh(self, ed: &mut Editor) {
        ed.qf_refresh();
    }

    fn set_entry_index(self, ed: &mut Editor, index: usize) {
        let list = match self {
            StackRef::Qf => ed.qf_state_mut().current_list_mut(),
            StackRef::Loc(win) => ed.qf_state_mut().loclist_current_mut(win),
        };
        if let Some(list) = list {
            list.idx = index;
        }
    }

    fn jump(self, ed: &mut Editor, index: usize) -> VimResult<bool> {
        match self {
            StackRef::Qf => ed.qf_jump_to(index),
            StackRef::Loc(win) => ed.loclist_jump_to(win, index),
        }
    }

    fn first(self, ed: &mut Editor) -> VimResult<bool> {
        match self {
            StackRef::Qf => ed.qf_first(),
            StackRef::Loc(win) => ed.loclist_first(win),
        }
    }

    fn last(self, ed: &mut Editor) -> VimResult<bool> {
        match self {
            StackRef::Qf => ed.qf_last(),
            StackRef::Loc(win) => ed.loclist_last(win),
        }
    }

    fn next(self, ed: &mut Editor, count: usize, wrap: bool) -> VimResult<bool> {
        match self {
            StackRef::Qf => ed.qf_next(count, wrap),
            StackRef::Loc(win) => ed.loclist_next(win, count, wrap),
        }
    }

    fn prev(self, ed: &mut Editor, count: usize, wrap: bool) -> VimResult<bool> {
        match self {
            StackRef::Qf => ed.qf_prev(count, wrap),
            StackRef::Loc(win) => ed.loclist_prev(win, count, wrap),
        }
    }

    fn older(self, ed: &mut Editor, count: usize) -> VimResult<()> {
        match self {
            StackRef::Qf => ed.qf_older(count),
            StackRef::Loc(win) => ed.loclist_older(win, count),
        }
    }

    fn newer(self, ed: &mut Editor, count: usize) -> VimResult<()> {
        match self {
            StackRef::Qf => ed.qf_newer(count),
            StackRef::Loc(win) => ed.loclist_newer(win, count),
        }
    }

    fn open_window(self, ed: &mut Editor, height: usize) -> VimResult<WindowId> {
        match self {
            StackRef::Qf => ed.qf_open_window(height),
            StackRef::Loc(win) => ed.loclist_open_window(win, height),
        }
    }

    fn close_window(self, ed: &mut Editor) -> VimResult<()> {
        match self {
            StackRef::Qf => ed.qf_close_window(),
            StackRef::Loc(win) => ed.loclist_close_window(win),
        }
    }

    fn has_valid_entry(self, ed: &Editor) -> bool {
        self.current(ed)
            .is_some_and(|list| list.entries.iter().any(|entry| entry.valid))
    }
}

/// Is a command name a location-list command?
fn is_loclist_name(name: &str) -> bool {
    name.starts_with('l')
}

/// Window whose location list an l* command addresses.
///
/// Inside a quickfix window the location list of the displayed source window is
/// used (vim keeps w_llist_ref); otherwise the current window is used.
fn loclist_window(ed: &Editor) -> WindowId {
    if ed.in_quickfix_window() {
        if let Some(source) = ed.qf_location_source() {
            return source;
        }
        if let Some(id) = ed
            .windows
            .ids()
            .into_iter()
            .find(|id| ed.windows.get(*id).is_some_and(|window| !window.quickfix))
        {
            return id;
        }
    }
    ed.current_win_id()
}

/// Pick the stack for a command name.
fn stack_for(ed: &Editor, name: &str) -> StackRef {
    if is_loclist_name(name) {
        StackRef::Loc(loclist_window(ed))
    } else {
        StackRef::Qf
    }
}

/// Fire an autocommand whose pattern is the current buffer name.
fn fire(ed: &mut Editor, env: &mut Env, state: &mut ScriptState, event: &str) {
    let pattern = autodispatch::buffer_event_name(ed);
    autodispatch::fire(ed, env, state, event, &pattern);
}

/// Fire BufEnter after a successful jump.
fn fire_buf_enter(ed: &mut Editor, env: &mut Env, state: &mut ScriptState) {
    fire(ed, env, state, "BufEnter");
}

fn no_errors() -> VimError {
    VimError::new("E42", "No errors")
}

fn no_more_items() -> VimError {
    VimError::new("E553", "No more valid entries")
}

fn invalid_argument(argument: &str) -> VimError {
    VimError::new("E474", format!("Invalid argument: {argument}"))
}

/// The title vim gives to a list created by an Ex command.
fn command_title(name: &str) -> String {
    format!(":{name}")
}

/// Compile an error format, mapping the parse diagnostics to Vim error codes.
fn compile_errorformat(spec: Option<&str>, fallback: &str) -> VimResult<ErrorFormat> {
    let spec = spec.unwrap_or(fallback);
    ErrorFormat::parse(spec).map_err(|message| {
        let code = if message.starts_with("E372") {
            "E372"
        } else if message.starts_with("E374") {
            "E374"
        } else {
            "E373"
        };
        VimError::new(code, message)
    })
}

/// Parse text lines into entries, keeping unmatched lines as invalid entries
/// exactly like vim's qf_parse_line_nomatch().
fn parse_lines(ed: &Editor, lines: &[String], efm: Option<&str>) -> VimResult<Vec<QfEntry>> {
    let fallback = ed.qf_state().errorformat().unwrap_or(default_errorformat());
    let format = compile_errorformat(efm, fallback)?;
    let bufnr = ed.current_bufnr();
    let mut entries = Vec::with_capacity(lines.len());
    for line in lines {
        match format.match_line(line) {
            Some(item) => {
                let mut entry = QfEntry::from(item);
                if entry.filename.is_empty() && entry.valid {
                    entry.bufnr = Some(bufnr);
                }
                entries.push(entry);
            }
            None => {
                let text = line.trim_end_matches(['\n', '\r']).to_string();
                entries.push(QfEntry {
                    filename: String::new(),
                    bufnr: None,
                    lnum: 0,
                    end_lnum: 0,
                    col: 0,
                    text,
                    entry_type: '\0',
                    valid: false,
                });
            }
        }
    }
    Ok(entries)
}

/// Turn a VimL String/List value into the lines to parse.
fn value_lines(value: &Value) -> VimResult<Vec<String>> {
    match value {
        Value::String(text) => Ok(text.lines().map(str::to_string).collect()),
        Value::List(items) => Ok(items
            .borrow()
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect()),
        _ => Err(VimError::new("E777", "String or List expected")),
    }
}

/// Evaluate the expression of :cexpr and return its parsed entries.
fn expression_entries(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<Vec<QfEntry>> {
    let source = p.args.trim();
    if source.is_empty() {
        return Err(VimError::new("E471", "Argument required"));
    }
    let value = crate::script::eval_expr(ed, env, state, source)?;
    let lines = value_lines(&value)?;
    parse_lines(ed, &lines, None)
}

/// The numeric count of an Ex address, if one was given.
fn range_number(ed: &Editor, p: &ParsedEx) -> VimResult<Option<usize>> {
    if p.addr_count == 0 {
        return Ok(None);
    }
    match &p.range {
        Range::None => Ok(None),
        Range::Lines(_, end) => Ok(Some(*end)),
        Range::Whole => Ok(Some(1)),
        Range::Current => Ok(Some(ed.cursor().line + 1)),
        Range::Deferred(text) => {
            let text = text.trim();
            if text == "." {
                return Ok(Some(ed.cursor().line + 1));
            }
            if text == "$" {
                return Ok(Some(ed.line_count()));
            }
            let (_, last) = crate::addr::resolve_deferred_pair(ed, text)?;
            Ok(Some(last + 1))
        }
    }
}

/// Count for :cnext/:colder and similar [count] commands.
fn count_argument(ed: &Editor, p: &ParsedEx) -> VimResult<usize> {
    if let Some(number) = range_number(ed, p)? {
        return Ok(number.max(1));
    }
    let argument = p.args.trim();
    if argument.is_empty() {
        return Ok(1);
    }
    argument
        .parse::<usize>()
        .map(|number| number.max(1))
        .map_err(|_| invalid_argument(argument))
}

/// Optional entry number for :cc/:cfirst/:clast.
fn entry_argument(ed: &Editor, stack: StackRef, p: &ParsedEx) -> VimResult<Option<usize>> {
    let argument = p.args.trim();
    if !argument.is_empty() {
        if argument == "." {
            return Ok(Some(entry_under_cursor(ed, stack)));
        }
        if argument == "$" {
            return Ok(Some(list_len(ed, stack).max(1)));
        }
        return argument
            .parse::<usize>()
            .map(Some)
            .map_err(|_| invalid_argument(argument));
    }
    range_number(ed, p)
}

fn list_len(ed: &Editor, stack: StackRef) -> usize {
    stack.current(ed).map_or(0, |list| list.entries.len())
}

/// Entry number of the quickfix buffer line under the cursor.
fn entry_under_cursor(ed: &Editor, stack: StackRef) -> usize {
    let len = list_len(ed, stack).max(1);
    let offset = match stack.current(ed) {
        Some(list) if !list.title.is_empty() => 1,
        _ => 0,
    };
    let line = if ed.in_quickfix_window() {
        (ed.cursor().line + 1).saturating_sub(offset)
    } else {
        ed.cursor().line + 1
    };
    line.max(1).min(len)
}

/// Height argument of :copen/:cwindow.
fn height_argument(ed: &Editor, p: &ParsedEx) -> VimResult<Option<usize>> {
    let argument = p.args.trim();
    if !argument.is_empty() {
        return argument
            .parse::<usize>()
            .map(Some)
            .map_err(|_| invalid_argument(argument));
    }
    match range_number(ed, p)? {
        Some(number) => Ok(Some(number.max(1))),
        None => Ok(None),
    }
}

/// Require a non-empty current list.
fn require_entries(ed: &Editor, stack: StackRef) -> VimResult<()> {
    match stack.current(ed) {
        Some(list) if !list.entries.is_empty() => Ok(()),
        _ => Err(no_errors()),
    }
}

/// Convert a boolean jump result into the Ex error vim would report.
fn jump_outcome(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stack: StackRef,
    jumped: bool,
) -> VimResult<ExOutcome> {
    if jumped {
        fire_buf_enter(ed, env, state);
        return Ok(ExOutcome::Continue);
    }
    if list_len(ed, stack) == 0 {
        Err(no_errors())
    } else {
        Err(no_more_items())
    }
}

/// Move to the next/previous valid entry.
fn navigate(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stack: StackRef,
    forward: bool,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    require_entries(ed, stack)?;
    let count = count_argument(ed, p)?;
    let wrap = ed.wopt_bool("wrapscan", true);
    let jumped = if forward {
        stack.next(ed, count, wrap)?
    } else {
        stack.prev(ed, count, wrap)?
    };
    jump_outcome(ed, env, state, stack, jumped)
}

/// :cfirst/:clast and their optional entry argument.
fn first_or_last(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stack: StackRef,
    last: bool,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    require_entries(ed, stack)?;
    let jumped = match entry_argument(ed, stack, p)? {
        Some(number) => stack.jump(ed, number)?,
        None if last => stack.last(ed)?,
        None => stack.first(ed)?,
    };
    jump_outcome(ed, env, state, stack, jumped)
}

/// :cc / :ll.
fn jump_to_entry(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stack: StackRef,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    require_entries(ed, stack)?;
    let target = entry_argument(ed, stack, p)?.unwrap_or(0);
    let jumped = stack.jump(ed, target)?;
    jump_outcome(ed, env, state, stack, jumped)
}

/// Whether an entry belongs to the buffer being searched by :cbelow/:cabove.
fn entry_in_buffer(ed: &Editor, entry: &QfEntry, bufnr: i32) -> bool {
    if entry.filename.is_empty() {
        return entry.bufnr.is_none_or(|number| number == bufnr);
    }
    if entry.bufnr == Some(bufnr) {
        return true;
    }
    let path = Path::new(&entry.filename);
    ed.buffer(bufnr)
        .ok()
        .and_then(|buffer| buffer.name.as_deref())
        .is_some_and(|name| name == path)
}

/// :cbelow / :cabove.
fn vertical_jump(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stack: StackRef,
    forward: bool,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    require_entries(ed, stack)?;
    let count = count_argument(ed, p)?;
    let bufnr = ed.current_bufnr();
    let line = ed.cursor().line + 1;
    let candidates: Vec<(usize, usize, usize)> = {
        let list = stack.current(ed).ok_or_else(no_errors)?;
        let mut found: Vec<(usize, usize, usize)> = list
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.valid && entry_in_buffer(ed, entry, bufnr))
            .map(|(index, entry)| (index + 1, entry.lnum, entry.col))
            .collect();
        found.sort_by_key(|(_, lnum, col)| (*lnum, *col));
        found
    };
    let chosen = if forward {
        let mut below: Vec<usize> = candidates
            .iter()
            .filter(|(_, lnum, _)| *lnum > line)
            .map(|(index, _, _)| *index)
            .collect();
        if below.len() > count {
            below.truncate(count);
        }
        below.last().copied()
    } else {
        let above: Vec<usize> = candidates
            .iter()
            .filter(|(_, lnum, _)| *lnum < line)
            .map(|(index, _, _)| *index)
            .collect();
        if above.len() < count {
            above.first().copied()
        } else {
            above.get(above.len() - count).copied()
        }
    };
    let Some(index) = chosen else {
        return Err(no_errors());
    };
    let jumped = stack.jump(ed, index)?;
    jump_outcome(ed, env, state, stack, jumped)
}

/// :copen / :lopen.
fn open_window(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stack: StackRef,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    require_entries(ed, stack)?;
    let height = height_argument(ed, p)?.unwrap_or(0);
    let before = ed.window_count();
    fire(ed, env, state, "WinLeave");
    stack.open_window(ed, height)?;
    if ed.window_count() > before {
        fire(ed, env, state, "WinNew");
    }
    fire(ed, env, state, "WinEnter");
    Ok(ExOutcome::Continue)
}

/// :cclose / :lclose.
fn close_window(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stack: StackRef,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    if !p.args.trim().is_empty() {
        return Err(invalid_argument(p.args.trim()));
    }
    let before = ed.window_count();
    fire(ed, env, state, "WinLeave");
    stack.close_window(ed)?;
    if ed.window_count() < before {
        let bufnr = ed.current_bufnr();
        ed.switch_buffer(bufnr)?;
        fire(ed, env, state, "WinEnter");
    }
    Ok(ExOutcome::Continue)
}

/// :cwindow / :lwindow.
fn toggle_window(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stack: StackRef,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let _ = height_argument(ed, p)?;
    if stack.has_valid_entry(ed) {
        return open_window(ed, env, state, stack, p);
    }
    fire(ed, env, state, "WinLeave");
    let before = ed.window_count();
    stack.close_window(ed)?;
    if ed.window_count() < before {
        let bufnr = ed.current_bufnr();
        ed.switch_buffer(bufnr)?;
        fire(ed, env, state, "WinEnter");
    }
    Ok(ExOutcome::Continue)
}

/// Display a list entry the way :clist does.
fn list_entry_text(ed: &Editor, index: usize, entry: &QfEntry) -> String {
    let file = if !entry.filename.is_empty() {
        entry.filename.clone()
    } else if let Some(bufnr) = entry.bufnr {
        ed.buffer(bufnr)
            .map(|buffer| buffer.display_name())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let mut line = format!("{index:>2}");
    if !file.is_empty() {
        line.push(' ');
        line.push_str(&file);
    }
    if entry.lnum > 0 {
        line.push(':');
        line.push_str(&entry.lnum.to_string());
        if entry.col > 0 {
            line.push_str(" col ");
            line.push_str(&entry.col.to_string());
        }
    }
    line.push_str(": ");
    let text = if entry.valid {
        entry.text.trim_start()
    } else {
        entry.text.as_str()
    };
    line.push_str(text);
    line
}

/// Parse the optional [from][,to] or +count argument of :clist.
fn parse_list_range(
    argument: &str,
    total: usize,
    current: usize,
) -> VimResult<Option<(usize, usize)>> {
    let argument = argument.trim();
    if argument.is_empty() {
        return Ok(Some((1, total)));
    }
    if let Some(count) = argument.strip_prefix('+') {
        let count = count
            .trim()
            .parse::<usize>()
            .map_err(|_| invalid_argument(argument))?;
        let from = current.max(1).min(total.max(1));
        return Ok(Some((from, (from + count).min(total))));
    }
    let mut parts = argument.split(',');
    let first = parts.next().unwrap_or("").trim();
    let second = parts.next().map(str::trim);
    if parts.next().is_some() {
        return Err(invalid_argument(argument));
    }
    let resolve = |text: &str, default: usize| -> VimResult<usize> {
        if text.is_empty() {
            return Ok(default);
        }
        let value = text
            .parse::<i64>()
            .map_err(|_| invalid_argument(argument))?;
        if value < 0 {
            let back = value.unsigned_abs() as usize;
            Ok(if back > total { 1 } else { total + 1 - back })
        } else {
            let value = value as usize;
            Ok(value.clamp(1, total.max(1)))
        }
    };
    let from = resolve(first, 1)?;
    let to = match second {
        Some(text) => resolve(text, total)?,
        None => from,
    };
    Ok(Some((from, to)))
}

/// :clist / :llist.
fn list_entries(ed: &mut Editor, stack: StackRef, p: &ParsedEx) -> VimResult<ExOutcome> {
    require_entries(ed, stack)?;
    let total = list_len(ed, stack);
    let current = stack.current(ed).map_or(0, |list| list.idx);
    let Some((from, to)) = parse_list_range(p.args.trim(), total, current)? else {
        return Ok(ExOutcome::Continue);
    };
    let all = p.bang;
    let mut printed = false;
    for index in from..=to.min(total) {
        let entry = stack
            .current(ed)
            .and_then(|list| list.entries.get(index.saturating_sub(1)));
        let Some(entry) = entry else {
            continue;
        };
        if !entry.valid && !all {
            continue;
        }
        let text = list_entry_text(ed, index, entry);
        ed.messages.info(text);
        printed = true;
    }
    if !printed && !all {
        return Err(no_errors());
    }
    Ok(ExOutcome::Continue)
}

/// The Ex command handler for every c* / l* quickfix command.
pub fn cmd_quickfix(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    let stack = stack_for(ed, name);
    match name {
        "cexpr" | "cgetexpr" | "lexpr" | "lgetexpr" => {
            let entries = expression_entries(ed, env, state, p)?;
            let title = command_title(name);
            stack.fill_new(ed, &title, entries);
            if matches!(name, "cexpr" | "lexpr") {
                let jumped = stack.first(ed)?;
                if jumped {
                    fire_buf_enter(ed, env, state);
                }
            }
            Ok(ExOutcome::Continue)
        }
        "caddexpr" | "laddexpr" => {
            let entries = expression_entries(ed, env, state, p)?;
            stack.append(ed, "", entries);
            Ok(ExOutcome::Continue)
        }
        "cfile" | "cgetfile" | "lfile" | "lgetfile" => {
            let argument = p.args.trim();
            let path = if argument.is_empty() {
                "errors.err".to_string()
            } else {
                argument.to_string()
            };
            let text = std::fs::read_to_string(&path).map_err(|error| {
                VimError::new("E484", format!("Can't open error file {path}: {error}"))
            })?;
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            let entries = parse_lines(ed, &lines, None)?;
            let title = command_title(name);
            stack.fill_new(ed, &title, entries);
            if matches!(name, "cfile" | "lfile") {
                let jumped = stack.first(ed)?;
                if jumped {
                    fire_buf_enter(ed, env, state);
                }
            }
            Ok(ExOutcome::Continue)
        }
        "copen" | "lopen" => open_window(ed, env, state, stack, p),
        "cclose" | "lclose" => close_window(ed, env, state, stack, p),
        "cwindow" | "lwindow" => toggle_window(ed, env, state, stack, p),
        "cnext" | "lnext" => navigate(ed, env, state, stack, true, p),
        "cprevious" | "lprevious" | "cNext" | "lNext" => navigate(ed, env, state, stack, false, p),
        "cfirst" | "lfirst" | "crewind" | "lrewind" => {
            first_or_last(ed, env, state, stack, false, p)
        }
        "clast" | "llast" => first_or_last(ed, env, state, stack, true, p),
        "cc" | "ll" => jump_to_entry(ed, env, state, stack, p),
        "cbelow" | "lbelow" => vertical_jump(ed, env, state, stack, true, p),
        "cabove" | "labove" => vertical_jump(ed, env, state, stack, false, p),
        "clist" | "llist" => list_entries(ed, stack, p),
        "colder" | "lolder" => {
            let count = count_argument(ed, p)?;
            stack.older(ed, count)?;
            Ok(ExOutcome::Continue)
        }
        "cnewer" | "lnewer" => {
            let count = count_argument(ed, p)?;
            stack.newer(ed, count)?;
            Ok(ExOutcome::Continue)
        }
        _ => Err(invalid_argument(name)),
    }
}

// ---------------------------------------------------------------------------
// getqflist() / setqflist() / getloclist() / setloclist()
// ---------------------------------------------------------------------------

fn dict_get(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Dict(dict) => dict.borrow().get(key).cloned(),
        _ => None,
    }
}

fn dict_has(value: &Value, key: &str) -> bool {
    dict_get(value, key).is_some()
}

fn entry_bufnr(ed: &Editor, entry: &QfEntry) -> i32 {
    if let Some(bufnr) = entry.bufnr {
        if ed.buffer(bufnr).is_ok() {
            return bufnr;
        }
    }
    if entry.filename.is_empty() {
        return 0;
    }
    let path = Path::new(&entry.filename);
    ed.all_buffers()
        .iter()
        .find(|buffer| buffer.name.as_deref() == Some(path))
        .map_or(0, |buffer| buffer.id)
}

fn item_value(ed: &Editor, entry: &QfEntry) -> Value {
    let mut dict = VimDict::new();
    dict.insert("bufnr", Value::Number(i64::from(entry_bufnr(ed, entry))));
    dict.insert("lnum", Value::Number(to_i64(entry.lnum)));
    dict.insert("end_lnum", Value::Number(to_i64(entry.end_lnum)));
    dict.insert("col", Value::Number(to_i64(entry.col)));
    dict.insert("end_col", Value::Number(0));
    dict.insert("vcol", Value::Number(0));
    dict.insert("nr", Value::Number(0));
    let entry_type = if entry.entry_type == '\0' {
        String::new()
    } else {
        entry.entry_type.to_string()
    };
    dict.insert("type", Value::String(entry_type));
    dict.insert("valid", Value::Number(if entry.valid { 1 } else { 0 }));
    dict.insert("text", Value::String(entry.text.clone()));
    dict.insert("module", Value::String(String::new()));
    dict.insert("pattern", Value::String(String::new()));
    Value::dict(dict)
}

fn entries_value(ed: &Editor, entries: &[QfEntry]) -> Value {
    Value::list(entries.iter().map(|entry| item_value(ed, entry)).collect())
}

fn entry_from_dict(ed: &Editor, dict: &VimDict) -> QfEntry {
    let filename = dict
        .get("filename")
        .map(Value::to_string_value)
        .unwrap_or_default();
    let mut bufnr = dict
        .get("bufnr")
        .map(|value| value.to_number())
        .unwrap_or(0)
        .max(0);
    let lnum = dict
        .get("lnum")
        .map(|value| value.to_number())
        .filter(|number| *number > 0)
        .and_then(|number| usize::try_from(number).ok())
        .unwrap_or(0);
    let end_lnum = dict
        .get("end_lnum")
        .map(|value| value.to_number())
        .filter(|number| *number > 0)
        .and_then(|number| usize::try_from(number).ok())
        .unwrap_or(0);
    let col = dict
        .get("col")
        .map(|value| value.to_number())
        .filter(|number| *number > 0)
        .and_then(|number| usize::try_from(number).ok())
        .unwrap_or(0);
    let pattern = dict
        .get("pattern")
        .map(Value::to_string_value)
        .unwrap_or_default();
    let text = dict
        .get("text")
        .map(Value::to_string_value)
        .unwrap_or_default();
    let entry_type = dict
        .get("type")
        .map(Value::to_string_value)
        .and_then(|text| text.chars().next())
        .unwrap_or('\0');
    if bufnr > 0 && ed.buffer(bufnr as i32).is_err() {
        bufnr = 0;
    }
    if bufnr == 0 && !filename.is_empty() {
        let path = Path::new(&filename);
        if let Some(buffer) = ed
            .all_buffers()
            .iter()
            .find(|buffer| buffer.name.as_deref() == Some(path))
        {
            bufnr = i64::from(buffer.id);
        }
    }
    let mut valid = (!filename.is_empty() || bufnr > 0) && (lnum > 0 || !pattern.is_empty());
    if let Some(value) = dict.get("valid") {
        valid = value.truthy();
    }
    QfEntry {
        filename,
        bufnr: i32::try_from(bufnr).ok(),
        lnum,
        end_lnum,
        col,
        text,
        entry_type,
        valid,
    }
}

fn entries_from_list(ed: &Editor, value: &Value) -> VimResult<Vec<QfEntry>> {
    let Value::List(items) = value else {
        return Err(VimError::new("E714", "List required"));
    };
    let items = items.borrow().clone();
    let mut entries = Vec::with_capacity(items.len());
    for item in &items {
        if let Value::Dict(dict) = item {
            let dict = dict.borrow();
            entries.push(entry_from_dict(ed, &dict));
        }
    }
    Ok(entries)
}

/// Resolve the window number/id argument of getloclist()/setloclist().
fn window_by_number(ed: &Editor, number: i64) -> Option<WindowId> {
    if number <= 0 {
        return Some(ed.current_win_id());
    }
    if let Ok(id) = i32::try_from(number) {
        if ed.windows.get(id).is_some() {
            return Some(id);
        }
    }
    let index = usize::try_from(number).ok()?.checked_sub(1)?;
    ed.windows.ids().get(index).copied()
}

/// Split raw getloclist()/setloclist() arguments into the target stack and the
/// arguments that follow the window number.
fn loclist_arguments<'a>(
    ed: &Editor,
    win: Option<WindowId>,
    args: &'a [Value],
) -> (StackRef, &'a [Value]) {
    if let Some(win) = win {
        return (StackRef::Loc(win), args);
    }
    let Some(first) = args.first() else {
        return (StackRef::Loc(ed.current_win_id()), args);
    };
    let win = window_by_number(ed, first.to_number()).unwrap_or_else(|| ed.current_win_id());
    (StackRef::Loc(win), &args[1..])
}

/// Select the list addressed by the nr/id entries of a "what" dictionary.
fn selected_index(ed: &Editor, stack: StackRef, what: &Value) -> Option<usize> {
    let len = stack.stack_len(ed);
    if len == 0 {
        return None;
    }
    let mut index = stack.stack_index(ed);
    if let Some(nr) = dict_get(what, "nr") {
        index = match nr {
            Value::Number(0) => index,
            Value::Number(number) if (1..=len as i64).contains(&number) => (number as usize) - 1,
            Value::String(ref text) if text == "$" => len - 1,
            _ => return None,
        };
    }
    if let Some(id) = dict_get(what, "id") {
        index = match id {
            Value::Number(0) => index,
            Value::Number(number) if (1..=len as i64).contains(&number) => (number as usize) - 1,
            _ => return None,
        };
    }
    Some(index)
}

/// Like selected_index() but rejects an out-of-range selector.
fn required_selection(ed: &Editor, stack: StackRef, what: &Value) -> VimResult<Option<usize>> {
    let has_selector = dict_has(what, "nr") || dict_has(what, "id");
    if !has_selector {
        return Ok(None);
    }
    match selected_index(ed, stack, what) {
        Some(index) => Ok(Some(index)),
        None => Err(VimError::new(
            "E684",
            format!(
                "List index out of range: {}",
                dict_get(what, "nr")
                    .or_else(|| dict_get(what, "id"))
                    .map(|value| value.to_string_value())
                    .unwrap_or_default()
            ),
        )),
    }
}

fn context_strings(value: &Value) -> Vec<String> {
    match value {
        Value::List(items) => items.borrow().iter().map(Value::to_string_value).collect(),
        other => vec![other.to_string_value()],
    }
}

fn set_context(ed: &mut Editor, stack: StackRef, value: &Value) {
    let context = context_strings(value);
    let list = match stack {
        StackRef::Qf => ed.qf_state_mut().current_list_mut(),
        StackRef::Loc(win) => ed.qf_state_mut().loclist_current_mut(win),
    };
    if let Some(list) = list {
        list.context = context;
    }
}

fn set_list_index(ed: &mut Editor, stack: StackRef, value: &Value) -> VimResult<()> {
    let len = stack.current(ed).map_or(0, |list| list.entries.len());
    if len == 0 {
        return Err(VimError::new("E684", "List index out of range: 0"));
    }
    let number = match value {
        Value::String(text) if text == "$" => len as i64,
        other => other.to_number(),
    };
    if number < 1 {
        return Err(VimError::new(
            "E684",
            format!("List index out of range: {number}"),
        ));
    }
    let index = (number as usize).min(len);
    stack.set_entry_index(ed, index);
    Ok(())
}

/// getqflist() / getloclist() with an optional "what" dictionary.
pub fn builtin_getqflist(
    ed: &mut Editor,
    args: &[Value],
    which: i32,
    win: Option<WindowId>,
) -> VimResult<Value> {
    let (stack, rest) = if which == 0 {
        (StackRef::Qf, args)
    } else {
        loclist_arguments(ed, win, args)
    };
    let Some(what) = rest.first().filter(|value| matches!(value, Value::Dict(_))) else {
        let entries = match stack.current(ed) {
            Some(list) => list.entries.as_slice(),
            None => &[],
        };
        return Ok(entries_value(ed, entries));
    };
    // The "lines" property parses a List of lines without touching a list.
    if let Some(lines) = dict_get(what, "lines") {
        let efm = dict_get(what, "efm").map(|value| value.to_string_value());
        let lines = value_lines(&lines)?;
        let entries = parse_lines(ed, &lines, efm.as_deref())?;
        let mut dict = VimDict::new();
        dict.insert("items", entries_value(ed, &entries));
        return Ok(Value::dict(dict));
    }
    let all = dict_has(what, "all");
    let index = selected_index(ed, stack, what);
    let list = index.and_then(|index| stack.at(ed, index));
    let mut dict = VimDict::new();
    if all || dict_has(what, "title") {
        dict.insert(
            "title",
            Value::String(list.map_or(String::new(), |list| list.title.clone())),
        );
    }
    if all || dict_has(what, "nr") {
        let number = index.map_or(0, |index| index as i64 + 1);
        dict.insert("nr", Value::Number(number));
    }
    if all || dict_has(what, "winid") {
        dict.insert(
            "winid",
            Value::Number(ed.qf_window_id().map_or(0, i64::from)),
        );
    }
    if all || dict_has(what, "context") {
        let context = list.map_or(&[][..], |list| list.context.as_slice());
        let items: Vec<Value> = context
            .iter()
            .map(|text| Value::String(text.clone()))
            .collect();
        dict.insert("context", Value::list(items));
    }
    if all || dict_has(what, "id") {
        let number = index.map_or(0, |index| index as i64 + 1);
        dict.insert("id", Value::Number(number));
    }
    if all || dict_has(what, "items") {
        let entry_index = dict_get(what, "idx")
            .map(|value| value.to_number())
            .unwrap_or(0);
        let entries: &[QfEntry] = match list {
            Some(list) if entry_index > 0 => {
                let position = entry_index as usize;
                if position >= 1 && position <= list.entries.len() {
                    &list.entries[position - 1..position]
                } else {
                    &[]
                }
            }
            Some(list) => list.entries.as_slice(),
            None => &[],
        };
        dict.insert("items", entries_value(ed, entries));
    }
    if all || dict_has(what, "idx") {
        let requested = dict_get(what, "idx")
            .map(|value| value.to_number())
            .unwrap_or(0);
        let index = if requested > 0 {
            requested
        } else {
            list.map_or(0, |list| to_i64(list.idx))
        };
        dict.insert("idx", Value::Number(index));
    }
    if all || dict_has(what, "size") {
        let size = list.map_or(0, |list| to_i64(list.entries.len()));
        dict.insert("size", Value::Number(size));
    }
    if all || dict_has(what, "changedtick") {
        dict.insert("changedtick", Value::Number(0));
    }
    if all || dict_has(what, "qfbufnr") {
        dict.insert("qfbufnr", Value::Number(i64::from(ed.qf_buffer_number())));
    }
    if all || dict_has(what, "filewinid") {
        if let StackRef::Loc(source) = stack {
            dict.insert("filewinid", Value::Number(i64::from(source)));
        }
    }
    if all || dict_has(what, "quickfixtextfunc") {
        dict.insert("quickfixtextfunc", Value::String(String::new()));
    }
    Ok(Value::dict(dict))
}

/// setqflist() / setloclist().
///
/// The supported subset is documented in the module header: actions ' ', 'a',
/// 'r' and 'f', with the what properties title, items, lines, efm, context,
/// idx, nr and id.
pub fn builtin_setqflist(
    ed: &mut Editor,
    args: &[Value],
    which: i32,
    win: Option<WindowId>,
) -> VimResult<Value> {
    let (stack, rest) = if which == 0 {
        (StackRef::Qf, args)
    } else {
        loclist_arguments(ed, win, args)
    };
    let Some(list_value) = rest.first() else {
        return Err(VimError::new("E471", "Argument required"));
    };
    if !matches!(list_value, Value::List(_)) {
        return Err(VimError::new("E714", "List required"));
    }
    let action_text = rest.get(1).map(Value::to_string_value).unwrap_or_default();
    let action = match action_text.as_str() {
        "" | " " => ' ',
        "a" => 'a',
        "r" => 'r',
        "f" => 'f',
        other => return Err(VimError::new("E475", format!("Invalid action: {other}"))),
    };
    let what = rest.get(2).filter(|value| matches!(value, Value::Dict(_)));
    if let Some(what) = what {
        if action != 'f' {
            let has_payload = dict_has(what, "items") || dict_has(what, "lines");
            let non_empty = match list_value {
                Value::List(items) => !items.borrow().is_empty(),
                _ => false,
            };
            if has_payload && non_empty {
                return Err(VimError::new(
                    "E475",
                    "cannot have both a list and a \"what\" argument",
                ));
            }
        }
    }
    if action == 'f' {
        stack.clear(ed);
        return Ok(Value::Number(0));
    }
    let mut entries = entries_from_list(ed, list_value)?;
    let title = what
        .and_then(|what| dict_get(what, "title"))
        .map(|value| value.to_string_value());
    if let Some(what) = what {
        if let Some(items) = dict_get(what, "items") {
            entries = entries_from_list(ed, &items)?;
        }
        if let Some(lines) = dict_get(what, "lines") {
            let lines = value_lines(&lines)?;
            let efm = dict_get(what, "efm").map(|value| value.to_string_value());
            entries = parse_lines(ed, &lines, efm.as_deref())?;
        }
    }
    let selection = match what {
        Some(what) => required_selection(ed, stack, what)?,
        None => None,
    };
    if action == ' ' || stack.stack_len(ed) == 0 {
        stack.fill_new(ed, title.as_deref().unwrap_or(""), entries);
    } else {
        if let Some(index) = selection {
            if !stack.select(ed, index) {
                return Err(VimError::new(
                    "E684",
                    format!("List index out of range: {}", index + 1),
                ));
            }
        }
        if action == 'a' {
            stack.append(ed, title.as_deref().unwrap_or(""), entries);
        } else {
            stack.replace(ed, title.as_deref(), entries);
        }
    }
    if let Some(what) = what {
        if let Some(context) = dict_get(what, "context") {
            set_context(ed, stack, &context);
        }
        if let Some(value) = dict_get(what, "idx") {
            set_list_index(ed, stack, &value)?;
        }
    }
    stack.refresh(ed);
    Ok(Value::Number(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rvim_editor::{OptValue, SetScope};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn ctx() -> (Editor, Env, ScriptState) {
        (Editor::new(), Env::new(), ScriptState::default())
    }

    fn run(
        ed: &mut Editor,
        env: &mut Env,
        state: &mut ScriptState,
        name: &str,
        args: &str,
    ) -> VimResult<ExOutcome> {
        let parsed = ParsedEx::simple(name, args);
        cmd_quickfix(ed, env, state, &parsed, name)
    }

    fn run_range(
        ed: &mut Editor,
        env: &mut Env,
        state: &mut ScriptState,
        name: &str,
        args: &str,
        range: Range,
        addr_count: usize,
    ) -> VimResult<ExOutcome> {
        let mut parsed = ParsedEx::simple(name, args);
        parsed.range = range;
        parsed.addr_count = addr_count;
        cmd_quickfix(ed, env, state, &parsed, name)
    }

    fn dict(pairs: &[(&str, Value)]) -> Value {
        let mut map = VimDict::new();
        for (key, value) in pairs {
            map.insert(*key, value.clone());
        }
        Value::dict(map)
    }

    fn item_dict(filename: &str, lnum: i64, col: i64, text: &str) -> Value {
        dict(&[
            ("filename", Value::String(filename.to_string())),
            ("lnum", Value::Number(lnum)),
            ("col", Value::Number(col)),
            ("text", Value::String(text.to_string())),
            ("type", Value::String("E".to_string())),
            ("valid", Value::Number(1)),
        ])
    }

    fn list_items(value: &Value) -> Vec<Value> {
        match value {
            Value::List(items) => items.borrow().clone(),
            _ => Vec::new(),
        }
    }

    fn temp_error_file(contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!("rvim_qf_{}_{}.txt", std::process::id(), id));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn qf_size(ed: &mut Editor) -> i64 {
        let what = dict(&[("size", Value::Number(1))]);
        let value = builtin_getqflist(ed, &[what], 0, None).unwrap();
        dict_get(&value, "size").map_or(0, |value| value.to_number())
    }

    #[test]
    fn error_item_conversion_carries_the_important_fields() {
        let item = ErrorItem {
            filename: Some("a.c".to_string()),
            lnum: 3,
            end_lnum: 4,
            col: 2,
            text: "broken".to_string(),
            entry_type: 'W',
            valid: true,
            ..ErrorItem::default()
        };
        let entry = QfEntry::from(item);
        assert_eq!(entry.filename, "a.c");
        assert_eq!(entry.lnum, 3);
        assert_eq!(entry.end_lnum, 4);
        assert_eq!(entry.col, 2);
        assert_eq!(entry.text, "broken");
        assert_eq!(entry.entry_type, 'W');
        assert!(entry.valid);

        let negative = QfEntry::from(ErrorItem {
            lnum: -4,
            col: -1,
            ..ErrorItem::default()
        });
        assert_eq!(negative.lnum, 0);
        assert_eq!(negative.col, 0);
    }

    #[test]
    fn cgetexpr_parses_a_viml_list() {
        let (mut ed, mut env, mut state) = ctx();
        run(
            &mut ed,
            &mut env,
            &mut state,
            "cgetexpr",
            "['a.c:12:3:oops', 'b.c:2:1:bad']",
        )
        .unwrap();
        let list = ed.qf_state().current_list().unwrap();
        assert_eq!(list.title, ":cgetexpr");
        assert_eq!(list.entries.len(), 2);
        assert_eq!(list.entries[0].filename, "a.c");
        assert_eq!(list.entries[0].lnum, 12);
        assert_eq!(list.entries[0].col, 3);
        assert_eq!(list.entries[0].text, "oops");
        assert!(list.entries[0].valid);
        assert_eq!(ed.qf_current(), Some(1));
    }

    #[test]
    fn cexpr_parses_a_string_and_keeps_unmatched_lines() {
        let (mut ed, mut env, mut state) = ctx();
        run(
            &mut ed,
            &mut env,
            &mut state,
            "cgetexpr",
            "\"a.c:1:1:ok\\nnot an error\"",
        )
        .unwrap();
        let list = ed.qf_state().current_list().unwrap();
        assert_eq!(list.entries.len(), 2);
        assert!(list.entries[0].valid);
        assert!(!list.entries[1].valid);
        assert_eq!(list.entries[1].text, "not an error");
        assert_eq!(list.entries[1].lnum, 0);
    }

    #[test]
    fn cexpr_jumps_to_the_first_error() {
        let mut ed = Editor::new();
        let mut env = Env::new();
        let mut state = ScriptState::default();
        let target = ed.new_buffer();
        {
            let buffer = ed.buffer_mut(target).unwrap();
            buffer.name = Some(PathBuf::from("hit.txt"));
            let lines = vec![
                "one".to_string(),
                "two".to_string(),
                "three".to_string(),
                "four".to_string(),
            ];
            buffer.replace_line_range(0, 1, lines).unwrap();
        }
        ed.switch_buffer(target).unwrap();
        run(&mut ed, &mut env, &mut state, "cexpr", "'hit.txt:3:2:oops'").unwrap();
        assert_eq!(ed.current_bufnr(), target);
        assert_eq!(ed.cursor(), rvim_core::Pos::new(2, 1));
        assert_eq!(ed.qf_current(), Some(1));
    }

    #[test]
    fn caddexpr_appends_and_cexpr_creates_new_lists() {
        let (mut ed, mut env, mut state) = ctx();
        run(&mut ed, &mut env, &mut state, "cgetexpr", "['a.c:1:1:one']").unwrap();
        run(&mut ed, &mut env, &mut state, "caddexpr", "'b.c:2:2:two'").unwrap();
        assert_eq!(ed.qf_state().lists().len(), 1);
        assert_eq!(ed.qf_state().current_list().unwrap().entries.len(), 2);

        run(
            &mut ed,
            &mut env,
            &mut state,
            "cgetexpr",
            "['c.c:3:3:three']",
        )
        .unwrap();
        assert_eq!(ed.qf_state().lists().len(), 2);
        assert_eq!(
            ed.qf_state().current_list().unwrap().entries[0].filename,
            "c.c"
        );
    }

    #[test]
    fn cfile_and_cgetfile_read_an_error_file() {
        let path = temp_error_file("a.c:3:2:oops\nb.c:4:1:bad\n");
        let (mut ed, mut env, mut state) = ctx();
        let argument = path.to_str().unwrap().to_string();
        let result = run(&mut ed, &mut env, &mut state, "cgetfile", &argument);
        let _ = std::fs::remove_file(&path);
        result.unwrap();
        let list = ed.qf_state().current_list().unwrap();
        assert_eq!(list.title, ":cgetfile");
        assert_eq!(list.entries.len(), 2);
        assert_eq!(list.entries[0].filename, "a.c");
        assert_eq!(list.entries[1].lnum, 4);
    }

    #[test]
    fn cfile_without_a_name_reports_e484() {
        let (mut ed, mut env, mut state) = ctx();
        let mut path = std::env::temp_dir();
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!("rvim_qf_missing_{}_{}.err", std::process::id(), id));
        let argument = path.to_str().unwrap().to_string();
        let error = run(&mut ed, &mut env, &mut state, "cgetfile", &argument).unwrap_err();
        assert_eq!(error.code, "E484");
    }

    #[test]
    fn navigation_skips_invalid_entries_and_reports_e553() {
        let (mut ed, mut env, mut state) = ctx();
        ed.set_option("wrapscan", OptValue::Bool(false), SetScope::Global)
            .unwrap();
        run(
            &mut ed,
            &mut env,
            &mut state,
            "cgetexpr",
            "['a.c:1:1:ok', 'unmatched', 'b.c:2:1:bad']",
        )
        .unwrap();
        assert_eq!(ed.qf_current(), Some(1));
        run(&mut ed, &mut env, &mut state, "cnext", "").unwrap();
        assert_eq!(ed.qf_current(), Some(3));
        let error = run(&mut ed, &mut env, &mut state, "cnext", "").unwrap_err();
        assert_eq!(error.code, "E553");
        run(&mut ed, &mut env, &mut state, "cprevious", "").unwrap();
        assert_eq!(ed.qf_current(), Some(1));
    }

    #[test]
    fn cnext_wraps_with_wrapscan() {
        let (mut ed, mut env, mut state) = ctx();
        run(
            &mut ed,
            &mut env,
            &mut state,
            "cgetexpr",
            "['a.c:1:1:one', 'b.c:2:2:two']",
        )
        .unwrap();
        assert_eq!(ed.qf_current(), Some(1));
        run(&mut ed, &mut env, &mut state, "cnext", "").unwrap();
        assert_eq!(ed.qf_current(), Some(2));
        run(&mut ed, &mut env, &mut state, "cnext", "").unwrap();
        assert_eq!(ed.qf_current(), Some(1));
    }

    #[test]
    fn cc_accepts_an_ex_address_and_a_count() {
        let (mut ed, mut env, mut state) = ctx();
        run(
            &mut ed,
            &mut env,
            &mut state,
            "cgetexpr",
            "['a.c:1:1:one', 'b.c:2:2:two', 'c.c:3:3:three']",
        )
        .unwrap();
        run_range(
            &mut ed,
            &mut env,
            &mut state,
            "cc",
            "",
            Range::Lines(2, 3),
            1,
        )
        .unwrap();
        assert_eq!(ed.qf_current(), Some(3));
        run(&mut ed, &mut env, &mut state, "cc", "1").unwrap();
        assert_eq!(ed.qf_current(), Some(1));
        let error = run(&mut ed, &mut env, &mut state, "cc", "bogus").unwrap_err();
        assert_eq!(error.code, "E474");
    }

    #[test]
    fn colder_and_cnewer_move_through_the_stack() {
        let (mut ed, mut env, mut state) = ctx();
        run(&mut ed, &mut env, &mut state, "cgetexpr", "['a.c:1:1:one']").unwrap();
        run(&mut ed, &mut env, &mut state, "cgetexpr", "['b.c:1:1:two']").unwrap();
        assert_eq!(ed.qf_state().lists().len(), 2);
        run(&mut ed, &mut env, &mut state, "colder", "").unwrap();
        assert_eq!(
            ed.qf_state().current_list().unwrap().entries[0].filename,
            "a.c"
        );
        let error = run(&mut ed, &mut env, &mut state, "colder", "").unwrap_err();
        assert_eq!(error.code, "E380");
        run(&mut ed, &mut env, &mut state, "cnewer", "").unwrap();
        assert_eq!(
            ed.qf_state().current_list().unwrap().entries[0].filename,
            "b.c"
        );
        let error = run(&mut ed, &mut env, &mut state, "cnewer", "").unwrap_err();
        assert_eq!(error.code, "E381");
    }

    #[test]
    fn empty_lists_report_e42() {
        let (mut ed, mut env, mut state) = ctx();
        for command in ["cnext", "cc", "cfirst", "clist", "copen"] {
            let error = run(&mut ed, &mut env, &mut state, command, "").unwrap_err();
            assert_eq!(error.code, "E42", "{command} needs a list");
        }
    }

    #[test]
    fn unknown_arguments_report_e474() {
        let (mut ed, mut env, mut state) = ctx();
        run(&mut ed, &mut env, &mut state, "cgetexpr", "['a.c:1:1:one']").unwrap();
        let error = run(&mut ed, &mut env, &mut state, "cnext", "bogus").unwrap_err();
        assert_eq!(error.code, "E474");
    }

    #[test]
    fn copen_and_cclose_manage_the_window() {
        let (mut ed, mut env, mut state) = ctx();
        run(&mut ed, &mut env, &mut state, "cgetexpr", "['a.c:1:1:one']").unwrap();
        run(&mut ed, &mut env, &mut state, "copen", "").unwrap();
        assert_eq!(ed.window_count(), 2);
        assert!(ed.in_quickfix_window());
        assert!(ed.qf_window_id().is_some());
        run(&mut ed, &mut env, &mut state, "cclose", "").unwrap();
        assert_eq!(ed.window_count(), 1);
        assert!(!ed.in_quickfix_window());
    }

    #[test]
    fn cwindow_closes_when_there_is_nothing_to_show() {
        let (mut ed, mut env, mut state) = ctx();
        run(&mut ed, &mut env, &mut state, "cwindow", "").unwrap();
        assert_eq!(ed.window_count(), 1);
    }

    #[test]
    fn clist_prints_valid_entries_and_bang_prints_all() {
        let (mut ed, mut env, mut state) = ctx();
        run(
            &mut ed,
            &mut env,
            &mut state,
            "cgetexpr",
            "['a.c:1:1:ok', 'unmatched', 'b.c:2:2:bad']",
        )
        .unwrap();
        run(&mut ed, &mut env, &mut state, "clist", "").unwrap();
        let history = ed.messages.history().join("\n");
        assert!(history.contains("a.c:1 col 1: ok"), "{history}");
        assert!(history.contains("b.c:2 col 2: bad"), "{history}");
        assert!(!history.contains("unmatched"), "{history}");

        ed.messages.clear();
        let mut parsed = ParsedEx::simple("clist", "");
        parsed.bang = true;
        cmd_quickfix(&mut ed, &mut env, &mut state, &parsed, "clist").unwrap();
        let history = ed.messages.history().join("\n");
        assert!(history.contains("unmatched"), "{history}");
    }

    #[test]
    fn clist_accepts_a_range() {
        let (mut ed, mut env, mut state) = ctx();
        run(
            &mut ed,
            &mut env,
            &mut state,
            "cgetexpr",
            "['a.c:1:1:one', 'b.c:2:2:two', 'c.c:3:3:three']",
        )
        .unwrap();
        run(&mut ed, &mut env, &mut state, "clist", "2").unwrap();
        let history = ed.messages.history().join("\n");
        assert!(!history.contains("one"), "{history}");
        assert!(history.contains("two"), "{history}");
        assert!(!history.contains("three"), "{history}");
    }

    #[test]
    fn cbelow_and_cabove_follow_the_current_buffer() {
        let (mut ed, mut env, mut state) = ctx();
        let lines: Vec<String> = (1..=8).map(|line| format!("line {line}")).collect();
        ed.replace_lines(0, 1, lines).unwrap();
        let bufnr = ed.current_bufnr();
        let make = |lnum: usize, text: &str| QfEntry {
            filename: String::new(),
            bufnr: Some(bufnr),
            lnum,
            end_lnum: 0,
            col: 0,
            text: text.to_string(),
            entry_type: 'E',
            valid: true,
        };
        ed.qf_set("lines", vec![make(2, "two"), make(6, "six")]);
        ed.set_cursor(rvim_core::Pos::new(3, 0));
        run(&mut ed, &mut env, &mut state, "cabove", "").unwrap();
        assert_eq!(ed.qf_current(), Some(1));
        assert_eq!(ed.cursor().line, 1);
        run(&mut ed, &mut env, &mut state, "cbelow", "").unwrap();
        assert_eq!(ed.qf_current(), Some(2));
        assert_eq!(ed.cursor().line, 5);
    }

    #[test]
    fn location_list_commands_work() {
        let (mut ed, mut env, mut state) = ctx();
        let source = ed.current_win_id();
        run(&mut ed, &mut env, &mut state, "lgetexpr", "['a.c:1:1:one']").unwrap();
        assert_eq!(ed.loclist(source).unwrap().entries.len(), 1);
        run(&mut ed, &mut env, &mut state, "lopen", "").unwrap();
        assert_eq!(ed.window_count(), 2);
        assert_eq!(ed.qf_location_source(), Some(source));
        run(&mut ed, &mut env, &mut state, "lclose", "").unwrap();
        assert_eq!(ed.window_count(), 1);
        assert!(ed.loclist(source).is_some());
    }

    #[test]
    fn location_list_stack_supports_lolder_and_lnewer() {
        let (mut ed, mut env, mut state) = ctx();
        let source = ed.current_win_id();
        run(&mut ed, &mut env, &mut state, "lgetexpr", "['a.c:1:1:one']").unwrap();
        run(&mut ed, &mut env, &mut state, "lgetexpr", "['b.c:2:2:two']").unwrap();
        assert_eq!(ed.qf_state().loclist_count(source), 2);
        run(&mut ed, &mut env, &mut state, "lolder", "").unwrap();
        assert_eq!(ed.loclist(source).unwrap().entries[0].filename, "a.c");
        run(&mut ed, &mut env, &mut state, "lnewer", "").unwrap();
        assert_eq!(ed.loclist(source).unwrap().entries[0].filename, "b.c");
    }

    #[test]
    fn getqflist_returns_a_list_of_item_dicts() {
        let (mut ed, _, _) = ctx();
        let list = Value::list(vec![item_dict("a.c", 3, 2, "broken")]);
        builtin_setqflist(&mut ed, &[list], 0, None).unwrap();
        let value = builtin_getqflist(&mut ed, &[], 0, None).unwrap();
        let items = list_items(&value);
        assert_eq!(items.len(), 1);
        assert_eq!(dict_get(&items[0], "lnum"), Some(Value::Number(3)));
        assert_eq!(dict_get(&items[0], "col"), Some(Value::Number(2)));
        assert_eq!(
            dict_get(&items[0], "text"),
            Some(Value::String("broken".to_string()))
        );
        assert_eq!(
            dict_get(&items[0], "type"),
            Some(Value::String("E".to_string()))
        );
        assert_eq!(dict_get(&items[0], "valid"), Some(Value::Number(1)));
        assert_eq!(dict_get(&items[0], "bufnr"), Some(Value::Number(0)));
    }

    #[test]
    fn getqflist_what_dictionary_forms() {
        let (mut ed, _, _) = ctx();
        let item = item_dict("a.c", 3, 2, "broken");
        let what = dict(&[("title", Value::String("my list".to_string()))]);
        builtin_setqflist(
            &mut ed,
            &[
                Value::list(vec![item]),
                Value::String("a".to_string()),
                what,
            ],
            0,
            None,
        )
        .unwrap();

        let title =
            builtin_getqflist(&mut ed, &[dict(&[("title", Value::Number(1))])], 0, None).unwrap();
        assert_eq!(
            dict_get(&title, "title"),
            Some(Value::String("my list".to_string()))
        );

        let last = builtin_getqflist(
            &mut ed,
            &[dict(&[("nr", Value::String("$".to_string()))])],
            0,
            None,
        )
        .unwrap();
        assert_eq!(dict_get(&last, "nr"), Some(Value::Number(1)));

        let items =
            builtin_getqflist(&mut ed, &[dict(&[("items", Value::Number(1))])], 0, None).unwrap();
        assert_eq!(list_items(&dict_get(&items, "items").unwrap()).len(), 1);

        let idx =
            builtin_getqflist(&mut ed, &[dict(&[("idx", Value::Number(0))])], 0, None).unwrap();
        assert_eq!(dict_get(&idx, "idx"), Some(Value::Number(1)));

        let id = builtin_getqflist(&mut ed, &[dict(&[("id", Value::Number(0))])], 0, None).unwrap();
        assert_eq!(dict_get(&id, "id"), Some(Value::Number(1)));

        let size =
            builtin_getqflist(&mut ed, &[dict(&[("size", Value::Number(1))])], 0, None).unwrap();
        assert_eq!(dict_get(&size, "size"), Some(Value::Number(1)));
    }

    #[test]
    fn setqflist_actions_append_replace_and_create() {
        let (mut ed, _, _) = ctx();
        let first = Value::list(vec![item_dict("a.c", 1, 1, "one")]);
        builtin_setqflist(&mut ed, &[first], 0, None).unwrap();
        assert_eq!(qf_size(&mut ed), 1);
        assert_eq!(ed.qf_state().lists().len(), 1);

        let appended = dict(&[("items", Value::list(vec![item_dict("b.c", 2, 2, "two")]))]);
        builtin_setqflist(
            &mut ed,
            &[
                Value::list(Vec::new()),
                Value::String("a".to_string()),
                appended,
            ],
            0,
            None,
        )
        .unwrap();
        assert_eq!(qf_size(&mut ed), 2);
        assert_eq!(ed.qf_state().lists().len(), 1);

        let replaced = dict(&[("items", Value::list(vec![item_dict("c.c", 3, 3, "three")]))]);
        builtin_setqflist(
            &mut ed,
            &[
                Value::list(Vec::new()),
                Value::String("r".to_string()),
                replaced,
            ],
            0,
            None,
        )
        .unwrap();
        assert_eq!(qf_size(&mut ed), 1);
        assert_eq!(ed.qf_state().lists().len(), 1);
        assert_eq!(
            ed.qf_state().current_list().unwrap().entries[0].filename,
            "c.c"
        );

        let second = Value::list(vec![item_dict("d.c", 4, 4, "four")]);
        builtin_setqflist(&mut ed, &[second], 0, None).unwrap();
        assert_eq!(ed.qf_state().lists().len(), 2);
    }

    #[test]
    fn setqflist_sets_idx_and_title() {
        let (mut ed, _, _) = ctx();
        let list = Value::list(vec![
            item_dict("a.c", 1, 1, "one"),
            item_dict("b.c", 2, 2, "two"),
        ]);
        let what = dict(&[
            ("title", Value::String("t".to_string())),
            ("idx", Value::Number(2)),
        ]);
        builtin_setqflist(
            &mut ed,
            &[list, Value::String(" ".to_string()), what],
            0,
            None,
        )
        .unwrap();
        assert_eq!(ed.qf_current(), Some(2));
        assert_eq!(ed.qf_state().current_list().unwrap().title, "t");
    }

    #[test]
    fn setqflist_rejects_a_non_list() {
        let (mut ed, _, _) = ctx();
        let error = builtin_setqflist(&mut ed, &[Value::Number(1)], 0, None).unwrap_err();
        assert_eq!(error.code, "E714");
    }

    #[test]
    fn getqflist_lines_parses_without_creating_a_list() {
        let (mut ed, _, _) = ctx();
        let what = dict(&[
            (
                "lines",
                Value::list(vec![Value::String("a.c:2:1:oops".to_string())]),
            ),
            ("efm", Value::String("%f:%l:%c:%m".to_string())),
        ]);
        let value = builtin_getqflist(&mut ed, &[what], 0, None).unwrap();
        let items = list_items(&dict_get(&value, "items").unwrap());
        assert_eq!(items.len(), 1);
        assert_eq!(dict_get(&items[0], "lnum"), Some(Value::Number(2)));
        assert!(ed.qf_state().lists().is_empty());
    }

    #[test]
    fn getloclist_and_setloclist_use_the_window_number() {
        let (mut ed, _, _) = ctx();
        let list = Value::list(vec![item_dict("a.c", 1, 1, "one")]);
        builtin_setqflist(&mut ed, &[Value::Number(1), list], 1, None).unwrap();
        let source = ed.current_win_id();
        assert_eq!(ed.loclist(source).unwrap().entries.len(), 1);
        let value = builtin_getqflist(&mut ed, &[Value::Number(1)], 1, None).unwrap();
        assert_eq!(list_items(&value).len(), 1);

        let title = builtin_getqflist(
            &mut ed,
            &[Value::Number(1), dict(&[("title", Value::Number(1))])],
            1,
            None,
        )
        .unwrap();
        assert_eq!(
            dict_get(&title, "title"),
            Some(Value::String(String::new()))
        );
    }

    #[test]
    fn setqflist_free_action_clears_the_stack() {
        let (mut ed, _, _) = ctx();
        let list = Value::list(vec![item_dict("a.c", 1, 1, "one")]);
        builtin_setqflist(&mut ed, &[list], 0, None).unwrap();
        builtin_setqflist(
            &mut ed,
            &[Value::list(Vec::new()), Value::String("f".to_string())],
            0,
            None,
        )
        .unwrap();
        assert!(ed.qf_state().lists().is_empty());
    }
}
