//! Host builtins for windows and tab pages, tags, mappings, screen geometry,
//! undo/state introspection and the remaining window/tab/misc builtins of the
//! M3 batch.
//!
//! # Model and honest limitations
//!
//! RustVim has a real multi-window/multi-tabpage model: `Editor::windows` owns
//! the split layout tree, window IDs start at 1000 (vim's numbering), and every
//! window carries its own buffer, cursor, viewport, screen rectangle and
//! window-local options.  This module maps vim's window/tab builtins onto that
//! model:
//!
//! * `0` means "current window" wherever vim accepts a window number and a
//!   value of 1000 or more is a window ID, exactly like
//!   `find_win_by_nr_or_id()`.  Window numbers are 1-based and per tab page;
//!   tab page numbers are 1-based (and `0` means "current tab page").
//! * `winheight()`, `winwidth()`, `win_screenpos()` and `getwininfo()` report
//!   the rectangles of the last `Editor::layout_windows()` pass.  Before that
//!   pass (headless `-es` batch mode) no screen geometry exists: the size
//!   entries are `-1`, while `win_screenpos()` reports `[rect.row + 1,
//!   rect.col + 1]` of the default (all zero) rectangle, i.e. `[1, 1]`.
//! * `wincol()`/`winline()` are 1-based positions inside the window, computed
//!   from the window's own cursor, `top_line` and `left`: tabs expand to
//!   'tabstop' and control characters take two cells (std has no `wcwidth`, so
//!   double-width characters count as one).  'wrap' is not modelled, so
//!   `winline()` counts buffer lines rather than wrapped screen rows.
//! * `getwininfo()` reports `height` as the buffer area (`w_height`, the
//!   frame's rows without its status line) and `winrow`/`wincol` as the 1-based
//!   top-left corner; `textoff` only counts the 'number'/'relativenumber'
//!   gutter (the digits of the buffer's line count plus one, the same width
//!   `rvim-term` draws).  The sign and fold columns are not modelled, so
//!   'signcolumn' adds nothing.  `loclist`, `winbar` and `terminal` are always
//!   `0`: there are no location lists, toolbars or terminal windows yet.
//!   `quickfix` is `1` for the quickfix window.
//! * `winlayout()` returns the real nested layout and `winrestcmd()` restores
//!   the requested `height`/`width` of every window of the current tab page.
//!   `win_move_separator()`/`win_move_statusline()` resize a window that really
//!   has a neighbour in that direction: `0` on success and `-1` when there is
//!   no separator/status line to move (a window on the tab page edge, or a
//!   window without a layout pass yet).
//! * `win_splitmove()` exchanges the two windows' contents (buffer, cursor,
//!   viewport, window-local options and the quickfix flag) instead of moving
//!   the frame node; `:splitmove`'s `{options}` ("vertical", "rightbelow",
//!   "bottomright") are accepted and ignored.  Unknown windows still raise
//!   `E957`.
//! * `win_execute()` runs the commands in the target window and restores the
//!   previous one; a failing command aborts the remaining list and is
//!   reported.  Its return value is the empty string because RustVim has no
//!   command-output capture (`:redir`) layer, so the output vim returns does
//!   not exist.
//! * `w:`/`t:` dictionaries are per window/tab page: rvim-viml keeps one live
//!   dictionary per scope in `ScriptState`, so this module mirrors the current
//!   window's/tab page's dictionary there and parks all others in
//!   [`WinState`]; it swaps them whenever a builtin runs or `win_gotoid()`/
//!   `win_execute()` switch.  A window or tab page created behind the
//!   builtins' back (`:split`, `:tabnew`) therefore starts with an empty
//!   dictionary once the next builtin runs.
//! * the jump list and the tag stack still live in [`WinState`] as one list
//!   for the whole editor instead of one per window; `getjumplist()` and
//!   `gettagstack()` validate their window argument but always report that
//!   shared list.
//! * `tabpagenr('#')` uses the tab page that was current on the previous
//!   builtin call (vim tracks every tab switch); it is only approximate when
//!   several switches happen between two calls.
//! * `winnr()` is not implemented in this module: `script.rs`'s
//!   `HOST_BUILTINS` answers it with a hardcoded `1`, so it keeps claiming a
//!   single window.  Fixing that needs a one-line change in `script.rs`
//!   (reported, not done here because only this file may be edited).
//! * `gettabinfo()` nests the same window dictionaries in its `windows` entry
//!   (vim 9 behaviour; the pinned reference vim returns bare window IDs there).
//!   `getwininfo()`'s `height` is the buffer area (`w_height` in vim's
//!   `get_win_info()`), so it agrees with `winheight()`.
//! * `getwinpos()`/`getwinposx()`/`getwinposy()` keep vim's no-terminal
//!   fallback of `-1`: nothing at this layer can query the terminal.
//! * `interrupt()` returns `0` and does **not** interrupt `:while`/`:for`:
//!   honouring the flag needs the M4 input layer to poll it inside the loop
//!   executor in `script.rs`.  It is deliberately not claimed to work.
//! * `state()` returns a `mode()`-style identifier derived from
//!   [`Mode`] (`"n"` in Normal mode) instead of Vim's transient state flags
//!   (`m`/`o`/`x`/`c`/`s`/...): those describe the input/command layers, which
//!   do not exist yet.  A `{what}` argument filters the returned characters.
//! * `getchangelist()` returns an empty change list: the linear undo history
//!   records no change-list positions.  `undotree()` reports the linear
//!   `UndoHistory` sequence numbers but no `entries` list, and `seq_cur` is
//!   only exact when no redo is available (the public undo API exposes no head
//!   index).
//! * `lispindent()` computes leading whitespace columns like `indent()`; the
//!   real Lisp indenter is not implemented.
//! * `virtcol()`/`virtcol2col()` use a simple display width: tabs expand with
//!   'tabstop', control characters occupy two cells and every other character
//!   one cell (std has no `wcwidth`, so double-width characters are counted as
//!   one).
//! * `winsaveview()`/`winrestview()` save and restore the cursor, `curswant`,
//!   `topline` and `leftcol` of the current window, per window; the columns are
//!   character based because RustVim's cursor columns are.  `skipcol` is
//!   reported but has no display effect ('wrap' is not modelled).
//! * tags: `tagfiles()`/`taglist()` read real `tags` files.  The `'tags'`
//!   option is not part of the M1 option table, so Vim's default `./tags,tags`
//!   is used when it is unset (a file reachable through several entries is
//!   reported once); `taglist()` matches with the M3 regex MVP (Vim's "magic"
//!   subset) and ignores the optional `{filename}` priority argument.
//! * mappings: `maparg()`/`mapcheck()`/`maplist()`/`mapset()` are backed by
//!   `ed.maps` (and `ed.abbrevs` for `{abbr}`).  The dictionary's `lhsraw` is
//!   the canonical notation because mappings are stored as parsed keys; the
//!   `sid`/`scriptversion`/`lnum` fields are reported as `0`; terminal (`'t'`)
//!   mappings are not modelled, so `mapset()` rejects a `"t"` mode with
//!   `E1276` and the lookup functions simply find nothing.

use std::collections::HashMap;
use std::path::PathBuf;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::options::{WINDOW_LOCAL, is_window_local};
use rvim_editor::window::{FIRST_WIN_ID, MIN_HEIGHT, MIN_WIDTH};
use rvim_editor::{
    Editor, Layout, MapEntry, MapMode, MapOptions, Mode, OptValue, Options, SetScope, TabpageId,
    Window, WindowId,
};
use rvim_viml::Env;
use rvim_viml::value::{Value, VimDict};

use crate::script::ScriptState;

/// Lowest number that is a window ID rather than a window number (vim's
/// `LOWEST_WIN_ID`); mirrors [`rvim_editor::window::FIRST_WIN_ID`].
const LOWEST_WIN_ID: i64 = FIRST_WIN_ID as i64;

/// A saved window view (`winsaveview()`); cursor columns are character based.
#[derive(Debug, Clone, Default)]
pub struct SavedView {
    pub lnum: i64,
    pub col: i64,
    pub curswant: i64,
    /// 1-based first displayed line (`w_topline`).
    pub topline: i64,
    pub leftcol: i64,
    pub skipcol: i64,
}

/// One jump-list entry (populated by the M4/M5 navigation layer).
#[derive(Debug, Clone, Default)]
pub struct JumpEntry {
    pub bufnr: i64,
    /// 1-based line number.
    pub lnum: i64,
    /// 0-based column number (Vim reports the raw cursor column here).
    pub col: i64,
    pub filename: Option<String>,
}

/// One tag-stack entry.  `from` is stored like Vim's internal mark:
/// `[bufnr, lnum, col-1, coladd]` with a 1-based line and a 0-based column.
#[derive(Debug, Clone, Default)]
pub struct TagStackEntry {
    pub tagname: String,
    /// 1-based match number.
    pub matchnr: i64,
    pub bufnr: i64,
    pub from: [i64; 4],
    pub user_data: Option<String>,
}

/// Builtin-level window/tab state: saved views, scoped variables, jump list,
/// tag stack.
#[derive(Debug, Default)]
pub struct WinState {
    /// Cursor/scroll state saved by `winsaveview()`, per window.
    pub saved_views: HashMap<WindowId, SavedView>,
    /// `w:` dictionaries of every window that is *not* the current one.  The
    /// current window's dictionary lives in [`ScriptState::winvars`] and is
    /// swapped in and out by [`sync_scopes`].
    pub winvars: HashMap<WindowId, HashMap<String, Value>>,
    /// `t:` dictionaries of every tab page that is *not* the current one, for
    /// the same reason ([`ScriptState::tabvars`]).
    pub tabvars: HashMap<TabpageId, HashMap<String, Value>>,
    /// Window whose dictionary is mirrored in [`ScriptState::winvars`].
    pub winvars_owner: Option<WindowId>,
    /// Tab page whose dictionary is mirrored in [`ScriptState::tabvars`].
    pub tabvars_owner: Option<TabpageId>,
    /// Current tab page index seen on the previous builtin call.
    pub last_tab: Option<usize>,
    /// Tab page index that was current before that (vim's `tabpagenr('#')`).
    pub prev_tab: Option<usize>,
    /// Jump list entries (`getjumplist()` returns it together with the index).
    pub jump_list: Vec<JumpEntry>,
    /// Current jump list index (`w:jumplistidx` equivalent, 0-based).
    pub jump_list_idx: usize,
    /// Tag stack entries (`gettagstack()`/`settagstack()`).
    pub tag_stack: Vec<TagStackEntry>,
    /// Current tag stack index (0-based; Vim's `curidx` is this plus one).
    pub tag_stack_idx: usize,
}

/// Names implemented in this module.
pub const FUNCS: &[&str] = &[
    "getchangelist",
    "getjumplist",
    "getmarklist",
    "gettabinfo",
    "gettabvar",
    "gettabwinvar",
    "gettagstack",
    "getwininfo",
    "getwinpos",
    "getwinposx",
    "getwinposy",
    "getwinvar",
    "indent",
    "interrupt",
    "lispindent",
    "maparg",
    "mapcheck",
    "maplist",
    "mapset",
    "settabvar",
    "settabwinvar",
    "settagstack",
    "setwinvar",
    "state",
    "tabpagebuflist",
    "tabpagenr",
    "tabpagewinnr",
    "tagfiles",
    "taglist",
    "undotree",
    "virtcol",
    "virtcol2col",
    "win_execute",
    "win_findbuf",
    "win_getid",
    "win_gettype",
    "win_gotoid",
    "win_id2tabwin",
    "win_id2win",
    "win_move_separator",
    "win_move_statusline",
    "win_screenpos",
    "win_splitmove",
    "winbufnr",
    "wincol",
    "winheight",
    "winlayout",
    "winline",
    "winrestcmd",
    "winrestview",
    "winsaveview",
    "winwidth",
    "wordcount",
];

/// Dispatch a call; `None` when the name is not handled here.
pub fn call(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
) -> Option<VimResult<Value>> {
    if !FUNCS.contains(&name) {
        return None;
    }
    // `w:`/`t:` are read and written through `ScriptState` by rvim-viml, so keep
    // its live dictionaries in step with the window/tab page the editor is on.
    sync_scopes(ed, state);
    Some((|| {
        check_arity(name, args)?;
        dispatch(name, args, ed, env, state)
    })())
}

/// Arity check against the generated baseline table (`E118`/`E119`).
fn check_arity(name: &str, args: &[Value]) -> VimResult<()> {
    let Some(def) = rvim_viml::FUNCTIONS.iter().find(|f| f.name == name) else {
        return Ok(());
    };
    let n = args.len() as u32;
    if n < def.min_args {
        return Err(err(
            "E119",
            format!("Not enough arguments for function: {name}"),
        ));
    }
    if def.max_args != 0 && n > def.max_args {
        return Err(err(
            "E118",
            format!("Too many arguments for function: {name}"),
        ));
    }
    Ok(())
}

fn dispatch(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
) -> VimResult<Value> {
    match name {
        // ------------------------------------------------ window info/vars
        "getwininfo" => f_getwininfo(args, ed, state),
        "getwinvar" => f_getwinvar(args, ed, state),
        "setwinvar" => f_setwinvar(args, ed, state),
        "getwinpos" => Ok(Value::list(vec![Value::Number(-1), Value::Number(-1)])),
        "getwinposx" | "getwinposy" => Ok(Value::Number(-1)),
        // -------------------------------------------------- tab info/vars
        "gettabinfo" => f_gettabinfo(args, ed, state),
        "gettabvar" => f_gettabvar(args, ed, state),
        "settabvar" => f_settabvar(args, ed, state),
        "gettabwinvar" => f_gettabwinvar(args, ed, state),
        "settabwinvar" => f_settabwinvar(args, ed, state),
        "tabpagenr" => f_tabpagenr(args, ed, state),
        "tabpagewinnr" => f_tabpagewinnr(args, ed),
        "tabpagebuflist" => f_tabpagebuflist(args, ed),
        // -------------------------------------------------- window identity
        "winbufnr" => f_winbufnr(args, ed),
        "wincol" => f_wincol(ed),
        "winline" => f_winline(ed),
        "winheight" => f_win_size(args, ed, false),
        "winwidth" => f_win_size(args, ed, true),
        "win_execute" => f_win_execute(args, ed, env, state),
        "win_findbuf" => f_win_findbuf(args, ed),
        "win_getid" => f_win_getid(args, ed),
        "win_gettype" => f_win_gettype(args, ed),
        "win_gotoid" => f_win_gotoid(args, ed, state),
        "win_id2tabwin" => f_win_id2tabwin(args, ed),
        "win_id2win" => f_win_id2win(args, ed),
        "winlayout" => f_winlayout(args, ed),
        "winrestcmd" => f_winrestcmd(ed),
        "winrestview" => f_winrestview(args, ed, state),
        "winsaveview" => f_winsaveview(ed, state),
        "win_screenpos" => f_win_screenpos(args, ed),
        "win_move_separator" => f_win_move(args, ed, true),
        "win_move_statusline" => f_win_move(args, ed, false),
        "win_splitmove" => f_win_splitmove(args, ed),
        // ------------------------------------------------------------ tags
        "tagfiles" => Ok(Value::list(
            tag_files(ed)
                .into_iter()
                .map(|(name, _)| Value::string(name))
                .collect(),
        )),
        "taglist" => f_taglist(args, ed),
        "gettagstack" => f_gettagstack(args, ed, state),
        "settagstack" => f_settagstack(args, ed, state),
        // -------------------------------------------------- jump/changelist
        "getjumplist" => f_getjumplist(args, ed, state),
        "getchangelist" => f_getchangelist(args, ed),
        "getmarklist" => f_getmarklist(args, ed),
        // ----------------------------------------------------- state/undo
        "state" => f_state(args, ed),
        "undotree" => f_undotree(ed),
        "interrupt" => Ok(Value::Number(0)),
        "wordcount" => f_wordcount(ed),
        "indent" | "lispindent" => f_indent(args, ed),
        "virtcol" => f_virtcol(args, ed),
        "virtcol2col" => f_virtcol2col(args, ed),
        // -------------------------------------------------------- mappings
        "maparg" => f_maparg(args, ed),
        "mapcheck" => f_mapcheck(args, ed),
        "maplist" => f_maplist(args, ed),
        "mapset" => f_mapset(args, ed),
        other => Err(err("E117", format!("Unknown function: {other}"))),
    }
}

// ---------------------------------------------------------------------------
// Small value helpers
// ---------------------------------------------------------------------------

fn err(code: &'static str, message: impl Into<String>) -> VimError {
    VimError::new(code, message)
}

fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).cloned().unwrap_or(Value::Null)
}

fn sarg(args: &[Value], index: usize) -> String {
    arg(args, index).to_string_value()
}

/// Optional string argument: absent means the empty string (an absent `Value`
/// is `Null`, whose `to_string_value()` is not empty).
fn opt_sarg(args: &[Value], index: usize) -> String {
    args.get(index)
        .map(Value::to_string_value)
        .unwrap_or_default()
}

fn narg(args: &[Value], index: usize) -> i64 {
    arg(args, index).to_number()
}

fn barg(args: &[Value], index: usize) -> bool {
    args.get(index).is_some_and(Value::truthy)
}

fn opt_value_to_value(value: &OptValue) -> Value {
    match value {
        OptValue::Bool(b) => Value::Number(i64::from(*b)),
        OptValue::Num(n) => Value::Number(*n),
        OptValue::Str(s) => Value::String(s.clone()),
    }
}

fn value_to_opt(value: &Value) -> OptValue {
    match value {
        Value::Number(n) => OptValue::Num(*n),
        Value::String(s) => OptValue::Str(s.clone()),
        Value::Bool(b) => OptValue::Bool(*b),
        other => OptValue::Str(other.to_string_value()),
    }
}

fn dict_clone(value: &Value) -> Option<VimDict> {
    match value {
        Value::Dict(d) => Some(d.borrow().clone()),
        _ => None,
    }
}

fn list_clone(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::List(l) => Some(l.borrow().clone()),
        _ => None,
    }
}

fn dict_number(dict: &VimDict, key: &str) -> Option<i64> {
    dict.get(key).map(Value::to_number)
}

fn dict_string(dict: &VimDict, key: &str) -> Option<String> {
    dict.get(key).map(Value::to_string_value)
}

fn vars_to_dict(vars: &HashMap<String, Value>) -> VimDict {
    let mut dict = VimDict::new();
    let mut keys: Vec<&String> = vars.keys().collect();
    keys.sort();
    for key in keys {
        if let Some(value) = vars.get(key) {
            dict.insert(key.clone(), value.clone());
        }
    }
    dict
}

// ---------------------------------------------------------------------------
// Window/tab model helpers
// ---------------------------------------------------------------------------

