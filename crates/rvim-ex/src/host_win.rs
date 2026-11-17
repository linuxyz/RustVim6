//! Host builtins for the single-window/tab model, tags, mappings, screen
//! geometry, undo/state introspection and the remaining window/tab/misc
//! builtins of the M3 batch.
//!
//! # Model and honest limitations
//!
//! RustVim (M1-M3) has exactly one window and one tab page, no TUI and no
//! display layer.  These builtins describe that model honestly instead of
//! pretending a multi-window editor exists:
//!
//! * window numbers **and** window IDs are both `1`; `0` means "current
//!   window" wherever Vim accepts a window number.
//! * `getwininfo()` / `gettabinfo()` describe that single window/tab, and
//!   `winlayout()` is the leaf layout `["leaf", 1]`.
//! * screen/terminal geometry is unknown: `winheight()`, `winwidth()`,
//!   `wincol()` and `winline()` return `-1` and the geometry entries of
//!   `getwininfo()` (`height`, `width`, `textoff`) are `-1`; the logical
//!   viewport always starts at line 1 because no scroll layer exists yet.
//!   `win_screenpos()` returns `[1, 1]` (the only window is anchored at the
//!   logical top-left corner) and `getwinpos()`/`getwinposx()`/`getwinposy()`
//!   use Vim's own no-display fallback of `-1`.
//! * `winrestcmd()` returns an empty string (there is no layout to restore),
//!   `win_move_separator()`/`win_move_statusline()` return `0` (there is no
//!   separator/status line to drag) and `win_splitmove()` always fails with
//!   `E957` because its two window arguments can only name the one window.
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
//! * the jump list and tag stack live in [`WinState`].  Nothing records jumps
//!   yet, but `settagstack()`/`gettagstack()` are fully functional.
//! * `lispindent()` computes leading whitespace columns like `indent()`; the
//!   real Lisp indenter is not implemented.
//! * `virtcol()`/`virtcol2col()` use a simple display width: tabs expand with
//!   'tabstop', control characters occupy two cells and every other character
//!   one cell (std has no `wcwidth`, so double-width characters are counted as
//!   one).
//! * `winsaveview()`/`winrestview()` store character columns (RustVim's cursor
//!   columns are character based) and there is no scroll offset to save.
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
use rvim_editor::{Editor, MapEntry, MapMode, MapOptions, Mode, OptValue};
use rvim_viml::Env;
use rvim_viml::value::{Value, VimDict};

use crate::script::ScriptState;

/// The single window number.
const WINNR: i64 = 1;
/// The single window ID.  M1 window IDs are small (1) instead of Vim's
/// >= 1000 range; every accessor accepts `0` for "current window" as well.
const WINID: i64 = 1;