/// Resolve a window number the way vim's `find_win_by_nr()` does: `0` is the
/// current window and a positive value is a 1-based window number in the
/// current tab page.  Out-of-range values resolve to `None` without an error.
fn win_by_nr(ed: &Editor, nr: i64) -> Option<WindowId> {
    if nr < 0 {
        return None;
    }
    if nr == 0 {
        return Some(ed.windows.current_id());
    }
    usize::try_from(nr)
        .ok()
        .and_then(|number| ed.windows.ids().get(number - 1).copied())
}

/// Resolve a window ID (never a number: ids start at 1000).
fn win_by_id(ed: &Editor, id: i64) -> Option<WindowId> {
    i32::try_from(id)
        .ok()
        .and_then(|id| ed.windows.get(id).map(|window| window.id))
}

/// Vim's `find_win_by_nr_or_id()`: a value at or above `LOWEST_WIN_ID` is a
/// window ID, anything else a window number.
fn win_by_nr_or_id(ed: &Editor, nr: i64) -> Option<WindowId> {
    if nr >= LOWEST_WIN_ID {
        win_by_id(ed, nr)
    } else {
        win_by_nr(ed, nr)
    }
}

/// Resolve a tab page argument: `0` is the current tab page, a positive value
/// is the 1-based tab page number.
fn tab_index(ed: &Editor, tabnr: i64) -> Option<usize> {
    if tabnr < 0 {
        return None;
    }
    if tabnr == 0 {
        return Some(ed.windows.current_tab_index());
    }
    let index = usize::try_from(tabnr).ok()? - 1;
    (index < ed.windows.tab_count()).then_some(index)
}

/// One window of tab page `index`; `winnr` is `0` for that tab page's current
/// window and otherwise a 1-based window number.
fn win_of_tab(ed: &Editor, index: usize, winnr: i64) -> Option<WindowId> {
    if winnr < 0 {
        return None;
    }
    if winnr == 0 {
        return ed.windows.tabs.get(index).map(|tab| tab.current);
    }
    usize::try_from(winnr)
        .ok()
        .and_then(|number| ed.windows.ids_of_tab(index + 1).get(number - 1).copied())
}

/// A window argument that must name a window of tab page `index`, accepting
/// both window numbers and window IDs (as vim's `gettabwinvar()` does).
fn win_of_tab_arg(ed: &Editor, index: usize, nr: i64) -> Option<WindowId> {
    let id = if nr >= LOWEST_WIN_ID {
        win_by_id(ed, nr)?
    } else {
        win_of_tab(ed, index, nr)?
    };
    (ed.windows.tab_of(id) == Some(index)).then_some(id)
}

/// The window ids of the tab page that holds `id`.
fn ids_of_window_tab(ed: &Editor, id: WindowId) -> Vec<WindowId> {
    ed.windows
        .tab_of(id)
        .map(|index| ed.windows.ids_of_tab(index + 1))
        .unwrap_or_default()
}

/// Read one window-local option of `id`, falling back to the global value (the
/// same rule as `Editor::wopt`, but for any window).  `None` for an option the
/// option table does not know.
fn window_option(ed: &Editor, id: WindowId, name: &str) -> Option<OptValue> {
    let canon = Options::canonical(name).ok()?;
    if is_window_local(&canon) {
        if let Some(value) = ed
            .windows
            .get(id)
            .and_then(|window| window.get_local(&canon))
        {
            return Some(value.clone());
        }
    }
    ed.options.get(&canon).ok().cloned()
}

fn window_option_bool(ed: &Editor, id: WindowId, name: &str) -> bool {
    window_option(ed, id, name)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Every window-local option of `id` (`getwinvar(nr, '&')`).
fn window_option_dict(ed: &Editor, id: WindowId) -> VimDict {
    let mut dict = VimDict::new();
    for name in WINDOW_LOCAL {
        if let Some(value) = window_option(ed, id, name) {
            dict.insert(*name, opt_value_to_value(&value));
        }
    }
    dict
}

/// The live `w:` dictionary of one window: the current window's dictionary is
/// the one [`ScriptState::winvars`] holds, every other one is parked in
/// [`WinState::winvars`].
fn window_vars<'a>(
    ed: &Editor,
    state: &'a ScriptState,
    id: WindowId,
) -> Option<&'a HashMap<String, Value>> {
    if id == ed.windows.current_id() {
        Some(&state.winvars)
    } else {
        state.win.winvars.get(&id)
    }
}

fn window_vars_mut<'a>(
    ed: &Editor,
    state: &'a mut ScriptState,
    id: WindowId,
) -> &'a mut HashMap<String, Value> {
    if id == ed.windows.current_id() {
        &mut state.winvars
    } else {
        state.win.winvars.entry(id).or_default()
    }
}

/// The live `t:` dictionary of tab page `index`, with the same mirroring rule.
fn tab_vars<'a>(
    ed: &Editor,
    state: &'a ScriptState,
    index: usize,
) -> Option<&'a HashMap<String, Value>> {
    if index == ed.windows.current_tab_index() {
        Some(&state.tabvars)
    } else {
        ed.windows
            .tabs
            .get(index)
            .and_then(|tab| state.win.tabvars.get(&tab.id))
    }
}

fn tab_vars_mut<'a>(
    ed: &Editor,
    state: &'a mut ScriptState,
    index: usize,
) -> &'a mut HashMap<String, Value> {
    if index == ed.windows.current_tab_index() {
        &mut state.tabvars
    } else {
        let id = ed.windows.tabs[index].id;
        state.win.tabvars.entry(id).or_default()
    }
}

/// Keep `ScriptState`'s live `w:`/`t:` dictionaries in step with the window and
/// tab page the editor is on, recording tab page switches for
/// `tabpagenr('#')`.
///
/// rvim-viml reads and writes `w:`/`t:` through the single dictionaries
/// `ScriptState::winvars`/`tabvars`, so the current window's/tab page's
/// dictionary lives there and all others are parked in [`WinState`] keyed by
/// id.
fn sync_scopes(ed: &Editor, state: &mut ScriptState) {
    let window = ed.windows.current_id();
    if state.win.winvars_owner != Some(window) {
        if let Some(previous) = state.win.winvars_owner {
            let vars = std::mem::take(&mut state.winvars);
            state.win.winvars.insert(previous, vars);
        }
        state.winvars = state.win.winvars.remove(&window).unwrap_or_default();
        state.win.winvars_owner = Some(window);
    }
    let tab = ed.windows.current_tab().id;
    if state.win.tabvars_owner != Some(tab) {
        if let Some(previous) = state.win.tabvars_owner {
            let vars = std::mem::take(&mut state.tabvars);
            state.win.tabvars.insert(previous, vars);
        }
        state.tabvars = state.win.tabvars.remove(&tab).unwrap_or_default();
        state.win.tabvars_owner = Some(tab);
    }
    let index = ed.windows.current_tab_index();
    if state.win.last_tab != Some(index) {
        if state.win.last_tab.is_some() {
            state.win.prev_tab = state.win.last_tab;
        }
        state.win.last_tab = Some(index);
    }
}

/// Display column (0-based cells) of the byte column `byte_col` in `text`,
/// expanding tabs to `tabstop` and giving control characters two cells.  This
/// mirrors `rvim_term::display_col()`; rvim-ex does not depend on rvim-term.
fn display_col(text: &str, byte_col: usize, tabstop: usize) -> usize {
    let mut col = 0;
    for (offset, ch) in text.char_indices() {
        if offset >= byte_col {
            break;
        }
        col += char_width(ch, col, tabstop);
    }
    col
}

/// Vim's `line()`/`tv_get_lnum()` translation: a Number is used as-is, a
/// String that does not start with a digit goes through `line()` (`.`, `$`,
/// `'x`), otherwise the leading number is parsed.  `0` means "invalid".
fn lnum_from_value(ed: &Editor, value: &Value) -> i64 {
    match value {
        Value::Number(n) => *n,
        Value::String(s) => {
            let text = s.trim();
            if text.is_empty() {
                return 0;
            }
            if let Some(first) = text.chars().next() {
                if first.is_ascii_digit() || (first == '-' && text.len() > 1) {
                    return parse_leading_number(text);
                }
            }
            match text {
                "." => ed.cursor().line as i64 + 1,
                "$" => ed.line_count() as i64,
                _ => {
                    if let Some(mark) = text.strip_prefix('\'') {
                        let name = mark.chars().next().unwrap_or('"');
                        ed.marks
                            .resolve(ed.current_bufnr(), name)
                            .map(|pos| pos.line as i64 + 1)
                            .unwrap_or(0)
                    } else {
                        0
                    }
                }
            }
        }
        _ => 0,
    }
}

fn parse_leading_number(text: &str) -> i64 {
    let mut end = 0;
    for (idx, ch) in text.char_indices() {
        if ch.is_ascii_digit() || (idx == 0 && (ch == '-' || ch == '+')) {
            end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    text[..end].parse().unwrap_or(0)
}

/// Display width of one character when it starts at `vcol` (0-based cells).
fn char_width(ch: char, vcol: usize, tabstop: usize) -> usize {
    match ch {
        '\t' => {
            let ts = tabstop.max(1);
            ts - (vcol % ts)
        }
        c if (c as u32) < 0x20 || c == '\u{7f}' => 2,
        _ => 1,
    }
}

/// Virtual column range (1-based, inclusive) occupied by the character at
/// `char_idx`; a position at/after end-of-line reports the column after the
/// last displayed character.
fn vcol_range(line: &str, char_idx: usize, tabstop: usize, off: i64) -> (i64, i64) {
    let mut width = 0usize;
    for (count, ch) in line.chars().enumerate() {
        let w = char_width(ch, width, tabstop);
        if count == char_idx {
            let start = width as i64 + 1 + off;
            return (start, start + w as i64 - 1 + off);
        }
        width += w;
    }
    // At or past end-of-line: Vim reports the end-of-line column (+ coladd).
    let eol = width as i64 + 1 + off;
    (eol, eol)
}

fn opt_tabstop(ed: &Editor) -> usize {
    ed.options.num_or("tabstop", 8).max(1) as usize
}

// ---------------------------------------------------------------------------
// Window variables / tab variables
// ---------------------------------------------------------------------------

/// `get_var_from()` semantics: `&opt` reads an option of window `id`, an empty
/// name returns all of its variables, anything else is a variable lookup with a
/// default.
fn get_scoped_var(
    ed: &Editor,
    id: WindowId,
    vars: &HashMap<String, Value>,
    name: &str,
    default: Value,
    window_options: bool,
) -> Value {
    if window_options {
        if let Some(option) = name.strip_prefix('&') {
            if option.is_empty() {
                return Value::dict(window_option_dict(ed, id));
            }
            return match window_option(ed, id, option) {
                Some(value) => opt_value_to_value(&value),
                None => default,
            };
        }
    }
    if name.is_empty() {
        return Value::dict(vars_to_dict(vars));
    }
    vars.get(name).cloned().unwrap_or(default)
}

/// `setwinvar()`/`settabvar()`/`settabwinvar()` semantics.
///
/// `&`-names set the option of window `id`: window-local options stay local to
/// that window (like vim's `OPT_LOCAL`), anything else is a global option.
fn set_scoped_var(
    ed: &mut Editor,
    id: WindowId,
    vars: &mut HashMap<String, Value>,
    name: &str,
    value: Value,
) -> VimResult<()> {
    if let Some(option) = name.strip_prefix('&') {
        if option.is_empty() {
            return Err(err("E518", "Unknown option: "));
        }
        let canon = Options::canonical(option)?;
        // Coerce to the option's own type, like Vim's `set_option_from_tv()`;
        // Options::set() itself parses strings for Bool/Num options.
        let coerced = match ed.options.get(&canon) {
            Ok(OptValue::Bool(_)) => match &value {
                Value::String(text) => OptValue::Str(text.clone()),
                other => OptValue::Bool(other.to_number() != 0),
            },
            Ok(OptValue::Num(_)) => match &value {
                Value::String(text) => OptValue::Str(text.clone()),
                other => OptValue::Num(other.to_number()),
            },
            Ok(OptValue::Str(_)) => OptValue::Str(value.to_string_value()),
            Err(_) => value_to_opt(&value),
        };
        if is_window_local(&canon) {
            return ed.set_window_option(id, &canon, coerced);
        }
        return ed.set_option(&canon, coerced, SetScope::Local);
    }
    if name.is_empty() {
        return Err(err("E461", "Illegal variable name: "));
    }
    vars.insert(name.to_string(), value);
    Ok(())
}

fn f_getwinvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let nr = narg(args, 0);
    let name = sarg(args, 1);
    let default = args.get(2).cloned().unwrap_or_else(|| Value::string(""));
    let Some(id) = win_by_nr_or_id(ed, nr) else {
        return Ok(default);
    };
    let empty = HashMap::new();
    let vars = window_vars(ed, state, id).unwrap_or(&empty);
    Ok(get_scoped_var(ed, id, vars, &name, default, true))
}

fn f_setwinvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let nr = narg(args, 0);
    let name = sarg(args, 1);
    let value = arg(args, 2);
    if let Some(id) = win_by_nr_or_id(ed, nr) {
        let vars = window_vars_mut(ed, state, id);
        set_scoped_var(ed, id, vars, &name, value)?;
    }
    Ok(Value::Number(0))
}

fn f_gettabvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let nr = narg(args, 0);
    let name = sarg(args, 1);
    let default = args.get(2).cloned().unwrap_or_else(|| Value::string(""));
    let Some(index) = tab_index(ed, nr) else {
        return Ok(default);
    };
    // Tab pages have no local options, so `&name` is a plain variable lookup.
    let current = ed.windows.current_id();
    let empty = HashMap::new();
    let vars = tab_vars(ed, state, index).unwrap_or(&empty);
    Ok(get_scoped_var(ed, current, vars, &name, default, false))
}

fn f_settabvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let nr = narg(args, 0);
    let name = sarg(args, 1);
    let value = arg(args, 2);
    if let Some(index) = tab_index(ed, nr) {
        // Tab pages have no local options, and Vim writes `t:{varname}`; an
        // empty name or an option-style name is therefore illegal.
        if name.is_empty() || name.starts_with('&') {
            return Err(err("E461", format!("Illegal variable name: {name}")));
        }
        tab_vars_mut(ed, state, index).insert(name, value);
    }
    Ok(Value::Number(0))
}

fn f_gettabwinvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let tabnr = narg(args, 0);
    let winnr = narg(args, 1);
    let name = sarg(args, 2);
    let default = args.get(3).cloned().unwrap_or_else(|| Value::string(""));
    let Some(index) = tab_index(ed, tabnr) else {
        return Ok(default);
    };
    let Some(id) = win_of_tab_arg(ed, index, winnr) else {
        return Ok(default);
    };
    let empty = HashMap::new();
    let vars = window_vars(ed, state, id).unwrap_or(&empty);
    Ok(get_scoped_var(ed, id, vars, &name, default, true))
}

fn f_settabwinvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let tabnr = narg(args, 0);
    let winnr = narg(args, 1);
    let name = sarg(args, 2);
    let value = arg(args, 3);
    if let Some(index) = tab_index(ed, tabnr) {
        if let Some(id) = win_of_tab_arg(ed, index, winnr) {
            let vars = window_vars_mut(ed, state, id);
            set_scoped_var(ed, id, vars, &name, value)?;
        }
    }
    Ok(Value::Number(0))
}

// ---------------------------------------------------------------------------
// Window/tab information
// ---------------------------------------------------------------------------

/// Width in cells of the 'number'/'relativenumber' gutter `window` draws (the
/// same `digits + 1` rule `rvim-term` uses).  The sign and fold columns are not
/// modelled, so they add nothing to `textoff`.
fn text_offset(ed: &Editor, id: WindowId, lines: usize) -> i64 {
    let numbered =
        window_option_bool(ed, id, "number") || window_option_bool(ed, id, "relativenumber");
    if numbered {
        (lines.max(1).to_string().len() + 1) as i64
    } else {
        0
    }
}

/// One `getwininfo()` entry.  `height` is the buffer area (`w_height` in
/// vim's `get_win_info()`), i.e. the frame's rows without its status line.
fn win_info_dict(
    ed: &Editor,
    state: &ScriptState,
    id: WindowId,
    tabnr: usize,
    winnr: usize,
) -> VimDict {
    let mut dict = VimDict::new();
    let Some(window) = ed.windows.get(id) else {
        return dict;
    };
    let lines = ed
        .buffer(window.bufnr)
        .map(|buffer| buffer.line_count())
        .unwrap_or(1)
        .max(1);
    let laid_out = window.rect.rows > 0 && window.rect.cols > 0;
    let text_rows = window.rect.text_rows();
    let topline = window.top_line + 1;
    let botline = if laid_out {
        (window.top_line + text_rows).clamp(1, lines) as i64
    } else {
        lines as i64
    };
    dict.insert("tabnr", Value::Number(tabnr as i64));
    dict.insert("winnr", Value::Number(winnr as i64));
    dict.insert("winid", Value::Number(id as i64));
    dict.insert(
        "height",
        Value::Number(if laid_out { text_rows as i64 } else { -1 }),
    );
    dict.insert(
        "width",
        Value::Number(if laid_out {
            window.rect.cols as i64
        } else {
            -1
        }),
    );
    dict.insert("winrow", Value::Number(window.rect.row as i64 + 1));
    dict.insert("wincol", Value::Number(window.rect.col as i64 + 1));
    dict.insert("textoff", Value::Number(text_offset(ed, id, lines)));
    dict.insert("topline", Value::Number(topline as i64));
    dict.insert("botline", Value::Number(botline));
    dict.insert("bufnr", Value::Number(window.bufnr as i64));
    dict.insert("winbar", Value::Number(0));
    dict.insert("terminal", Value::Number(0));
    dict.insert("quickfix", Value::Number(i64::from(window.quickfix)));
    dict.insert("loclist", Value::Number(0));
    let empty = HashMap::new();
    let vars = window_vars(ed, state, id).unwrap_or(&empty);
    dict.insert("variables", Value::dict(vars_to_dict(vars)));
    dict
}

/// All windows of every tab page, or the one window with the given ID.
fn f_getwininfo(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let mut out = Vec::new();
    match args.first() {
        Some(value) => {
            let Some(id) = win_by_id(ed, value.to_number()) else {
                return Ok(Value::list(Vec::new()));
            };
            let Some(index) = ed.windows.tab_of(id) else {
                return Ok(Value::list(Vec::new()));
            };
            let winnr = ed.windows.winnr(id);
            out.push(Value::dict(win_info_dict(ed, state, id, index + 1, winnr)));
        }
        None => {
            for tabnr in 1..=ed.windows.tab_count() {
                for id in ed.windows.ids_of_tab(tabnr) {
                    let winnr = ed.windows.winnr(id);
                    out.push(Value::dict(win_info_dict(ed, state, id, tabnr, winnr)));
                }
            }
        }
    }
    Ok(Value::list(out))
}

/// One `gettabinfo()` entry per tab page (or only the requested one), whose
/// `windows` entry holds the same dictionaries `getwininfo()` returns.
fn f_gettabinfo(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let indexes: Vec<usize> = match args.first() {
        Some(value) => match tab_index(ed, value.to_number()) {
            Some(index) => vec![index],
            None => return Ok(Value::list(Vec::new())),
        },
        None => (0..ed.windows.tab_count()).collect(),
    };
    let mut out = Vec::new();
    for index in indexes {
        let tabnr = index + 1;
        let mut dict = VimDict::new();
        dict.insert("tabnr", Value::Number(tabnr as i64));
        let windows = ed
            .windows
            .ids_of_tab(tabnr)
            .into_iter()
            .map(|id| {
                let winnr = ed.windows.winnr(id);
                Value::dict(win_info_dict(ed, state, id, tabnr, winnr))
            })
            .collect();
        dict.insert("windows", Value::list(windows));
        let empty = HashMap::new();
        let vars = tab_vars(ed, state, index).unwrap_or(&empty);
        dict.insert("variables", Value::dict(vars_to_dict(vars)));
        dict.insert("winbar", Value::Number(0));
        out.push(Value::dict(dict));
    }
    Ok(Value::list(out))
}

fn f_tabpagenr(args: &[Value], ed: &Editor, state: &ScriptState) -> VimResult<Value> {
    let current = (ed.windows.current_tab_index() + 1) as i64;
    let Some(value) = args.first() else {
        return Ok(Value::Number(current));
    };
    match value.to_string_value().as_str() {
        "$" => Ok(Value::Number(ed.windows.tab_count() as i64)),
        // The tab page that was current on the previous builtin call; vim
        // reports the current tab page while there is no other one.
        "#" => Ok(Value::Number(
            state
                .win
                .prev_tab
                .map(|index| index + 1)
                .unwrap_or(ed.windows.current_tab_index() + 1) as i64,
        )),
        other => Err(err("E15", format!("Invalid expression: {other}"))),
    }
}

fn f_tabpagewinnr(args: &[Value], ed: &Editor) -> VimResult<Value> {
    let tabnr = narg(args, 0);
    let Some(index) = tab_index(ed, tabnr) else {
        return Ok(Value::Number(0));
    };
    let ids = ed.windows.ids_of_tab(index + 1);
    let Some(value) = args.get(1) else {
        let current = ed.windows.tabs.get(index).map(|tab| tab.current);
        let winnr = current
            .and_then(|id| ids.iter().position(|other| *other == id))
            .map(|position| position + 1)
            .unwrap_or(0);
        return Ok(Value::Number(winnr as i64));
    };
    match value.to_string_value().as_str() {
        "$" => Ok(Value::Number(ids.len() as i64)),
        "#" => {
            let previous = ed.windows.tabs.get(index).map(|tab| tab.prev);
            let winnr = previous
                .and_then(|id| ids.iter().position(|other| *other == id))
                .map(|position| position + 1)
                .unwrap_or(0);
            Ok(Value::Number(winnr as i64))
        }
        other => Err(err("E15", format!("Invalid expression: {other}"))),
    }
}

fn f_tabpagebuflist(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let index = match args.first() {
        Some(value) => match tab_index(ed, value.to_number()) {
            Some(index) => index,
            // Vim returns the number zero for an invalid tab page here.
            None => return Ok(Value::Number(0)),
        },
        None => ed.windows.current_tab_index(),
    };
    let buffers = ed
        .windows
        .ids_of_tab(index + 1)
        .into_iter()
        .filter_map(|id| {
            ed.windows
                .get(id)
                .map(|window| Value::Number(window.bufnr as i64))
        })
        .collect();
    Ok(Value::list(buffers))
}

// ---------------------------------------------------------------------------
// Window identity / layout
// ---------------------------------------------------------------------------

/// `winbufnr({nr})`: `-1` when the window does not exist.
fn f_winbufnr(args: &[Value], ed: &Editor) -> VimResult<Value> {
    match win_by_nr_or_id(ed, narg(args, 0)).and_then(|id| ed.windows.get(id)) {
        Some(window) => Ok(Value::Number(window.bufnr as i64)),
        None => Ok(Value::Number(-1)),
    }
}

/// `wincol()`: 1-based screen column of the cursor inside the current
/// window, counting from the window's left edge ('leftcol').
fn f_wincol(ed: &Editor) -> VimResult<Value> {
    let window = ed.windows.current();
    if window.rect.cols == 0 {
        return Ok(Value::Number(-1));
    }
    let text = ed
        .current_buffer()
        .line_str(window.cursor.line)
        .unwrap_or("")
        .to_string();
    let column = display_col(&text, window.cursor.col, opt_tabstop(ed));
    Ok(Value::Number(column.saturating_sub(window.left) as i64 + 1))
}

/// `winline()`: 1-based screen line of the cursor inside the current
/// window.  'wrap' is not modelled, so this counts buffer lines from
/// `top_line` rather than wrapped screen rows.
fn f_winline(ed: &Editor) -> VimResult<Value> {
    let window = ed.windows.current();
    if window.rect.rows == 0 {
        return Ok(Value::Number(-1));
    }
    Ok(Value::Number(
        (window.cursor.line.saturating_sub(window.top_line) + 1) as i64,
    ))
}

/// `winheight()`/`winwidth()`: buffer-area size from the last layout pass,
/// `-1` when there is no layout yet or the window does not exist.
fn f_win_size(args: &[Value], ed: &Editor, width: bool) -> VimResult<Value> {
    let Some(window) = win_by_nr_or_id(ed, narg(args, 0)).and_then(|id| ed.windows.get(id)) else {
        return Ok(Value::Number(-1));
    };
    if window.rect.rows == 0 || window.rect.cols == 0 {
        return Ok(Value::Number(-1));
    }
    let size = if width {
        window.rect.cols
    } else {
        window.rect.text_rows()
    };
    Ok(Value::Number(size as i64))
}

fn f_win_execute(
    args: &[Value],
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
) -> VimResult<Value> {
    let Some(id) = win_by_id(ed, narg(args, 0)) else {
        return Ok(Value::string(""));
    };
    let previous = ed.windows.current_id();
    ed.goto_window(id)?;
    sync_scopes(ed, state);
    // Run the commands with the editor on the target window.  A failing command
    // aborts the rest (like vim's win_execute()), but the previous window is
    // restored in every case.
    let outcome = match args.get(1) {
        Some(Value::List(commands)) => commands.borrow().iter().try_for_each(|command| {
            crate::exec::run_line_with(ed, env, state, &command.to_string_value()).map(|_| ())
        }),
        Some(command) => {
            crate::exec::run_line_with(ed, env, state, &command.to_string_value()).map(|_| ())
        }
        None => Ok(()),
    };
    ed.goto_window(previous)?;
    sync_scopes(ed, state);
    outcome?;
    // Vim returns the captured `:execute` output; RustVim has no command
    // output capture/`:redir` layer yet, so the (honest) result is an empty
    // string.
    Ok(Value::string(""))
}

fn f_win_findbuf(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let bufnr = narg(args, 0);
    let bufnr = if bufnr == 0 {
        ed.current_bufnr()
    } else {
        bufnr as i32
    };
    let mut found = Vec::new();
    for tabnr in 1..=ed.windows.tab_count() {
        for id in ed.windows.ids_of_tab(tabnr) {
            if ed
                .windows
                .get(id)
                .is_some_and(|window| window.bufnr == bufnr)
            {
                found.push(Value::Number(id as i64));
            }
        }
    }
    Ok(Value::list(found))
}

fn f_win_getid(args: &[Value], ed: &Editor) -> VimResult<Value> {
    if args.is_empty() {
        return Ok(Value::Number(ed.windows.current_id() as i64));
    }
    let winnr = narg(args, 0);
    let index = if args.len() > 1 {
        match tab_index(ed, narg(args, 1)) {
            Some(index) => index,
            // Vim reports -1 for a tab page that does not exist.
            None => return Ok(Value::Number(-1)),
        }
    } else {
        ed.windows.current_tab_index()
    };
    match win_of_tab(ed, index, winnr) {
        Some(id) => Ok(Value::Number(id as i64)),
        None => Ok(Value::Number(0)),
    }
}

fn f_win_gettype(args: &[Value], ed: &Editor) -> VimResult<Value> {
    let id = match args.first() {
        None => Some(ed.windows.current_id()),
        Some(value) => win_by_nr_or_id(ed, value.to_number()),
    };
    Ok(Value::string(match id {
        Some(id) if ed.windows.get(id).is_some_and(|window| window.quickfix) => "quickfix",
        Some(_) => "",
        None => "unknown",
    }))
}

fn f_win_gotoid(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let Some(id) = win_by_id(ed, narg(args, 0)) else {
        return Ok(Value::Number(0));
    };
    ed.goto_window(id)?;
    sync_scopes(ed, state);
    Ok(Value::Number(1))
}

fn f_win_id2tabwin(args: &[Value], ed: &Editor) -> VimResult<Value> {
    let id = narg(args, 0);
    let pair = win_by_id(ed, id)
        .and_then(|id| {
            ed.windows
                .tab_of(id)
                .map(|index| (index + 1, ed.windows.winnr(id)))
        })
        .unwrap_or((0, 0));
    Ok(Value::list(vec![
        Value::Number(pair.0 as i64),
        Value::Number(pair.1 as i64),
    ]))
}

fn f_win_id2win(args: &[Value], ed: &Editor) -> VimResult<Value> {
    let id = narg(args, 0);
    // A window in another tab page has no number here: vim returns zero.
    let number = win_by_id(ed, id)
        .filter(|id| ed.windows.tab_of(*id) == Some(ed.windows.current_tab_index()))
        .map(|id| ed.windows.winnr(id))
        .unwrap_or(0);
    Ok(Value::Number(number as i64))
}

/// `win_screenpos({nr})`: 1-based top-left corner from the last layout
/// pass, `[0, 0]` when the window does not exist.
fn f_win_screenpos(args: &[Value], ed: &Editor) -> VimResult<Value> {
    let Some(rect) = win_by_nr_or_id(ed, narg(args, 0))
        .and_then(|id| ed.windows.get(id))
        .map(|window| window.rect)
    else {
        return Ok(Value::list(vec![Value::Number(0), Value::Number(0)]));
    };
    Ok(Value::list(vec![
        Value::Number(rect.row as i64 + 1),
        Value::Number(rect.col as i64 + 1),
    ]))
}

/// `winlayout([{tabnr}])`: the real frame tree as nested lists
/// (`["leaf", id]`, `["row", [..]]` side by side and
/// `["col", [..]]` stacked), matching `Layout`.
fn f_winlayout(args: &[Value], ed: &Editor) -> VimResult<Value> {
    let index = match args.first() {
        Some(value) => match tab_index(ed, value.to_number()) {
            Some(index) => index,
            None => return Ok(Value::list(Vec::new())),
        },
        None => ed.windows.current_tab_index(),
    };
    match ed.windows.tabs.get(index) {
        Some(tab) => Ok(layout_value(&tab.layout)),
        None => Ok(Value::list(Vec::new())),
    }
}

fn layout_value(layout: &Layout) -> Value {
    match layout {
        Layout::Leaf(id) => Value::list(vec![Value::string("leaf"), Value::Number(*id as i64)]),
        Layout::Row(children) => Value::list(vec![
            Value::string("row"),
            Value::list(children.iter().map(layout_value).collect()),
        ]),
        Layout::Col(children) => Value::list(vec![
            Value::string("col"),
            Value::list(children.iter().map(layout_value).collect()),
        ]),
    }
}

/// True when the frame tree contains a horizontal group, i.e. when there are
/// vertical splits whose widths `winrestcmd()` has to restore.
fn layout_has_vertical(layout: &Layout) -> bool {
    match layout {
        Layout::Leaf(_) => false,
        Layout::Row(_) => true,
        Layout::Col(children) => children.iter().any(layout_has_vertical),
    }
}

/// `winrestcmd()`: the `:resize`/`:vertical resize` commands that
/// restore the requested sizes of the current tab page's windows.  Vim's format
/// is `{height}resize` per window plus `vert {width}resize` when there are
/// vertical splits, separated by `|` and ending with one.
fn f_winrestcmd(ed: &Editor) -> VimResult<Value> {
    let vertical = layout_has_vertical(&ed.windows.current_tab().layout);
    let mut command = String::new();
    for (index, id) in ed.windows.ids().into_iter().enumerate() {
        let Some(window) = ed.windows.get(id) else {
            continue;
        };
        let number = index + 1;
        command.push_str(&format!("{number}resize {}|", window.height));
        if vertical {
            command.push_str(&format!("vert {number}resize {}|", window.width));
        }
    }
    Ok(Value::string(command))
}

/// The window next to `id` in the last layout pass: to its right (so a
/// vertical separator sits between them) or directly below (so its status line
/// separates them).
fn frame_neighbour(ed: &Editor, id: WindowId, horizontal: bool) -> Option<WindowId> {
    let rect = ed.windows.get(id)?.rect;
    if rect.rows == 0 || rect.cols == 0 {
        return None;
    }
    ids_of_window_tab(ed, id).into_iter().find(|other| {
        if *other == id {
            return false;
        }
        let Some(other) = ed.windows.get(*other).map(|window| window.rect) else {
            return false;
        };
        let rows_overlap = other.row < rect.row + rect.rows && rect.row < other.row + other.rows;
        let cols_overlap = other.col < rect.col + rect.cols && rect.col < other.col + other.cols;
        if horizontal {
            // The `|` separator column sits between the two frames.
            other.col == rect.col + rect.cols + 1 && rows_overlap
        } else {
            // Stacked frames touch: the lower one starts right after the
            // status line row of the upper one.
            other.row == rect.row + rect.rows && cols_overlap
        }
    })
}

/// `win_move_separator()` (columns) and `win_move_statusline()` (rows).
///
/// Both return `0` on success and `-1` when there is no separator/status line
/// to move (a window on the tab page edge, or a window without a layout pass
/// yet), i.e. for a window that cannot be resized.
fn f_win_move(args: &[Value], ed: &mut Editor, separator: bool) -> VimResult<Value> {
    let Some(id) = win_by_nr_or_id(ed, narg(args, 0)) else {
        return Ok(Value::Number(-1));
    };
    if frame_neighbour(ed, id, separator).is_none() {
        return Ok(Value::Number(-1));
    }
    let offset = narg(args, 1);
    if let Some(window) = ed.windows.get_mut(id) {
        if separator {
            window.width = (window.width as i64 + offset).max(MIN_WIDTH as i64) as usize;
        } else {
            window.height = (window.height as i64 + offset).max(MIN_HEIGHT as i64) as usize;
        }
    }
    Ok(Value::Number(0))
}

/// Swap everything the two windows show; the frame tree (and therefore each
/// window's rectangle) is left untouched.  This is the documented
/// approximation of `:splitmove` for this model.
fn swap_window_contents(ed: &mut Editor, a: WindowId, b: WindowId) {
    let (Some(first), Some(second)) = (ed.windows.get(a).cloned(), ed.windows.get(b).cloned())
    else {
        return;
    };
    fn copy(target: &mut Window, source: &Window) {
        target.bufnr = source.bufnr;
        target.cursor = source.cursor;
        target.top_line = source.top_line;
        target.left = source.left;
        target.curswant = source.curswant;
        target.local = source.local.clone();
        target.quickfix = source.quickfix;
    }
    if let Some(window) = ed.windows.get_mut(a) {
        copy(window, &second);
    }
    if let Some(window) = ed.windows.get_mut(b) {
        copy(window, &first);
    }
    let current = ed.windows.current().bufnr;
    ed.sync_fileformat_option(current);
}

fn f_win_splitmove(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let source = win_by_nr_or_id(ed, narg(args, 0));
    let target = win_by_nr_or_id(ed, narg(args, 1));
    // Vim's E957 covers both "window does not exist" and "both arguments name
    // the same window".
    let (Some(source), Some(target)) = (source, target) else {
        return Err(err("E957", "Invalid window number"));
    };
    if source == target {
        return Err(err("E957", "Invalid window number"));
    }
    // The `{options}` argument ("vertical"/"rightbelow"/"bottomright") is
    // accepted and ignored: this implementation exchanges the contents of the
    // two windows instead of moving a frame node.
    swap_window_contents(ed, source, target);
    Ok(Value::Number(0))
}

fn f_winsaveview(ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let id = ed.windows.current_id();
    let window = ed.windows.current();
    let view = SavedView {
        lnum: window.cursor.line as i64 + 1,
        col: window.cursor.col as i64,
        curswant: window.curswant as i64,
        topline: window.top_line as i64 + 1,
        leftcol: window.left as i64,
        skipcol: 0,
    };
    let mut dict = VimDict::new();
    dict.insert("lnum", Value::Number(view.lnum));
    dict.insert("col", Value::Number(view.col));
    dict.insert("coladd", Value::Number(0));
    dict.insert("curswant", Value::Number(view.curswant));
    dict.insert("topline", Value::Number(view.topline));
    dict.insert("topfill", Value::Number(0));
    dict.insert("leftcol", Value::Number(view.leftcol));
    dict.insert("skipcol", Value::Number(view.skipcol));
    state.win.saved_views.insert(id, view);
    Ok(Value::dict(dict))
}

fn f_winrestview(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let dict = dict_clone(&arg(args, 0)).ok_or_else(|| err("E474", "Invalid argument"))?;
    let id = ed.windows.current_id();
    let window = ed.windows.current();
    let current = SavedView {
        lnum: window.cursor.line as i64 + 1,
        col: window.cursor.col as i64,
        curswant: window.curswant as i64,
        topline: window.top_line as i64 + 1,
        leftcol: window.left as i64,
        skipcol: 0,
    };
    let lnum = dict_number(&dict, "lnum").unwrap_or(current.lnum);
    let col = dict_number(&dict, "col").unwrap_or(current.col);
    let topline = dict_number(&dict, "topline").unwrap_or(current.topline);
    let leftcol = dict_number(&dict, "leftcol").unwrap_or(current.leftcol);
    let skipcol = dict_number(&dict, "skipcol").unwrap_or(current.skipcol);
    let curswant = dict_number(&dict, "curswant").unwrap_or(col);
    let line = lnum.saturating_sub(1).max(0) as usize;
    let column = col.max(0) as usize;
    ed.set_cursor(Pos::new(line, column));
    // Windows own a real viewport now, so the scroll position is restored too.
    let max_top = ed.line_count().saturating_sub(1);
    if let Some(window) = ed.windows.get_mut(id) {
        window.top_line = (topline.saturating_sub(1).max(0) as usize).min(max_top);
        window.left = leftcol.max(0) as usize;
        window.curswant = curswant.max(0) as usize;
    }
    state.win.saved_views.insert(
        id,
        SavedView {
            lnum: ed.cursor().line as i64 + 1,
            col: ed.cursor().col as i64,
            curswant,
            topline,
            leftcol,
            skipcol,
        },
    );
    Ok(Value::Number(0))
}