/// A saved window view (`winsaveview()`); cursor columns are character based.
#[derive(Debug, Clone, Default)]
pub struct SavedView {
    pub lnum: i64,
    pub col: i64,
    pub curswant: i64,
    /// Logical first displayed line; the M1 viewport never scrolls.
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

/// State for the (single) window/tab model: saved views, jump list, tag stack.
#[derive(Debug, Default)]
pub struct WinState {
    /// Cursor/scroll state saved by `winsaveview()`.
    pub saved_view: Option<SavedView>,
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
        "tabpagenr" => f_tabpagenr(args),
        "tabpagewinnr" => f_tabpagewinnr(args),
        "tabpagebuflist" => f_tabpagebuflist(args, ed),
        // -------------------------------------------------- window identity
        "winbufnr" => Ok(Value::Number(if win_ref_valid(narg(args, 0)) {
            ed.current_bufnr() as i64
        } else {
            -1
        })),
        "wincol" | "winline" => Ok(Value::Number(-1)),
        "winheight" | "winwidth" => Ok(Value::Number(-1)),
        "win_execute" => f_win_execute(args, ed, env, state),
        "win_findbuf" => f_win_findbuf(args, ed),
        "win_getid" => f_win_getid(args),
        "win_gettype" => f_win_gettype(args),
        "win_gotoid" => Ok(Value::Number(i64::from(narg(args, 0) == WINID))),
        "win_id2tabwin" => Ok(Value::list(vec![
            Value::Number(if narg(args, 0) == WINID { 1 } else { 0 }),
            Value::Number(if narg(args, 0) == WINID { 1 } else { 0 }),
        ])),
        "win_id2win" => Ok(Value::Number(if narg(args, 0) == WINID {
            WINNR
        } else {
            0
        })),
        "winlayout" => f_winlayout(args),
        "winrestcmd" => Ok(Value::string("")),
        "winrestview" => f_winrestview(args, ed, state),
        "winsaveview" => f_winsaveview(ed, state),
        "win_screenpos" => Ok(Value::list(vec![
            Value::Number(if win_ref_valid(narg(args, 0)) { 1 } else { 0 }),
            Value::Number(if win_ref_valid(narg(args, 0)) { 1 } else { 0 }),
        ])),
        "win_move_separator" | "win_move_statusline" => {
            // There is no window layout to move: report failure without lying
            // about a successful resize.
            Ok(Value::Number(0))
        }
        "win_splitmove" => Err(err(
            "E957",
            format!(
                "Invalid window number: RustVim has a single window, so {} cannot be split-moved",
                WINID
            ),
        )),
        // ------------------------------------------------------------ tags
        "tagfiles" => Ok(Value::list(
            tag_files(ed)
                .into_iter()
                .map(|(name, _)| Value::string(name))
                .collect(),
        )),
        "taglist" => f_taglist(args, ed),
        "gettagstack" => f_gettagstack(args, state),
        "settagstack" => f_settagstack(args, state),
        // -------------------------------------------------- jump/changelist
        "getjumplist" => f_getjumplist(args, state),
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

/// True for the values Vim's `find_win_by_nr_or_id()` accepts: `0` (current)
/// or the single window's number/id.
fn win_ref_valid(nr: i64) -> bool {
    nr == 0 || nr == WINNR
}

/// True for the values Vim's `win_id2wp()` accepts: the single window's ID.
fn win_id_valid(id: i64) -> bool {
    id == WINID
}

/// True for a valid tab page number (`0` means the current tab page).
fn tab_ref_valid(nr: i64) -> bool {
    nr == 0 || nr == 1
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

/// The window-local options modelled by the M1 option table.  RustVim stores
/// every option globally, but with a single window the window-local value is
/// exactly the stored value, so exposing these is honest.
const WINDOW_LOCAL_OPTIONS: &[&str] = &["foldenable", "list", "number", "relativenumber", "wrap"];

fn window_option_dict(ed: &Editor) -> VimDict {
    let mut dict = VimDict::new();
    for name in WINDOW_LOCAL_OPTIONS {
        if let Ok(value) = ed.options.get(name) {
            dict.insert(*name, opt_value_to_value(value));
        }
    }
    dict
}

/// `get_var_from()` semantics: `&opt` reads an option, an empty name returns
/// all variables, anything else is a variable lookup with a default.
fn get_scoped_var(
    ed: &Editor,
    vars: &HashMap<String, Value>,
    name: &str,
    default: Value,
    window_options: bool,
) -> Value {
    if window_options {
        if let Some(option) = name.strip_prefix('&') {
            if option.is_empty() {
                return Value::dict(window_option_dict(ed));
            }
            return match ed.options.get(option) {
                Ok(value) => opt_value_to_value(value),
                Err(_) => default,
            };
        }
    }
    if name.is_empty() {
        return Value::dict(vars_to_dict(vars));
    }
    vars.get(name).cloned().unwrap_or(default)
}

/// `setwinvar()`/`settabvar()` semantics.
fn set_scoped_var(
    ed: &mut Editor,
    vars: &mut HashMap<String, Value>,
    name: &str,
    value: Value,
) -> VimResult<()> {
    if let Some(option) = name.strip_prefix('&') {
        if option.is_empty() {
            return Err(err("E518", "Unknown option: "));
        }
        // Coerce to the option's own type, like Vim's `set_option_from_tv()`;
        // Options::set() itself parses strings for Bool/Num options.
        let coerced = match ed.options.get(option) {
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
        return ed.options.set(option, coerced);
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
    if !win_ref_valid(nr) {
        return Ok(default);
    }
    Ok(get_scoped_var(ed, &state.winvars, &name, default, true))
}

fn f_setwinvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let nr = narg(args, 0);
    let name = sarg(args, 1);
    let value = arg(args, 2);
    if win_ref_valid(nr) {
        set_scoped_var(ed, &mut state.winvars, &name, value)?;
    }
    Ok(Value::Number(0))
}

fn f_gettabvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let nr = narg(args, 0);
    let name = sarg(args, 1);
    let default = args.get(2).cloned().unwrap_or_else(|| Value::string(""));
    if !tab_ref_valid(nr) {
        return Ok(default);
    }
    // Tab pages have no local options, so `&name` is a plain variable lookup.
    Ok(get_scoped_var(ed, &state.tabvars, &name, default, false))
}

fn f_settabvar(args: &[Value], _ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let nr = narg(args, 0);
    let name = sarg(args, 1);
    let value = arg(args, 2);
    if tab_ref_valid(nr) {
        // Tab pages have no local options, and Vim writes `t:{varname}`; an
        // empty name or an option-style name is therefore illegal.
        if name.is_empty() || name.starts_with('&') {
            return Err(err("E461", format!("Illegal variable name: {name}")));
        }
        state.tabvars.insert(name, value);
    }
    Ok(Value::Number(0))
}

fn f_gettabwinvar(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let tabnr = narg(args, 0);
    let winnr = narg(args, 1);
    let name = sarg(args, 2);
    let default = args.get(3).cloned().unwrap_or_else(|| Value::string(""));
    if !tab_ref_valid(tabnr) || !win_ref_valid(winnr) {
        return Ok(default);
    }
    Ok(get_scoped_var(ed, &state.winvars, &name, default, true))
}

// ---------------------------------------------------------------------------
// Window/tab information
// ---------------------------------------------------------------------------

fn win_info_dict(ed: &Editor, state: &ScriptState) -> VimDict {
    let mut dict = VimDict::new();
    dict.insert("tabnr", Value::Number(1));
    dict.insert("winnr", Value::Number(WINNR));
    dict.insert("winid", Value::Number(WINID));
    // No display/terminal: sizes are unknown, the logical viewport starts at
    // line 1 and shows the whole (never scrolled) buffer.
    dict.insert("height", Value::Number(-1));
    dict.insert("width", Value::Number(-1));
    dict.insert("winrow", Value::Number(1));
    dict.insert("wincol", Value::Number(1));
    dict.insert("textoff", Value::Number(-1));
    dict.insert("topline", Value::Number(1));
    dict.insert("botline", Value::Number(ed.line_count() as i64));
    dict.insert("bufnr", Value::Number(ed.current_bufnr() as i64));
    dict.insert("winbar", Value::Number(0));
    dict.insert("terminal", Value::Number(0));
    dict.insert("quickfix", Value::Number(0));
    dict.insert("loclist", Value::Number(0));
    dict.insert("variables", Value::dict(vars_to_dict(&state.winvars)));
    dict
}

fn f_getwininfo(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    if let Some(value) = args.first() {
        if !win_id_valid(value.to_number()) {
            return Ok(Value::list(Vec::new()));
        }
    }
    Ok(Value::list(vec![Value::dict(win_info_dict(ed, state))]))
}

fn f_gettabinfo(args: &[Value], _ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    if let Some(value) = args.first() {
        if !tab_ref_valid(value.to_number()) {
            return Ok(Value::list(Vec::new()));
        }
    }
    let mut dict = VimDict::new();
    dict.insert("tabnr", Value::Number(1));
    dict.insert("windows", Value::list(vec![Value::Number(WINID)]));
    dict.insert("variables", Value::dict(vars_to_dict(&state.tabvars)));
    Ok(Value::list(vec![Value::dict(dict)]))
}

fn f_tabpagenr(args: &[Value]) -> VimResult<Value> {
    let Some(value) = args.first() else {
        return Ok(Value::Number(1));
    };
    match value.to_string_value().as_str() {
        "$" => Ok(Value::Number(1)),
        // The last accessed tab page is the only tab page.
        "#" => Ok(Value::Number(1)),
        other => Err(err("E15", format!("Invalid expression: {other}"))),
    }
}

fn f_tabpagewinnr(args: &[Value]) -> VimResult<Value> {
    let tabnr = narg(args, 0);
    if !tab_ref_valid(tabnr) {
        return Ok(Value::Number(0));
    }
    let Some(value) = args.get(1) else {
        return Ok(Value::Number(WINNR));
    };
    match value.to_string_value().as_str() {
        "$" => Ok(Value::Number(1)),
        // No window has been left yet, so there is no previous window.
        "#" => Ok(Value::Number(0)),
        other => Err(err("E15", format!("Invalid expression: {other}"))),
    }
}

fn f_tabpagebuflist(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    if let Some(value) = args.first() {
        if !tab_ref_valid(value.to_number()) {
            // Vim returns the number zero for an invalid tab page here.
            return Ok(Value::Number(0));
        }
    }
    Ok(Value::list(vec![Value::Number(ed.current_bufnr() as i64)]))
}

// ---------------------------------------------------------------------------
// Window identity / layout
// ---------------------------------------------------------------------------

fn f_win_execute(
    args: &[Value],
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
) -> VimResult<Value> {
    let id = narg(args, 0);
    if win_id_valid(id) {
        match args.get(1) {
            Some(Value::List(commands)) => {
                for command in commands.borrow().iter() {
                    crate::exec::run_line_with(ed, env, state, &command.to_string_value())?;
                }
            }
            Some(command) => {
                crate::exec::run_line_with(ed, env, state, &command.to_string_value())?;
            }
            None => {}
        }
    }
    // Vim returns the captured `:execute` output; RustVim has no command output
    // capture/`:redir` layer yet, so the (honest) result is an empty string.
    Ok(Value::string(""))
}

fn f_win_findbuf(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let bufnr = narg(args, 0);
    if bufnr == ed.current_bufnr() as i64 {
        Ok(Value::list(vec![Value::Number(WINID)]))
    } else {
        Ok(Value::list(Vec::new()))
    }
}

fn f_win_getid(args: &[Value]) -> VimResult<Value> {
    if args.is_empty() {
        return Ok(Value::Number(WINID));
    }
    let winnr = narg(args, 0);
    if winnr <= 0 {
        return Ok(Value::Number(0));
    }
    if args.len() > 1 && narg(args, 1) != 1 {
        // No such tab page.
        return Ok(Value::Number(-1));
    }
    Ok(Value::Number(if winnr == WINNR { WINID } else { 0 }))
}

fn f_win_gettype(args: &[Value]) -> VimResult<Value> {
    let Some(value) = args.first() else {
        return Ok(Value::string(""));
    };
    // The single window is a regular editing window; anything else is unknown.
    Ok(Value::string(if win_ref_valid(value.to_number()) {
        ""
    } else {
        "unknown"
    }))
}

fn f_winlayout(args: &[Value]) -> VimResult<Value> {
    if let Some(value) = args.first() {
        if !tab_ref_valid(value.to_number()) {
            return Ok(Value::list(Vec::new()));
        }
    }
    Ok(Value::list(vec![
        Value::string("leaf"),
        Value::Number(WINID),
    ]))
}

fn f_winsaveview(ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let cursor = ed.cursor();
    let view = SavedView {
        lnum: cursor.line as i64 + 1,
        col: cursor.col as i64,
        curswant: cursor.col as i64,
        topline: 1,
        leftcol: 0,
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
    state.win.saved_view = Some(view);
    Ok(Value::dict(dict))
}

fn f_winrestview(args: &[Value], ed: &mut Editor, state: &mut ScriptState) -> VimResult<Value> {
    let dict = dict_clone(&arg(args, 0)).ok_or_else(|| err("E474", "Invalid argument"))?;
    let cursor = ed.cursor();
    let current_lnum = cursor.line as i64 + 1;
    let current_col = cursor.col as i64;
    let lnum = dict_number(&dict, "lnum").unwrap_or(current_lnum);
    let col = dict_number(&dict, "col").unwrap_or(current_col);
    let topline = dict_number(&dict, "topline").unwrap_or(1);
    let leftcol = dict_number(&dict, "leftcol").unwrap_or(0);
    let skipcol = dict_number(&dict, "skipcol").unwrap_or(0);
    let curswant = dict_number(&dict, "curswant").unwrap_or(col);
    let line = lnum.saturating_sub(1).max(0) as usize;
    let column = col.max(0) as usize;
    ed.set_cursor(Pos::new(line, column));
    // The scroll offset ('topline'/'leftcol'/'skipcol') has no display layer to
    // apply to yet; record the restored view so later M5 code can use it.
    state.win.saved_view = Some(SavedView {
        lnum: ed.cursor().line as i64 + 1,
        col: ed.cursor().col as i64,
        curswant,
        topline,
        leftcol,
        skipcol,
    });
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

fn f_gettagstack(args: &[Value], state: &mut ScriptState) -> VimResult<Value> {
    let mut dict = VimDict::new();
    if let Some(value) = args.first() {
        if !win_ref_valid(value.to_number()) {
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

fn f_settagstack(args: &[Value], state: &mut ScriptState) -> VimResult<Value> {
    let win = narg(args, 0);
    if !win_ref_valid(win) {
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

fn f_getjumplist(args: &[Value], state: &mut ScriptState) -> VimResult<Value> {
    if let Some(value) = args.first() {
        if !win_ref_valid(value.to_number()) {
            return Ok(Value::list(Vec::new()));
        }
    }
    if args.len() > 1 && !tab_ref_valid(narg(args, 1)) {
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
    if winid != 0 && !win_id_valid(winid) {
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

    #[test]
    fn window_info_and_variables() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["one", "two", "three"]);
        let bufnr = ed.current_bufnr() as i64;

        let info = run("getwininfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&info), 1);
        let window = lget(&info, 0);
        assert_eq!(dnum(&window, "tabnr"), 1);
        assert_eq!(dnum(&window, "winnr"), 1);
        assert_eq!(dnum(&window, "winid"), WINID);
        assert_eq!(dnum(&window, "bufnr"), bufnr);
        // Unknown display geometry is reported as -1, not as a plausible size.
        assert_eq!(dnum(&window, "height"), -1);
        assert_eq!(dnum(&window, "width"), -1);
        assert_eq!(dnum(&window, "textoff"), -1);
        assert_eq!(dnum(&window, "topline"), 1);
        assert_eq!(dnum(&window, "botline"), 3);
        assert!(matches!(dget(&window, "variables"), Value::Dict(_)));
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
        assert_eq!(
            llen(&run(
                "getwininfo",
                vec![num(WINID)],
                &mut ed,
                &mut env,
                &mut state
            )),
            1
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
        assert_eq!(
            run(
                "getwinvar",
                vec![num(0), text("myvar")],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            42
        );
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
        let screenpos = run("win_screenpos", vec![num(1)], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&screenpos, 0).to_number(), 1);
        assert_eq!(lget(&screenpos, 1).to_number(), 1);
        let invalid = run("win_screenpos", vec![num(2)], &mut ed, &mut env, &mut state);
        assert_eq!(lget(&invalid, 0).to_number(), 0);
        assert_eq!(lget(&invalid, 1).to_number(), 0);
        // No layout to restore or drag.
        assert_eq!(
            run("winrestcmd", vec![], &mut ed, &mut env, &mut state).to_string_value(),
            ""
        );
        assert_eq!(
            run(
                "win_move_separator",
                vec![num(1), num(5)],
                &mut ed,
                &mut env,
                &mut state
            )
            .to_number(),
            0
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
            0
        );
        let error = run_err(
            "win_splitmove",
            vec![num(1), num(1)],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert_eq!(error.code, "E957");
    }

    #[test]
    fn tab_info_and_variables() {
        let (mut ed, mut env, mut state) = setup();
        let bufnr = ed.current_bufnr() as i64;
        let info = run("gettabinfo", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(llen(&info), 1);
        let tab = lget(&info, 0);
        assert_eq!(dnum(&tab, "tabnr"), 1);
        let windows = dget(&tab, "windows");
        assert_eq!(llen(&windows), 1);
        assert_eq!(lget(&windows, 0).to_number(), WINID);
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

        // gettabwinvar() reaches window variables of a window in a tab page.
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
            0
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
        assert_eq!(
            run("winbufnr", vec![num(0)], &mut ed, &mut env, &mut state).to_number(),
            bufnr
        );
        assert_eq!(
            run("winbufnr", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            bufnr
        );
        assert_eq!(
            run("winbufnr", vec![num(2)], &mut ed, &mut env, &mut state).to_number(),
            -1
        );
        assert_eq!(
            run("win_getid", vec![], &mut ed, &mut env, &mut state).to_number(),
            WINID
        );
        assert_eq!(
            run("win_getid", vec![num(1)], &mut ed, &mut env, &mut state).to_number(),
            WINID
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
            WINID
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
            -1
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
                vec![num(WINID)],
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
            vec![num(WINID)],
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
                vec![num(WINID)],
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
        assert_eq!(lget(&found, 0).to_number(), WINID);
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
        assert_eq!(lget(&layout, 1).to_number(), WINID);
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
    fn win_execute_runs_commands_in_the_window() {
        let (mut ed, mut env, mut state) = setup();
        assert_eq!(
            run(
                "win_execute",
                vec![num(WINID), text("let g:win_exec_a = 41")],
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
                num(WINID),
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
            vec![num(99), text("let g:win_exec_d = 1")],
            &mut ed,
            &mut env,
            &mut state,
        );
        assert!(!env.globals.contains_key("win_exec_d"));
    }

    #[test]
    fn save_and_restore_view() {
        let (mut ed, mut env, mut state) = setup();
        set_lines(&mut ed, &["alpha", "beta", "gamma"]);
        ed.set_cursor(Pos::new(1, 2));
        let view = run("winsaveview", vec![], &mut ed, &mut env, &mut state);
        assert_eq!(dnum(&view, "lnum"), 2);
        assert_eq!(dnum(&view, "col"), 2);
        assert_eq!(dnum(&view, "topline"), 1);
        assert_eq!(dnum(&view, "leftcol"), 0);
        ed.set_cursor(Pos::new(0, 0));
        run("winrestview", vec![view], &mut ed, &mut env, &mut state);
        assert_eq!(ed.cursor().line, 1);
        assert_eq!(ed.cursor().col, 2);
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
        let value =
            crate::script::eval_expr(&mut ed, &mut ctx.env, &mut ctx.state, "win_getid()").unwrap();
        assert_eq!(value.to_number(), WINID);
        let value = crate::script::eval_expr(
            &mut ed,
            &mut ctx.env,
            &mut ctx.state,
            "getwininfo()[0].winid",
        )
        .unwrap();
        assert_eq!(value.to_number(), WINID);
        let value =
            crate::script::eval_expr(&mut ed, &mut ctx.env, &mut ctx.state, "tabpagebuflist()")
                .unwrap();
        assert_eq!(llen(&value), 1);
        let value =
            crate::script::eval_expr(&mut ed, &mut ctx.env, &mut ctx.state, "indent(1)").unwrap();
        assert_eq!(value.to_number(), 0);
    }
}