// ---------------------------------------------------------------------------
// Tags files
// ---------------------------------------------------------------------------

/// Options are split like Vim's `copy_option_part()` for 'tags': on commas and
/// spaces.
fn split_option_list(value: &str) -> Vec<String> {
    value
        .split([',', ' '])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// The `tags` files that exist, in search order: `(name as written, path)`.
/// Files reached through more than one `'tags'` entry are reported once.
fn tag_files(ed: &Editor) -> Vec<(String, PathBuf)> {
    let option = ed.options.str_or("tags", "");
    // 'tags' is not in the M1 option table yet: fall back to Vim's default.
    let value = if option.trim().is_empty() {
        "./tags,tags".to_string()
    } else {
        option
    };
    let mut found = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for part in split_option_list(&value) {
        let raw = PathBuf::from(&part);
        let path = if raw.is_absolute() {
            raw
        } else {
            ed.cwd.join(&raw)
        };
        if !path.is_file() {
            continue;
        }
        let identity = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen.contains(&identity) {
            continue;
        }
        seen.push(identity);
        found.push((part, path));
    }
    found
}

/// Parse one ctags line into a `taglist()` dictionary, or `None` for a
/// malformed/pseudo tag.
fn parse_tag_line(line: &str) -> Option<VimDict> {
    let line = line.trim_end_matches(['\r']);
    if line.is_empty() || line.starts_with("!_TAG_") {
        return None;
    }
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() < 3 {
        return None;
    }
    let name = fields[0];
    let filename = fields[1];
    if name.is_empty() || filename.is_empty() {
        return None;
    }
    // The third field is the Ex command; extended fields may follow `;"`.
    let command_field = fields[2];
    let (command, extra_start) = match command_field.find(";\"") {
        Some(index) => (&command_field[..index], Some(&command_field[index + 2..])),
        None => (command_field, None),
    };
    let mut extras: Vec<&str> = Vec::new();
    if let Some(rest) = extra_start {
        let rest = rest.strip_prefix('\t').unwrap_or(rest);
        if !rest.is_empty() {
            extras.push(rest);
        }
    }
    extras.extend(fields[3..].iter().copied());

    let mut dict = VimDict::new();
    dict.insert("name", Value::string(name));
    dict.insert("filename", Value::string(filename));
    dict.insert("cmd", Value::string(command));
    let mut static_tag = 0i64;
    let mut kind: Option<String> = None;
    for extra in extras {
        if extra.is_empty() {
            continue;
        }
        if let Some(value) = extra.strip_prefix("file:") {
            // Static tag marker; the file name is already known.
            let _ = value;
            static_tag = 1;
            continue;
        }
        if let Some(value) = extra.strip_prefix("kind:") {
            if !value.is_empty() {
                kind = Some(value.to_string());
            }
            continue;
        }
        if let Some((key, value)) = extra.split_once(':') {
            if !key.is_empty() {
                dict.insert(key, Value::string(value));
            }
            continue;
        }
        // A bare field is the tag kind (ctags writes it right after `;"`).
        if kind.is_none() {
            kind = Some(extra.to_string());
        }
    }
    if let Some(kind) = kind {
        dict.insert("kind", Value::string(kind));
    }
    dict.insert("static", Value::Number(static_tag));
    Some(dict)
}

fn f_taglist(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let pattern = sarg(args, 0);
    if pattern.is_empty() {
        return Ok(Value::list(Vec::new()));
    }
    // `{filename}` (tag priority) is accepted but not used; see module docs.
    let mut result = Vec::new();
    for (_, path) in tag_files(ed) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let Some(dict) = parse_tag_line(line) else {
                continue;
            };
            let name = dict_string(&dict, "name").unwrap_or_default();
            // TAG_REGEXP without case folding, like Vim's taglist().
            if rvim_viml::regex_mvp::is_match(&pattern, &name, false) {
                result.push(Value::dict(dict));
            }
        }
    }
    Ok(Value::list(result))
}

// ---------------------------------------------------------------------------
// Tag stack
// ---------------------------------------------------------------------------

fn tag_stack_item_dict(entry: &TagStackEntry) -> VimDict {
    let mut dict = VimDict::new();
    dict.insert("tagname", Value::string(entry.tagname.clone()));
    dict.insert("matchnr", Value::Number(entry.matchnr));
    dict.insert("bufnr", Value::Number(entry.bufnr));
    dict.insert(
        "from",
        Value::list(vec![
            Value::Number(entry.from[0]),
            Value::Number(entry.from[1]),
            Value::Number(entry.from[2] + 1),
            Value::Number(entry.from[3]),
        ]),
    );
    if let Some(user_data) = &entry.user_data {
        dict.insert("user_data", Value::string(user_data.clone()));
    }
    dict
}

fn f_gettagstack(args: &[Value], ed: &Editor, state: &mut ScriptState) -> VimResult<Value> {
    let mut dict = VimDict::new();
    if let Some(value) = args.first() {
        if win_by_nr_or_id(ed, value.to_number()).is_none() {
            return Ok(Value::dict(dict));
        }
    }
    dict.insert("length", Value::Number(state.win.tag_stack.len() as i64));
    dict.insert("curidx", Value::Number(state.win.tag_stack_idx as i64 + 1));
    dict.insert(
        "items",
        Value::list(
            state
                .win
                .tag_stack
                .iter()
                .map(|entry| Value::dict(tag_stack_item_dict(entry)))
                .collect(),
        ),
    );
    Ok(Value::dict(dict))
}

fn f_settagstack(args: &[Value], ed: &Editor, state: &mut ScriptState) -> VimResult<Value> {
    let win = narg(args, 0);
    if win_by_nr_or_id(ed, win).is_none() {
        return Ok(Value::Number(-1));
    }
    let dict = dict_clone(&arg(args, 1)).ok_or_else(|| err("E715", "Dictionary required"))?;
    let action = match args.get(2) {
        None => 'r',
        Some(Value::String(text)) => match text.as_str() {
            "r" | "a" | "t" => text.chars().next().unwrap_or('r'),
            other => {
                return Err(err("E962", format!("Invalid action: '{other}'")));
            }
        },
        Some(_) => return Err(err("E715", "Dictionary required")),
    };
    // `curidx` takes effect before the stack is modified.
    if let Some(curidx) = dict_number(&dict, "curidx") {
        let index = (curidx - 1).max(0) as usize;
        state.win.tag_stack_idx = index.min(state.win.tag_stack.len());
    }
    if action == 't' {
        state.win.tag_stack.truncate(state.win.tag_stack_idx);
    }
    if let Some(items) = dict.get("items") {
        let items = list_clone(items).ok_or_else(|| err("E714", "List required"))?;
        if action == 'r' {
            state.win.tag_stack.clear();
        }
        for item in items {
            let Some(item) = dict_clone(&item) else {
                continue;
            };
            let Some(from) = item.get("from").and_then(list_clone) else {
                continue;
            };
            if from.len() < 4 {
                continue;
            }
            let Some(tagname) = dict_string(&item, "tagname") else {
                continue;
            };
            let matchnr = dict_number(&item, "matchnr").unwrap_or(1).max(1);
            let entry = TagStackEntry {
                tagname,
                matchnr,
                bufnr: dict_number(&item, "bufnr").unwrap_or(0),
                from: [
                    from[0].to_number(),
                    from[1].to_number(),
                    from[2].to_number() - 1,
                    from[3].to_number(),
                ],
                user_data: dict_string(&item, "user_data"),
            };
            state.win.tag_stack.push(entry);
        }
        state.win.tag_stack_idx = state.win.tag_stack.len();
    }
    Ok(Value::Number(0))
}

// ---------------------------------------------------------------------------
// Jump list / change list / marks
// ---------------------------------------------------------------------------

fn jump_entry_dict(entry: &JumpEntry) -> VimDict {
    let mut dict = VimDict::new();
    dict.insert("bufnr", Value::Number(entry.bufnr));
    dict.insert("lnum", Value::Number(entry.lnum));
    dict.insert("col", Value::Number(entry.col));
    dict.insert("coladd", Value::Number(0));
    if let Some(filename) = &entry.filename {
        dict.insert("filename", Value::string(filename.clone()));
    }
    dict
}

fn f_getjumplist(args: &[Value], ed: &Editor, state: &mut ScriptState) -> VimResult<Value> {
    if let Some(value) = args.first() {
        if win_by_nr_or_id(ed, value.to_number()).is_none() {
            return Ok(Value::list(Vec::new()));
        }
    }
    if args.len() > 1 && tab_index(ed, narg(args, 1)).is_none() {
        return Ok(Value::list(Vec::new()));
    }
    let entries: Vec<Value> = state
        .win
        .jump_list
        .iter()
        .map(|entry| Value::dict(jump_entry_dict(entry)))
        .collect();
    let index = state.win.jump_list_idx.min(state.win.jump_list.len()) as i64;
    Ok(Value::list(vec![
        Value::list(entries),
        Value::Number(index),
    ]))
}

/// Resolve a `{buf}` argument the way Vim's `tv_get_buf()` does for the
/// builtins here: `Number` = buffer number (`0` = current), `String` = buffer
/// name / `[No Name]`, empty string = current buffer.
fn resolve_bufnr(ed: &Editor, value: &Value) -> Option<i32> {
    match value {
        Value::Number(n) => {
            let n = *n;
            if n == 0 || n == ed.current_bufnr() as i64 {
                return ed
                    .all_buffers()
                    .iter()
                    .any(|buffer| buffer.id == ed.current_bufnr())
                    .then_some(ed.current_bufnr());
            }
            ed.all_buffers()
                .iter()
                .find(|buffer| buffer.id as i64 == n)
                .map(|buffer| buffer.id)
        }
        Value::String(name) if name.is_empty() => Some(ed.current_bufnr()),
        Value::String(name) => ed
            .all_buffers()
            .iter()
            .find(|buffer| buffer.display_name() == *name)
            .map(|buffer| buffer.id),
        _ => None,
    }
}

fn f_getchangelist(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let bufnr = match args.first() {
        None => ed.current_bufnr(),
        Some(value) => match resolve_bufnr(ed, value) {
            Some(bufnr) => bufnr,
            None => return Ok(Value::list(Vec::new())),
        },
    };
    if ed.buffer(bufnr).is_err() {
        return Ok(Value::list(Vec::new()));
    }
    // No change-list positions are recorded by the linear undo history yet.
    Ok(Value::list(vec![Value::list(Vec::new()), Value::Number(0)]))
}

fn mark_dict(mark: &str, bufnr: i32, line: usize, col: usize, file: Option<String>) -> VimDict {
    let mut dict = VimDict::new();
    dict.insert("mark", Value::string(mark));
    dict.insert(
        "pos",
        Value::list(vec![
            Value::Number(bufnr as i64),
            Value::Number(line as i64),
            Value::Number(col as i64),
            Value::Number(0),
        ]),
    );
    if let Some(file) = file {
        dict.insert("file", Value::string(file));
    }
    dict
}

fn f_getmarklist(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let mut marks = Vec::new();
    let Some(value) = args.first() else {
        // No argument: global marks 'A'-'Z' and '0'-'9'.
        for name in ('A'..='Z').chain('0'..='9') {
            let Some(global) = ed.marks.get_global(name) else {
                continue;
            };
            let file = ed
                .buffer(global.bufnr)
                .ok()
                .and_then(|buffer| buffer.name.as_ref())
                .map(|path| path.display().to_string());
            marks.push(Value::dict(mark_dict(
                &format!("'{name}"),
                global.bufnr,
                global.pos.line + 1,
                global.pos.col + 1,
                file,
            )));
        }
        return Ok(Value::list(marks));
    };
    let Some(bufnr) = resolve_bufnr(ed, value) else {
        return Ok(Value::list(marks));
    };
    if ed.buffer(bufnr).is_err() {
        return Ok(Value::list(marks));
    }
    // Local marks 'a'-'z'.  RustVim's MarkStore only stores named marks, so the
    // special marks ('[', ']', '.', ...) are not reported.
    for name in 'a'..='z' {
        let Some(pos) = ed.marks.get_local(bufnr, name) else {
            continue;
        };
        marks.push(Value::dict(mark_dict(
            &format!("'{name}"),
            bufnr,
            pos.line + 1,
            pos.col + 1,
            None,
        )));
    }
    Ok(Value::list(marks))
}

// ---------------------------------------------------------------------------
// State / undo / buffer statistics
// ---------------------------------------------------------------------------

fn f_state(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let mode = mode_identifier(ed.mode);
    let text = match args.first() {
        None => mode.to_string(),
        Some(include) => {
            let include = include.to_string_value();
            mode.chars().filter(|ch| include.contains(*ch)).collect()
        }
    };
    Ok(Value::string(text))
}

/// Vim's `mode()` identifier for the current editor mode.
fn mode_identifier(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "n",
        Mode::Insert => "i",
        Mode::Replace => "R",
        Mode::Visual => "v",
        Mode::VisualLine => "V",
        Mode::VisualBlock => "\u{16}",
        Mode::Select => "s",
        Mode::CommandLine => "c",
        Mode::OperatorPending | Mode::MotionPending { .. } => "no",
        Mode::RegisterPending => "n",
        Mode::Terminal => "t",
    }
}

fn f_undotree(ed: &mut Editor) -> VimResult<Value> {
    let bufnr = ed.current_bufnr();
    let history = ed.history_for(bufnr);
    let changes = history.map(|h| h.change_count()).unwrap_or(0) as i64;
    let can_redo = history.is_some_and(|h| h.can_redo());
    // Linear undo: seq_last is the number of recorded changes.  The public
    // UndoHistory API does not expose its head index, so seq_cur is only exact
    // at the two ends of the history (0 before the first change, seq_last when
    // there is nothing to redo).
    let seq_cur = if can_redo { 0 } else { changes };
    let mut dict = VimDict::new();
    dict.insert("synced", Value::Number(1));
    dict.insert("seq_last", Value::Number(changes));
    dict.insert("seq_cur", Value::Number(seq_cur));
    dict.insert("time_cur", Value::Number(0));
    dict.insert("save_last", Value::Number(0));
    dict.insert("save_cur", Value::Number(0));
    dict.insert("entries", Value::list(Vec::new()));
    Ok(Value::dict(dict))
}

/// Vim's `line_count_info()`: bytes/chars in the (possibly partial) line, and
/// whitespace-separated "words".
fn line_count_info(text: &str, words: &mut i64, chars: &mut i64) -> i64 {
    let mut count = 0i64;
    let mut in_word = false;
    for ch in text.chars() {
        if in_word {
            if rvim_core::charclass::is_space(ch) {
                *words += 1;
                in_word = false;
            }
        } else if !rvim_core::charclass::is_space(ch) {
            in_word = true;
        }
        count += 1;
    }
    if in_word {
        *words += 1;
    }
    *chars += count;
    text.len() as i64
}

/// Byte/char/word statistics with the cursor position included, like
/// `wordcount()` (which is Vim's `g CTRL-G`).
fn f_wordcount(ed: &mut Editor) -> VimResult<Value> {
    let cursor = ed.cursor();
    let eol_size = match ed.current_buffer().fileformat() {
        rvim_buffer::FileFormat::Dos => 2,
        _ => 1,
    };
    let final_newline = ed.current_buffer().has_final_newline();
    let line_count = ed.line_count();
    let mut bytes = 0i64;
    let mut chars = 0i64;
    let mut words = 0i64;
    let mut cursor_bytes = 0i64;
    let mut cursor_chars = 0i64;
    let mut cursor_words = 0i64;
    let text_lines: Vec<String> = ed
        .current_buffer()
        .lines()
        .iter()
        .map(|line| line.as_str().to_string())
        .collect();
    for (index, text) in text_lines.iter().enumerate() {
        let line_no = index + 1;
        let last_line = line_no == line_count;
        if line_no == cursor.line + 1 {
            cursor_bytes = bytes;
            cursor_chars = chars;
            cursor_words = words;
            let prefix_chars = (cursor.col + 1).min(text.chars().count());
            let prefix: String = text.chars().take(prefix_chars).collect();
            line_count_info(&prefix, &mut cursor_words, &mut cursor_chars);
            cursor_bytes += rvim_core::text::char_col_to_byte(text, prefix_chars) as i64;
        }
        bytes += line_count_info(text, &mut words, &mut chars);
        let has_eol = !(last_line && !final_newline);
        if has_eol {
            bytes += eol_size;
            chars += 1;
        }
    }
    let mut dict = VimDict::new();
    dict.insert("words", Value::Number(words));
    dict.insert("chars", Value::Number(chars));
    dict.insert("bytes", Value::Number(bytes));
    dict.insert("cursor_bytes", Value::Number(cursor_bytes));
    dict.insert("cursor_chars", Value::Number(cursor_chars));
    dict.insert("cursor_words", Value::Number(cursor_words));
    Ok(Value::dict(dict))
}

fn f_indent(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let lnum = lnum_from_value(ed, &arg(args, 0));
    if lnum < 1 || lnum > ed.line_count() as i64 {
        return Ok(Value::Number(-1));
    }
    let text = ed
        .current_buffer()
        .line_str(lnum as usize - 1)
        .unwrap_or("")
        .to_string();
    let tabstop = opt_tabstop(ed);
    let mut width = 0;
    for ch in text.chars() {
        if !rvim_core::charclass::is_blank(ch) {
            break;
        }
        width += char_width(ch, width, tabstop);
    }
    Ok(Value::Number(width as i64))
}

// ---------------------------------------------------------------------------
// virtcol() / virtcol2col()
// ---------------------------------------------------------------------------

/// Parse `virtcol()`'s position argument.  Returns `(line, char index, off)`
/// for the current buffer, or `None` for an invalid position.
fn virtcol_position(ed: &Editor, value: &Value) -> Option<(usize, usize, i64)> {
    match value {
        Value::List(items) => {
            let items = items.borrow();
            if items.is_empty() {
                return None;
            }
            // Vim's var2fpos(): [lnum, col, coladd]; line numbers below 1 and
            // above the buffer size are invalid.
            let lnum = items[0].to_number();
            if lnum < 1 || lnum > ed.line_count() as i64 {
                return None;
            }
            let text = ed.current_buffer().line_str(lnum as usize - 1).ok()?;
            let char_len = text.chars().count();
            let col = match items.get(1) {
                Some(Value::String(text)) if text == "$" => char_len as i64 + 1,
                Some(other) => other.to_number(),
                None => 0,
            };
            if col == 0 || col > char_len as i64 + 1 {
                return None;
            }
            let off = items.get(2).map(Value::to_number).unwrap_or(0);
            Some((lnum as usize - 1, (col - 1).max(0) as usize, off))
        }
        Value::String(text) => match text.as_str() {
            "." | "v" => Some((ed.cursor().line, ed.cursor().col, 0)),
            "$" => Some((ed.cursor().line, usize::MAX, 0)),
            _ => {
                let mark = text.strip_prefix('\'')?;
                let name = mark.chars().next()?;
                let pos = ed.marks.resolve(ed.current_bufnr(), name)?;
                Some((pos.line, pos.col, 0))
            }
        },
        _ => None,
    }
}

fn f_virtcol(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let Some((line, col, off)) = virtcol_position(ed, &arg(args, 0)) else {
        return Ok(Value::Number(0));
    };
    let text = ed.current_buffer().line_str(line).unwrap_or("").to_string();
    let char_len = text.chars().count();
    let tabstop = opt_tabstop(ed);
    let (start, end) = if col == usize::MAX || col >= char_len {
        vcol_range(&text, char_len, tabstop, off)
    } else {
        vcol_range(&text, col, tabstop, off)
    };
    if barg(args, 1) {
        Ok(Value::list(vec![Value::Number(start), Value::Number(end)]))
    } else {
        Ok(Value::Number(end))
    }
}

fn f_virtcol2col(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let winid = narg(args, 0);
    if winid != 0 && win_by_id(ed, winid).is_none() {
        return Ok(Value::Number(-1));
    }
    let lnum = narg(args, 1);
    if lnum < 1 || lnum > ed.line_count() as i64 {
        return Ok(Value::Number(-1));
    }
    let vcol = narg(args, 2);
    if vcol < 0 {
        return Ok(Value::Number(-1));
    }
    let text = ed
        .current_buffer()
        .line_str(lnum as usize - 1)
        .unwrap_or("")
        .to_string();
    let tabstop = opt_tabstop(ed);
    // Vim's mouse.c:vcol2col(): advance whole characters while the counted
    // width is smaller than the requested virtual column and return the byte
    // offset after the last consumed character.
    let mut width = 0i64;
    let mut byte = 0usize;
    if width < vcol {
        for (offset, ch) in text.char_indices() {
            let cells = char_width(ch, width as usize, tabstop) as i64;
            width += cells;
            byte = offset + ch.len_utf8();
            if width >= vcol {
                break;
            }
        }
    }
    Ok(Value::Number(byte as i64))
}

// ---------------------------------------------------------------------------
// Mappings
// ---------------------------------------------------------------------------

/// A merged view of map entries that differ only in their mode, matching one
/// `:map` line / one Vim mapblock.
struct MapGroup<'a> {
    first: &'a MapEntry,
    modes: Vec<MapMode>,
}

fn map_groups(ed: &Editor) -> Vec<MapGroup<'_>> {
    let mut groups: Vec<MapGroup<'_>> = Vec::new();
    for entry in ed.maps.entries() {
        if let Some(group) = groups.iter_mut().find(|group| {
            group.first.lhs == entry.lhs
                && group.first.rhs == entry.rhs
                && group.first.opts == entry.opts
        }) {
            for mode in &entry.modes {
                if !group.modes.contains(mode) {
                    group.modes.push(*mode);
                }
            }
        } else {
            groups.push(MapGroup {
                first: entry,
                modes: entry.modes.clone(),
            });
        }
    }
    groups
}

/// Map a `{mode}` string argument (`maparg()`/`mapset()`) to editor modes.
/// `None` means the string contains an illegal mode character; `Some(vec![])`
/// means the mode is legal but not modelled (terminal mappings).
fn modes_from_arg(which: &str) -> Option<Vec<MapMode>> {
    let mut modes = Vec::new();
    let push = |modes: &mut Vec<MapMode>, mode: MapMode| {
        if !modes.contains(&mode) {
            modes.push(mode);
        }
    };
    if which.is_empty() {
        // Vim's `""` means Normal, Visual, Select and Operator-pending.
        for mode in [
            MapMode::Normal,
            MapMode::Visual,
            MapMode::VisualOnly,
            MapMode::Select,
            MapMode::Operator,
        ] {
            push(&mut modes, mode);
        }
        return Some(modes);
    }
    for ch in which.chars() {
        match ch {
            'n' => push(&mut modes, MapMode::Normal),
            'v' => {
                push(&mut modes, MapMode::Visual);
                push(&mut modes, MapMode::VisualOnly);
                push(&mut modes, MapMode::Select);
            }
            'x' => push(&mut modes, MapMode::VisualOnly),
            's' => push(&mut modes, MapMode::Select),
            'o' => push(&mut modes, MapMode::Operator),
            'i' => push(&mut modes, MapMode::Insert),
            'c' => push(&mut modes, MapMode::Cmdline),
            'l' => {
                push(&mut modes, MapMode::Insert);
                push(&mut modes, MapMode::Cmdline);
                push(&mut modes, MapMode::Language);
            }
            ' ' => {
                for mode in [
                    MapMode::Normal,
                    MapMode::Visual,
                    MapMode::VisualOnly,
                    MapMode::Select,
                    MapMode::Operator,
                ] {
                    push(&mut modes, mode);
                }
            }
            '!' => {
                push(&mut modes, MapMode::Insert);
                push(&mut modes, MapMode::Cmdline);
            }
            // Terminal mappings are a legal Vim mode we do not model.
            't' => {}
            _ => return None,
        }
    }
    Some(modes)
}

fn map_mode_bits(mode: MapMode) -> i64 {
    match mode {
        MapMode::Normal => 0x01,
        MapMode::Visual | MapMode::VisualOnly => 0x02,
        MapMode::Operator => 0x04,
        MapMode::Cmdline => 0x08,
        MapMode::Insert => 0x10,
        MapMode::Language => 0x20,
        MapMode::Select => 0x40,
    }
}

fn modes_bits(modes: &[MapMode]) -> i64 {
    modes
        .iter()
        .fold(0, |bits, mode| bits | map_mode_bits(*mode))
}

/// Vim's `map_mode_to_chars()` rendering for a set of modes.
fn modes_to_string(modes: &[MapMode]) -> String {
    let has = |mode: MapMode| modes.contains(&mode);
    let visual = has(MapMode::Visual) || has(MapMode::VisualOnly);
    let select = has(MapMode::Select);
    if has(MapMode::Insert) && has(MapMode::Cmdline) {
        return "!".to_string();
    }
    if has(MapMode::Insert) {
        return "i".to_string();
    }
    if has(MapMode::Language) {
        return "l".to_string();
    }
    if has(MapMode::Cmdline) {
        return "c".to_string();
    }
    if has(MapMode::Normal) && visual && select && has(MapMode::Operator) {
        return " ".to_string();
    }
    let mut text = String::new();
    if has(MapMode::Normal) {
        text.push('n');
    }
    if has(MapMode::Operator) {
        text.push('o');
    }
    if visual && select {
        text.push('v');
    } else {
        if visual {
            text.push('x');
        }
        if select {
            text.push('s');
        }
    }
    text
}

fn mapping_dict(group: &MapGroup<'_>) -> VimDict {
    let entry = group.first;
    let mut dict = VimDict::new();
    dict.insert("lhs", Value::string(entry.lhs_notation()));
    // Mappings are stored as parsed keys, not raw bytes: the raw form is the
    // canonical notation (documented deviation).
    dict.insert("lhsraw", Value::string(entry.lhs_notation()));
    dict.insert("rhs", Value::string(entry.rhs_raw.clone()));
    dict.insert("noremap", Value::Number(i64::from(!entry.opts.recursive)));
    dict.insert("script", Value::Number(i64::from(entry.opts.script)));
    dict.insert("expr", Value::Number(i64::from(entry.opts.expr)));
    dict.insert("silent", Value::Number(i64::from(entry.opts.silent)));
    dict.insert("nowait", Value::Number(i64::from(entry.opts.nowait)));
    dict.insert("buffer", Value::Number(i64::from(entry.opts.buffer)));
    dict.insert("sid", Value::Number(0));
    dict.insert("scriptversion", Value::Number(0));
    dict.insert("lnum", Value::Number(0));
    dict.insert("abbr", Value::Number(0));
    dict.insert("mode", Value::string(modes_to_string(&group.modes)));
    dict.insert("mode_bits", Value::Number(modes_bits(&group.modes)));
    dict
}

fn mapping_rhs_text(group: &MapGroup<'_>) -> String {
    if group.first.rhs.is_empty() {
        "<Nop>".to_string()
    } else {
        group.first.rhs_notation()
    }
}

/// Find a mapping group by parsed lhs and requested modes.  `exact` selects
/// `maparg()` (equal lhs) versus `mapcheck()` (prefix in either direction).
fn find_mapping<'a>(
    ed: &'a Editor,
    lhs: &str,
    modes: &[MapMode],
    exact: bool,
) -> Option<MapGroup<'a>> {
    let keys = rvim_editor::maps::parse_keys(lhs);
    let groups = map_groups(ed);
    // Buffer-local mappings are checked before global ones.
    let mut candidates: Vec<MapGroup<'a>> = groups
        .into_iter()
        .filter(|group| group.modes.iter().any(|mode| modes.contains(mode)))
        .filter(|group| {
            if exact {
                group.first.lhs == keys
            } else {
                group.first.lhs.starts_with(&keys) || keys.starts_with(&group.first.lhs)
            }
        })
        .collect();
    candidates.sort_by_key(|group| !group.first.opts.buffer);
    candidates.into_iter().next()
}

fn abbrev_modes_from_arg(which: &str) -> Option<Vec<MapMode>> {
    let modes = modes_from_arg(which)?;
    // Abbreviations only exist in Insert and Cmdline modes (Vim's MASK_BANG).
    if modes
        .iter()
        .any(|mode| !matches!(mode, MapMode::Insert | MapMode::Cmdline))
    {
        return None;
    }
    Some(modes)
}

fn abbrev_dict(ed: &Editor, index: usize) -> VimDict {
    let entry = &ed.abbrevs.entries()[index];
    let modes: Vec<MapMode> = entry
        .modes
        .iter()
        .map(|mode| match mode {
            rvim_editor::AbbrevMode::Insert => MapMode::Insert,
            rvim_editor::AbbrevMode::Cmdline => MapMode::Cmdline,
        })
        .collect();
    let mut dict = VimDict::new();
    dict.insert("lhs", Value::string(entry.lhs.clone()));
    dict.insert("lhsraw", Value::string(entry.lhs.clone()));
    dict.insert("rhs", Value::string(entry.rhs.clone()));
    dict.insert("noremap", Value::Number(i64::from(!entry.opts.recursive)));
    dict.insert("script", Value::Number(i64::from(entry.opts.script)));
    dict.insert("expr", Value::Number(i64::from(entry.opts.expr)));
    dict.insert("silent", Value::Number(i64::from(entry.opts.silent)));
    dict.insert("nowait", Value::Number(i64::from(entry.opts.nowait)));
    dict.insert("buffer", Value::Number(i64::from(entry.opts.buffer)));
    dict.insert("sid", Value::Number(0));
    dict.insert("scriptversion", Value::Number(0));
    dict.insert("lnum", Value::Number(0));
    dict.insert("abbr", Value::Number(1));
    dict.insert("mode", Value::string(modes_to_string(&modes)));
    dict.insert("mode_bits", Value::Number(modes_bits(&modes)));
    dict
}

fn f_maparg(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let lhs = sarg(args, 0);
    if lhs.is_empty() {
        return Ok(Value::string(""));
    }
    let which = opt_sarg(args, 1);
    let abbr = barg(args, 2);
    let want_dict = barg(args, 3);
    if abbr {
        let Some(modes) = abbrev_modes_from_arg(&which) else {
            return Ok(if want_dict {
                Value::dict(VimDict::new())
            } else {
                Value::string("")
            });
        };
        for (index, entry) in ed.abbrevs.entries().iter().enumerate() {
            let entry_modes: Vec<MapMode> = entry
                .modes
                .iter()
                .map(|mode| match mode {
                    rvim_editor::AbbrevMode::Insert => MapMode::Insert,
                    rvim_editor::AbbrevMode::Cmdline => MapMode::Cmdline,
                })
                .collect();
            if entry.lhs == lhs && entry_modes.iter().any(|mode| modes.contains(mode)) {
                if want_dict {
                    return Ok(Value::dict(abbrev_dict(ed, index)));
                }
                return Ok(Value::string(entry.rhs.clone()));
            }
        }
        return Ok(if want_dict {
            Value::dict(VimDict::new())
        } else {
            Value::string("")
        });
    }
    let Some(modes) = modes_from_arg(&which) else {
        return Ok(if want_dict {
            Value::dict(VimDict::new())
        } else {
            Value::string("")
        });
    };
    match find_mapping(ed, &lhs, &modes, true) {
        Some(group) => Ok(if want_dict {
            Value::dict(mapping_dict(&group))
        } else {
            Value::string(mapping_rhs_text(&group))
        }),
        None => Ok(if want_dict {
            Value::dict(VimDict::new())
        } else {
            Value::string("")
        }),
    }
}

fn f_mapcheck(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let lhs = sarg(args, 0);
    if lhs.is_empty() {
        return Ok(Value::string(""));
    }
    let which = opt_sarg(args, 1);
    let abbr = barg(args, 2);
    if abbr {
        let Some(modes) = abbrev_modes_from_arg(&which) else {
            return Ok(Value::string(""));
        };
        for entry in ed.abbrevs.entries() {
            let entry_modes: Vec<MapMode> = entry
                .modes
                .iter()
                .map(|mode| match mode {
                    rvim_editor::AbbrevMode::Insert => MapMode::Insert,
                    rvim_editor::AbbrevMode::Cmdline => MapMode::Cmdline,
                })
                .collect();
            let hit = entry.lhs.starts_with(&lhs) || lhs.starts_with(&entry.lhs);
            if hit && entry_modes.iter().any(|mode| modes.contains(mode)) {
                return Ok(Value::string(entry.rhs.clone()));
            }
        }
        return Ok(Value::string(""));
    }
    let Some(modes) = modes_from_arg(&which) else {
        return Ok(Value::string(""));
    };
    match find_mapping(ed, &lhs, &modes, false) {
        Some(group) => Ok(Value::string(mapping_rhs_text(&group))),
        None => Ok(Value::string("")),
    }
}

fn f_maplist(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let abbr = barg(args, 0);
    if abbr {
        let list = (0..ed.abbrevs.entries().len())
            .map(|index| Value::dict(abbrev_dict(ed, index)))
            .collect();
        return Ok(Value::list(list));
    }
    let list = map_groups(ed)
        .iter()
        .map(|group| Value::dict(mapping_dict(group)))
        .collect();
    Ok(Value::list(list))
}

fn f_mapset(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let dict_only = matches!(args.first(), Some(Value::Dict(_)));
    let (dict, which, abbr) = if dict_only {
        let dict = dict_clone(&arg(args, 0)).ok_or_else(|| err("E715", "Dictionary required"))?;
        let which = dict_string(&dict, "mode")
            .ok_or_else(|| err("E460", "Entries missing in mapset() dict argument"))?;
        let abbr = dict_number(&dict, "abbr")
            .ok_or_else(|| err("E460", "Entries missing in mapset() dict argument"))?;
        (dict, which, abbr != 0)
    } else {
        let which = sarg(args, 0);
        let abbr = barg(args, 1);
        let dict = dict_clone(&arg(args, 2)).ok_or_else(|| err("E715", "Dictionary required"))?;
        (dict, which, abbr)
    };
    let parsed_modes = if abbr {
        abbrev_modes_from_arg(&which)
    } else {
        modes_from_arg(&which)
    };
    let Some(modes) = parsed_modes else {
        return Err(err("E1276", format!("Illegal map mode string: '{which}'")));
    };
    if modes.is_empty() {
        // Only terminal mappings resolve to no modelled mode here.
        return Err(err(
            "E1276",
            format!("Illegal map mode string: '{which}' (terminal mappings are not modelled)"),
        ));
    }
    let lhs = dict_string(&dict, "lhs")
        .ok_or_else(|| err("E460", "Entries missing in mapset() dict argument"))?;
    // `lhsraw` is required by Vim but RustVim stores parsed keys; accept and
    // ignore it after validating its presence for compatibility.
    if dict.get("lhsraw").is_none() {
        return Err(err("E460", "Entries missing in mapset() dict argument"));
    }
    let rhs = dict_string(&dict, "rhs")
        .ok_or_else(|| err("E460", "Entries missing in mapset() dict argument"))?;
    let opts = MapOptions {
        recursive: dict_number(&dict, "noremap").unwrap_or(0) == 0,
        script: dict_number(&dict, "script").unwrap_or(0) != 0,
        expr: dict_number(&dict, "expr").unwrap_or(0) != 0,
        silent: dict_number(&dict, "silent").unwrap_or(0) != 0,
        nowait: dict_number(&dict, "nowait").unwrap_or(0) != 0,
        buffer: dict_number(&dict, "buffer").unwrap_or(0) != 0,
        ..MapOptions::default()
    };
    if abbr {
        let abbrev_modes: Vec<rvim_editor::AbbrevMode> = modes
            .iter()
            .filter_map(|mode| match mode {
                MapMode::Insert => Some(rvim_editor::AbbrevMode::Insert),
                MapMode::Cmdline => Some(rvim_editor::AbbrevMode::Cmdline),
                _ => None,
            })
            .collect();
        let _ = ed.abbrevs.unabbreviate(&lhs, &abbrev_modes);
        ed.abbrevs.abbreviate(&lhs, &rhs, &abbrev_modes, opts)?;
        return Ok(Value::Number(0));
    }
    let _ = ed.maps.unmap(&lhs, &modes);
    ed.maps.map(&lhs, &rhs, &modes, opts)?;
    Ok(Value::Number(0))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_editor::maps::MapOptions;

    type Fixture = (Editor, Env, ScriptState);

    fn setup() -> Fixture {
        (Editor::new(), Env::new(), ScriptState::default())
    }

    fn run(
        name: &str,
        args: Vec<Value>,
        ed: &mut Editor,
        env: &mut Env,
        state: &mut ScriptState,
    ) -> Value {
        call(name, &args, ed, env, state)
            .unwrap_or_else(|| panic!("{name} is not dispatched by host_win"))
            .unwrap_or_else(|error| panic!("{name} returned an error: {error}"))
    }

    fn run_err(
        name: &str,
        args: Vec<Value>,
        ed: &mut Editor,
        env: &mut Env,
        state: &mut ScriptState,
    ) -> VimError {
        match call(name, &args, ed, env, state) {
            Some(Err(error)) => error,
            Some(Ok(value)) => panic!("{name} unexpectedly returned {value:?}"),
            None => panic!("{name} is not dispatched by host_win"),
        }
    }

    fn num(value: i64) -> Value {
        Value::Number(value)
    }

    fn text(value: &str) -> Value {
        Value::string(value)
    }

    fn dget(value: &Value, key: &str) -> Value {
        match value {
            Value::Dict(dict) => dict
                .borrow()
                .get(key)
                .cloned()
                .unwrap_or_else(|| panic!("missing dict key {key}")),
            other => panic!("expected a dict, got {other:?}"),
        }
    }

    fn dnum(value: &Value, key: &str) -> i64 {
        dget(value, key).to_number()
    }

    fn dstr(value: &Value, key: &str) -> String {
        dget(value, key).to_string_value()
    }

    fn dlen(value: &Value) -> usize {
        match value {
            Value::Dict(dict) => dict.borrow().len(),
            other => panic!("expected a dict, got {other:?}"),
        }
    }

    fn lget(value: &Value, index: usize) -> Value {
        match value {
            Value::List(items) => items
                .borrow()
                .get(index)
                .cloned()
                .unwrap_or_else(|| panic!("missing list index {index}")),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    fn llen(value: &Value) -> usize {
        match value {
            Value::List(items) => items.borrow().len(),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    fn set_lines(ed: &mut Editor, lines: &[&str]) {
        let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        ed.replace_lines(0, ed.line_count(), lines).unwrap();
        ed.set_cursor(Pos::new(0, 0));
    }

    fn map_opts(recursive: bool) -> MapOptions {
        MapOptions {
            recursive,
            ..MapOptions::default()
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rvim_host_win_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn funcs_list_is_dispatched_and_matches_the_builtin_table() {
        let (mut ed, mut env, mut state) = setup();
        for name in FUNCS {
            assert!(
                rvim_viml::FUNCTIONS.iter().any(|def| def.name == *name),
                "{name} is not in the baseline builtin table"
            );
            let result = call(name, &[], &mut ed, &mut env, &mut state)
                .unwrap_or_else(|| panic!("{name} is listed in FUNCS but not dispatched"));
            match rvim_viml::FUNCTIONS.iter().find(|def| def.name == *name) {
                Some(def) if def.min_args > 0 => {
                    assert_eq!(
                        result.unwrap_err().code,
                        "E119",
                        "{name} should require arguments"
                    );
                }
                _ => assert!(result.is_ok(), "{name} should accept no arguments"),
            }
        }
        assert!(call("no_such_host_builtin", &[], &mut ed, &mut env, &mut state).is_none());
    }

    /// Run one Ex command through the real dispatcher (windows and tab pages
    /// are created the way a user creates them, not by poking the editor).
    fn run_ex(ed: &mut Editor, env: &mut Env, state: &mut ScriptState, command: &str) {
        crate::exec::run_line_with(ed, env, state, command).unwrap();
    }

    #[test]
    fn window_info_and_variables() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["one", "two", "three"]);
        let bufnr = ed.current_bufnr() as i64;
        let current = ed.current_win_id() as i64;

        let info = run("getwininfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&info), 1);
        let window = lget(&info, 0);
        assert_eq!(dnum(&window, "tabnr"), 1);
        assert_eq!(dnum(&window, "winnr"), 1);
        assert_eq!(dnum(&window, "winid"), current);
        assert_eq!(dnum(&window, "bufnr"), bufnr);
        // No layout pass yet: the size is unknown, the position is the default
        // top-left corner and the gutter width is already known.
        assert_eq!(dnum(&window, "height"), -1);
        assert_eq!(dnum(&window, "width"), -1);
        assert_eq!(dnum(&window, "winrow"), 1);
        assert_eq!(dnum(&window, "wincol"), 1);
        assert_eq!(dnum(&window, "textoff"), 0, "'number' is off");
        assert_eq!(dnum(&window, "topline"), 1);
        assert_eq!(dnum(&window, "botline"), 3);
        assert_eq!(dnum(&window, "loclist"), 0);
        assert_eq!(dnum(&window, "quickfix"), 0);
        assert_eq!(dnum(&window, "terminal"), 0);
        assert_eq!(dnum(&window, "winbar"), 0);
        assert!(matches!(dget(&window, "variables"), Value::Dict(_)));

        // After a layout pass the real geometry is reported; 'height' includes
        // the window's own status line, like vim's getwininfo().
        ed.layout_windows(23, 80);
        let info = run("getwininfo", vec![], &mut ed, &mut env, &mut state);
        let window = lget(&info, 0);
        // 'height' is the buffer area, like vim's get_win_info().
        assert_eq!(dnum(&window, "height"), 22);
        assert_eq!(dnum(&window, "width"), 80);
        assert_eq!(
            run("winheight", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            22
        );
        // 'number' adds the number gutter to textoff: one digit for a three line
        // buffer plus the separating space.
        ed.set_option("number", OptValue::Bool(true), SetScope::Local)
            .unwrap();
        let info = run("getwininfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&lget(&info, 0), "textoff"), 2);

        // getwininfo() takes a window ID (not a window number).
        assert_eq!(
            llen(&run(
                "getwininfo",
                vec![num(current)],
                &mut ed,
                &mut env,
                &mut state
            )),
            1
        );
        assert_eq!(
            llen(&run(
                "getwininfo",
                vec![num(2)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );

        // Window variables: current window, by number and by ID alike.
        assert_eq!(
            run(
                "setwinvar",
                vec![num(1), text("myvar"), num(42)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        for reference in [num(1), num(0), num(current)] {
            assert_eq!(
                run(
                    "getwinvar",
                    vec![reference, text("myvar")],
                    &mut ed,
                    &mut env,
                    &mut state
                )
                .to_number(),
                42
            );
        }
        assert_eq!(
            run(
                "getwinvar",
                vec![num(2), text("myvar")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            ""
        );
        assert_eq!(
            run(
                "getwinvar",
                vec![num(2), text("myvar"), text("fallback")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "fallback"
        );
        // Invalid window numbers are silently ignored by setwinvar().
        assert_eq!(
            run(
                "setwinvar",
                vec![num(2), text("myvar"), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        assert_eq!(
            run(
                "getwinvar",
                vec![num(1), text("myvar")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            42
        );

        // Options through '&name' and all variables through the empty name.
        assert_eq!(
            run(
                "getwinvar",
                vec![num(0), text("&list")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        run(
            "setwinvar",
            vec![num(0), text("&list"), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            run(
                "getwinvar",
                vec![num(0), text("&list")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );
        let vars = run(
            "getwinvar",
            vec![num(0), text("")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(dnum(&vars, "myvar"), 42);
        let options = run(
            "getwinvar",
            vec![num(0), text("&")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert!(matches!(dget(&options, "list"), Value::Number(_)));
        assert!(
            matches!(dget(&options, "number"), Value::Number(_)),
            "every window-local option is reported"
        );
    }

    #[test]
    fn window_position_and_geometry_are_honest() {
        let (mut ed, mut env, mut state) = setup();
        let pos = run("getwinpos", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&pos, 0).to_number(), -1);
        assert_eq!(lget(&pos, 1).to_number(), -1);
        assert_eq!(
            run("getwinposx", vec![], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        assert_eq!(
            run("getwinposy", vec![], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        // Without a layout pass there is no screen geometry.
        assert_eq!(
            run("wincol", vec![], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        assert_eq!(
            run("winline", vec![], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        assert_eq!(
            run("winheight", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        assert_eq!(
            run("winwidth", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        assert_eq!(
            run("winheight", vec![num(7)], &mut ed, &mut env, &mut state).to_number(),
            -1,
            "an invalid window has no size either"
        );
        let screenpos = run("win_screenpos", vec![num(1)], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&screenpos, 0).to_number(), 1);
        assert_eq!(lget(&screenpos, 1).to_number(), 1);
        let invalid = run("win_screenpos", vec![num(2)], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&invalid, 0).to_number(), 0);
        assert_eq!(lget(&invalid, 1).to_number(), 0);

        // One window is 22 text rows in a 23 row frame.
        set_lines(&mut ed, &["\tabc", "\u{1}xy", "plain"]);
        ed.layout_windows(23, 80);
        assert_eq!(
            run("winheight", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            22
        );
        assert_eq!(
            run("winwidth", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            80
        );
        // winline()/wincol() are 1-based positions inside the window; a tab
        // expands to 'tabstop' (8) and a control character takes two cells.
        ed.set_cursor(Pos::new(0, 1));
        assert_eq!(
            run("winline", vec![], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        assert_eq!(
            run("wincol", vec![], &mut ed, &mut env, &mut state).to_number(),
            9,
            "the cursor after a tab is in column 9"
        );
        ed.set_cursor(Pos::new(1, 1));
        assert_eq!(
            run("winline", vec![], &mut ed, &mut env, &mut state).to_number(),
            2
        );
        assert_eq!(
            run("wincol", vec![], &mut ed, &mut env, &mut state).to_number(),
            3,
            "one control character occupies two cells"
        );
        // The window's own viewport is taken into account.
        ed.windows.current_mut().top_line = 1;
        ed.windows.current_mut().left = 2;
        assert_eq!(
            run("winline", vec![], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        assert_eq!(
            run("wincol", vec![], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        ed.windows.current_mut().top_line = 0;
        ed.windows.current_mut().left = 0;

        // winrestcmd() always produces the command that restores the sizes.
        let command = run("winrestcmd", vec![], &mut ed, &mut env, &mut state).to_string_value();
        assert_eq!(command, "1resize 20|");
        // A single window has neither a vertical separator nor a status line to
        // drag: vim's -1 for "this window cannot be moved".
        assert_eq!(
            run(
                "win_move_separator",
                vec![num(1), num(5)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            -1
        );
        assert_eq!(
            run(
                "win_move_statusline",
                vec![num(1), num(5)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            -1
        );
        // With one window both arguments can only name the same window, which
        // is vim's E957.
        assert_eq!(
            run_err(
                "win_splitmove",
                vec![num(1), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )
            .code,
            "E957"
        );
        assert_eq!(
            run_err(
                "win_splitmove",
                vec![num(1), num(7)],
                &mut ed,
                &mut env,
                &mut state
            )
            .code,
            "E957"
        );
    }

    #[test]
    fn tab_info_and_variables() {
        let (mut ed, mut env, mut state) = setup();
        let bufnr = ed.current_bufnr() as i64;
        let current = ed.current_win_id() as i64;
        let info = run("gettabinfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&info), 1);
        let tab = lget(&info, 0);
        assert_eq!(dnum(&tab, "tabnr"), 1);
        let windows = dget(&tab, "windows");
        assert_eq!(llen(&windows), 1, "windows holds window dictionaries");
        assert_eq!(dnum(&lget(&windows, 0), "winid"), current);
        assert_eq!(dnum(&lget(&windows, 0), "winnr"), 1);
        assert_eq!(dnum(&lget(&windows, 0), "bufnr"), bufnr);
        assert_eq!(dnum(&tab, "winbar"), 0);
        assert!(matches!(dget(&tab, "variables"), Value::Dict(_)));
        assert_eq!(
            llen(&run(
                "gettabinfo",
                vec![num(2)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
        assert_eq!(
            llen(&run(
                "gettabinfo",
                vec![num(1)],
                &mut ed,
                &mut env,
                &mut state
            )),
            1
        );

        assert_eq!(
            run(
                "settabvar",
                vec![num(1), text("tv"), num(7)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        assert_eq!(
            run(
                "gettabvar",
                vec![num(1), text("tv")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            7
        );
        assert_eq!(
            run(
                "gettabvar",
                vec![num(0), text("tv")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            7
        );
        assert_eq!(
            run(
                "gettabvar",
                vec![num(2), text("tv"), text("def")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "def"
        );
        let vars = run(
            "gettabvar",
            vec![num(1), text("")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(dnum(&vars, "tv"), 7);
        run(
            "settabvar",
            vec![num(2), text("tv"), num(9)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            run(
                "gettabvar",
                vec![num(1), text("tv")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            7
        );
        // Tab pages have no local options: option-style names are illegal.
        assert_eq!(
            run_err(
                "settabvar",
                vec![num(1), text("&list"), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )
            .code,
            "E461"
        );
        assert_eq!(
            run_err(
                "settabvar",
                vec![num(1), text(""), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )
            .code,
            "E461"
        );

        // gettabwinvar()/settabwinvar() reach window variables of a window in a
        // tab page.
        run(
            "setwinvar",
            vec![num(1), text("wv"), num(11)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            run(
                "gettabwinvar",
                vec![num(1), num(1), text("wv")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            11
        );
        assert_eq!(
            run(
                "gettabwinvar",
                vec![num(2), num(1), text("wv"), text("d")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "d"
        );
        assert_eq!(
            run(
                "settabwinvar",
                vec![num(1), num(1), text("wv"), num(12)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        assert_eq!(
            run(
                "getwinvar",
                vec![num(1), text("wv")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            12
        );
        // An option of that window is reachable through gettabwinvar() as well.
        run(
            "settabwinvar",
            vec![num(1), num(1), text("&list"), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            run(
                "gettabwinvar",
                vec![num(1), num(1), text("&list")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );

        let buflist = run("tabpagebuflist", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&buflist), 1);
        assert_eq!(lget(&buflist, 0).to_number(), bufnr);
        assert_eq!(
            run(
                "tabpagebuflist",
                vec![num(2)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
    }

    #[test]
    fn tab_page_number_builtins() {
        let (mut ed, mut env, mut state) = setup();
        assert_eq!(
            run("tabpagenr", vec![], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        assert_eq!(
            run("tabpagenr", vec![text("$")], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        assert_eq!(
            run("tabpagenr", vec![text("#")], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        assert_eq!(
            run_err("tabpagenr", vec![text("x")], &mut ed, &mut env, &mut state).code,
            "E15"
        );
        assert_eq!(
            run("tabpagewinnr", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        assert_eq!(
            run(
                "tabpagewinnr",
                vec![num(1), text("$")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );
        assert_eq!(
            run(
                "tabpagewinnr",
                vec![num(1), text("#")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1,
            "the only window is also the previously used one"
        );
        assert_eq!(
            run("tabpagewinnr", vec![num(7)], &mut ed, &mut env, &mut state).to_number(),
            0
        );
        assert_eq!(
            run_err(
                "tabpagewinnr",
                vec![num(1), text("z")],
                &mut ed,
                &mut env,
                &mut state
            )
            .code,
            "E15"
        );
    }

    #[test]
    fn window_identity_builtins() {
        let (mut ed, mut env, mut state) = setup();
        let bufnr = ed.current_bufnr() as i64;
        let current = ed.current_win_id() as i64;
        assert_eq!(
            run("winbufnr", vec![num(0)], &mut ed, &mut env, &mut state).to_number(),
            bufnr
        );
        assert_eq!(
            run("winbufnr", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            bufnr
        );
        assert_eq!(
            run(
                "winbufnr",
                vec![num(current)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            bufnr,
            "winbufnr() takes a window ID too"
        );
        assert_eq!(
            run("winbufnr", vec![num(2)], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        assert_eq!(
            run("win_getid", vec![], &mut ed, &mut env, &mut state).to_number(),
            current
        );
        assert_eq!(
            run("win_getid", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            current
        );
        assert_eq!(
            run(
                "win_getid",
                vec![num(1), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            current
        );
        assert_eq!(
            run("win_getid", vec![num(2)], &mut ed, &mut env, &mut state).to_number(),
            0
        );
        assert_eq!(
            run(
                "win_getid",
                vec![num(1), num(2)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            -1,
            "an unknown tab page is -1"
        );
        assert_eq!(
            run("win_gettype", vec![], &mut ed, &mut env, &mut state).to_string_value(),
            ""
        );
        assert_eq!(
            run("win_gettype", vec![num(1)], &mut ed, &mut env, &mut state).to_string_value(),
            ""
        );
        assert_eq!(
            run("win_gettype", vec![num(2)], &mut ed, &mut env, &mut state).to_string_value(),
            "unknown"
        );
        assert_eq!(
            run(
                "win_gotoid",
                vec![num(current)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );
        assert_eq!(
            run("win_gotoid", vec![num(99)], &mut ed, &mut env, &mut state).to_number(),
            0
        );
        let tabwin = run(
            "win_id2tabwin",
            vec![num(current)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(lget(&tabwin, 0).to_number(), 1);
        assert_eq!(lget(&tabwin, 1).to_number(), 1);
        let invalid = run(
            "win_id2tabwin",
            vec![num(99)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(lget(&invalid, 0).to_number(), 0);
        assert_eq!(lget(&invalid, 1).to_number(), 0);
        assert_eq!(
            run(
                "win_id2win",
                vec![num(current)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );
        assert_eq!(
            run("win_id2win", vec![num(99)], &mut ed, &mut env, &mut state).to_number(),
            0
        );
        let found = run(
            "win_findbuf",
            vec![num(bufnr)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(llen(&found), 1);
        assert_eq!(lget(&found, 0).to_number(), current);
        assert_eq!(
            llen(&run(
                "win_findbuf",
                vec![num(999)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
        let layout = run("winlayout", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&layout, 0).to_string_value(), "leaf");
        assert_eq!(lget(&layout, 1).to_number(), current);
        assert_eq!(
            llen(&run(
                "winlayout",
                vec![num(2)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
    }

    #[test]
    fn split_creates_a_second_window_with_real_geometry() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["one", "two", "three"]);
        let first = ed.current_win_id();
        run_ex(&mut ed, &mut env, &mut state, "split");
        assert_eq!(ed.window_count(), 2);
        let second = ed.current_win_id();
        assert_ne!(second, first);
        // The new window is created above the old one and becomes current.
        let ids = ed.windows.ids();
        assert_eq!(ids, vec![second, first]);
        assert_eq!(
            run(
                "win_id2win",
                vec![num(ids[0] as i64)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );
        assert_eq!(
            run(
                "win_id2win",
                vec![num(ids[1] as i64)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            2
        );
        // Both windows are in getwininfo(), numbered 1 and 2.
        let info = run("getwininfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&info), 2);
        assert_eq!(dnum(&lget(&info, 0), "winnr"), 1);
        assert_eq!(dnum(&lget(&info, 0), "winid"), ids[0] as i64);
        assert_eq!(dnum(&lget(&info, 1), "winnr"), 2);
        assert_eq!(dnum(&lget(&info, 1), "winid"), ids[1] as i64);
        assert_eq!(
            dnum(&lget(&info, 1), "bufnr"),
            dnum(&lget(&info, 0), "bufnr")
        );
        // No layout yet: the sizes stay unknown.
        assert_eq!(
            run("winheight", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        ed.layout_windows(23, 80);
        let top = run("winheight", vec![num(1)], &mut ed, &mut env, &mut state).to_number();
        let bottom = run("winheight", vec![num(2)], &mut ed, &mut env, &mut state).to_number();
        assert!(top >= 1 && bottom >= 1, "both windows keep a text row");
        assert_eq!(
            top + bottom + 2,
            23,
            "two buffer areas plus two status lines fill 23 rows"
        );
        assert_eq!(
            run(
                "winheight",
                vec![num(second as i64)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            top,
            "a window ID names the same window as its number"
        );
        assert_eq!(
            run("winwidth", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            80
        );
        // The screen positions really differ now (and an ID names the same
        // window as its number).
        let top_pos = run("win_screenpos", vec![num(1)], &mut ed, &mut env, &mut state);
        let by_id = run(
            "win_screenpos",
            vec![num(second as i64)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(lget(&by_id, 0).to_number(), lget(&top_pos, 0).to_number());
        assert_eq!(lget(&by_id, 1).to_number(), lget(&top_pos, 1).to_number());
        let bottom_pos = run("win_screenpos", vec![num(2)], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&top_pos, 0).to_number(), 1);
        assert_eq!(lget(&top_pos, 1).to_number(), 1);
        assert_eq!(
            lget(&bottom_pos, 0).to_number(),
            top + 2,
            "the lower window starts after the upper status line"
        );
        assert_eq!(lget(&bottom_pos, 1).to_number(), 1);
        // winlayout() is a real "col" of two leaves.
        let layout = run("winlayout", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&layout, 0).to_string_value(), "col");
        let children = lget(&layout, 1);
        assert_eq!(llen(&children), 2);
        assert_eq!(lget(&lget(&children, 0), 0).to_string_value(), "leaf");
        assert_eq!(lget(&lget(&children, 0), 1).to_number(), ids[0] as i64);
        assert_eq!(lget(&lget(&children, 1), 0).to_string_value(), "leaf");
        assert_eq!(lget(&lget(&children, 1), 1).to_number(), ids[1] as i64);
        // gettabinfo() nests the same dictionaries, and each window has a real
        // botline of its own.
        let info = run("gettabinfo", vec![], &mut ed, &mut env, &mut state);
        let windows = dget(&lget(&info, 0), "windows");
        assert_eq!(llen(&windows), 2);
        assert_eq!(dnum(&lget(&windows, 1), "winid"), ids[1] as i64);
        assert_eq!(dnum(&lget(&windows, 0), "botline"), 3);
    }

    #[test]
    fn vertical_split_reports_a_row_layout() {
        let (mut ed, mut env, mut state) = setup();
        run_ex(&mut ed, &mut env, &mut state, "vsplit");
        assert_eq!(ed.window_count(), 2);
        let ids = ed.windows.ids();
        ed.layout_windows(23, 80);
        let left = run("winwidth", vec![num(1)], &mut ed, &mut env, &mut state).to_number();
        let right = run("winwidth", vec![num(2)], &mut ed, &mut env, &mut state).to_number();
        assert!(left >= 10 && right >= 10);
        assert_eq!(left + right + 1, 80, "one separator column between them");
        let layout = run("winlayout", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&layout, 0).to_string_value(), "row");
        let children = lget(&layout, 1);
        assert_eq!(llen(&children), 2);
        assert_eq!(lget(&lget(&children, 1), 1).to_number(), ids[1] as i64);
        let pos = run("win_screenpos", vec![num(2)], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&pos, 0).to_number(), 1);
        assert_eq!(lget(&pos, 1).to_number(), left + 2);
        // Both windows are as tall as the frame.
        assert_eq!(
            run("winheight", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            22
        );
    }

    #[test]
    fn win_splitmove_exchanges_the_two_windows() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["one", "two", "three"]);
        run_ex(&mut ed, &mut env, &mut state, "split");
        let ids = ed.windows.ids();
        let top = ids[0];
        let bottom = ids[1];
        // Give the current (upper) window its own buffer so the exchange shows.
        let second_buffer = ed.new_buffer();
        ed.switch_buffer(second_buffer).unwrap();
        let first_buffer = ed.windows.get(bottom).unwrap().bufnr;
        assert_ne!(first_buffer, second_buffer);
        assert_eq!(
            run(
                "win_splitmove",
                vec![num(top as i64), num(bottom as i64)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        assert_eq!(
            ed.windows.get(top).unwrap().bufnr,
            first_buffer,
            "the target's buffer moved into the source window"
        );
        assert_eq!(ed.windows.get(bottom).unwrap().bufnr, second_buffer);
        assert_eq!(ed.window_count(), 2, "the frame tree is untouched");
        // Both arguments naming the same window is vim's E957.
        assert_eq!(
            run_err(
                "win_splitmove",
                vec![num(top as i64), num(top as i64)],
                &mut ed,
                &mut env,
                &mut state
            )
            .code,
            "E957"
        );
    }

    #[test]
    fn window_variables_are_per_window() {
        let (mut ed, mut env, mut state) = setup();
        run_ex(&mut ed, &mut env, &mut state, "split");
        let ids = ed.windows.ids();
        let top = ids[0];
        let bottom = ids[1];
        assert_eq!(ed.current_win_id(), top, "the split window is current");
        run(
            "setwinvar",
            vec![num(1), text("mine"), text("top")],
            &mut ed,
            &mut env,
            &mut state,
        );
        run(
            "setwinvar",
            vec![num(2), text("mine"), text("bottom")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            run(
                "getwinvar",
                vec![num(1), text("mine")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "top"
        );
        assert_eq!(
            run(
                "getwinvar",
                vec![num(2), text("mine")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "bottom"
        );
        assert_eq!(
            run(
                "getwinvar",
                vec![num(bottom as i64), text("mine")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "bottom",
            "window IDs reach the same dictionary"
        );
        assert_eq!(
            run(
                "gettabwinvar",
                vec![num(1), num(2), text("mine")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "bottom"
        );
        // getwininfo() reports each window's own w: dictionary.
        let info = run("getwininfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dstr(&dget(&lget(&info, 0), "variables"), "mine"), "top");
        assert_eq!(dstr(&dget(&lget(&info, 1), "variables"), "mine"), "bottom");
        // The w: scope inside VimL follows the window as well.
        run(
            "win_execute",
            vec![num(bottom as i64), text("let g:seen = w:mine")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            env.globals.get("seen").map(Value::to_string_value),
            Some("bottom".to_string())
        );
        run(
            "win_execute",
            vec![num(top as i64), text("let g:seen = w:mine")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            env.globals.get("seen").map(Value::to_string_value),
            Some("top".to_string())
        );
    }

    #[test]
    fn win_gotoid_switches_the_current_window() {
        let (mut ed, mut env, mut state) = setup();
        run_ex(&mut ed, &mut env, &mut state, "split");
        let ids = ed.windows.ids();
        let top = ids[0];
        let bottom = ids[1];
        assert_eq!(ed.current_win_id(), top);
        assert_eq!(
            run(
                "win_gotoid",
                vec![num(bottom as i64)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );
        assert_eq!(ed.current_win_id(), bottom);
        assert_eq!(
            run(
                "win_id2win",
                vec![num(bottom as i64)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            2
        );
        let tabwin = run(
            "win_id2tabwin",
            vec![num(bottom as i64)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(lget(&tabwin, 0).to_number(), 1);
        assert_eq!(lget(&tabwin, 1).to_number(), 2);
        // A bogus id is reported as 0 and changes nothing.
        assert_eq!(
            run("win_gotoid", vec![num(4242)], &mut ed, &mut env, &mut state).to_number(),
            0
        );
        assert_eq!(ed.current_win_id(), bottom);
        // A window of another tab page has no number in the current one.
        let tab_id = ed.new_tabpage(None).unwrap();
        assert_eq!(tab_id, 2);
        let other_win = ed.current_win_id();
        ed.goto_tabpage(1).unwrap();
        assert_eq!(
            run(
                "win_id2win",
                vec![num(other_win as i64)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        let tabwin = run(
            "win_id2tabwin",
            vec![num(other_win as i64)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(lget(&tabwin, 0).to_number(), 2);
        assert_eq!(lget(&tabwin, 1).to_number(), 1);
        assert_eq!(
            run(
                "win_getid",
                vec![num(1), num(2)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            other_win as i64
        );
    }

    #[test]
    fn win_execute_switches_windows_and_restores_them() {
        let (mut ed, mut env, mut state) = setup();
        run_ex(&mut ed, &mut env, &mut state, "split");
        let ids = ed.windows.ids();
        let top = ids[0];
        let bottom = ids[1];
        let previous = ed.current_win_id();
        let other = if previous == top { bottom } else { top };
        assert_eq!(
            run(
                "win_execute",
                vec![num(other as i64), text("let g:exec_win = win_getid()")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            ""
        );
        assert_eq!(
            env.globals.get("exec_win").map(Value::to_number),
            Some(other as i64),
            "the command ran in the other window"
        );
        assert_eq!(ed.current_win_id(), previous, "the previous window is back");
        // A failing command aborts the rest of the list and is reported.
        let error = run_err(
            "win_execute",
            vec![
                num(other as i64),
                Value::list(vec![
                    text("let g:exec_first = 1"),
                    text("zzz_bogus_command"),
                    text("let g:exec_after = 1"),
                ]),
            ],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(error.code, "E492");
        assert_eq!(env.globals.get("exec_first").map(Value::to_number), Some(1));
        assert!(
            !env.globals.contains_key("exec_after"),
            "the commands after the failing one do not run"
        );
        assert_eq!(
            ed.current_win_id(),
            previous,
            "the window is still restored"
        );
    }

    #[test]
    fn second_tabpage_is_visible_to_the_tab_builtins() {
        let (mut ed, mut env, mut state) = setup();
        // Record the current tab page before switching (tabpagenr('#')).
        assert_eq!(
            run("tabpagenr", vec![], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        let first_window = ed.current_win_id();
        let first_buffer = ed.current_bufnr() as i64;
        assert_eq!(ed.new_tabpage(None).ok(), Some(2));
        assert_eq!(ed.tabpage_count(), 2);
        let second_window = ed.current_win_id();
        let second_buffer = ed.current_bufnr() as i64;
        assert_ne!(second_window, first_window);
        assert_eq!(
            run("tabpagenr", vec![], &mut ed, &mut env, &mut state).to_number(),
            2
        );
        assert_eq!(
            run("tabpagenr", vec![text("$")], &mut ed, &mut env, &mut state).to_number(),
            2
        );
        assert_eq!(
            run("tabpagenr", vec![text("#")], &mut ed, &mut env, &mut state).to_number(),
            1,
            "the tab page used before the switch"
        );
        // gettabinfo() has one dictionary per tab page, each with its own
        // window dictionaries.
        let info = run("gettabinfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&info), 2);
        let first = lget(&info, 0);
        let second = lget(&info, 1);
        assert_eq!(dnum(&first, "tabnr"), 1);
        assert_eq!(dnum(&second, "tabnr"), 2);
        for (tab, window) in [(&first, first_window), (&second, second_window)] {
            let windows = dget(tab, "windows");
            assert_eq!(llen(&windows), 1);
            assert_eq!(dnum(&lget(&windows, 0), "winid"), window as i64);
            assert_eq!(dnum(&lget(&windows, 0), "tabnr"), dnum(tab, "tabnr"));
        }
        assert_eq!(
            dnum(&lget(&dget(&first, "windows"), 0), "bufnr"),
            first_buffer
        );
        assert_eq!(
            dnum(&lget(&dget(&second, "windows"), 0), "bufnr"),
            second_buffer
        );
        assert_eq!(
            run("tabpagewinnr", vec![num(2)], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        assert_eq!(
            run(
                "tabpagewinnr",
                vec![num(1), text("$")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );
        let buffers = run(
            "tabpagebuflist",
            vec![num(2)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(llen(&buffers), 1);
        assert_eq!(lget(&buffers, 0).to_number(), second_buffer);
        // A window ID only resolves inside its own tab page.
        run(
            "settabwinvar",
            vec![num(2), num(second_window as i64), text("cross"), num(5)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            run(
                "gettabwinvar",
                vec![num(2), num(second_window as i64), text("cross")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            5
        );
        assert_eq!(
            run(
                "gettabwinvar",
                vec![
                    num(1),
                    num(second_window as i64),
                    text("cross"),
                    text("none")
                ],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "none",
            "the window belongs to the other tab page"
        );
        // win_findbuf() searches every tab page.
        let found = run(
            "win_findbuf",
            vec![num(second_buffer)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(llen(&found), 1);
        assert_eq!(lget(&found, 0).to_number(), second_window as i64);
        // t: is per tab page.
        run(
            "settabvar",
            vec![num(1), text("tv"), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        run(
            "settabvar",
            vec![num(2), text("tv"), num(2)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            run(
                "gettabvar",
                vec![num(1), text("tv")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            1
        );
        assert_eq!(
            run(
                "gettabvar",
                vec![num(2), text("tv")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            2
        );
        let info = run("gettabinfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&dget(&lget(&info, 0), "variables"), "tv"), 1);
        assert_eq!(dnum(&dget(&lget(&info, 1), "variables"), "tv"), 2);
        // getwininfo() covers every tab page as well.
        let info = run("getwininfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&info), 2);
        assert_eq!(dnum(&lget(&info, 1), "tabnr"), 2);
        // win_execute() reaches a window of another tab page and comes back.
        run(
            "win_execute",
            vec![
                num(first_window as i64),
                text("let g:tab_exec = tabpagenr()"),
            ],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(env.globals.get("tab_exec").map(Value::to_number), Some(1));
        assert_eq!(
            ed.windows.current_tab_index(),
            1,
            "the original tab page is current again"
        );
    }

    #[test]
    fn winrestcmd_reports_the_sizes_of_every_window() {
        let (mut ed, mut env, mut state) = setup();
        let command = run("winrestcmd", vec![], &mut ed, &mut env, &mut state).to_string_value();
        let height = ed.windows.current().height;
        assert_eq!(command, format!("1resize {height}|"));
        run_ex(&mut ed, &mut env, &mut state, "split");
        let ids = ed.windows.ids();
        let command = run("winrestcmd", vec![], &mut ed, &mut env, &mut state).to_string_value();
        let expected = format!(
            "1resize {}|2resize {}|",
            ed.windows.get(ids[0]).unwrap().height,
            ed.windows.get(ids[1]).unwrap().height
        );
        assert_eq!(command, expected);
        assert!(command.ends_with('|'));
        // A vertical split also restores the widths.
        run_ex(&mut ed, &mut env, &mut state, "vsplit");
        let command = run("winrestcmd", vec![], &mut ed, &mut env, &mut state).to_string_value();
        assert!(command.contains("vert 1resize "), "{command}");
        assert_eq!(
            command.matches("resize").count(),
            6,
            "three windows, each with a height and a width: {command}"
        );
        assert!(command.ends_with('|'));
    }

    #[test]
    fn win_execute_runs_commands_in_the_window() {
        let (mut ed, mut env, mut state) = setup();
        let current = ed.current_win_id() as i64;
        assert_eq!(
            run(
                "win_execute",
                vec![num(current), text("let g:win_exec_a = 41")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            ""
        );
        assert_eq!(
            env.globals.get("win_exec_a").map(Value::to_number),
            Some(41)
        );
        run(
            "win_execute",
            vec![
                num(current),
                Value::list(vec![
                    text("let g:win_exec_b = 1"),
                    text("let g:win_exec_c = 2"),
                ]),
            ],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(env.globals.get("win_exec_b").map(Value::to_number), Some(1));
        assert_eq!(env.globals.get("win_exec_c").map(Value::to_number), Some(2));
        // Unknown window IDs do not execute anything.
        run(
            "win_execute",
            vec![num(4242), text("let g:win_exec_d = 1")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert!(!env.globals.contains_key("win_exec_d"));
    }

    #[test]
    fn save_and_restore_view() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["alpha", "beta", "gamma", "delta"]);
        ed.set_cursor(Pos::new(1, 2));
        ed.windows.current_mut().top_line = 1;
        let view = run("winsaveview", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&view, "lnum"), 2);
        assert_eq!(dnum(&view, "col"), 2);
        assert_eq!(dnum(&view, "topline"), 2, "the window's own viewport");
        assert_eq!(dnum(&view, "leftcol"), 0);
        ed.set_cursor(Pos::new(0, 0));
        ed.windows.current_mut().top_line = 0;
        run("winrestview", vec![view], &mut ed, &mut env, &mut state);
        assert_eq!(ed.cursor().line, 1);
        assert_eq!(ed.cursor().col, 2);
        assert_eq!(
            ed.windows.current().top_line,
            1,
            "winrestview() restores the viewport"
        );
        // Saved views are per window: a split window has none of its own yet.
        let original = ed.current_win_id();
        run_ex(&mut ed, &mut env, &mut state, "split");
        let fresh = ed.current_win_id();
        assert_ne!(fresh, original);
        assert!(state.win.saved_views.contains_key(&original));
        assert!(!state.win.saved_views.contains_key(&fresh));
        let error = run_err("winrestview", vec![num(1)], &mut ed, &mut env, &mut state);
        assert_eq!(error.code, "E474");
    }

    #[test]
    fn tagfiles_and_taglist_read_a_real_tags_file() {
        let (mut ed, mut env, mut state) = setup();
        let dir = temp_dir("tags");
        std::fs::write(
            dir.join("tags"),
            "alpha\tfoo.txt\t/^alpha$/;\"\tf\tline:1\n\
             beta\tbar.txt\t2\n\
             !_TAG_FILE_FORMAT\t2\n",
        )
        .unwrap();
        ed.cwd = dir.clone();

        let files = run("tagfiles", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&files), 1);
        let names: Vec<String> = match &files {
            Value::List(items) => items.borrow().iter().map(Value::to_string_value).collect(),
            other => panic!("expected a list, got {other:?}"),
        };
        // The default 'tags' value lists the same file twice; it is reported
        // once (the first spelling found).
        assert!(
            names[0] == "tags" || names[0] == "./tags",
            "unexpected tagfiles() entry: {names:?}"
        );

        let tags = run(
            "taglist",
            vec![text("^alpha$")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(llen(&tags), 1);
        let tag = lget(&tags, 0);
        assert_eq!(dstr(&tag, "name"), "alpha");
        assert_eq!(dstr(&tag, "filename"), "foo.txt");
        assert_eq!(dstr(&tag, "cmd"), "/^alpha$/");
        assert_eq!(dstr(&tag, "kind"), "f");
        assert_eq!(dnum(&tag, "static"), 0);

        let tags = run("taglist", vec![text("beta")], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&tags), 1);
        assert_eq!(dstr(&lget(&tags, 0), "cmd"), "2");
        assert_eq!(
            llen(&run(
                "taglist",
                vec![text("nope")],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
        assert_eq!(
            llen(&run(
                "taglist",
                vec![text("")],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
        // The optional {filename} argument is accepted (priority is ignored).
        assert_eq!(
            llen(&run(
                "taglist",
                vec![text("alpha"), text("/tmp/foo.txt")],
                &mut ed,
                &mut env,
                &mut state
            )),
            1
        );

        let empty = temp_dir("tags_empty");
        ed.cwd = empty.clone();
        assert_eq!(
            llen(&run("tagfiles", vec![], &mut ed, &mut env, &mut state)),
            0
        );
        assert_eq!(
            llen(&run(
                "taglist",
                vec![text("alpha")],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn tag_stack_round_trips() {
        let (mut ed, mut env, mut state) = setup();
        let stack = run("gettagstack", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&stack, "length"), 0);
        assert_eq!(dnum(&stack, "curidx"), 1);
        assert_eq!(llen(&dget(&stack, "items")), 0);

        let mut item = VimDict::new();
        item.insert("tagname", text("main"));
        item.insert("matchnr", num(2));
        item.insert("bufnr", num(3));
        item.insert("from", Value::list(vec![num(3), num(10), num(5), num(0)]));
        let mut set = VimDict::new();
        set.insert("items", Value::list(vec![Value::dict(item)]));
        assert_eq!(
            run(
                "settagstack",
                vec![num(1), Value::dict(set.clone())],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        let stack = run("gettagstack", vec![num(1)], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&stack, "length"), 1);
        assert_eq!(dnum(&stack, "curidx"), 2);
        let items = dget(&stack, "items");
        let item = lget(&items, 0);
        assert_eq!(dstr(&item, "tagname"), "main");
        assert_eq!(dnum(&item, "matchnr"), 2);
        assert_eq!(dnum(&item, "bufnr"), 3);
        let from = dget(&item, "from");
        assert_eq!(lget(&from, 0).to_number(), 3);
        assert_eq!(lget(&from, 1).to_number(), 10);
        assert_eq!(lget(&from, 2).to_number(), 5);
        assert_eq!(lget(&from, 3).to_number(), 0);

        // 'a' appends another entry.
        run(
            "settagstack",
            vec![num(1), Value::dict(set), text("a")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            dnum(
                &run("gettagstack", vec![], &mut ed, &mut env, &mut state),
                "length"
            ),
            2
        );
        // 't' truncates down to the current index (curidx 2 => keep one).
        let mut truncate = VimDict::new();
        truncate.insert("curidx", num(2));
        run(
            "settagstack",
            vec![num(1), Value::dict(truncate), text("t")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(
            dnum(
                &run("gettagstack", vec![], &mut ed, &mut env, &mut state),
                "length"
            ),
            1
        );

        assert_eq!(
            run(
                "settagstack",
                vec![num(9), Value::dict(VimDict::new())],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            -1
        );
        assert_eq!(
            run_err(
                "settagstack",
                vec![num(1), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )
            .code,
            "E715"
        );
        assert_eq!(
            run_err(
                "settagstack",
                vec![num(1), Value::dict(VimDict::new()), text("z")],
                &mut ed,
                &mut env,
                &mut state
            )
            .code,
            "E962"
        );
        let empty = run("gettagstack", vec![num(9)], &mut ed, &mut env, &mut state);
        // An invalid window returns an empty dictionary (Vim's behaviour).
        assert_eq!(dlen(&empty), 0);
    }

    #[test]
    fn jump_list_uses_win_state() {
        let (mut ed, mut env, mut state) = setup();
        let empty = run("getjumplist", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&lget(&empty, 0)), 0);
        assert_eq!(lget(&empty, 1).to_number(), 0);
        state.win.jump_list.push(JumpEntry {
            bufnr: 7,
            lnum: 3,
            col: 2,
            filename: Some("file.txt".to_string()),
        });
        state.win.jump_list_idx = 1;
        let jumps = run("getjumplist", vec![], &mut ed, &mut env, &mut state);
        let entries = lget(&jumps, 0);
        assert_eq!(llen(&entries), 1);
        let entry = lget(&entries, 0);
        assert_eq!(dnum(&entry, "bufnr"), 7);
        assert_eq!(dnum(&entry, "lnum"), 3);
        assert_eq!(dnum(&entry, "col"), 2);
        assert_eq!(dstr(&entry, "filename"), "file.txt");
        assert_eq!(lget(&jumps, 1).to_number(), 1);
        assert_eq!(
            llen(&run(
                "getjumplist",
                vec![num(2)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
        assert_eq!(
            llen(&run(
                "getjumplist",
                vec![num(1), num(2)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
    }

    #[test]
    fn changelist_is_empty_and_honest() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["a", "b"]);
        ed.set_line_text(0, "A".to_string()).unwrap();
        let changes = run("getchangelist", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&changes), 2);
        assert_eq!(llen(&lget(&changes, 0)), 0);
        assert_eq!(lget(&changes, 1).to_number(), 0);
        assert_eq!(
            llen(&run(
                "getchangelist",
                vec![num(999)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
        assert_eq!(
            llen(&run(
                "getchangelist",
                vec![num(ed.current_bufnr() as i64)],
                &mut ed,
                &mut env,
                &mut state
            )),
            2
        );
    }

    #[test]
    fn mark_list_reports_global_and_local_marks() {
        let (mut ed, mut env, mut state) = setup();
        assert_eq!(
            llen(&run("getmarklist", vec![], &mut ed, &mut env, &mut state)),
            0
        );
        let bufnr = ed.current_bufnr();
        ed.marks.set_global('A', bufnr, Pos::new(2, 3));
        ed.marks.set_local(bufnr, 'a', Pos::new(0, 1));
        let global = run("getmarklist", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&global), 1);
        let mark = lget(&global, 0);
        assert_eq!(dstr(&mark, "mark"), "'A");
        let pos = dget(&mark, "pos");
        assert_eq!(lget(&pos, 0).to_number(), bufnr as i64);
        assert_eq!(lget(&pos, 1).to_number(), 3);
        assert_eq!(lget(&pos, 2).to_number(), 4);
        let local = run(
            "getmarklist",
            vec![num(bufnr as i64)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(llen(&local), 1);
        let mark = lget(&local, 0);
        assert_eq!(dstr(&mark, "mark"), "'a");
        let pos = dget(&mark, "pos");
        assert_eq!(lget(&pos, 1).to_number(), 1);
        assert_eq!(lget(&pos, 2).to_number(), 2);
        assert_eq!(
            llen(&run(
                "getmarklist",
                vec![num(999)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );
    }

    #[test]
    fn state_reports_the_editor_mode() {
        let (mut ed, mut env, mut state) = setup();
        assert_eq!(
            run("state", vec![], &mut ed, &mut env, &mut state).to_string_value(),
            "n"
        );
        assert_eq!(
            run("state", vec![text("s")], &mut ed, &mut env, &mut state).to_string_value(),
            ""
        );
        ed.mode = Mode::Insert;
        assert_eq!(
            run("state", vec![], &mut ed, &mut env, &mut state).to_string_value(),
            "i"
        );
        ed.mode = Mode::OperatorPending;
        assert_eq!(
            run("state", vec![], &mut ed, &mut env, &mut state).to_string_value(),
            "no"
        );
        assert_eq!(
            run("state", vec![text("o")], &mut ed, &mut env, &mut state).to_string_value(),
            "o"
        );
    }

    #[test]
    fn interrupt_returns_zero_without_claiming_loop_support() {
        let (mut ed, mut env, mut state) = setup();
        assert_eq!(
            run("interrupt", vec![], &mut ed, &mut env, &mut state).to_number(),
            0
        );
    }

    #[test]
    fn undotree_reports_the_linear_history() {
        let (mut ed, mut env, mut state) = setup();
        let tree = run("undotree", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&tree, "synced"), 1);
        assert_eq!(dnum(&tree, "seq_last"), 0);
        assert_eq!(dnum(&tree, "seq_cur"), 0);
        assert_eq!(llen(&dget(&tree, "entries")), 0);
        set_lines(&mut ed, &["one"]);
        ed.set_line_text(0, "two".to_string()).unwrap();
        let tree = run("undotree", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&tree, "seq_last"), 2);
        assert_eq!(dnum(&tree, "seq_cur"), 2);
        ed.undo().unwrap();
        let tree = run("undotree", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&tree, "seq_last"), 2);
        // Linear history exposes no head index; while redo is available the
        // documented conservative value is 0.
        assert_eq!(dnum(&tree, "seq_cur"), 0);
        assert_eq!(dnum(&tree, "time_cur"), 0);
        assert_eq!(dnum(&tree, "save_last"), 0);
    }

    #[test]
    fn wordcount_counts_bytes_chars_and_words() {
        let (mut ed, mut env, mut state) = setup();
        // Pin 'fileformat' so the byte counts do not depend on the platform
        // default (dos on Windows, unix elsewhere).
        ed.current_buffer_mut()
            .set_fileformat(rvim_buffer::FileFormat::Unix);
        set_lines(&mut ed, &["hello world", "foo"]);
        let counts = run("wordcount", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&counts, "words"), 3);
        assert_eq!(dnum(&counts, "chars"), 16);
        assert_eq!(dnum(&counts, "bytes"), 16);
        // Cursor on the first column of the first line: one word/char/byte.
        assert_eq!(dnum(&counts, "cursor_words"), 1);
        assert_eq!(dnum(&counts, "cursor_chars"), 1);
        assert_eq!(dnum(&counts, "cursor_bytes"), 1);
        ed.set_cursor(Pos::new(1, 0));
        let counts = run("wordcount", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&counts, "cursor_words"), 3);
        assert_eq!(dnum(&counts, "cursor_chars"), 13);
        assert_eq!(dnum(&counts, "cursor_bytes"), 13);

        let (mut ed, mut env, mut state) = setup();
        ed.current_buffer_mut()
            .set_fileformat(rvim_buffer::FileFormat::Unix);
        let counts = run("wordcount", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&counts, "words"), 0);
        assert_eq!(dnum(&counts, "chars"), 1);
        assert_eq!(dnum(&counts, "bytes"), 1);
    }

    #[test]
    fn indent_and_lispindent_measure_leading_whitespace() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["    abc", "\tfoo", ""]);
        assert_eq!(
            run("indent", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            4
        );
        // 'tabstop' default is 8.
        assert_eq!(
            run("indent", vec![num(2)], &mut ed, &mut env, &mut state).to_number(),
            8
        );
        assert_eq!(
            run("indent", vec![num(3)], &mut ed, &mut env, &mut state).to_number(),
            0
        );
        assert_eq!(
            run("indent", vec![text(".")], &mut ed, &mut env, &mut state).to_number(),
            4
        );
        assert_eq!(
            run("indent", vec![num(9)], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        assert_eq!(
            run("lispindent", vec![num(2)], &mut ed, &mut env, &mut state).to_number(),
            8
        );
        assert_eq!(
            run("lispindent", vec![num(9)], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
    }

    #[test]
    fn virtcol_measures_screen_columns() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["abc"]);
        assert_eq!(
            run("virtcol", vec![text(".")], &mut ed, &mut env, &mut state).to_number(),
            1
        );
        let range = run(
            "virtcol",
            vec![text("."), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(lget(&range, 0).to_number(), 1);
        assert_eq!(lget(&range, 1).to_number(), 1);
        // '$' is the number of displayed characters plus one.
        assert_eq!(
            run("virtcol", vec![text("$")], &mut ed, &mut env, &mut state).to_number(),
            4
        );
        ed.set_cursor(Pos::new(0, 1));
        assert_eq!(
            run("virtcol", vec![text(".")], &mut ed, &mut env, &mut state).to_number(),
            2
        );
        assert_eq!(
            run(
                "virtcol",
                vec![Value::list(vec![num(1), num(2)])],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            2
        );
        assert_eq!(
            run(
                "virtcol",
                vec![Value::list(vec![num(1), text("$")])],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            4
        );
        // A <Tab> occupies the whole next tabstop; its last column is reported.
        set_lines(&mut ed, &["\tab"]);
        ed.set_cursor(Pos::new(0, 1));
        assert_eq!(
            run("virtcol", vec![text(".")], &mut ed, &mut env, &mut state).to_number(),
            9
        );
        let range = run(
            "virtcol",
            vec![Value::list(vec![num(1), num(1)]), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(lget(&range, 0).to_number(), 1);
        assert_eq!(lget(&range, 1).to_number(), 8);
        assert_eq!(
            run("virtcol", vec![text("$")], &mut ed, &mut env, &mut state).to_number(),
            // <Tab> counts as 8 cells and "ab" as two more, plus one.
            11
        );
        // Unset marks and out-of-range positions report 0.
        assert_eq!(
            run("virtcol", vec![text("'z")], &mut ed, &mut env, &mut state).to_number(),
            0
        );
        assert_eq!(
            run(
                "virtcol",
                vec![Value::list(vec![num(9), num(1)])],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
    }

    #[test]
    fn virtcol2col_matches_vims_mouse_column_algorithm() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["a\tb\tc"]);
        for (vcol, byte) in [(1, 1), (2, 2), (8, 2), (9, 3), (10, 4), (16, 4), (17, 5)] {
            assert_eq!(
                run(
                    "virtcol2col",
                    vec![num(0), num(1), num(vcol)],
                    &mut ed,
                    &mut env,
                    &mut state
                )
                .to_number(),
                byte,
                "virtcol2col(0, 1, {vcol})"
            );
        }
        assert_eq!(
            run(
                "virtcol2col",
                vec![num(0), num(1), num(20)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            5
        );
        for (winid, lnum, col) in [(10, 1, 1), (0, 10, 1), (0, -1, 1), (0, 1, -1)] {
            assert_eq!(
                run(
                    "virtcol2col",
                    vec![num(winid), num(lnum), num(col)],
                    &mut ed,
                    &mut env,
                    &mut state
                )
                .to_number(),
                -1
            );
        }
    }

    #[test]
    fn maparg_mapcheck_maplist_and_mapset() {
        let (mut ed, mut env, mut state) = setup();
        ed.maps
            .map("x", "y", &[MapMode::Normal], map_opts(false))
            .unwrap();
        assert_eq!(
            run("maparg", vec![text("x")], &mut ed, &mut env, &mut state).to_string_value(),
            "y"
        );
        assert_eq!(
            run(
                "maparg",
                vec![text("x"), text("n")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "y"
        );
        assert_eq!(
            run("maparg", vec![text("q")], &mut ed, &mut env, &mut state).to_string_value(),
            ""
        );
        let dict = run(
            "maparg",
            vec![text("x"), text("n"), num(0), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(dstr(&dict, "lhs"), "x");
        assert_eq!(dstr(&dict, "rhs"), "y");
        assert_eq!(dnum(&dict, "noremap"), 1);
        assert_eq!(dnum(&dict, "abbr"), 0);
        assert_eq!(dstr(&dict, "mode"), "n");
        assert_eq!(dnum(&dict, "mode_bits"), 0x01);
        assert_eq!(
            dlen(&run(
                "maparg",
                vec![text("q"), text("n"), num(0), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )),
            0
        );

        // mapcheck() matches a prefix in either direction.
        let (mut ed, mut env, mut state) = setup();
        ed.maps
            .map("ab", "Z", &[MapMode::Normal], map_opts(false))
            .unwrap();
        assert_eq!(
            run("mapcheck", vec![text("a")], &mut ed, &mut env, &mut state).to_string_value(),
            "Z"
        );
        assert_eq!(
            run("mapcheck", vec![text("abc")], &mut ed, &mut env, &mut state).to_string_value(),
            "Z"
        );
        assert_eq!(
            run("mapcheck", vec![text("b")], &mut ed, &mut env, &mut state).to_string_value(),
            ""
        );
        assert_eq!(
            run("mapcheck", vec![text("ab")], &mut ed, &mut env, &mut state).to_string_value(),
            "Z"
        );

        // maplist() merges the per-mode entries of one :map into one dict.
        let (mut ed, mut env, mut state) = setup();
        ed.maps
            .map(
                "x",
                "y",
                &[
                    MapMode::Normal,
                    MapMode::Visual,
                    MapMode::VisualOnly,
                    MapMode::Select,
                    MapMode::Operator,
                ],
                map_opts(true),
            )
            .unwrap();
        let maps = run("maplist", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&maps), 1);
        let map = lget(&maps, 0);
        assert_eq!(dnum(&map, "mode_bits"), 0x47);
        assert_eq!(dstr(&map, "mode"), " ");
        assert_eq!(dnum(&map, "noremap"), 0);

        // mapset() restores a mapping from a maparg() dictionary.
        let (mut ed, mut env, mut state) = setup();
        ed.maps
            .map("K", "hello", &[MapMode::Normal], map_opts(false))
            .unwrap();
        let saved = run(
            "maparg",
            vec![text("K"), text("n"), num(0), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        ed.maps.unmap("K", &[MapMode::Normal]).unwrap();
        assert_eq!(
            run(
                "maparg",
                vec![text("K"), text("n")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            ""
        );
        assert_eq!(
            run(
                "mapset",
                vec![text("n"), num(0), saved.clone()],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
        );
        assert_eq!(
            run(
                "maparg",
                vec![text("K"), text("n")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "hello"
        );
        // The dictionary-only form takes mode/abbr from the dict itself.
        ed.maps.unmap("K", &[MapMode::Normal]).unwrap();
        run("mapset", vec![saved], &mut ed, &mut env, &mut state);
        assert_eq!(
            run(
                "maparg",
                vec![text("K"), text("n")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "hello"
        );
        assert_eq!(
            run_err("mapset", vec![text("n")], &mut ed, &mut env, &mut state).code,
            "E715"
        );

        // Abbreviations are visible through the same functions with {abbr}=1.
        let (mut ed, mut env, mut state) = setup();
        ed.abbrevs
            .abbreviate(
                "teh",
                "the",
                &[rvim_editor::AbbrevMode::Insert],
                map_opts(false),
            )
            .unwrap();
        assert_eq!(
            run(
                "maparg",
                vec![text("teh"), text("i"), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "the"
        );
        let abbrevs = run("maplist", vec![num(1)], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&abbrevs), 1);
        assert_eq!(dnum(&lget(&abbrevs, 0), "abbr"), 1);
        let saved = run(
            "maparg",
            vec![text("teh"), text("i"), num(1), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        ed.abbrevs
            .unabbreviate("teh", &[rvim_editor::AbbrevMode::Insert])
            .unwrap();
        run("mapset", vec![saved], &mut ed, &mut env, &mut state);
        assert_eq!(
            run(
                "maparg",
                vec![text("teh"), text("i"), num(1)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_string_value(),
            "the"
        );
    }

    #[test]
    fn builtins_are_reachable_through_viml_expressions() {
        let mut ed = Editor::new();
        let mut ctx = crate::script::Ctx::new();
        let current = ed.current_win_id() as i64;
        let value =
            crate::script::eval_expr(&mut ed, &mut ctx.env, &mut ctx.state, "win_getid()").unwrap();
        assert_eq!(value.to_number(), current);
        let value = crate::script::eval_expr(
            &mut ed,
            &mut ctx.env,
            &mut ctx.state,
            "getwininfo()[0].winid",
        )
        .unwrap();
        assert_eq!(value.to_number(), current);
        let value =
            crate::script::eval_expr(&mut ed, &mut ctx.env, &mut ctx.state, "tabpagebuflist()")
                .unwrap();
        assert_eq!(llen(&value), 1);
        let value =
            crate::script::eval_expr(&mut ed, &mut ctx.env, &mut ctx.state, "indent(1)").unwrap();
        assert_eq!(value.to_number(), 0);
    }
}
