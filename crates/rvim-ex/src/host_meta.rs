//! Host builtins backed by editor metadata that has no display yet: popups,
//! text properties, signs, match highlighting, command history and balloons.
//!
//! # State model, not rendering
//!
//! The TUI is M4. Everything implemented here is a **fully queryable state
//! model**: popups/signs/text-properties/matches are recorded in [`MetaState`],
//! their ids and dictionary shapes match Vim's documented API, and the query
//! functions (`popup_getpos()`, `popup_getoptions()`, `popup_list()`,
//! `popup_locate()`, `prop_list()`, `prop_find()`, `sign_getdefined()`,
//! `sign_getplaced()`, `getmatches()`, ...) read them back. **Nothing is drawn**
//! until the TUI exists; mention of "screen" below therefore refers to a
//! nominal 80x24 grid (`'columns'` falls back to 80, `'lines'` to 24) used to
//! compute popup geometry deterministically. Display-cell widths are
//! approximated by character counts until the renderer lands (wide CJK cells,
//! combining characters and 'showbreak' are M4 concerns).
//!
//! Deliberate, documented limits of this milestone:
//!
//! - `popup_menu()` blocks for keyboard input in Vim. There is no input layer
//!   yet, so it validates its arguments, records the request in
//!   [`MetaState::deferred_ui`] and returns `-1`, as if the popup had been
//!   cancelled. `popup_dialog()`/`popup_filter_*()` are out of scope here.
//! - Popup `filter`/`callback` callbacks are stored (and returned by
//!   `popup_getoptions()`) but are not invoked: they are driven by UI events.
//! - `balloon_show()`/`balloon_gettext()` honestly report "no balloon": the
//!   show function returns `0` and the getter returns `""`.
//!   `balloon_split()` is pure text splitting and is implemented for real.
//! - Highlight-group names (matches, text properties, signs) are recorded
//!   without validating them against a syntax table; the syntax/highlight
//!   registry is M6.
//! - Buffer-local text property types use the buffer's number as namespace
//!   exactly like Vim; popup text properties (`{lines with "props"}`) are
//!   accepted but not registered in the text-property store because popup
//!   buffers do not exist yet.
//! - Popup geometry (`popup_getpos()`) is computed against a nominal 80x24
//!   screen (`'columns'` is not a registered option yet and falls back to 80,
//!   `'lines'` to 24); display cells are approximated by character counts.
//! - Text properties and signs do **not** follow buffer edits yet: deleting or
//!   inserting lines does not shift their line numbers (Vim's memline change
//!   hooks arrive with the M4/M5 editor loop).
//! - History numbering is position-based: entries are numbered `1..=len`,
//!   `histnr()` returns `len` (`0` for an empty history, where Vim reports `-1`
//!   when no entry has ever been set) and `histget()`/`histdel()` accept the
//!   same `+`/`-` indexes. Vim's `hisnum` counter is monotonic even after
//!   deletions; deletes renumber the tail here.
//!
//! Error handling: whenever Vim reports an error message for bad arguments
//! (`E118`/`E119`, `E450`, `E474`, `E475`, `E714`, `E715`, `E745`, `E798`,
//! `E964`-`E971`, `E155`, `E157`, `E239`, ...), this module returns the same
//! `E` code as an error value. Vim's "print message and continue with a default
//! value" behaviour is therefore surfaced as a VimL error, which is how the
//! rest of RustVim reports bad builtin arguments.

use std::collections::HashMap;
use std::rc::Rc;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::Editor;
use rvim_viml::Env;
use rvim_viml::value::{FuncRef, Value, VimDict};

use crate::script::ScriptState;

/// Lowest popup window id (Vim window ids start at `LOWEST_WIN_ID = 1000`; the
/// current window owns 1000, popups increment from there).
pub const FIRST_POPUP_ID: i64 = 1001;

/// Default `zindex` for normal popups (`POPUPWIN_DEFAULT_ZINDEX`).
pub const DEFAULT_ZINDEX: i64 = 50;
/// `zindex` used by `popup_menu()` (`POPUPWIN_DIALOG_ZINDEX`).
pub const DIALOG_ZINDEX: i64 = 200;
/// `zindex` used by `popup_notification()` (`POPUPWIN_NOTIFICATION_ZINDEX`).
pub const NOTIFICATION_ZINDEX: i64 = 300;

/// Default sign priority (`SIGN_DEF_PRIO`).
pub const SIGN_DEF_PRIO: i64 = 10;

/// Number of entries matchaddpos() can store (`MAXPOSMATCH`).
const MAXPOSMATCH: usize = 8;

/// Which popup constructor created a popup; drives Vim's per-type defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupKind {
    /// `popup_create()`
    Normal,
    /// `popup_atcursor()`
    AtCursor,
    /// `popup_beval()`
    Beval,
    /// `popup_notification()` (a global popup, `tabpage == -1`)
    Notification,
    /// `popup_menu()` (needs the M4 input loop)
    Menu,
}

/// Resolved options of one popup. Field names and defaults mirror
/// `popup_create-arguments` and `popup_getoptions()`.
#[derive(Debug, Clone)]
pub struct PopupOptions {
    /// Wanted screen line (1-based; `0` means "center vertically").
    pub line: i64,
    /// Wanted screen column (1-based; `0` means "center horizontally").
    pub col: i64,
    /// `"topleft"`, `"topright"`, `"botleft"`, `"botright"` or `"center"`.
    pub pos: String,
    pub posinvert: bool,
    pub fixed: bool,
    pub scrollbar: bool,
    pub title: String,
    pub wrap: bool,
    pub drag: bool,
    pub dragall: bool,
    pub mapping: bool,
    pub resize: bool,
    pub cursorline: bool,
    /// `"none"`, `"button"` or `"click"`.
    pub close: String,
    pub highlight: String,
    pub zindex: i64,
    pub time: i64,
    pub firstline: i64,
    pub minwidth: i64,
    pub minheight: i64,
    pub maxwidth: i64,
    pub maxheight: i64,
    /// top, right, bottom, left
    pub padding: [i64; 4],
    /// top, right, bottom, left
    pub border: [i64; 4],
    pub borderhighlight: [String; 4],
    /// clockwise from top-left, like Vim's eight border characters.
    pub borderchars: [String; 8],
    pub textprop: String,
    pub textpropwin: i64,
    pub textpropid: i64,
    /// `[lnum, mincol, maxcol]` from `set_moved_values()`.
    pub moved: [i64; 3],
    /// `[row, mincol, maxcol]` from `set_mousemoved_values()`.
    pub mousemoved: [i64; 3],
    pub filter: Option<Value>,
    pub callback: Option<Value>,
    /// Tab page of the popup: `0` = current tab, negative = global.
    pub tabpage: i64,
}

impl PopupOptions {
    fn new(kind: PopupKind, ed: &Editor) -> Self {
        let cursor_line = ed.cursor().line as i64;
        let cursor_col = ed.cursor().col as i64;
        let mut opts = Self {
            line: 0,
            col: 0,
            pos: "topleft".to_string(),
            posinvert: true,
            fixed: false,
            scrollbar: true,
            title: String::new(),
            wrap: true,
            drag: false,
            dragall: false,
            mapping: true,
            resize: false,
            cursorline: false,
            close: "none".to_string(),
            highlight: String::new(),
            zindex: DEFAULT_ZINDEX,
            time: 0,
            firstline: 0,
            minwidth: 0,
            minheight: 0,
            maxwidth: 0,
            maxheight: 0,
            padding: [0; 4],
            border: [0; 4],
            borderhighlight: std::array::from_fn(|_| String::new()),
            borderchars: std::array::from_fn(|_| String::new()),
            textprop: String::new(),
            textpropwin: 0,
            textpropid: 0,
            moved: [0; 3],
            mousemoved: [0; 3],
            filter: None,
            callback: None,
            tabpage: 0,
        };
        match kind {
            PopupKind::Normal => {}
            PopupKind::AtCursor => {
                // wantline for "botleft" is a zero-based screen row; a popup
                // anchored on the first line is flipped to the top instead.
                opts.pos = "botleft".to_string();
                opts.line = cursor_line;
                opts.col = cursor_col + 1;
                if opts.line == 0 {
                    opts.line = 2;
                    opts.pos = "topleft".to_string();
                }
                opts.moved = [cursor_line + 1, cursor_col, cursor_col];
            }
            PopupKind::Beval => {
                // No mouse in the headless editor: use the cursor position.
                opts.pos = "botleft".to_string();
                opts.line = cursor_line;
                if opts.line <= 0 {
                    opts.line = 2;
                    opts.pos = "topleft".to_string();
                }
                opts.col = cursor_col + 1;
                opts.mousemoved = [cursor_line, cursor_col, cursor_col];
            }
            PopupKind::Notification => {
                opts.line = 1;
                opts.col = 10;
                opts.zindex = NOTIFICATION_ZINDEX;
                opts.minwidth = 20;
                opts.drag = true;
                opts.close = "click".to_string();
                opts.border = [1; 4];
                opts.padding = [0, 1, 0, 1];
                opts.highlight = "WarningMsg".to_string();
                opts.tabpage = -1;
            }
            PopupKind::Menu => {
                opts.pos = "center".to_string();
                opts.zindex = DIALOG_ZINDEX;
                opts.drag = true;
                opts.mapping = false;
                opts.border = [1; 4];
                opts.padding = [0, 1, 0, 1];
                opts.wrap = false;
                opts.cursorline = true;
                opts.filter = Some(Value::Funcref(Rc::new(FuncRef::new("popup_filter_menu"))));
            }
        }
        opts
    }
}

/// One popup window: text, merged options, visibility and computed geometry.
#[derive(Debug, Clone)]
pub struct Popup {
    pub id: i64,
    pub kind: PopupKind,
    /// Buffer lines shown by the popup.
    pub text: Vec<String>,
    pub opts: PopupOptions,
    /// Hidden by `popup_hide()` (still exists, `popup_getpos().visible == 0`).
    pub hidden: bool,
    /// 1-based screen line of the popup's outer top edge.
    pub line: i64,
    /// 1-based screen column of the popup's outer left edge.
    pub col: i64,
    /// Outer width: core + left/right padding + border.
    pub width: i64,
    /// Outer height: core + top/bottom padding + border (+ title row).
    pub height: i64,
    pub core_line: i64,
    pub core_col: i64,
    pub core_width: i64,
    pub core_height: i64,
    pub firstline: i64,
    pub lastline: i64,
    pub scrollbar: bool,
}

impl Popup {
    /// Visible on the (nominal) screen.
    pub fn visible(&self) -> bool {
        !self.hidden
    }

    fn left_extra(&self) -> i64 {
        self.opts.border[3] + self.opts.padding[3]
    }

    fn right_extra(&self) -> i64 {
        self.opts.border[1] + self.opts.padding[1]
    }

    fn top_extra(&self) -> i64 {
        let extra = self.opts.border[0] + self.opts.padding[0];
        if extra == 0 && !self.opts.title.is_empty() {
            1
        } else {
            extra
        }
    }

    fn bottom_extra(&self) -> i64 {
        self.opts.border[2] + self.opts.padding[2]
    }
}

/// One text property type (`prop_type_add()`); `bufnr == None` is a global type.
#[derive(Debug, Clone)]
pub struct PropType {
    pub name: String,
    pub bufnr: Option<i64>,
    /// Numeric type id, assigned monotonically like Vim's `proptype_id`.
    pub id: i64,
    pub highlight: String,
    pub priority: i64,
    pub combine: bool,
    pub start_incl: bool,
    pub end_incl: bool,
}

/// One per-line entry of a text property. A property spanning several lines is
/// stored as one entry per line with `start`/`end` marking the boundaries.
#[derive(Debug, Clone)]
pub struct MetadataProp {
    pub id: i64,
    pub prop_type: String,
    pub bufnr: i64,
    pub lnum: i64,
    pub col: i64,
    pub length: i64,
    /// The placement dictionary as passed to `prop_add()`.
    pub props: VimDict,
    pub start: bool,
    pub end: bool,
}

/// One defined sign (`sign_define()`).
#[derive(Debug, Clone)]
pub struct SignDef {
    pub name: String,
    pub icon: Option<String>,
    pub text: Option<String>,
    pub linehl: Option<String>,
    pub texthl: Option<String>,
    pub culhl: Option<String>,
    pub numhl: Option<String>,
}

/// One placed sign. `group` is empty for the global sign group.
#[derive(Debug, Clone)]
pub struct SignPlace {
    pub group: String,
    pub bufnr: i64,
    pub lnum: i64,
    pub id: i64,
    pub name: String,
    pub priority: i64,
}

/// Metadata state that is queryable but not rendered yet.
#[derive(Debug)]
pub struct MetaState {
    /// Popups in creation order.
    pub popups: Vec<Popup>,
    pub next_popup_id: i64,
    /// Text property instances, one entry per line covered by a property.
    pub textprops: Vec<MetadataProp>,
    /// Defined text property types (global and buffer-local).
    pub prop_types: Vec<PropType>,
    pub next_prop_type_id: i64,
    /// Sign definitions in definition order.
    pub sign_defs: Vec<SignDef>,
    /// Placed signs in placement order.
    pub sign_places: Vec<SignPlace>,
    /// Next automatically allocated sign id in the global sign group.
    pub next_sign_id: i64,
    /// Next automatically allocated sign id per named sign group.
    pub sign_group_ids: HashMap<String, i64>,
    /// `matchadd()`/`matchaddpos()` state: (id, dictionary as getmatches() shows it).
    pub matches: Vec<(i64, VimDict)>,
    pub next_match_id: i64,
    /// Command-line (`:` / `"cmd"`) history, oldest entry first.
    pub cmd_history: Vec<String>,
    /// Search (`/`) history, oldest entry first.
    pub search_history: Vec<String>,
    /// Expression (`=`) history, oldest entry first.
    pub expr_history: Vec<String>,
    /// Input (`@`) history, oldest entry first.
    pub input_history: Vec<String>,
    /// Debug (`>`) history, oldest entry first.
    pub debug_history: Vec<String>,
    /// Requests that need the M4 input/display layer, recorded for the report
    /// (currently only `popup_menu()`).
    pub deferred_ui: Vec<String>,
}

impl Default for MetaState {
    fn default() -> Self {
        Self {
            popups: Vec::new(),
            next_popup_id: FIRST_POPUP_ID,
            textprops: Vec::new(),
            prop_types: Vec::new(),
            next_prop_type_id: 1,
            sign_defs: Vec::new(),
            sign_places: Vec::new(),
            next_sign_id: 1,
            sign_group_ids: HashMap::new(),
            matches: Vec::new(),
            next_match_id: 1,
            cmd_history: Vec::new(),
            search_history: Vec::new(),
            expr_history: Vec::new(),
            input_history: Vec::new(),
            debug_history: Vec::new(),
            deferred_ui: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Small argument helpers (mirroring the typval.c/tv_get_* family)
// ---------------------------------------------------------------------------

fn err(code: &'static str, message: impl Into<String>) -> VimError {
    VimError::new(code, message)
}

/// Vim's arity diagnostics (`E119`/`E118`).
fn arity(name: &str, args: &[Value], min: usize, max: usize) -> VimResult<()> {
    if args.len() < min {
        return Err(err(
            "E119",
            format!("Not enough arguments for function: {name}"),
        ));
    }
    if args.len() > max {
        return Err(err(
            "E118",
            format!("Too many arguments for function: {name}"),
        ));
    }
    Ok(())
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Null)
}

/// Strict `tv_get_string()`: numbers/floats/bools convert, containers do not.
fn get_string(value: &Value) -> VimResult<String> {
    match value {
        Value::List(_) => Err(err("E730", "Using a List as a String")),
        Value::Dict(_) => Err(err("E731", "Using a Dictionary as a String")),
        Value::Funcref(_) => Err(err("E729", "Using a Funcref as a String")),
        Value::Blob(_) => Err(err("E976", "Using a Blob as a String")),
        other => Ok(other.to_string_value()),
    }
}

/// Strict number conversion with Vim's `E745` for containers.
fn get_number(value: &Value) -> VimResult<i64> {
    value.as_number()
}

fn as_dict(value: &Value, what: &str) -> VimResult<VimDict> {
    match value {
        Value::Dict(d) => Ok(d.borrow().clone()),
        _ => Err(err(
            "E715",
            format!("Dictionary required for argument {what}"),
        )),
    }
}

fn as_list(value: &Value, what: &str) -> VimResult<Vec<Value>> {
    match value {
        Value::List(l) => Ok(l.borrow().clone()),
        _ => Err(err("E714", format!("List required for argument {what}"))),
    }
}

fn dict_num(dict: &VimDict, key: &str) -> Option<i64> {
    dict.get(key).map(Value::to_number)
}

fn dict_bool(dict: &VimDict, key: &str, default: bool) -> bool {
    dict.get(key).map(Value::truthy).unwrap_or(default)
}

fn dict_string(dict: &VimDict, key: &str) -> Option<String> {
    dict.get(key).map(Value::to_string_value)
}

fn dictionary(items: VimDict) -> Value {
    Value::dict(items)
}

fn callback_value(value: &Value) -> Option<Value> {
    match value {
        Value::Funcref(_) => Some(value.clone()),
        Value::String(name) => Some(Value::Funcref(Rc::new(FuncRef::new(name.clone())))),
        _ => None,
    }
}

/// Approximate display width of a string (one cell per character until the
/// renderer knows about wide/combining characters).
fn text_cells(text: &str) -> i64 {
    text.chars().count() as i64
}

// ---------------------------------------------------------------------------
// Buffer helpers
// ---------------------------------------------------------------------------

fn buffer_line(ed: &Editor, bufnr: i32, lnum: i64) -> Option<String> {
    if lnum < 1 {
        return None;
    }
    ed.buffer(bufnr)
        .ok()
        .and_then(|b| b.line_str(lnum as usize - 1).ok())
        .map(str::to_string)
}

fn buffer_line_count(ed: &Editor, bufnr: i32) -> i64 {
    ed.buffer(bufnr).map(|b| b.line_count() as i64).unwrap_or(0)
}

/// Resolve a Vim "buffer name or number" argument (see `tv_get_buf()`).
/// `0` as a number is *not* the current buffer here (matching `get_buf_arg`).
fn resolve_buffer(ed: &Editor, value: &Value) -> VimResult<i32> {
    let invalid = |v: &Value| {
        err(
            "E158",
            format!("Invalid buffer name: {}", v.to_string_value()),
        )
    };
    match value {
        Value::Number(n) => ed
            .all_buffers()
            .iter()
            .find(|b| b.id == *n as i32)
            .map(|b| b.id)
            .ok_or_else(|| invalid(value)),
        Value::String(name) => {
            if name.is_empty() {
                return Ok(ed.current_bufnr());
            }
            if name == "$" {
                return Ok(ed.all_buffers().last().map(|b| b.id).unwrap_or(1));
            }
            ed.all_buffers()
                .iter()
                .find(|b| {
                    b.display_name() == *name
                        || b.id.to_string() == *name
                        || b.name
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|f| f.to_string_lossy() == *name)
                            .unwrap_or(false)
                })
                .map(|b| b.id)
                .ok_or_else(|| invalid(value))
        }
        _ => Err(invalid(value)),
    }
}

/// Resolve the `{buf}` argument of the sign functions: a number `0` is invalid.
fn resolve_sign_buffer(ed: &Editor, value: &Value) -> VimResult<i32> {
    resolve_buffer(ed, value)
}

// ---------------------------------------------------------------------------
// Popup implementation
// ---------------------------------------------------------------------------

/// Popup positions accepted by the `pos` option (`poppos_entries[]`).
const POPUP_POSITIONS: &[&str] = &["topleft", "topright", "botleft", "botright", "center"];

fn popup_by_id(meta: &MetaState, id: i64) -> Option<&Popup> {
    meta.popups.iter().find(|p| p.id == id)
}

fn popup_index(meta: &MetaState, id: i64) -> Option<usize> {
    meta.popups.iter().position(|p| p.id == id)
}

/// Build the popup text lines from `{what}` (`popup_set_buffer_text()`).
fn popup_text_from_value(ed: &Editor, value: &Value) -> VimResult<Vec<String>> {
    match value {
        Value::String(text) => Ok(text.split('\n').map(str::to_string).collect()),
        Value::Number(bufnr) => {
            let id = *bufnr as i32;
            let Some(buffer) = ed.all_buffers().iter().find(|b| b.id == id) else {
                return Err(err("E86", format!("Buffer {bufnr} does not exist")));
            };
            Ok(buffer
                .lines()
                .iter()
                .map(|l| l.as_str().to_string())
                .collect())
        }
        Value::List(items) => {
            let items = items.borrow().clone();
            if items.is_empty() {
                return Ok(vec![String::new()]);
            }
            match items.first() {
                Some(Value::String(_)) => {
                    let mut lines = Vec::with_capacity(items.len());
                    for item in &items {
                        match item {
                            Value::String(text) => lines.push(text.clone()),
                            _ => {
                                return Err(err(
                                    "E475",
                                    "Invalid argument: popup text list must contain strings",
                                ));
                            }
                        }
                    }
                    Ok(lines)
                }
                _ => {
                    // A list of dictionaries, each with an optional "text" entry.
                    let mut lines = Vec::with_capacity(items.len());
                    for (i, item) in items.iter().enumerate() {
                        match item {
                            Value::Dict(d) => {
                                let text = d
                                    .borrow()
                                    .get("text")
                                    .map(Value::to_string_value)
                                    .unwrap_or_default();
                                lines.push(text);
                            }
                            _ => {
                                return Err(err(
                                    "E1284",
                                    format!("Argument 1, list item {}: Dictionary required", i + 1),
                                ));
                            }
                        }
                    }
                    Ok(lines)
                }
            }
        }
        _ => Err(err("E450", "Buffer number, text or a list required")),
    }
}

/// Parse one `line`/`col` popup option (`popup_options_one()`); `None` when the
/// key is absent.
fn popup_pos_option(dict: &VimDict, key: &str, ed: &Editor) -> VimResult<Option<i64>> {
    let Some(value) = dict.get(key) else {
        return Ok(None);
    };
    let text = get_string(value)?;
    if let Some(rest) = text.strip_prefix("cursor") {
        let mut offset = 0i64;
        if !rest.is_empty() {
            let parsed: i64 = rest.trim().parse().map_err(|_| {
                err(
                    "E15",
                    format!("Invalid expression: \"{}\"", value.to_string_value()),
                )
            })?;
            offset = parsed;
        }
        // Screen row/column of the cursor: the headless editor has no scroll
        // offset, so the cursor line is the screen row.
        let base = if key == "line" {
            ed.cursor().line as i64 + 1
        } else {
            ed.cursor().col as i64 + 1
        };
        let n = base + offset;
        return Ok(Some(if n == 0 { -1 } else { n }));
    }
    Ok(Some(get_number(value)?))
}

/// Apply the "move" subset of popup options (`apply_move_options()`).
fn apply_popup_move_options(opts: &mut PopupOptions, dict: &VimDict, ed: &Editor) -> VimResult<()> {
    for (key, field) in [
        ("minwidth", &mut opts.minwidth),
        ("minheight", &mut opts.minheight),
        ("maxwidth", &mut opts.maxwidth),
        ("maxheight", &mut opts.maxheight),
    ] {
        if let Some(n) = dict_num(dict, key)
            && n >= 0
        {
            *field = n;
        }
    }
    if let Some(line) = popup_pos_option(dict, "line", ed)? {
        opts.line = line;
    }
    if let Some(col) = popup_pos_option(dict, "col", ed)? {
        opts.col = col;
    }
    if let Some(fixed) = dict.get("fixed") {
        opts.fixed = fixed.truthy();
    }
    if let Some(pos) = dict_string(dict, "pos") {
        if !POPUP_POSITIONS.contains(&pos.as_str()) {
            return Err(err("E475", format!("Invalid argument: {pos}")));
        }
        opts.pos = pos;
    }
    if let Some(textprop) = dict_string(dict, "textprop") {
        opts.textprop = textprop;
        if !opts.textprop.is_empty() {
            if let Some(win) = dict_num(dict, "textpropwin") {
                opts.textpropwin = win;
            }
            if let Some(id) = dict_num(dict, "textpropid") {
                opts.textpropid = id;
            }
        }
    }
    Ok(())
}

/// Parse and merge a `padding`/`border` list (`set_padding_border()`).
fn popup_padding_border(dict: &VimDict, name: &str, max_value: i64) -> VimResult<Option<[i64; 4]>> {
    let Some(value) = dict.get(name) else {
        return Ok(None);
    };
    let list = as_list(value, name)?;
    let mut array = [1i64; 4];
    for (i, item) in list.iter().take(4).enumerate() {
        let nr = get_number(item)?;
        if nr >= 0 {
            array[i] = nr.min(max_value);
        }
    }
    Ok(Some(array))
}

fn popup_borderchars(dict: &VimDict) -> VimResult<Option<[String; 8]>> {
    let Some(value) = dict.get("borderchars") else {
        return Ok(None);
    };
    let list = as_list(value, "borderchars")?;
    let mut array: [String; 8] = std::array::from_fn(|_| String::new());
    for (i, item) in list.iter().take(8).enumerate() {
        array[i] = get_string(item)?;
    }
    if list.len() == 1 {
        let first = array[0].clone();
        for slot in array.iter_mut().skip(1) {
            *slot = first.clone();
        }
    } else if list.len() == 2 {
        let first = array[0].clone();
        let second = array[1].clone();
        for slot in array.iter_mut().take(4) {
            *slot = first.clone();
        }
        for slot in array.iter_mut().skip(4) {
            *slot = second.clone();
        }
    }
    Ok(Some(array))
}

fn popup_borderhighlight(dict: &VimDict) -> VimResult<Option<[String; 4]>> {
    let Some(value) = dict.get("borderhighlight") else {
        return Ok(None);
    };
    let list = as_list(value, "borderhighlight")?;
    let mut array: [String; 4] = std::array::from_fn(|_| String::new());
    for (i, item) in list.iter().take(4).enumerate() {
        array[i] = get_string(item)?;
    }
    if list.len() == 1 {
        let first = array[0].clone();
        for slot in array.iter_mut().skip(1) {
            *slot = first.clone();
        }
    }
    Ok(Some(array))
}

/// Apply `moved`/`mousemoved` (`handle_moved_argument()`). Both accept a list of
/// numbers; `moved` additionally accepts "word", "WORD", "expr" and "any", for
/// which the cursor word is used (the exact `FIND_EVAL` rules are M4 work).
fn popup_moved_option(
    dict: &VimDict,
    key: &str,
    ed: &Editor,
    mousemoved: bool,
) -> VimResult<Option<[i64; 3]>> {
    let Some(value) = dict.get(key) else {
        return Ok(None);
    };
    let default = if mousemoved {
        [
            ed.cursor().line as i64,
            ed.cursor().col as i64,
            ed.cursor().col as i64,
        ]
    } else {
        [
            ed.cursor().line as i64 + 1,
            ed.cursor().col as i64,
            ed.cursor().col as i64,
        ]
    };
    match value {
        Value::String(word) => match word.as_str() {
            "word" | "WORD" | "expr" | "any" => Ok(Some(default)),
            other => Err(err("E475", format!("Invalid argument: {other}"))),
        },
        Value::List(items) => {
            let items = items.borrow().clone();
            if items.len() != 2 && items.len() != 3 {
                return Err(err(
                    "E475",
                    format!("Invalid argument: {}", value.to_string_value()),
                ));
            }
            let mut out = default;
            let mut index = 0;
            if items.len() == 3 {
                out[0] = get_number(&items[0])?;
                index = 1;
            }
            out[1] = get_number(&items[index])?;
            out[2] = get_number(&items[index + 1])?;
            Ok(Some(out))
        }
        _ => Err(err(
            "E474",
            format!("Invalid argument: {}", value.to_string_value()),
        )),
    }
}

/// Apply the general popup options (`apply_general_options()`).
fn apply_popup_general_options(
    opts: &mut PopupOptions,
    dict: &VimDict,
    ed: &Editor,
) -> VimResult<()> {
    for (key, field) in [
        ("scrollbar", &mut opts.scrollbar),
        ("wrap", &mut opts.wrap),
        ("drag", &mut opts.drag),
        ("dragall", &mut opts.dragall),
        ("posinvert", &mut opts.posinvert),
        ("resize", &mut opts.resize),
        ("mapping", &mut opts.mapping),
        ("cursorline", &mut opts.cursorline),
    ] {
        if let Some(value) = dict.get(key) {
            *field = value.truthy();
        }
    }
    if let Some(title) = dict_string(dict, "title") {
        opts.title = title;
    }
    if let Some(close) = dict_string(dict, "close") {
        match close.as_str() {
            "none" | "button" | "click" => opts.close = close,
            other => {
                return Err(err(
                    "E475",
                    format!("Invalid value for argument close: {other}"),
                ));
            }
        }
    }
    if let Some(highlight) = dict_string(dict, "highlight") {
        opts.highlight = highlight;
    }
    if let Some(padding) = popup_padding_border(dict, "padding", 999)? {
        opts.padding = padding;
    }
    if let Some(border) = popup_padding_border(dict, "border", 1)? {
        opts.border = border;
    }
    if let Some(highlight) = popup_borderhighlight(dict)? {
        opts.borderhighlight = highlight;
    }
    if let Some(chars) = popup_borderchars(dict)? {
        opts.borderchars = chars;
    }
    if let Some(zindex) = dict_num(dict, "zindex") {
        opts.zindex = if zindex < 1 {
            DEFAULT_ZINDEX
        } else {
            zindex.min(32000)
        };
    }
    if let Some(firstline) = dict_num(dict, "firstline") {
        opts.firstline = if firstline < 0 { -1 } else { firstline };
    }
    if dict.get("time").is_some() {
        let time = dict_num(dict, "time").unwrap_or(0);
        opts.time = time.max(0);
    }
    if let Some(value) = dict.get("mask") {
        let list = as_list(value, "mask")?;
        for item in &list {
            match item {
                Value::List(inner) if inner.borrow().len() == 4 => {}
                _ => return Err(err("E475", "Invalid value for argument mask")),
            }
        }
    }
    if let Some(moved) = popup_moved_option(dict, "moved", ed, false)? {
        opts.moved = moved;
    }
    if let Some(mousemoved) = popup_moved_option(dict, "mousemoved", ed, true)? {
        opts.mousemoved = mousemoved;
    }
    if let Some(value) = dict.get("filter")
        && let Some(cb) = callback_value(value)
    {
        opts.filter = Some(cb);
    }
    if let Some(value) = dict.get("callback")
        && let Some(cb) = callback_value(value)
    {
        opts.callback = Some(cb);
    }
    if let Some(tabpage) = dict_num(dict, "tabpage")
        && tabpage > 0
        && tabpage != 1
    {
        return Err(err("E997", format!("Tabpage not found: {tabpage}")));
    }
    Ok(())
}

/// Compute the popup geometry on the nominal 80x24 screen
/// (`popup_adjust_position()`, simplified where it depends on real rendering).
fn layout_popup(popup: &mut Popup, ed: &Editor, others: &[Popup]) {
    let screen_w = ed.options.num_or("columns", 80).max(1);
    let screen_h = ed.options.num_or("lines", 24).max(1);
    let left_extra = popup.left_extra();
    let right_extra = popup.right_extra();
    let top_extra = popup.top_extra();
    let bottom_extra = popup.bottom_extra();
    let extra_width = left_extra + right_extra;
    let extra_height = top_extra + bottom_extra;

    // Core width: the widest line, clamped by minwidth/maxwidth and screen.
    let maxspace = (screen_w - extra_width).max(1);
    let mut core_width = popup
        .text
        .iter()
        .map(|line| text_cells(line))
        .max()
        .unwrap_or(1)
        .max(1);
    let mut maxwidth = maxspace;
    if popup.opts.maxwidth > 0 {
        maxwidth = maxwidth.min(popup.opts.maxwidth);
    }
    if popup.opts.minwidth > 0 {
        core_width = core_width.max(popup.opts.minwidth);
    }
    let mut wrapped = 0;
    if popup.opts.wrap && maxwidth > 0 {
        let mut wrapped_width = 0;
        for line in &popup.text {
            let cells = text_cells(line);
            if cells > maxwidth {
                wrapped += (cells + maxwidth - 1) / maxwidth - 1;
                wrapped_width = maxwidth;
            }
        }
        core_width = core_width.max(wrapped_width);
    }
    core_width = core_width.min(maxwidth).min(maxspace).max(1);

    // Core height: one row per line (plus wrapping), clamped.
    let mut core_height = popup.text.len().max(1) as i64 + wrapped;
    if popup.opts.minheight > 0 {
        core_height = core_height.max(popup.opts.minheight);
    }
    if popup.opts.maxheight > 0 {
        core_height = core_height.min(popup.opts.maxheight);
    }
    core_height = core_height.min(screen_h.max(1));

    let total_width = core_width + extra_width;
    let total_height = core_height + extra_height;

    let want_line = if popup.opts.line < 0 {
        0
    } else {
        popup.opts.line
    };
    let want_col = popup.opts.col.max(0);
    let center = popup.opts.pos == "center";
    let center_vert = center || want_line == 0;
    let center_hor = center || want_col == 0;

    let mut row: i64 = 0;
    let mut col: i64 = 0;
    if !center_vert {
        match popup.opts.pos.as_str() {
            "topright" | "topleft" => {
                row = want_line - 1;
                if popup.opts.posinvert && row + total_height > screen_h && want_line * 2 > screen_h
                {
                    // Flip above the anchor when there is no room below.
                    row = want_line - 2 - total_height;
                    self_clamp(&mut row, 0, screen_h - 1);
                }
            }
            _ => {
                // botleft/botright: want_line is the 1-based bottom row.
                if total_height <= want_line {
                    row = want_line - total_height;
                } else if want_line * 2 >= screen_h || !popup.opts.posinvert {
                    row = 0;
                    core_height = (want_line - extra_height).max(1);
                } else {
                    row = want_line + 1;
                }
            }
        }
    }
    if !center_hor {
        match popup.opts.pos.as_str() {
            "topright" | "botright" => {
                col = want_col - total_width;
            }
            _ => {
                col = want_col - 1;
                if popup.opts.posinvert && col + total_width > screen_w {
                    col = screen_w - total_width;
                }
            }
        }
    }
    if center_vert {
        row = (screen_h - total_height) / 2;
    }
    if center_hor {
        col = (screen_w - total_width) / 2;
    }
    self_clamp(&mut row, 0, screen_h - 1);
    self_clamp(&mut col, 0, screen_w.saturating_sub(1));

    // Notifications stack below earlier notifications instead of overlapping.
    if popup.kind == PopupKind::Notification {
        let mut moved = true;
        let mut rounds = 0;
        while moved && rounds < 32 {
            moved = false;
            rounds += 1;
            for other in others {
                if other.kind != PopupKind::Notification || !other.visible() {
                    continue;
                }
                let overlap =
                    row < other.line - 1 + other.height && other.line - 1 < row + total_height;
                if overlap {
                    row = other.line - 1 + other.height;
                    moved = true;
                    break;
                }
            }
        }
        self_clamp(&mut row, 0, screen_h - 1);
    }

    popup.core_width = core_width;
    popup.core_height = core_height;
    popup.width = total_width;
    popup.height = total_height;
    popup.line = row + 1;
    popup.col = col + 1;
    popup.core_line = popup.line + top_extra;
    popup.core_col = popup.col + left_extra;
    popup.firstline = if popup.opts.firstline > 0 {
        popup.opts.firstline.min(popup.text.len().max(1) as i64)
    } else {
        1
    };
    popup.lastline = (popup.firstline + core_height - 1).min(popup.text.len().max(1) as i64);
    popup.scrollbar =
        popup.opts.scrollbar && (popup.firstline > 1 || popup.lastline < popup.text.len() as i64);
}

fn self_clamp(value: &mut i64, min: i64, max: i64) {
    if *value < min {
        *value = min;
    }
    if *value > max {
        *value = max.max(min);
    }
}

/// `popup_create()` and friends.
fn validate_popup_textprop(meta: &MetaState, ed: &Editor, opts: &PopupOptions) -> VimResult<()> {
    if opts.textprop.is_empty() {
        return Ok(());
    }
    let bufnr = if opts.textpropwin != 0 {
        opts.textpropwin
    } else {
        ed.current_bufnr() as i64
    };
    lookup_prop_type(meta, &opts.textprop, bufnr)
        .map(|_| ())
        .map_err(|_| err("E475", format!("Invalid argument: {}", opts.textprop)))
}

fn popup_create_call(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    meta: &mut MetaState,
) -> VimResult<Value> {
    arity(name, args, 2, 2)?;
    let kind = match name {
        "popup_atcursor" => PopupKind::AtCursor,
        "popup_beval" => PopupKind::Beval,
        "popup_notification" => PopupKind::Notification,
        _ => PopupKind::Normal,
    };
    let text = popup_text_from_value(ed, &arg(args, 0))?;
    let dict = as_dict(&arg(args, 1), "options")?;
    let mut popup = Popup {
        id: meta.next_popup_id,
        kind,
        text,
        opts: PopupOptions::new(kind, ed),
        hidden: false,
        line: 0,
        col: 0,
        width: 0,
        height: 0,
        core_line: 0,
        core_col: 0,
        core_width: 0,
        core_height: 0,
        firstline: 1,
        lastline: 1,
        scrollbar: false,
    };
    // `popup_create()` runs move options first and general options second.
    apply_popup_move_options(&mut popup.opts, &dict, ed)?;
    apply_popup_general_options(&mut popup.opts, &dict, ed)?;
    if let Some(hidden) = dict.get("hidden") {
        popup.hidden = hidden.truthy();
    }
    validate_popup_textprop(meta, ed, &popup.opts)?;
    layout_popup(&mut popup, ed, &meta.popups);
    meta.next_popup_id += 1;
    let id = popup.id;
    meta.popups.push(popup);
    Ok(Value::Number(id))
}

/// `popup_menu()`: needs the M4 input loop; validate, record and return -1.
fn popup_menu_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("popup_menu", args, 2, 2)?;
    let text = popup_text_from_value(ed, &arg(args, 0))?;
    let dict = as_dict(&arg(args, 1), "options")?;
    let mut popup = Popup {
        id: meta.next_popup_id,
        kind: PopupKind::Menu,
        text: text.clone(),
        opts: PopupOptions::new(PopupKind::Menu, ed),
        hidden: false,
        line: 0,
        col: 0,
        width: 0,
        height: 0,
        core_line: 0,
        core_col: 0,
        core_width: 0,
        core_height: 0,
        firstline: 1,
        lastline: 1,
        scrollbar: false,
    };
    apply_popup_move_options(&mut popup.opts, &dict, ed)?;
    apply_popup_general_options(&mut popup.opts, &dict, ed)?;
    meta.deferred_ui.push(format!(
        "popup_menu: needs the M4 input/display loop to select one of {} item(s)",
        text.len()
    ));
    Ok(Value::Number(-1))
}

fn popup_clear_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("popup_clear", args, 0, 1)?;
    if let Some(force) = args.first() {
        // The only use of {force} is closing a popup that is the current
        // window; there is no popup window focus yet, so it is validated only.
        let _ = force.truthy();
    }
    meta.popups.clear();
    Ok(Value::Number(0))
}

fn popup_close_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("popup_close", args, 1, 2)?;
    let id = get_number(&arg(args, 0))?;
    if let Some(index) = popup_index(meta, id) {
        meta.popups.remove(index);
    }
    Ok(Value::Number(0))
}

fn popup_hide_show_call(name: &str, args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity(name, args, 1, 1)?;
    let id = get_number(&arg(args, 0))?;
    if let Some(popup) = meta.popups.iter_mut().find(|p| p.id == id) {
        popup.hidden = name == "popup_hide";
    }
    Ok(Value::Number(0))
}

/// Move the popup out of `meta.popups`, mutate it, re-layout and put it back.
fn with_popup<F>(meta: &mut MetaState, id: i64, ed: &Editor, apply: F) -> VimResult<()>
where
    F: FnOnce(&mut Popup) -> VimResult<()>,
{
    let Some(index) = popup_index(meta, id) else {
        return Ok(());
    };
    let mut popup = meta.popups.remove(index);
    apply(&mut popup)?;
    layout_popup(&mut popup, ed, &meta.popups);
    meta.popups.insert(index, popup);
    Ok(())
}

fn popup_move_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("popup_move", args, 2, 2)?;
    let id = get_number(&arg(args, 0))?;
    let dict = as_dict(&arg(args, 1), "options")?;
    with_popup(meta, id, ed, |popup| {
        apply_popup_move_options(&mut popup.opts, &dict, ed)
    })?;
    if let Some(popup) = popup_by_id(meta, id) {
        validate_popup_textprop(meta, ed, &popup.opts)?;
    }
    Ok(Value::Number(0))
}

fn popup_setoptions_call(
    args: &[Value],
    ed: &mut Editor,
    meta: &mut MetaState,
) -> VimResult<Value> {
    arity("popup_setoptions", args, 2, 2)?;
    let id = get_number(&arg(args, 0))?;
    let dict = as_dict(&arg(args, 1), "options")?;
    with_popup(meta, id, ed, |popup| {
        apply_popup_move_options(&mut popup.opts, &dict, ed)?;
        apply_popup_general_options(&mut popup.opts, &dict, ed)
    })?;
    // `apply_options()` also honours "hidden", but only to hide a popup.
    if dict_bool(&dict, "hidden", false)
        && let Some(popup) = meta.popups.iter_mut().find(|p| p.id == id)
    {
        popup.hidden = true;
    }
    if let Some(popup) = popup_by_id(meta, id) {
        validate_popup_textprop(meta, ed, &popup.opts)?;
    }
    Ok(Value::Number(0))
}

fn popup_settext_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("popup_settext", args, 2, 2)?;
    let id = get_number(&arg(args, 0))?;
    let value = arg(args, 1);
    let text = match &value {
        Value::String(_) | Value::List(_) => popup_text_from_value(ed, &value)?,
        other => {
            return Err(err(
                "E475",
                format!("Invalid argument: {}", other.to_string_value()),
            ));
        }
    };
    with_popup(meta, id, ed, |popup| {
        popup.text = text;
        Ok(())
    })?;
    Ok(Value::Number(0))
}

fn popup_getpos_dict(popup: &Popup) -> VimDict {
    let mut dict = VimDict::new();
    dict.insert("line", Value::Number(popup.line));
    dict.insert("col", Value::Number(popup.col));
    dict.insert("width", Value::Number(popup.width));
    dict.insert("height", Value::Number(popup.height));
    dict.insert("core_line", Value::Number(popup.core_line));
    dict.insert("core_col", Value::Number(popup.core_col));
    dict.insert("core_width", Value::Number(popup.core_width));
    dict.insert("core_height", Value::Number(popup.core_height));
    dict.insert("scrollbar", Value::Number(i64::from(popup.scrollbar)));
    dict.insert("firstline", Value::Number(popup.firstline));
    dict.insert("lastline", Value::Number(popup.lastline));
    dict.insert("visible", Value::Number(i64::from(popup.visible())));
    dict
}

fn popup_getpos_call(args: &[Value], meta: &MetaState) -> VimResult<Value> {
    arity("popup_getpos", args, 1, 1)?;
    let id = get_number(&arg(args, 0))?;
    Ok(match popup_by_id(meta, id) {
        Some(popup) => dictionary(popup_getpos_dict(popup)),
        None => Value::dict(VimDict::new()),
    })
}

fn push_padding_border(dict: &mut VimDict, array: [i64; 4], name: &str) {
    if array.iter().all(|value| *value == 0) {
        return;
    }
    let mut items = Vec::new();
    if !array.iter().all(|value| *value == 1) {
        for value in array {
            items.push(Value::Number(value));
        }
    }
    dict.insert(name, Value::list(items));
}

fn push_borderhighlight(dict: &mut VimDict, array: &[String; 4]) {
    if array.iter().all(String::is_empty) {
        return;
    }
    dict.insert(
        "borderhighlight",
        Value::list(array.iter().cloned().map(Value::String).collect()),
    );
}

fn push_borderchars(dict: &mut VimDict, array: &[String; 8]) {
    if array.iter().all(String::is_empty) {
        return;
    }
    dict.insert(
        "borderchars",
        Value::list(array.iter().cloned().map(Value::String).collect()),
    );
}

fn popup_getoptions_dict(popup: &Popup) -> VimDict {
    let opts = &popup.opts;
    let mut dict = VimDict::new();
    dict.insert("line", Value::Number(opts.line));
    dict.insert("col", Value::Number(opts.col));
    dict.insert("minwidth", Value::Number(opts.minwidth));
    dict.insert("minheight", Value::Number(opts.minheight));
    dict.insert("maxheight", Value::Number(opts.maxheight));
    dict.insert("maxwidth", Value::Number(opts.maxwidth));
    dict.insert("firstline", Value::Number(opts.firstline));
    dict.insert("scrollbar", Value::Number(i64::from(opts.scrollbar)));
    dict.insert("zindex", Value::Number(opts.zindex));
    dict.insert("fixed", Value::Number(i64::from(opts.fixed)));
    if !opts.textprop.is_empty() {
        dict.insert("textprop", Value::String(opts.textprop.clone()));
        dict.insert("textpropwin", Value::Number(opts.textpropwin));
        dict.insert("textpropid", Value::Number(opts.textpropid));
    }
    dict.insert("title", Value::String(opts.title.clone()));
    dict.insert("wrap", Value::Number(i64::from(opts.wrap)));
    dict.insert("drag", Value::Number(i64::from(opts.drag)));
    dict.insert("dragall", Value::Number(i64::from(opts.dragall)));
    dict.insert("mapping", Value::Number(i64::from(opts.mapping)));
    dict.insert("resize", Value::Number(i64::from(opts.resize)));
    dict.insert("posinvert", Value::Number(i64::from(opts.posinvert)));
    dict.insert("cursorline", Value::Number(i64::from(opts.cursorline)));
    dict.insert("highlight", Value::String(opts.highlight.clone()));
    dict.insert(
        "tabpage",
        Value::Number(if popup.kind == PopupKind::Notification {
            -1
        } else {
            0
        }),
    );
    push_padding_border(&mut dict, opts.padding, "padding");
    push_padding_border(&mut dict, opts.border, "border");
    push_borderhighlight(&mut dict, &opts.borderhighlight);
    push_borderchars(&mut dict, &opts.borderchars);
    dict.insert(
        "moved",
        Value::list(opts.moved.iter().map(|n| Value::Number(*n)).collect()),
    );
    dict.insert(
        "mousemoved",
        Value::list(opts.mousemoved.iter().map(|n| Value::Number(*n)).collect()),
    );
    if let Some(filter) = &opts.filter {
        dict.insert("filter", filter.clone());
    }
    if let Some(callback) = &opts.callback {
        dict.insert("callback", callback.clone());
    }
    dict.insert("pos", Value::String(opts.pos.clone()));
    dict.insert("close", Value::String(opts.close.clone()));
    dict.insert("time", Value::Number(opts.time));
    dict
}

fn popup_getoptions_call(args: &[Value], meta: &MetaState) -> VimResult<Value> {
    arity("popup_getoptions", args, 1, 1)?;
    let id = get_number(&arg(args, 0))?;
    Ok(match popup_by_id(meta, id) {
        Some(popup) => dictionary(popup_getoptions_dict(popup)),
        None => Value::dict(VimDict::new()),
    })
}

/// `popup_list()`: tab-local popups (newest first), then global notifications
/// (oldest first), matching Vim's insertion order in the two popup lists.
fn popup_list_call(args: &[Value], meta: &MetaState) -> VimResult<Value> {
    arity("popup_list", args, 0, 0)?;
    let mut ids = Vec::new();
    for popup in meta.popups.iter().rev() {
        if popup.kind != PopupKind::Notification {
            ids.push(Value::Number(popup.id));
        }
    }
    for popup in &meta.popups {
        if popup.kind == PopupKind::Notification {
            ids.push(Value::Number(popup.id));
        }
    }
    Ok(Value::list(ids))
}

fn popup_locate_call(args: &[Value], meta: &MetaState) -> VimResult<Value> {
    arity("popup_locate", args, 2, 2)?;
    let row = get_number(&arg(args, 0))?;
    let col = get_number(&arg(args, 1))?;
    let mut best: Option<&Popup> = None;
    for popup in meta.popups.iter().filter(|p| p.visible()) {
        let inside = row >= popup.line
            && row < popup.line + popup.height
            && col >= popup.col
            && col < popup.col + popup.width;
        if !inside {
            continue;
        }
        let better = match best {
            None => true,
            Some(current) => (popup.opts.zindex, popup.id) > (current.opts.zindex, current.id),
        };
        if better {
            best = Some(popup);
        }
    }
    Ok(Value::Number(best.map(|p| p.id).unwrap_or(0)))
}

fn popup_findinfo_preview_call(name: &str, args: &[Value]) -> VimResult<Value> {
    arity(name, args, 0, 0)?;
    // The quickfix info/preview popups do not exist in the headless editor.
    Ok(Value::Number(0))
}

// ---------------------------------------------------------------------------
// Text property implementation (reference: textprop.c)
// ---------------------------------------------------------------------------

/// `get_bufnr_from_arg()`: the buffer a placement/type dictionary targets.
/// A missing `bufnr` or the number `0` means the current buffer.
fn prop_dict_buffer(ed: &Editor, dict: &VimDict) -> VimResult<i64> {
    match dict.get("bufnr") {
        None | Some(Value::Number(0)) => Ok(ed.current_bufnr() as i64),
        Some(value) => Ok(resolve_buffer(ed, value)? as i64),
    }
}

/// Namespace selected by a type dictionary: `None` is the global namespace.
/// A present `bufnr` (even the empty string) selects a buffer-local namespace.
fn prop_type_scope(ed: &Editor, dict: &VimDict) -> VimResult<Option<i64>> {
    match dict.get("bufnr") {
        None | Some(Value::Number(0)) => Ok(None),
        Some(value) => Ok(Some(resolve_buffer(ed, value)? as i64)),
    }
}

fn prop_type_index(meta: &MetaState, name: &str, scope: Option<i64>) -> Option<usize> {
    meta.prop_types
        .iter()
        .position(|t| t.name == name && t.bufnr == scope)
}

/// `lookup_prop_type()`: buffer-local first, then global; `E971` when absent.
fn lookup_prop_type(meta: &MetaState, name: &str, bufnr: i64) -> VimResult<usize> {
    prop_type_index(meta, name, Some(bufnr))
        .or_else(|| prop_type_index(meta, name, None))
        .ok_or_else(|| err("E971", format!("Property type {name} does not exist")))
}

enum PropTypePlacement {
    /// Name plus the buffer number of a buffer-local type (0 for global types).
    Found {
        name: String,
        type_bufnr: i64,
    },
    Missing,
}

fn prop_type_placement(meta: &MetaState, name: &str, bufnr: i64) -> PropTypePlacement {
    if prop_type_index(meta, name, Some(bufnr)).is_some() {
        PropTypePlacement::Found {
            name: name.to_string(),
            type_bufnr: bufnr,
        }
    } else if prop_type_index(meta, name, None).is_some() {
        PropTypePlacement::Found {
            name: name.to_string(),
            type_bufnr: 0,
        }
    } else {
        PropTypePlacement::Missing
    }
}

/// The line/column span of one text property.
#[derive(Debug, Clone, Copy)]
struct PropSpan {
    start_lnum: i64,
    start_col: i64,
    end_lnum: i64,
    end_col: i64,
}

/// Add one property, spanning `span.start_lnum`..`span.end_lnum`
/// (`prop_add_one()`).
fn prop_add_one(
    ed: &Editor,
    meta: &mut MetaState,
    bufnr: i64,
    type_name: &str,
    id: i64,
    span: PropSpan,
    props: &VimDict,
) -> VimResult<()> {
    let PropSpan {
        start_lnum,
        start_col,
        end_lnum,
        end_col,
    } = span;
    lookup_prop_type(meta, type_name, bufnr)?;
    let line_count = buffer_line_count(ed, bufnr as i32);
    if start_lnum < 1 || start_lnum > line_count {
        return Err(err("E966", format!("Invalid line number: {start_lnum}")));
    }
    if end_lnum < start_lnum || end_lnum > line_count {
        return Err(err("E966", format!("Invalid line number: {end_lnum}")));
    }
    for lnum in start_lnum..=end_lnum {
        let textlen = buffer_line(ed, bufnr as i32, lnum)
            .map(|line| line.len() as i64)
            .unwrap_or(0);
        let col = if lnum == start_lnum { start_col } else { 1 };
        if col - 1 > textlen {
            return Err(err("E964", format!("Invalid column number: {start_col}")));
        }
        let mut length = if lnum == end_lnum {
            end_col - col
        } else {
            textlen - col + 1
        };
        if length > textlen {
            length = textlen;
        }
        if length < 0 {
            length = 0;
        }
        meta.textprops.push(MetadataProp {
            id,
            prop_type: type_name.to_string(),
            bufnr,
            lnum,
            col,
            length,
            props: props.clone(),
            start: lnum == start_lnum,
            end: lnum == end_lnum,
        });
    }
    Ok(())
}

/// Shared by `prop_add()`: resolve the span from the placement dictionary
/// (`prop_add_common()`).
fn prop_add_common(
    ed: &Editor,
    meta: &mut MetaState,
    start_lnum: i64,
    start_col: i64,
    dict: &VimDict,
) -> VimResult<()> {
    let type_name = match dict.get("type") {
        Some(value) => get_string(value)?,
        None => return Err(err("E965", "Missing property type name")),
    };
    let mut end_lnum = match dict.get("end_lnum") {
        Some(value) => {
            let end = get_number(value)?;
            if end < start_lnum {
                return Err(err("E475", "Invalid value for argument end_lnum"));
            }
            end
        }
        None => start_lnum,
    };
    let end_col;
    if let Some(value) = dict.get("length") {
        let length = get_number(value)?;
        if length < 0 || end_lnum > start_lnum {
            return Err(err("E475", "Invalid value for argument length"));
        }
        end_col = start_col + length;
    } else if let Some(value) = dict.get("end_col") {
        let col = get_number(value)?;
        if col <= 0 {
            return Err(err("E475", "Invalid value for argument end_col"));
        }
        end_col = col;
    } else if start_lnum == end_lnum {
        end_col = start_col;
    } else {
        end_col = 1;
    }
    let id = dict_num(dict, "id").unwrap_or(0);
    let bufnr = prop_dict_buffer(ed, dict)?;
    if end_lnum == 0 {
        end_lnum = start_lnum;
    }
    prop_add_one(
        ed,
        meta,
        bufnr,
        &type_name,
        id,
        PropSpan {
            start_lnum,
            start_col,
            end_lnum,
            end_col,
        },
        dict,
    )
}

fn prop_add_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_add", args, 3, 3)?;
    let start_lnum = get_number(&arg(args, 0))?;
    let start_col = get_number(&arg(args, 1))?;
    if start_col < 1 {
        return Err(err("E964", format!("Invalid column number: {start_col}")));
    }
    let dict = as_dict(&arg(args, 2), "props")?;
    prop_add_common(ed, meta, start_lnum, start_col, &dict)?;
    Ok(Value::Number(0))
}

fn prop_add_list_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_add_list", args, 2, 2)?;
    let dict = as_dict(&arg(args, 0), "props")?;
    let type_name = match dict.get("type") {
        Some(value) => get_string(value)?,
        None => return Err(err("E965", "Missing property type name")),
    };
    let id = dict_num(&dict, "id").unwrap_or(0);
    let bufnr = prop_dict_buffer(ed, &dict)?;
    let positions = as_list(&arg(args, 1), "positions")?;
    for item in &positions {
        let values = as_list(item, "position")?;
        if values.len() < 4 {
            return Err(err("E474", "Invalid argument"));
        }
        let start_lnum = get_number(&values[0])?;
        let start_col = get_number(&values[1])?;
        let end_lnum = get_number(&values[2])?;
        let end_col = get_number(&values[3])?;
        if start_lnum <= 0 || start_col <= 0 || end_lnum <= 0 || end_col <= 0 {
            return Err(err("E474", "Invalid argument"));
        }
        prop_add_one(
            ed,
            meta,
            bufnr,
            &type_name,
            id,
            PropSpan {
                start_lnum,
                start_col,
                end_lnum,
                end_col,
            },
            &dict,
        )?;
    }
    Ok(Value::Number(0))
}

fn prop_clear_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_clear", args, 1, 3)?;
    let start = get_number(&arg(args, 0))?;
    let end = if args.len() > 1 {
        get_number(&arg(args, 1))?
    } else {
        start
    };
    let mut bufnr = ed.current_bufnr() as i64;
    if args.len() > 2 {
        let dict = as_dict(&arg(args, 2), "bufnr")?;
        bufnr = prop_dict_buffer(ed, &dict)?;
    }
    if start < 1 || end < 1 {
        return Err(err("E16", "Invalid range"));
    }
    meta.textprops
        .retain(|p| !(p.bufnr == bufnr && p.lnum >= start && p.lnum <= end));
    Ok(Value::Number(0))
}

/// The dictionary `prop_list()`/`prop_find()` return for one property
/// (`prop_fill_dict()`).
fn prop_fill_dict(meta: &MetaState, prop: &MetadataProp, with_lnum: bool) -> VimDict {
    let mut dict = VimDict::new();
    dict.insert("col", Value::Number(prop.col));
    dict.insert("length", Value::Number(prop.length));
    dict.insert("id", Value::Number(prop.id));
    dict.insert("start", Value::Number(i64::from(prop.start)));
    dict.insert("end", Value::Number(i64::from(prop.end)));
    if let PropTypePlacement::Found { name, type_bufnr } =
        prop_type_placement(meta, &prop.prop_type, prop.bufnr)
    {
        dict.insert("type", Value::String(name));
        dict.insert("type_bufnr", Value::Number(type_bufnr));
    } else {
        // Vim's prop_fill_dict() reports the buffer as the type namespace when
        // the type has been deleted after the property was placed.
        dict.insert("type_bufnr", Value::Number(prop.bufnr));
    }
    if with_lnum {
        dict.insert("lnum", Value::Number(prop.lnum));
    }
    dict
}

fn prop_covering_start<'a>(
    meta: &'a MetaState,
    prop: &MetadataProp,
    bufnr: i64,
) -> Option<&'a MetadataProp> {
    // Find the entry where this multi-line property starts.
    meta.textprops
        .iter()
        .filter(|p| {
            p.start
                && p.bufnr == bufnr
                && p.id == prop.id
                && p.prop_type == prop.prop_type
                && p.lnum <= prop.lnum
        })
        .max_by_key(|p| p.lnum)
}

fn prop_covers_position(meta: &MetaState, prop: &MetadataProp, lnum: i64, col: i64) -> bool {
    let Some(start) = prop_covering_start(meta, prop, prop.bufnr) else {
        return false;
    };
    let end_lnum = meta
        .textprops
        .iter()
        .filter(|p| {
            p.bufnr == prop.bufnr
                && p.id == prop.id
                && p.prop_type == prop.prop_type
                && p.lnum >= start.lnum
        })
        .max_by_key(|p| p.lnum)
        .map(|p| p.lnum)
        .unwrap_or(start.lnum);
    if lnum < start.lnum || lnum > end_lnum {
        return false;
    }
    if lnum > start.lnum {
        return true;
    }
    let last = start.col + start.length - i64::from(start.length != 0);
    col >= start.col && col <= last
}

fn prop_find_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_find", args, 1, 2)?;
    let dict = as_dict(&arg(args, 0), "props")?;
    let mut backward = false;
    if args.len() > 1 {
        let direction = get_string(&arg(args, 1))?;
        if direction.starts_with('b') {
            backward = true;
        } else if !direction.starts_with('f') {
            return Err(err("E474", "Invalid argument"));
        }
    }
    let bufnr = prop_dict_buffer(ed, &dict)?;
    let line_count = buffer_line_count(ed, bufnr as i32);
    let mut lnum = dict_num(&dict, "lnum").unwrap_or(-1);
    let mut col = dict_num(&dict, "col").unwrap_or(-1);
    if lnum == -1 {
        lnum = ed.cursor().line as i64 + 1;
        col = ed.cursor().col as i64 + 1;
    } else if col == -1 {
        col = 1;
    }
    if lnum < 1 || lnum > line_count {
        return Err(err("E16", "Invalid range"));
    }
    let skipstart = dict_bool(&dict, "skipstart", false);
    let id = dict.get("id").map(Value::to_number);
    let type_name = match dict.get("type") {
        Some(value) => Some(get_string(value)?),
        None => None,
    };
    if let Some(name) = &type_name {
        lookup_prop_type(meta, name, bufnr)?;
    }
    let both = dict_bool(&dict, "both", false);
    if id.is_none() && type_name.is_none() {
        return Err(err("E968", "Need at least one of 'id' or 'type'"));
    }
    if both && (id.is_none() || type_name.is_none()) {
        return Err(err("E860", "Need 'id' and 'type' with 'both'"));
    }
    let matches = |prop: &MetadataProp| -> bool {
        match (id, &type_name) {
            (Some(id), Some(name)) if both => prop.id == id && prop.prop_type == *name,
            _ => {
                id.is_some_and(|id| prop.id == id)
                    || type_name
                        .as_ref()
                        .is_some_and(|name| prop.prop_type == *name)
            }
        }
    };
    let mut candidates: Vec<&MetadataProp> = meta
        .textprops
        .iter()
        .filter(|p| p.bufnr == bufnr && matches(p))
        .collect();
    candidates.sort_by_key(|p| (p.lnum, p.col));
    let ordered: Vec<&MetadataProp> = if backward {
        candidates.into_iter().rev().collect()
    } else {
        candidates
    };
    for prop in ordered {
        if prop.lnum > line_count {
            continue;
        }
        if backward {
            if prop.lnum > lnum {
                continue;
            }
        } else if prop.lnum < lnum {
            continue;
        }
        if prop.lnum == lnum {
            if backward {
                if prop.col > col {
                    continue;
                }
            } else {
                let last = prop.col + prop.length - i64::from(prop.length != 0);
                if last < col {
                    continue;
                }
            }
        }
        if skipstart && prop_covers_position(meta, prop, lnum, col) {
            continue;
        }
        let result = if prop.start || skipstart {
            prop
        } else {
            prop_covering_start(meta, prop, bufnr).unwrap_or(prop)
        };
        return Ok(dictionary(prop_fill_dict(meta, result, true)));
    }
    Ok(Value::dict(VimDict::new()))
}

fn prop_list_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_list", args, 1, 2)?;
    let start_lnum = get_number(&arg(args, 0))?;
    let mut end_lnum = start_lnum;
    let mut add_lnum = false;
    let mut bufnr = ed.current_bufnr() as i64;
    let mut types: Option<Vec<String>> = None;
    let mut ids: Option<Vec<i64>> = None;
    if args.len() > 1 {
        let dict = as_dict(&arg(args, 1), "options")?;
        bufnr = prop_dict_buffer(ed, &dict)?;
        if let Some(value) = dict.get("end_lnum") {
            if !matches!(value, Value::Number(_)) {
                return Err(err("E521", "Number required after ="));
            }
            let requested = get_number(value)?;
            end_lnum = if requested < 0 {
                buffer_line_count(ed, bufnr as i32) + requested + 1
            } else {
                requested.min(buffer_line_count(ed, bufnr as i32))
            };
            add_lnum = true;
        }
        if let Some(value) = dict.get("types") {
            let items = as_list(value, "types")?;
            let mut names = Vec::new();
            for item in &items {
                let name = get_string(item)?;
                lookup_prop_type(meta, &name, bufnr)?;
                names.push(name);
            }
            if !names.is_empty() {
                types = Some(names);
            }
        }
        if let Some(value) = dict.get("ids") {
            let items = as_list(value, "ids")?;
            let mut numbers = Vec::new();
            for item in &items {
                numbers.push(get_number(item)?);
            }
            if !numbers.is_empty() {
                ids = Some(numbers);
            }
        }
    }
    let line_count = buffer_line_count(ed, bufnr as i32);
    if start_lnum < 1 || start_lnum > line_count || end_lnum < 1 || end_lnum < start_lnum {
        return Err(err("E16", "Invalid range"));
    }
    let mut matched: Vec<&MetadataProp> = meta
        .textprops
        .iter()
        .filter(|p| {
            p.bufnr == bufnr
                && p.lnum >= start_lnum
                && p.lnum <= end_lnum
                && types
                    .as_ref()
                    .is_none_or(|names| names.contains(&p.prop_type))
                && ids.as_ref().is_none_or(|numbers| numbers.contains(&p.id))
        })
        .collect();
    matched.sort_by_key(|p| (p.lnum, p.col));
    let items = matched
        .into_iter()
        .map(|prop| dictionary(prop_fill_dict(meta, prop, add_lnum)))
        .collect();
    Ok(Value::list(items))
}

fn prop_remove_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_remove", args, 1, 3)?;
    let dict = match &arg(args, 0) {
        Value::Dict(d) => d.borrow().clone(),
        _ => return Err(err("E474", "Invalid argument")),
    };
    let bufnr = prop_dict_buffer(ed, &dict)?;
    let line_count = buffer_line_count(ed, bufnr as i32);
    let mut start = 1i64;
    let mut end = line_count;
    if args.len() > 1 {
        start = get_number(&arg(args, 1))?;
        end = if args.len() > 2 {
            get_number(&arg(args, 2))?
        } else {
            start
        };
        if start < 1 || end < 1 {
            return Err(err("E16", "Invalid range"));
        }
    }
    let do_all = dict_bool(&dict, "all", false);
    let id = dict.get("id").map(Value::to_number);
    let type_name = match dict.get("type") {
        Some(value) => Some(get_string(value)?),
        None => None,
    };
    if let Some(name) = &type_name {
        lookup_prop_type(meta, name, bufnr)?;
    }
    let both = dict_bool(&dict, "both", false);
    if id.is_none() && type_name.is_none() {
        return Err(err("E968", "Need at least one of 'id' or 'type'"));
    }
    if both && (id.is_none() || type_name.is_none()) {
        return Err(err("E860", "Need 'id' and 'type' with 'both'"));
    }
    let matches = |prop: &MetadataProp| -> bool {
        match (id, &type_name) {
            (Some(id), Some(name)) if both => prop.id == id && prop.prop_type == *name,
            _ => {
                id.is_some_and(|id| prop.id == id)
                    || type_name
                        .as_ref()
                        .is_some_and(|name| prop.prop_type == *name)
            }
        }
    };
    let line_count = buffer_line_count(ed, bufnr as i32);
    let mut lines_done: Vec<i64> = Vec::new();
    let mut remove = vec![false; meta.textprops.len()];
    let mut removed = 0i64;
    for (index, prop) in meta.textprops.iter().enumerate() {
        if prop.bufnr != bufnr
            || prop.lnum < start
            || prop.lnum > end
            || prop.lnum > line_count
            || !matches(prop)
        {
            continue;
        }
        if !do_all && lines_done.contains(&prop.lnum) {
            continue;
        }
        lines_done.push(prop.lnum);
        remove[index] = true;
        removed += 1;
    }
    let mut index = 0usize;
    meta.textprops.retain(|_| {
        let keep = !remove[index];
        index += 1;
        keep
    });
    Ok(Value::Number(removed))
}

/// Shared by `prop_type_add()` and `prop_type_change()` (`prop_type_set()`).
fn prop_type_set_call(
    name: &str,
    dict: &VimDict,
    ed: &Editor,
    meta: &mut MetaState,
    add: bool,
) -> VimResult<Value> {
    let type_name = name.to_string();
    if type_name.is_empty() {
        return Err(err("E474", "Invalid argument"));
    }
    let scope = prop_type_scope(ed, dict)?;
    let index = if add {
        if prop_type_index(meta, &type_name, scope).is_some() {
            return Err(err(
                "E969",
                format!("Property type {type_name} already defined"),
            ));
        }
        let id = meta.next_prop_type_id;
        meta.next_prop_type_id += 1;
        meta.prop_types.push(PropType {
            name: type_name.clone(),
            bufnr: scope,
            id,
            highlight: String::new(),
            priority: 0,
            combine: true,
            start_incl: false,
            end_incl: false,
        });
        meta.prop_types.len() - 1
    } else {
        prop_type_index(meta, &type_name, scope)
            .ok_or_else(|| err("E971", format!("Property type {type_name} does not exist")))?
    };
    let prop = &mut meta.prop_types[index];
    if let Some(value) = dict.get("highlight") {
        let highlight = get_string(value)?;
        prop.highlight = highlight;
    }
    if let Some(value) = dict.get("combine") {
        prop.combine = value.truthy();
    }
    if let Some(value) = dict.get("priority") {
        prop.priority = get_number(value)?;
    }
    if let Some(value) = dict.get("start_incl") {
        prop.start_incl = value.truthy();
    }
    if let Some(value) = dict.get("end_incl") {
        prop.end_incl = value.truthy();
    }
    Ok(Value::Number(0))
}

fn prop_type_add_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_type_add", args, 2, 2)?;
    let name = get_string(&arg(args, 0))?;
    let dict = as_dict(&arg(args, 1), "props")?;
    prop_type_set_call(&name, &dict, ed, meta, true)
}

fn prop_type_change_call(
    args: &[Value],
    ed: &mut Editor,
    meta: &mut MetaState,
) -> VimResult<Value> {
    arity("prop_type_change", args, 2, 2)?;
    let name = get_string(&arg(args, 0))?;
    let dict = as_dict(&arg(args, 1), "props")?;
    prop_type_set_call(&name, &dict, ed, meta, false)
}

fn prop_type_delete_call(
    args: &[Value],
    ed: &mut Editor,
    meta: &mut MetaState,
) -> VimResult<Value> {
    arity("prop_type_delete", args, 1, 2)?;
    let name = get_string(&arg(args, 0))?;
    if name.is_empty() {
        return Err(err("E474", "Invalid argument"));
    }
    let mut scope = None;
    if args.len() > 1 {
        let dict = as_dict(&arg(args, 1), "props")?;
        scope = prop_type_scope(ed, &dict)?;
    }
    if let Some(index) = prop_type_index(meta, &name, scope) {
        meta.prop_types.remove(index);
    }
    Ok(Value::Number(0))
}

fn prop_type_get_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_type_get", args, 1, 2)?;
    let name = get_string(&arg(args, 0))?;
    if name.is_empty() {
        return Err(err("E474", "Invalid argument"));
    }
    let mut scope = None;
    if args.len() > 1 {
        let dict = as_dict(&arg(args, 1), "props")?;
        scope = prop_type_scope(ed, &dict)?;
    }
    let Some(index) = prop_type_index(meta, &name, scope) else {
        return Ok(Value::dict(VimDict::new()));
    };
    let prop = &meta.prop_types[index];
    let mut dict = VimDict::new();
    if !prop.highlight.is_empty() {
        dict.insert("highlight", Value::String(prop.highlight.clone()));
    }
    dict.insert("priority", Value::Number(prop.priority));
    dict.insert("combine", Value::Number(i64::from(prop.combine)));
    dict.insert("start_incl", Value::Number(i64::from(prop.start_incl)));
    dict.insert("end_incl", Value::Number(i64::from(prop.end_incl)));
    if let Some(bufnr) = prop.bufnr {
        dict.insert("bufnr", Value::Number(bufnr));
    }
    Ok(dictionary(dict))
}

fn prop_type_list_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("prop_type_list", args, 0, 1)?;
    let mut scope = None;
    if let Some(value) = args.first() {
        let dict = as_dict(value, "props")?;
        scope = prop_type_scope(ed, &dict)?;
    }
    let names = meta
        .prop_types
        .iter()
        .filter(|t| t.bufnr == scope)
        .map(|t| Value::String(t.name.clone()))
        .collect();
    Ok(Value::list(names))
}

// ---------------------------------------------------------------------------
// Sign implementation (reference: sign.c)
// ---------------------------------------------------------------------------

fn sign_def_index(meta: &MetaState, name: &str) -> Option<usize> {
    meta.sign_defs.iter().position(|d| d.name == name)
}

/// `sign_in_group()`: `None` selects the global group only, `Some("*")` all
/// groups, `Some(name)` one named group.
fn sign_in_group(place_group: &str, filter: Option<&str>) -> bool {
    match filter {
        None => place_group.is_empty(),
        Some("*") => true,
        Some(name) => place_group == name,
    }
}

/// Define or update one sign; returns 0 on success and -1 for an empty name
/// (Vim is silent in that case).
fn sign_define_one(meta: &mut MetaState, name: &str, dict: Option<&VimDict>) -> VimResult<i64> {
    if name.is_empty() {
        return Ok(-1);
    }
    let mut icon = None;
    let mut text = None;
    let mut linehl = None;
    let mut texthl = None;
    let mut culhl = None;
    let mut numhl = None;
    if let Some(dict) = dict {
        if let Some(value) = dict.get("icon") {
            icon = Some(get_string(value)?);
        }
        if let Some(value) = dict.get("text") {
            text = Some(get_string(value)?);
        }
        if let Some(value) = dict.get("linehl") {
            linehl = Some(get_string(value)?);
        }
        if let Some(value) = dict.get("texthl") {
            texthl = Some(get_string(value)?);
        }
        if let Some(value) = dict.get("culhl") {
            culhl = Some(get_string(value)?);
        }
        if let Some(value) = dict.get("numhl") {
            numhl = Some(get_string(value)?);
        }
    }
    if let Some(raw) = &text {
        // Backslashes are removed so a space can be used; the result must be
        // printable and one or two display cells wide (`sign_define_init_text`).
        let cleaned: String = {
            let mut out = String::new();
            let mut chars = raw.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\' && chars.peek().is_some() {
                    out.push(chars.next().unwrap());
                } else {
                    out.push(c);
                }
            }
            out
        };
        let cells = text_cells(&cleaned);
        let printable = cleaned.chars().all(|c| !c.is_control());
        if !printable || !(1..=2).contains(&cells) {
            return Err(err("E239", format!("Invalid sign text: {cleaned}")));
        }
        let padded = if cells == 1 {
            format!("{cleaned} ")
        } else {
            cleaned
        };
        text = Some(padded);
    }
    let index = sign_def_index(meta, name);
    let def = match index {
        Some(index) => &mut meta.sign_defs[index],
        None => {
            meta.sign_defs.push(SignDef {
                name: name.to_string(),
                icon: None,
                text: None,
                linehl: None,
                texthl: None,
                culhl: None,
                numhl: None,
            });
            meta.sign_defs.last_mut().unwrap()
        }
    };
    let set_or_clear = |slot: &mut Option<String>, value: Option<String>| {
        if let Some(value) = value {
            *slot = if value.is_empty() { None } else { Some(value) };
        }
    };
    if icon.is_some() {
        def.icon = icon;
    }
    if text.is_some() {
        def.text = text;
    }
    set_or_clear(&mut def.linehl, linehl);
    set_or_clear(&mut def.texthl, texthl);
    set_or_clear(&mut def.culhl, culhl);
    set_or_clear(&mut def.numhl, numhl);
    Ok(0)
}

fn sign_define_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("sign_define", args, 1, 2)?;
    if matches!(arg(args, 0), Value::List(_)) && args.len() == 1 {
        let items = as_list(&arg(args, 0), "signs")?;
        let mut results = Vec::new();
        for item in &items {
            let result = match item {
                Value::Dict(d) => {
                    let dict = d.borrow().clone();
                    let name = dict_string(&dict, "name").unwrap_or_default();
                    sign_define_one(meta, &name, Some(&dict))?
                }
                _ => -1,
            };
            results.push(Value::Number(result));
        }
        return Ok(Value::list(results));
    }
    let name = get_string(&arg(args, 0))?;
    let dict = if args.len() > 1 {
        if !matches!(arg(args, 1), Value::Dict(_)) {
            return Err(err("E715", "Dictionary required"));
        }
        Some(as_dict(&arg(args, 1), "attributes")?)
    } else {
        None
    };
    Ok(Value::Number(sign_define_one(meta, &name, dict.as_ref())?))
}

fn sign_def_dict(def: &SignDef) -> VimDict {
    let mut dict = VimDict::new();
    dict.insert("name", Value::String(def.name.clone()));
    if let Some(icon) = &def.icon {
        dict.insert("icon", Value::String(icon.clone()));
    }
    if let Some(text) = &def.text {
        dict.insert("text", Value::String(text.clone()));
    }
    for (key, value) in [
        ("linehl", &def.linehl),
        ("texthl", &def.texthl),
        ("culhl", &def.culhl),
        ("numhl", &def.numhl),
    ] {
        if let Some(value) = value {
            dict.insert(key, Value::String(value.clone()));
        }
    }
    dict
}

fn sign_getdefined_call(args: &[Value], meta: &MetaState) -> VimResult<Value> {
    arity("sign_getdefined", args, 0, 1)?;
    let mut items = Vec::new();
    if let Some(value) = args.first() {
        let name = get_string(value)?;
        if let Some(def) = meta.sign_defs.iter().find(|d| d.name == name) {
            items.push(dictionary(sign_def_dict(def)));
        }
    } else {
        for def in &meta.sign_defs {
            items.push(dictionary(sign_def_dict(def)));
        }
    }
    Ok(Value::list(items))
}

fn sign_place_dict(meta: &MetaState, place: &SignPlace) -> VimDict {
    let mut dict = VimDict::new();
    dict.insert("id", Value::Number(place.id));
    dict.insert("group", Value::String(place.group.clone()));
    dict.insert("lnum", Value::Number(place.lnum));
    let name = if meta.sign_defs.iter().any(|d| d.name == place.name) {
        place.name.clone()
    } else {
        String::new()
    };
    dict.insert("name", Value::String(name));
    dict.insert("priority", Value::Number(place.priority));
    dict
}

/// The signs list of one buffer in `sign_getplaced()` shape.
fn sign_placed_in_buffer(
    meta: &MetaState,
    bufnr: i64,
    lnum: i64,
    id: i64,
    group: Option<&str>,
) -> Vec<Value> {
    let mut places: Vec<&SignPlace> = meta
        .sign_places
        .iter()
        .filter(|p| {
            p.bufnr == bufnr
                && sign_in_group(&p.group, group)
                && ((lnum == 0 && id == 0)
                    || (id == 0 && lnum == p.lnum)
                    || (lnum == 0 && id == p.id)
                    || (lnum == p.lnum && id == p.id))
        })
        .collect();
    places.sort_by_key(|p| (p.lnum, p.priority, p.id));
    places
        .into_iter()
        .map(|place| dictionary(sign_place_dict(meta, place)))
        .collect()
}

fn sign_getplaced_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("sign_getplaced", args, 0, 2)?;
    let mut lnum = 0i64;
    let mut id = 0i64;
    let mut group: Option<String> = None;
    let mut dict = None;
    if args.len() > 1 {
        dict = Some(as_dict(&arg(args, 1), "options")?);
    }
    if let Some(dict) = &dict {
        if let Some(value) = dict.get("lnum") {
            lnum = get_number(value)?;
        }
        if let Some(value) = dict.get("id") {
            id = get_number(value)?;
        }
        if let Some(value) = dict.get("group") {
            let name = get_string(value)?;
            if !name.is_empty() {
                group = Some(name);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(value) = args.first() {
        let bufnr = resolve_sign_buffer(ed, value)? as i64;
        let mut entry = VimDict::new();
        entry.insert("bufnr", Value::Number(bufnr));
        entry.insert(
            "signs",
            Value::list(sign_placed_in_buffer(
                meta,
                bufnr,
                lnum,
                id,
                group.as_deref(),
            )),
        );
        out.push(dictionary(entry));
    } else {
        let buffers: Vec<i64> = ed.all_buffers().iter().map(|b| b.id as i64).collect();
        for bufnr in buffers {
            if !meta.sign_places.iter().any(|p| p.bufnr == bufnr) {
                continue;
            }
            let mut entry = VimDict::new();
            entry.insert("bufnr", Value::Number(bufnr));
            entry.insert(
                "signs",
                Value::list(sign_placed_in_buffer(meta, bufnr, 0, id, group.as_deref())),
            );
            out.push(dictionary(entry));
        }
    }
    Ok(Value::list(out))
}

fn sign_jump_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("sign_jump", args, 3, 3)?;
    let id = get_number(&arg(args, 0))?;
    if id <= 0 {
        return Err(err("E474", "Invalid argument"));
    }
    let group = get_string(&arg(args, 1))?;
    let group = if group.is_empty() { None } else { Some(group) };
    let bufnr = resolve_sign_buffer(ed, &arg(args, 2))? as i64;
    let lnum = meta
        .sign_places
        .iter()
        .find(|p| p.id == id && p.bufnr == bufnr && sign_in_group(&p.group, group.as_deref()))
        .map(|p| p.lnum)
        .ok_or_else(|| err("E157", format!("Invalid sign ID: {id}")))?;
    if ed.current_bufnr() as i64 != bufnr {
        ed.switch_buffer(bufnr as i32)?;
    }
    ed.set_cursor(Pos::new(lnum.saturating_sub(1).max(0) as usize, 0));
    Ok(Value::Number(lnum))
}

/// Place (or update) a sign (`sign_place()`); returns the sign id or -1 for
/// silent failures.
fn place_sign(
    meta: &mut MetaState,
    group: &str,
    name: &str,
    bufnr: i64,
    mut id: i64,
    lnum: i64,
    priority: i64,
) -> VimResult<i64> {
    // A group name may not start with '*'.
    if group.starts_with('*') {
        return Ok(-1);
    }
    if sign_def_index(meta, name).is_none() {
        return Err(err("E155", format!("Unknown sign: {name}")));
    }
    if id == 0 {
        let mut next = if group.is_empty() {
            meta.next_sign_id
        } else {
            *meta.sign_group_ids.get(group).unwrap_or(&1)
        };
        loop {
            let used = meta
                .sign_places
                .iter()
                .any(|p| p.bufnr == bufnr && p.group == group && p.id == next);
            if !used {
                break;
            }
            next += 1;
        }
        id = next;
        if group.is_empty() {
            meta.next_sign_id = next + 1;
        } else {
            meta.sign_group_ids.insert(group.to_string(), next + 1);
        }
    }
    if lnum > 0 {
        if let Some(place) = meta
            .sign_places
            .iter_mut()
            .find(|p| p.bufnr == bufnr && p.group == group && p.id == id && p.lnum == lnum)
        {
            place.name = name.to_string();
            place.priority = priority;
        } else {
            meta.sign_places.push(SignPlace {
                group: group.to_string(),
                bufnr,
                lnum,
                id,
                name: name.to_string(),
                priority,
            });
        }
    } else {
        // No line given: change the type/priority of the placed sign.
        let Some(place) = meta
            .sign_places
            .iter_mut()
            .find(|p| p.bufnr == bufnr && p.group == group && p.id == id)
        else {
            return Err(err("E885", format!("Not possible to change sign {name}")));
        };
        place.name = name.to_string();
        place.priority = priority;
    }
    Ok(id)
}

fn sign_place_from_dict(
    meta: &mut MetaState,
    ed: &Editor,
    id_value: Option<&Value>,
    group_value: Option<&Value>,
    name_value: Option<&Value>,
    buffer_value: Option<&Value>,
    dict: Option<&VimDict>,
) -> VimResult<i64> {
    let id = match id_value {
        Some(value) => get_number(value)?,
        None => dict.and_then(|d| dict_num(d, "id")).unwrap_or(0),
    };
    if id < 0 {
        return Err(err("E474", "Invalid argument"));
    }
    let group = match group_value {
        Some(value) => get_string(value)?,
        None => dict
            .and_then(|d| dict_string(d, "group"))
            .unwrap_or_default(),
    };
    let name = match name_value {
        Some(value) => get_string(value)?,
        None => match dict.and_then(|d| dict_string(d, "name")) {
            Some(name) => name,
            None => return Ok(-1),
        },
    };
    let buffer_value = match buffer_value {
        Some(value) => value.clone(),
        None => match dict.and_then(|d| d.get("buffer")) {
            Some(value) => value.clone(),
            None => return Ok(-1),
        },
    };
    let bufnr = resolve_sign_buffer(ed, &buffer_value)? as i64;
    let lnum = match dict.and_then(|d| d.get("lnum")) {
        Some(value) => {
            let lnum = sign_lnum(value, ed)?;
            if lnum <= 0 {
                return Err(err("E474", "Invalid argument"));
            }
            lnum
        }
        None => 0,
    };
    let priority = dict
        .and_then(|d| dict_num(d, "priority"))
        .unwrap_or(SIGN_DEF_PRIO);
    place_sign(meta, &group, &name, bufnr, id, lnum, priority)
}

/// Resolve a `lnum` dict entry with `tv_get_lnum()` semantics.
fn sign_lnum(value: &Value, ed: &Editor) -> VimResult<i64> {
    match value {
        Value::Number(n) => Ok(*n),
        Value::String(text) if text == "$" => Ok(buffer_line_count(ed, ed.current_bufnr())),
        Value::String(text) if text == "." => Ok(ed.cursor().line as i64 + 1),
        Value::String(text) if text.starts_with('\'') => {
            let name = text.chars().nth(1).unwrap_or('"');
            Ok(ed
                .marks
                .resolve(ed.current_bufnr(), name)
                .map(|pos| pos.line as i64 + 1)
                .unwrap_or(-1))
        }
        other => Ok(other.to_number()),
    }
}

fn sign_place_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("sign_place", args, 4, 5)?;
    let dict = if args.len() > 4 {
        if !matches!(arg(args, 4), Value::Dict(_)) {
            return Err(err("E715", "Dictionary required"));
        }
        Some(as_dict(&arg(args, 4), "options")?)
    } else {
        None
    };
    let id = sign_place_from_dict(
        meta,
        ed,
        Some(&arg(args, 0)),
        Some(&arg(args, 1)),
        Some(&arg(args, 2)),
        Some(&arg(args, 3)),
        dict.as_ref(),
    )?;
    Ok(Value::Number(id))
}

fn sign_placelist_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("sign_placelist", args, 1, 1)?;
    let items = as_list(&arg(args, 0), "signs")?;
    let mut results = Vec::new();
    for item in &items {
        match item {
            Value::Dict(d) => {
                let dict = d.borrow().clone();
                let id = sign_place_from_dict(meta, ed, None, None, None, None, Some(&dict))?;
                results.push(Value::Number(id));
            }
            _ => return Err(err("E715", "Dictionary required")),
        }
    }
    Ok(Value::list(results))
}

fn sign_undefine_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("sign_undefine", args, 0, 1)?;
    if matches!(arg(args, 0), Value::List(_)) && args.len() == 1 {
        let items = as_list(&arg(args, 0), "signs")?;
        let mut results = Vec::new();
        for item in &items {
            let name = get_string(item)?;
            let result = match sign_def_index(meta, &name) {
                Some(index) => {
                    meta.sign_defs.remove(index);
                    0
                }
                None => -1,
            };
            results.push(Value::Number(result));
        }
        return Ok(Value::list(results));
    }
    if args.is_empty() {
        meta.sign_defs.clear();
        return Ok(Value::Number(0));
    }
    let name = get_string(&arg(args, 0))?;
    match sign_def_index(meta, &name) {
        Some(index) => {
            meta.sign_defs.remove(index);
            Ok(Value::Number(0))
        }
        None => Err(err("E155", format!("Unknown sign: {name}"))),
    }
}

/// Remove places matching `id`/`group` in one buffer; `Ok(true)` when the
/// per-buffer operation succeeded (Vim's `sign_unplace()` result).
fn unplace_in_buffer(meta: &mut MetaState, bufnr: i64, id: i64, group: Option<&str>) -> bool {
    if id == 0 {
        meta.sign_places
            .retain(|p| !(p.bufnr == bufnr && sign_in_group(&p.group, group)));
        return true;
    }
    let Some(index) = meta
        .sign_places
        .iter()
        .position(|p| p.bufnr == bufnr && p.id == id && sign_in_group(&p.group, group))
    else {
        return false;
    };
    meta.sign_places.remove(index);
    true
}

fn sign_unplace_from_dict(
    meta: &mut MetaState,
    ed: &Editor,
    group_value: Option<&Value>,
    dict: Option<&VimDict>,
) -> VimResult<i64> {
    let raw_group = match group_value {
        Some(value) => get_string(value)?,
        None => dict
            .and_then(|d| dict_string(d, "group"))
            .unwrap_or_default(),
    };
    let group = if raw_group.is_empty() {
        None
    } else {
        Some(raw_group.as_str())
    };
    let mut id = 0i64;
    let mut bufnr: Option<i64> = None;
    if let Some(dict) = dict {
        if let Some(value) = dict.get("buffer") {
            bufnr = Some(resolve_sign_buffer(ed, value)? as i64);
        }
        if let Some(value) = dict.get("id") {
            id = get_number(value)?;
            if id <= 0 {
                return Err(err("E474", "Invalid argument"));
            }
        }
    }
    match bufnr {
        Some(bufnr) => {
            let ok = unplace_in_buffer(meta, bufnr, id, group);
            Ok(if ok { 0 } else { -1 })
        }
        None => {
            let mut result = 0;
            let buffers: Vec<i64> = ed.all_buffers().iter().map(|b| b.id as i64).collect();
            for bufnr in buffers {
                let has_signs = meta.sign_places.iter().any(|p| p.bufnr == bufnr);
                if !has_signs {
                    continue;
                }
                if !unplace_in_buffer(meta, bufnr, id, group) {
                    result = -1;
                }
            }
            Ok(result)
        }
    }
}

fn sign_unplace_call(args: &[Value], ed: &mut Editor, meta: &mut MetaState) -> VimResult<Value> {
    arity("sign_unplace", args, 1, 2)?;
    if !matches!(arg(args, 0), Value::String(_)) {
        return Err(err("E474", "Invalid argument"));
    }
    let dict = if args.len() > 1 {
        if !matches!(arg(args, 1), Value::Dict(_)) {
            return Err(err("E715", "Dictionary required"));
        }
        Some(as_dict(&arg(args, 1), "options")?)
    } else {
        None
    };
    let result = sign_unplace_from_dict(meta, ed, Some(&arg(args, 0)), dict.as_ref())?;
    Ok(Value::Number(result))
}

fn sign_unplacelist_call(
    args: &[Value],
    ed: &mut Editor,
    meta: &mut MetaState,
) -> VimResult<Value> {
    arity("sign_unplacelist", args, 1, 1)?;
    let items = as_list(&arg(args, 0), "signs")?;
    let mut results = Vec::new();
    for item in &items {
        match item {
            Value::Dict(d) => {
                let dict = d.borrow().clone();
                let result = sign_unplace_from_dict(meta, ed, None, Some(&dict))?;
                results.push(Value::Number(result));
            }
            _ => return Err(err("E715", "Dictionary required")),
        }
    }
    Ok(Value::list(results))
}

// ---------------------------------------------------------------------------
// Match highlighting implementation (reference: match.c)
// ---------------------------------------------------------------------------

fn match_index(meta: &MetaState, id: i64) -> Option<usize> {
    meta.matches
        .iter()
        .position(|(match_id, _)| *match_id == id)
}

/// Insert a match keeping the list ordered by priority (ascending), stable for
/// equal priorities — the same order `matchadd()` maintains.
fn insert_match(meta: &mut MetaState, prio: i64, id: i64, dict: VimDict) {
    let position = meta
        .matches
        .iter()
        .position(|(_, existing)| dict_num(existing, "priority").unwrap_or(10) > prio)
        .unwrap_or(meta.matches.len());
    meta.matches.insert(position, (id, dict));
}

fn next_unused_match_id(meta: &mut MetaState) -> i64 {
    loop {
        let id = meta.next_match_id;
        meta.next_match_id += 1;
        if match_index(meta, id).is_none() {
            return id;
        }
    }
}

/// The single window model: `0`, `1` and the current window id `1000` are the
/// current window; anything else is `E957`.
fn check_match_window(value: Option<&Value>) -> VimResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let number = get_number(value)?;
    if number == 0 || number == 1 || number == 1000 {
        return Ok(());
    }
    Err(err("E957", format!("Invalid window number: {number}")))
}

/// Parse the optional `{dict}` argument of `matchadd()`/`matchaddpos()` and
/// return the conceal character.
fn match_dict_arg(value: Option<&Value>) -> VimResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let dict = as_dict(value, "options")?;
    if let Some(window) = dict.get("window") {
        check_match_window(Some(window))?;
    }
    Ok(dict.get("conceal").map(Value::to_string_value))
}

fn match_add(
    meta: &mut MetaState,
    group: &str,
    pattern: Option<&str>,
    priority: i64,
    mut id: i64,
    positions: Option<&[Value]>,
    conceal: Option<&str>,
) -> VimResult<i64> {
    if group.is_empty() || pattern.is_some_and(str::is_empty) {
        return Ok(-1);
    }
    if id < -1 || id == 0 {
        return Err(err(
            "E799",
            format!("Invalid ID: {id} (must be greater than or equal to 1)"),
        ));
    }
    if id != -1 && match_index(meta, id).is_some() {
        return Err(err("E801", format!("ID already taken: {id}")));
    }
    if id == -1 {
        id = next_unused_match_id(meta);
    }
    let mut dict = VimDict::new();
    if let Some(pattern) = pattern {
        dict.insert("pattern", Value::String(pattern.to_string()));
    }
    if let Some(positions) = positions {
        let mut slot = 0usize;
        for item in positions.iter().take(MAXPOSMATCH) {
            match item {
                Value::Number(lnum) => {
                    if *lnum == 0 {
                        continue;
                    }
                    dict.insert(
                        format!("pos{}", slot + 1),
                        Value::list(vec![Value::Number(*lnum)]),
                    );
                    slot += 1;
                }
                Value::List(values) => {
                    let values = values.borrow().clone();
                    if values.is_empty() {
                        return Err(err("E474", "Invalid argument"));
                    }
                    let lnum = get_number(&values[0])?;
                    if lnum == 0 {
                        continue;
                    }
                    let mut entries = vec![Value::Number(lnum)];
                    if values.len() > 1 {
                        entries.push(Value::Number(get_number(&values[1])?));
                        if values.len() > 2 {
                            entries.push(Value::Number(get_number(&values[2])?));
                        }
                    }
                    dict.insert(format!("pos{}", slot + 1), Value::list(entries));
                    slot += 1;
                }
                _ => return Err(err("E290", "List or number required")),
            }
        }
    }
    dict.insert("group", Value::String(group.to_string()));
    dict.insert("priority", Value::Number(priority));
    dict.insert("id", Value::Number(id));
    if let Some(conceal) = conceal {
        dict.insert("conceal", Value::String(conceal.to_string()));
    }
    insert_match(meta, priority, id, dict);
    Ok(id)
}

fn matchadd_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("matchadd", args, 2, 5)?;
    let group = get_string(&arg(args, 0))?;
    let pattern = get_string(&arg(args, 1))?;
    let mut priority = 10i64;
    let mut id = -1i64;
    if args.len() > 2 {
        priority = get_number(&arg(args, 2))?;
        if args.len() > 3 {
            id = get_number(&arg(args, 3))?;
        }
    }
    let conceal = match_dict_arg(args.get(4))?;
    if (1..=3).contains(&id) {
        return Err(err("E798", format!("ID is reserved for \":match\": {id}")));
    }
    let id = match_add(
        meta,
        &group,
        Some(&pattern),
        priority,
        id,
        None,
        conceal.as_deref(),
    )?;
    Ok(Value::Number(id))
}

fn matchaddpos_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("matchaddpos", args, 2, 5)?;
    let group = get_string(&arg(args, 0))?;
    if !matches!(arg(args, 1), Value::List(_)) {
        return Err(err("E686", "Argument of matchaddpos() must be a List"));
    }
    let positions = as_list(&arg(args, 1), "positions")?;
    let mut priority = 10i64;
    let mut id = -1i64;
    if args.len() > 2 {
        priority = get_number(&arg(args, 2))?;
        if args.len() > 3 {
            id = get_number(&arg(args, 3))?;
        }
    }
    let conceal = match_dict_arg(args.get(4))?;
    if id == 1 || id == 2 {
        return Err(err("E798", format!("ID is reserved for \":match\": {id}")));
    }
    let id = match_add(
        meta,
        &group,
        None,
        priority,
        id,
        Some(&positions),
        conceal.as_deref(),
    )?;
    Ok(Value::Number(id))
}

fn matchdelete_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("matchdelete", args, 1, 2)?;
    check_match_window(args.get(1))?;
    let id = get_number(&arg(args, 0))?;
    if id < 1 {
        return Err(err(
            "E802",
            format!("Invalid ID: {id} (must be greater than or equal to 1)"),
        ));
    }
    let Some(index) = match_index(meta, id) else {
        return Err(err("E803", format!("ID not found: {id}")));
    };
    meta.matches.remove(index);
    Ok(Value::Number(0))
}

fn clearmatches_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("clearmatches", args, 0, 1)?;
    check_match_window(args.first())?;
    meta.matches.clear();
    Ok(Value::Number(0))
}

fn getmatches_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("getmatches", args, 0, 1)?;
    check_match_window(args.first())?;
    let items = meta
        .matches
        .iter()
        .map(|(_, dict)| dictionary(dict.clone()))
        .collect();
    Ok(Value::list(items))
}

fn setmatches_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("setmatches", args, 1, 2)?;
    check_match_window(args.get(1))?;
    let items = as_list(&arg(args, 0), "matches")?;
    struct ParsedMatch {
        group: String,
        id: i64,
        priority: i64,
        pattern: Option<String>,
        positions: Vec<Value>,
    }

    let mut parsed: Vec<ParsedMatch> = Vec::new();
    for item in &items {
        let dict = as_dict(item, "match")?;
        let has_pattern = dict.get("pattern").is_some();
        let has_pos = dict.get("pos1").is_some();
        if dict.get("group").is_none()
            || (!has_pattern && !has_pos)
            || dict.get("priority").is_none()
            || dict.get("id").is_none()
        {
            return Err(err("E474", "Invalid argument"));
        }
        let group = get_string(dict.get("group").unwrap())?;
        let priority = get_number(dict.get("priority").unwrap())?;
        let id = get_number(dict.get("id").unwrap())?;
        let pattern = match dict.get("pattern") {
            Some(value) => Some(get_string(value)?),
            None => None,
        };
        let mut positions = Vec::new();
        for slot in 1..=MAXPOSMATCH {
            match dict.get(&format!("pos{slot}")) {
                Some(value) => positions.push(value.clone()),
                None => break,
            }
        }
        parsed.push(ParsedMatch {
            group,
            id,
            priority,
            pattern,
            positions,
        });
    }
    meta.matches.clear();
    for parsed in parsed {
        let positions = if parsed.positions.is_empty() {
            None
        } else {
            Some(parsed.positions.as_slice())
        };
        // setmatches() ignores individual match failures in Vim; keep that.
        let _ = match_add(
            meta,
            &parsed.group,
            parsed.pattern.as_deref(),
            parsed.priority,
            parsed.id,
            positions,
            None,
        );
    }
    Ok(Value::Number(0))
}

fn matcharg_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("matcharg", args, 1, 1)?;
    let id = get_number(&arg(args, 0))?;
    if !(1..=3).contains(&id) {
        return Ok(Value::list(Vec::new()));
    }
    match meta.matches.iter().find(|(match_id, _)| *match_id == id) {
        Some((_, dict)) => Ok(Value::list(vec![
            Value::String(dict_string(dict, "group").unwrap_or_default()),
            Value::String(dict_string(dict, "pattern").unwrap_or_default()),
        ])),
        None => Ok(Value::list(vec![
            Value::String(String::new()),
            Value::String(String::new()),
        ])),
    }
}

// ---------------------------------------------------------------------------
// Command-line history (reference: cmdhist.c)
// ---------------------------------------------------------------------------

/// History name → list of entries (oldest first). Vim accepts the names
/// `cmd`, `search`, `expr`, `input`, `debug` plus the one-character forms
/// `: = @ > ? /` and significant prefixes thereof.
fn history_mut<'a>(meta: &'a mut MetaState, name: &str) -> Option<&'a mut Vec<String>> {
    let lower = name.to_ascii_lowercase();
    let kind = match lower.as_str() {
        ":" | "c" | "cm" | "cmd" => 0,
        "=" | "e" | "ex" | "exp" | "expr" => 1,
        "@" | "i" | "in" | "inp" | "inpu" | "input" => 2,
        ">" | "d" | "de" | "deb" | "debu" | "debug" => 3,
        "?" | "/" | "s" | "se" | "sea" | "sear" | "searc" | "search" => 4,
        "" => 0,
        _ => return None,
    };
    Some(match kind {
        0 => &mut meta.cmd_history,
        1 => &mut meta.expr_history,
        2 => &mut meta.input_history,
        3 => &mut meta.debug_history,
        _ => &mut meta.search_history,
    })
}

fn history_ref<'a>(meta: &'a MetaState, name: &str) -> Option<&'a Vec<String>> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        ":" | "c" | "cm" | "cmd" | "" => Some(&meta.cmd_history),
        "=" | "e" | "ex" | "exp" | "expr" => Some(&meta.expr_history),
        "@" | "i" | "in" | "inp" | "inpu" | "input" => Some(&meta.input_history),
        ">" | "d" | "de" | "deb" | "debu" | "debug" => Some(&meta.debug_history),
        "?" | "/" | "s" | "se" | "sea" | "sear" | "searc" | "search" => Some(&meta.search_history),
        _ => None,
    }
}

/// `calc_hist_idx()` under the position-based numbering used by this module
/// (entries are numbered 1..=len; negative indexes count from the newest).
fn history_index(entries: &[String], number: i64) -> Option<usize> {
    if entries.is_empty() || number == 0 {
        return None;
    }
    if number > 0 {
        let index = number as usize;
        if index <= entries.len() {
            Some(index - 1)
        } else {
            None
        }
    } else {
        let index = entries.len() as i64 + number + 1;
        if index >= 1 && index <= entries.len() as i64 {
            Some(index as usize - 1)
        } else {
            None
        }
    }
}

fn histadd_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("histadd", args, 2, 2)?;
    let name = get_string(&arg(args, 0))?;
    let entry = get_string(&arg(args, 1))?;
    let Some(history) = history_mut(meta, &name) else {
        return Ok(Value::Number(0));
    };
    if entry.is_empty() {
        return Ok(Value::Number(0));
    }
    // A duplicate is moved to the newest position instead of duplicated.
    history.retain(|existing| existing != &entry);
    history.push(entry);
    Ok(Value::Number(1))
}

fn histdel_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("histdel", args, 1, 2)?;
    let name = get_string(&arg(args, 0))?;
    let Some(history) = history_mut(meta, &name) else {
        return Ok(Value::Number(0));
    };
    match args.get(1) {
        None => {
            history.clear();
            Ok(Value::Number(1))
        }
        Some(Value::Number(number)) => match history_index(history, *number) {
            Some(index) => {
                history.remove(index);
                Ok(Value::Number(1))
            }
            None => Ok(Value::Number(0)),
        },
        Some(value) => {
            let pattern = get_string(value)?;
            if pattern.is_empty() {
                return Ok(Value::Number(0));
            }
            let before = history.len();
            history.retain(|entry| !rvim_viml::regex_mvp::is_match(&pattern, entry, false));
            Ok(Value::Number(i64::from(history.len() != before)))
        }
    }
}

fn histget_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("histget", args, 1, 2)?;
    let name = get_string(&arg(args, 0))?;
    let Some(history) = history_ref(meta, &name) else {
        return Ok(Value::String(String::new()));
    };
    let index = match args.get(1) {
        None => {
            if history.is_empty() {
                return Ok(Value::String(String::new()));
            }
            history.len() - 1
        }
        Some(value) => history_index(history, get_number(value)?).unwrap_or(usize::MAX),
    };
    Ok(Value::String(
        history.get(index).cloned().unwrap_or_default(),
    ))
}

fn histnr_call(args: &[Value], meta: &mut MetaState) -> VimResult<Value> {
    arity("histnr", args, 1, 1)?;
    let name = get_string(&arg(args, 0))?;
    match history_ref(meta, &name) {
        Some(history) => Ok(Value::Number(history.len() as i64)),
        None => Ok(Value::Number(-1)),
    }
}

// ---------------------------------------------------------------------------
// Balloon (reference: evalfunc.c f_balloon_*)
// ---------------------------------------------------------------------------

fn balloon_gettext_call(args: &[Value]) -> VimResult<Value> {
    arity("balloon_gettext", args, 0, 0)?;
    // No balloon evaluation window exists yet; the honest answer is "no text".
    Ok(Value::String(String::new()))
}

fn balloon_show_call(args: &[Value]) -> VimResult<Value> {
    arity("balloon_show", args, 1, 1)?;
    // Without `balloonEval` Vim's balloon_show() silently does nothing.
    Ok(Value::Number(0))
}

/// Split `{msg}` into lines that fit in half the screen width, preferring to
/// break at spaces while preserving explicit newlines. This is the pure part
/// of Vim's terminal-balloon handling (`balloon_split()`/`split_message()`).
fn split_balloon_text(msg: &str, width: usize) -> Vec<String> {
    if msg.is_empty() {
        return Vec::new();
    }
    let width = width.max(1);
    let mut segments: Vec<&str> = msg.split('\n').collect();
    if msg.ends_with('\n') {
        segments.pop();
    }
    let mut lines = Vec::new();
    for segment in segments {
        let chars: Vec<char> = segment.chars().collect();
        if chars.len() <= width {
            lines.push(String::new());
            if let Some(last) = lines.last_mut() {
                last.extend(chars.iter().copied());
            }
            continue;
        }
        let mut start = 0usize;
        while start < chars.len() {
            let remaining = chars.len() - start;
            if remaining <= width {
                lines.push(chars[start..].iter().collect());
                break;
            }
            let window = &chars[start..start + width];
            let break_at = window
                .iter()
                .rposition(|c| c.is_whitespace())
                .filter(|i| *i > 0);
            let (piece_end, next_start) = match break_at {
                Some(index) => {
                    let mut next = start + index;
                    while next < chars.len() && chars[next].is_whitespace() {
                        next += 1;
                    }
                    (start + index, next)
                }
                None => (start + width, start + width),
            };
            let piece: String = chars[start..piece_end].iter().collect();
            lines.push(piece.trim_end().to_string());
            start = next_start;
        }
    }
    lines
}

fn balloon_split_call(args: &[Value], ed: &Editor) -> VimResult<Value> {
    arity("balloon_split", args, 1, 1)?;
    let msg = match get_string(&arg(args, 0)) {
        Ok(msg) => msg,
        Err(_) => return Ok(Value::list(Vec::new())),
    };
    let width = (ed.options.num_or("columns", 80) / 2).max(1) as usize;
    Ok(Value::list(
        split_balloon_text(&msg, width)
            .into_iter()
            .map(Value::String)
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Names implemented in this module (exactly the functions dispatched below).
pub const FUNCS: &[&str] = &[
    "balloon_gettext",
    "balloon_show",
    "balloon_split",
    "clearmatches",
    "getmatches",
    "histadd",
    "histdel",
    "histget",
    "histnr",
    "matchadd",
    "matchaddpos",
    "matcharg",
    "matchdelete",
    "popup_atcursor",
    "popup_beval",
    "popup_clear",
    "popup_close",
    "popup_create",
    "popup_findinfo",
    "popup_findpreview",
    "popup_getoptions",
    "popup_getpos",
    "popup_hide",
    "popup_list",
    "popup_locate",
    "popup_menu",
    "popup_move",
    "popup_notification",
    "popup_setoptions",
    "popup_settext",
    "popup_show",
    "prop_add",
    "prop_add_list",
    "prop_clear",
    "prop_find",
    "prop_list",
    "prop_remove",
    "prop_type_add",
    "prop_type_change",
    "prop_type_delete",
    "prop_type_get",
    "prop_type_list",
    "setmatches",
    "sign_define",
    "sign_getdefined",
    "sign_getplaced",
    "sign_jump",
    "sign_place",
    "sign_placelist",
    "sign_undefine",
    "sign_unplace",
    "sign_unplacelist",
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
    let _ = env;
    let meta = &mut state.meta;
    Some(match name {
        "popup_create" | "popup_atcursor" | "popup_beval" | "popup_notification" => {
            popup_create_call(name, args, ed, meta)
        }
        "popup_menu" => popup_menu_call(args, ed, meta),
        "popup_clear" => popup_clear_call(args, meta),
        "popup_close" => popup_close_call(args, meta),
        "popup_hide" | "popup_show" => popup_hide_show_call(name, args, meta),
        "popup_move" => popup_move_call(args, ed, meta),
        "popup_setoptions" => popup_setoptions_call(args, ed, meta),
        "popup_settext" => popup_settext_call(args, ed, meta),
        "popup_getpos" => popup_getpos_call(args, meta),
        "popup_getoptions" => popup_getoptions_call(args, meta),
        "popup_list" => popup_list_call(args, meta),
        "popup_locate" => popup_locate_call(args, meta),
        "popup_findinfo" | "popup_findpreview" => popup_findinfo_preview_call(name, args),
        "prop_add" => prop_add_call(args, ed, meta),
        "prop_add_list" => prop_add_list_call(args, ed, meta),
        "prop_clear" => prop_clear_call(args, ed, meta),
        "prop_find" => prop_find_call(args, ed, meta),
        "prop_list" => prop_list_call(args, ed, meta),
        "prop_remove" => prop_remove_call(args, ed, meta),
        "prop_type_add" => prop_type_add_call(args, ed, meta),
        "prop_type_change" => prop_type_change_call(args, ed, meta),
        "prop_type_delete" => prop_type_delete_call(args, ed, meta),
        "prop_type_get" => prop_type_get_call(args, ed, meta),
        "prop_type_list" => prop_type_list_call(args, ed, meta),
        "sign_define" => sign_define_call(args, meta),
        "sign_getdefined" => sign_getdefined_call(args, meta),
        "sign_getplaced" => sign_getplaced_call(args, ed, meta),
        "sign_jump" => sign_jump_call(args, ed, meta),
        "sign_place" => sign_place_call(args, ed, meta),
        "sign_placelist" => sign_placelist_call(args, ed, meta),
        "sign_undefine" => sign_undefine_call(args, meta),
        "sign_unplace" => sign_unplace_call(args, ed, meta),
        "sign_unplacelist" => sign_unplacelist_call(args, ed, meta),
        "matchadd" => matchadd_call(args, meta),
        "matchaddpos" => matchaddpos_call(args, meta),
        "matchdelete" => matchdelete_call(args, meta),
        "clearmatches" => clearmatches_call(args, meta),
        "getmatches" => getmatches_call(args, meta),
        "setmatches" => setmatches_call(args, meta),
        "matcharg" => matcharg_call(args, meta),
        "histadd" => histadd_call(args, meta),
        "histdel" => histdel_call(args, meta),
        "histget" => histget_call(args, meta),
        "histnr" => histnr_call(args, meta),
        "balloon_gettext" => balloon_gettext_call(args),
        "balloon_show" => balloon_show_call(args),
        "balloon_split" => balloon_split_call(args, ed),
        // `FUNCS` and this dispatch are kept in sync; this arm never runs.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_viml::Env;

    struct Fixture {
        ed: Editor,
        env: Env,
        state: ScriptState,
    }

    impl Fixture {
        fn new() -> Self {
            let mut ed = Editor::new();
            ed.replace_lines(
                0,
                1,
                vec![
                    "alpha beta".to_string(),
                    "second line".to_string(),
                    "third".to_string(),
                ],
            )
            .unwrap();
            Self {
                ed,
                env: Env::new(),
                state: ScriptState::default(),
            }
        }

        fn invoke(&mut self, name: &str, args: Vec<Value>) -> VimResult<Value> {
            match call(name, &args, &mut self.ed, &mut self.env, &mut self.state) {
                Some(result) => result,
                None => panic!("{name} is in FUNCS but not dispatched"),
            }
        }

        fn eval(&mut self, src: &str) -> VimResult<Value> {
            crate::script::eval_expr(&mut self.ed, &mut self.env, &mut self.state, src)
        }

        fn number(&mut self, name: &str, args: Vec<Value>) -> i64 {
            self.invoke(name, args).expect(name).to_number()
        }
    }

    fn string(text: impl Into<String>) -> Value {
        Value::String(text.into())
    }

    fn number(value: i64) -> Value {
        Value::Number(value)
    }

    fn dict(pairs: &[(&str, Value)]) -> Value {
        let mut result = VimDict::new();
        for (key, value) in pairs {
            result.insert(*key, value.clone());
        }
        Value::dict(result)
    }

    fn list(items: Vec<Value>) -> Value {
        Value::list(items)
    }

    fn get_field(value: &Value, key: &str) -> Value {
        match value {
            Value::Dict(d) => d.borrow().get(key).cloned().unwrap_or(Value::Null),
            other => panic!("expected dict, got {}", other.type_name()),
        }
    }

    fn field_num(value: &Value, key: &str) -> i64 {
        get_field(value, key).to_number()
    }

    fn field_str(value: &Value, key: &str) -> String {
        get_field(value, key).to_string_value()
    }

    fn items(value: &Value) -> Vec<Value> {
        match value {
            Value::List(l) => l.borrow().clone(),
            other => panic!("expected list, got {}", other.type_name()),
        }
    }

    // --- popups ---------------------------------------------------------

    #[test]
    fn popup_create_default_geometry_and_queries() {
        let mut f = Fixture::new();
        let id = f.number("popup_create", vec![string("hi"), dict(&[])]);
        assert!(id >= FIRST_POPUP_ID);
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "width"), 2);
        assert_eq!(field_num(&pos, "height"), 1);
        // Centered on the nominal 80x24 screen.
        assert_eq!(field_num(&pos, "line"), 12);
        assert_eq!(field_num(&pos, "col"), 40);
        assert_eq!(field_num(&pos, "visible"), 1);
        assert_eq!(field_num(&pos, "firstline"), 1);
        assert_eq!(field_num(&pos, "lastline"), 1);
        let opts = f.invoke("popup_getoptions", vec![number(id)]).unwrap();
        assert_eq!(field_num(&opts, "zindex"), DEFAULT_ZINDEX);
        assert_eq!(field_num(&opts, "wrap"), 1);
        assert_eq!(field_num(&opts, "mapping"), 1);
        assert_eq!(field_num(&opts, "posinvert"), 1);
        assert_eq!(field_str(&opts, "pos"), "topleft");
        assert_eq!(field_str(&opts, "close"), "none");
        assert_eq!(field_num(&opts, "tabpage"), 0);
        assert_eq!(field_num(&opts, "line"), 0);
        assert_eq!(
            f.invoke("popup_list", vec![]).unwrap().to_string_value(),
            format!("[{id}]")
        );
    }

    #[test]
    fn popup_atcursor_and_beval_anchor_on_the_cursor() {
        let mut f = Fixture::new();
        f.ed.set_cursor(Pos::new(1, 3));
        let at = f.number("popup_atcursor", vec![string("hi"), dict(&[])]);
        let opts = f.invoke("popup_getoptions", vec![number(at)]).unwrap();
        assert_eq!(field_str(&opts, "pos"), "botleft");
        assert_eq!(field_num(&opts, "line"), 1);
        assert_eq!(field_num(&opts, "col"), 4);
        let beval = f.number("popup_beval", vec![string("hi"), dict(&[])]);
        let opts = f.invoke("popup_getoptions", vec![number(beval)]).unwrap();
        assert_eq!(field_str(&opts, "pos"), "botleft");
        assert_eq!(field_num(&opts, "col"), 4);
        // A popup anchored on the first line is flipped to the top.
        f.ed.set_cursor(Pos::new(0, 0));
        let first = f.number("popup_atcursor", vec![string("hi"), dict(&[])]);
        let opts = f.invoke("popup_getoptions", vec![number(first)]).unwrap();
        assert_eq!(field_str(&opts, "pos"), "topleft");
        assert_eq!(field_num(&opts, "line"), 2);
    }

    #[test]
    fn popup_notification_defaults() {
        let mut f = Fixture::new();
        let id = f.number("popup_notification", vec![string("msg"), dict(&[])]);
        let opts = f.invoke("popup_getoptions", vec![number(id)]).unwrap();
        assert_eq!(field_num(&opts, "zindex"), NOTIFICATION_ZINDEX);
        assert_eq!(field_num(&opts, "minwidth"), 20);
        assert_eq!(field_num(&opts, "drag"), 1);
        assert_eq!(field_num(&opts, "tabpage"), -1);
        assert_eq!(field_str(&opts, "close"), "click");
        assert_eq!(field_num(&opts, "highlight"), 0);
        assert_eq!(field_str(&opts, "highlight"), "WarningMsg");
        // border is all ones (rendered as an empty list), padding is [0,1,0,1].
        let border = get_field(&opts, "border");
        assert!(items(&border).is_empty());
        let padding = items(&get_field(&opts, "padding"));
        assert_eq!(
            padding.iter().map(Value::to_number).collect::<Vec<_>>(),
            vec![0, 1, 0, 1]
        );
    }

    #[test]
    fn popup_show_hide_close_clear() {
        let mut f = Fixture::new();
        let id = f.number("popup_create", vec![string("x"), dict(&[])]);
        f.invoke("popup_hide", vec![number(id)]).unwrap();
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "visible"), 0);
        f.invoke("popup_show", vec![number(id)]).unwrap();
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "visible"), 1);
        // Unknown ids are silently ignored.
        assert_eq!(f.number("popup_close", vec![number(9999)]), 0);
        assert_eq!(f.number("popup_close", vec![number(id)]), 0);
        assert!(items(&f.invoke("popup_list", vec![]).unwrap()).is_empty());
        f.number("popup_create", vec![string("a"), dict(&[])]);
        f.number("popup_create", vec![string("b"), dict(&[])]);
        assert_eq!(f.number("popup_clear", vec![]), 0);
        assert!(items(&f.invoke("popup_list", vec![]).unwrap()).is_empty());
    }

    #[test]
    fn popup_list_orders_tab_popups_then_globals() {
        let mut f = Fixture::new();
        let first = f.number("popup_create", vec![string("a"), dict(&[])]);
        let global = f.number("popup_notification", vec![string("n"), dict(&[])]);
        let last = f.number("popup_create", vec![string("b"), dict(&[])]);
        let listed = items(&f.invoke("popup_list", vec![]).unwrap());
        let ids: Vec<i64> = listed.iter().map(Value::to_number).collect();
        assert_eq!(ids, vec![last, first, global]);
    }

    #[test]
    fn popup_findinfo_findpreview_are_zero() {
        let mut f = Fixture::new();
        assert_eq!(f.number("popup_findinfo", vec![]), 0);
        assert_eq!(f.number("popup_findpreview", vec![]), 0);
    }

    #[test]
    fn popup_getpos_and_getoptions_unknown_id_return_empty_dict() {
        let mut f = Fixture::new();
        let pos = f.invoke("popup_getpos", vec![number(9999)]).unwrap();
        assert!(matches!(pos, Value::Dict(ref d) if d.borrow().is_empty()));
        let opts = f.invoke("popup_getoptions", vec![number(9999)]).unwrap();
        assert!(matches!(opts, Value::Dict(ref d) if d.borrow().is_empty()));
    }

    #[test]
    fn popup_locate_finds_the_popup_under_a_position() {
        let mut f = Fixture::new();
        let id = f.number(
            "popup_create",
            vec![
                string("x"),
                dict(&[("line", number(4)), ("col", number(1))]),
            ],
        );
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "line"), 4);
        assert_eq!(field_num(&pos, "col"), 1);
        assert_eq!(f.number("popup_locate", vec![number(4), number(1)]), id);
        assert_eq!(f.number("popup_locate", vec![number(3), number(1)]), 0);
    }

    #[test]
    fn popup_move_and_setoptions_merge_options() {
        let mut f = Fixture::new();
        let id = f.number("popup_create", vec![string("hello"), dict(&[])]);
        f.invoke(
            "popup_move",
            vec![
                number(id),
                dict(&[("line", number(10)), ("col", number(20))]),
            ],
        )
        .unwrap();
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "line"), 10);
        assert_eq!(field_num(&pos, "col"), 20);
        f.invoke(
            "popup_setoptions",
            vec![
                number(id),
                dict(&[
                    ("title", string("T")),
                    ("wrap", number(0)),
                    ("line", number(3)),
                    ("zindex", number(321)),
                ]),
            ],
        )
        .unwrap();
        let opts = f.invoke("popup_getoptions", vec![number(id)]).unwrap();
        assert_eq!(field_str(&opts, "title"), "T");
        assert_eq!(field_num(&opts, "wrap"), 0);
        assert_eq!(field_num(&opts, "line"), 3);
        assert_eq!(field_num(&opts, "zindex"), 321);
        // A title adds one row above the core.
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "line"), 3);
        assert_eq!(field_num(&pos, "core_line"), 4);
        assert_eq!(field_num(&pos, "height"), 2);
    }

    #[test]
    fn popup_settext_replaces_the_lines() {
        let mut f = Fixture::new();
        let id = f.number("popup_create", vec![string("a\nb"), dict(&[])]);
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "core_height"), 2);
        f.invoke(
            "popup_settext",
            vec![
                number(id),
                list(vec![string("x"), string("y"), string("z")]),
            ],
        )
        .unwrap();
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "core_height"), 3);
        assert_eq!(field_num(&pos, "core_width"), 1);
        // Wrong text type is E475 and leaves the popup untouched.
        let err = f
            .invoke("popup_settext", vec![number(id), number(3)])
            .unwrap_err();
        assert_eq!(err.code, "E475");
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "core_height"), 3);
    }

    #[test]
    fn popup_options_are_applied_and_returned() {
        let mut f = Fixture::new();
        let id = f.number(
            "popup_create",
            vec![
                string("x"),
                dict(&[
                    ("line", number(5)),
                    ("col", number(6)),
                    (
                        "padding",
                        list(vec![number(1), number(2), number(3), number(4)]),
                    ),
                    (
                        "border",
                        list(vec![number(1), number(0), number(0), number(1)]),
                    ),
                    ("title", string("title")),
                    ("close", string("button")),
                    ("callback", string("MyCallback")),
                    ("wrap", number(0)),
                    ("zindex", number(99)),
                ]),
            ],
        );
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        // 1 core column + left border/padding (1+4) + right border/padding (0+2).
        assert_eq!(field_num(&pos, "width"), 8);
        let opts = f.invoke("popup_getoptions", vec![number(id)]).unwrap();
        assert_eq!(field_str(&opts, "close"), "button");
        assert_eq!(field_str(&opts, "title"), "title");
        assert_eq!(field_num(&opts, "zindex"), 99);
        match get_field(&opts, "callback") {
            Value::Funcref(callback) => assert_eq!(callback.name, "MyCallback"),
            other => panic!("callback should be a Funcref, got {}", other.type_name()),
        }
        let _ = pos;
    }

    #[test]
    fn popup_create_validates_arguments() {
        let mut f = Fixture::new();
        let err = f
            .invoke("popup_create", vec![string("hi"), Value::Null])
            .unwrap_err();
        assert_eq!(err.code, "E715");
        let err = f
            .invoke("popup_create", vec![number(42), dict(&[])])
            .unwrap_err();
        assert_eq!(err.code, "E86");
        assert_eq!(f.eval("popup_create('hi')").unwrap_err().code, "E119");
        assert_eq!(
            f.eval("popup_create('hi', {}, 1)").unwrap_err().code,
            "E118"
        );
        let err = f
            .invoke(
                "popup_create",
                vec![string("hi"), dict(&[("pos", string("nonsense"))])],
            )
            .unwrap_err();
        assert_eq!(err.code, "E475");
    }

    #[test]
    fn popup_menu_reports_the_missing_input_layer() {
        let mut f = Fixture::new();
        assert_eq!(
            f.number(
                "popup_menu",
                vec![list(vec![string("a"), string("b")]), dict(&[])]
            ),
            -1
        );
        assert_eq!(f.state.meta.deferred_ui.len(), 1);
        assert!(f.state.meta.deferred_ui[0].contains("popup_menu"));
        assert!(f.state.meta.popups.is_empty());
    }

    #[test]
    fn popup_cursor_relative_position_and_textprop_validation() {
        let mut f = Fixture::new();
        f.ed.set_cursor(Pos::new(1, 2));
        let id = f.number(
            "popup_create",
            vec![
                string("x"),
                dict(&[("line", string("cursor+1")), ("col", string("cursor"))]),
            ],
        );
        let opts = f.invoke("popup_getoptions", vec![number(id)]).unwrap();
        assert_eq!(field_num(&opts, "line"), 3);
        assert_eq!(field_num(&opts, "col"), 3);
        // `textprop` must name a defined text property type.
        let err = f
            .invoke(
                "popup_create",
                vec![string("x"), dict(&[("textprop", string("Missing"))])],
            )
            .unwrap_err();
        assert_eq!(err.code, "E475");
        add_type(&mut f, "T");
        let with_prop = f.number(
            "popup_create",
            vec![string("x"), dict(&[("textprop", string("T"))])],
        );
        let opts = f
            .invoke("popup_getoptions", vec![number(with_prop)])
            .unwrap();
        assert_eq!(field_str(&opts, "textprop"), "T");
    }

    #[test]
    fn popup_setoptions_can_hide_a_popup() {
        let mut f = Fixture::new();
        let id = f.number("popup_create", vec![string("x"), dict(&[])]);
        f.invoke(
            "popup_setoptions",
            vec![number(id), dict(&[("hidden", number(1))])],
        )
        .unwrap();
        let pos = f.invoke("popup_getpos", vec![number(id)]).unwrap();
        assert_eq!(field_num(&pos, "visible"), 0);
    }

    #[test]
    fn prop_find_honours_direction() {
        let mut f = Fixture::new();
        add_type(&mut f, "T");
        f.invoke(
            "prop_add",
            vec![
                number(1),
                number(2),
                dict(&[
                    ("type", string("T")),
                    ("id", number(7)),
                    ("length", number(3)),
                ]),
            ],
        )
        .unwrap();
        let backward = f
            .invoke(
                "prop_find",
                vec![dict(&[("id", number(7)), ("lnum", number(2))]), string("b")],
            )
            .unwrap();
        assert_eq!(field_num(&backward, "lnum"), 1);
        let forward = f
            .invoke(
                "prop_find",
                vec![dict(&[("id", number(7)), ("lnum", number(1))]), string("f")],
            )
            .unwrap();
        assert_eq!(field_num(&forward, "lnum"), 1);
        assert_eq!(
            f.invoke("prop_find", vec![dict(&[("id", number(7))]), string("x")])
                .unwrap_err()
                .code,
            "E474"
        );
    }

    // --- text properties ------------------------------------------------

    fn add_type(f: &mut Fixture, name: &str) {
        f.invoke("prop_type_add", vec![string(name), dict(&[])])
            .unwrap();
    }

    #[test]
    fn prop_type_add_get_list_change_delete() {
        let mut f = Fixture::new();
        add_type(&mut f, "T");
        let info = f.invoke("prop_type_get", vec![string("T")]).unwrap();
        assert_eq!(field_num(&info, "priority"), 0);
        assert_eq!(field_num(&info, "combine"), 1);
        assert_eq!(field_num(&info, "start_incl"), 0);
        assert_eq!(field_num(&info, "end_incl"), 0);
        assert_eq!(get_field(&info, "highlight"), Value::Null);
        assert_eq!(get_field(&info, "bufnr"), Value::Null);
        assert_eq!(
            f.invoke("prop_type_list", vec![])
                .unwrap()
                .to_string_value(),
            "['T']"
        );
        f.invoke(
            "prop_type_change",
            vec![
                string("T"),
                dict(&[("priority", number(5)), ("highlight", string("Error"))]),
            ],
        )
        .unwrap();
        let info = f.invoke("prop_type_get", vec![string("T")]).unwrap();
        assert_eq!(field_num(&info, "priority"), 5);
        assert_eq!(field_str(&info, "highlight"), "Error");
        assert_eq!(
            f.invoke("prop_type_add", vec![string("T"), dict(&[])])
                .unwrap_err()
                .code,
            "E969"
        );
        assert_eq!(
            f.invoke("prop_type_change", vec![string("Missing"), dict(&[])])
                .unwrap_err()
                .code,
            "E971"
        );
        f.invoke("prop_type_delete", vec![string("T")]).unwrap();
        assert!(items(&f.invoke("prop_type_list", vec![]).unwrap()).is_empty());
        assert!(matches!(
            f.invoke("prop_type_get", vec![string("T")]).unwrap(),
            Value::Dict(ref d) if d.borrow().is_empty()
        ));
    }

    #[test]
    fn prop_type_buffer_local_namespace() {
        let mut f = Fixture::new();
        f.invoke(
            "prop_type_add",
            vec![string("B"), dict(&[("bufnr", number(1))])],
        )
        .unwrap();
        let info = f
            .invoke(
                "prop_type_get",
                vec![string("B"), dict(&[("bufnr", number(1))])],
            )
            .unwrap();
        assert_eq!(field_num(&info, "bufnr"), 1);
        assert_eq!(
            f.invoke("prop_type_list", vec![dict(&[("bufnr", number(1))])])
                .unwrap()
                .to_string_value(),
            "['B']"
        );
        // The global namespace does not see the buffer-local type.
        assert!(items(&f.invoke("prop_type_list", vec![]).unwrap()).is_empty());
    }

    #[test]
    fn prop_add_and_prop_list() {
        let mut f = Fixture::new();
        add_type(&mut f, "T");
        f.invoke(
            "prop_add",
            vec![number(1), number(1), dict(&[("type", string("T"))])],
        )
        .unwrap();
        f.invoke(
            "prop_add",
            vec![
                number(1),
                number(2),
                dict(&[
                    ("type", string("T")),
                    ("id", number(7)),
                    ("length", number(3)),
                ]),
            ],
        )
        .unwrap();
        let props = items(&f.invoke("prop_list", vec![number(1)]).unwrap());
        assert_eq!(props.len(), 2);
        let first = &props[0];
        assert_eq!(field_num(first, "col"), 1);
        assert_eq!(field_num(first, "length"), 0);
        assert_eq!(field_str(first, "type"), "T");
        assert_eq!(field_num(first, "type_bufnr"), 0);
        assert_eq!(field_num(first, "start"), 1);
        assert_eq!(field_num(first, "end"), 1);
        let second = &props[1];
        assert_eq!(field_num(second, "id"), 7);
        assert_eq!(field_num(second, "length"), 3);
        assert_eq!(get_field(second, "lnum"), Value::Null);
        // Multi-line property reports one entry per line, with lnum when a range
        // is requested.
        f.invoke(
            "prop_add",
            vec![
                number(2),
                number(1),
                dict(&[
                    ("type", string("T")),
                    ("id", number(9)),
                    ("end_lnum", number(3)),
                    ("end_col", number(2)),
                ]),
            ],
        )
        .unwrap();
        let ranged = items(
            &f.invoke(
                "prop_list",
                vec![number(2), dict(&[("end_lnum", number(3))])],
            )
            .unwrap(),
        );
        assert_eq!(ranged.len(), 2);
        assert_eq!(field_num(&ranged[0], "lnum"), 2);
        assert_eq!(field_num(&ranged[0], "length"), 11);
        assert_eq!(field_num(&ranged[1], "lnum"), 3);
        assert_eq!(field_num(&ranged[1], "start"), 0);
        assert_eq!(field_num(&ranged[1], "end"), 1);
    }

    #[test]
    fn prop_add_list_and_clear() {
        let mut f = Fixture::new();
        add_type(&mut f, "T");
        f.invoke(
            "prop_add_list",
            vec![
                dict(&[("type", string("T")), ("id", number(3))]),
                list(vec![
                    list(vec![number(1), number(1), number(1), number(3)]),
                    list(vec![number(2), number(1), number(2), number(2)]),
                ]),
            ],
        )
        .unwrap();
        assert_eq!(
            items(&f.invoke("prop_list", vec![number(1)]).unwrap()).len(),
            1
        );
        assert_eq!(
            items(&f.invoke("prop_list", vec![number(2)]).unwrap()).len(),
            1
        );
        f.invoke("prop_clear", vec![number(1), number(2)]).unwrap();
        assert!(items(&f.invoke("prop_list", vec![number(1)]).unwrap()).is_empty());
        assert!(items(&f.invoke("prop_list", vec![number(2)]).unwrap()).is_empty());
        // The type itself survives prop_clear().
        assert_eq!(
            f.invoke("prop_type_list", vec![])
                .unwrap()
                .to_string_value(),
            "['T']"
        );
    }

    #[test]
    fn prop_find_by_id_and_type() {
        let mut f = Fixture::new();
        add_type(&mut f, "T");
        f.invoke(
            "prop_add",
            vec![
                number(1),
                number(2),
                dict(&[
                    ("type", string("T")),
                    ("id", number(7)),
                    ("length", number(3)),
                ]),
            ],
        )
        .unwrap();
        f.ed.set_cursor(Pos::new(0, 0));
        let found = f
            .invoke("prop_find", vec![dict(&[("id", number(7))])])
            .unwrap();
        assert_eq!(field_str(&found, "type"), "T");
        assert_eq!(field_num(&found, "lnum"), 1);
        assert_eq!(field_num(&found, "col"), 2);
        assert_eq!(field_num(&found, "length"), 3);
        let found = f
            .invoke("prop_find", vec![dict(&[("type", string("T"))])])
            .unwrap();
        assert_eq!(field_num(&found, "id"), 7);
        // Unknown id finds nothing and returns an empty dict.
        let missing = f
            .invoke("prop_find", vec![dict(&[("id", number(99))])])
            .unwrap();
        assert!(matches!(missing, Value::Dict(ref d) if d.borrow().is_empty()));
        assert_eq!(
            f.invoke("prop_find", vec![dict(&[])]).unwrap_err().code,
            "E968"
        );
        assert_eq!(
            f.invoke(
                "prop_find",
                vec![dict(&[("both", number(1)), ("id", number(1))])]
            )
            .unwrap_err()
            .code,
            "E860"
        );
        // Multi-line property: searching on the continuation line returns the
        // property start.
        f.invoke(
            "prop_add",
            vec![
                number(2),
                number(1),
                dict(&[
                    ("type", string("T")),
                    ("id", number(11)),
                    ("end_lnum", number(3)),
                    ("end_col", number(2)),
                ]),
            ],
        )
        .unwrap();
        let found = f
            .invoke(
                "prop_find",
                vec![dict(&[("id", number(11)), ("lnum", number(3))])],
            )
            .unwrap();
        assert_eq!(field_num(&found, "lnum"), 2);
        assert_eq!(field_num(&found, "start"), 1);
    }

    #[test]
    fn prop_remove_counts_removed_properties() {
        let mut f = Fixture::new();
        add_type(&mut f, "T");
        for id in [1, 2] {
            f.invoke(
                "prop_add",
                vec![
                    number(1),
                    number(1),
                    dict(&[("type", string("T")), ("id", number(id))]),
                ],
            )
            .unwrap();
        }
        // `all` is off by default: at most one property per line is removed.
        assert_eq!(f.number("prop_remove", vec![dict(&[("id", number(1))])]), 1);
        assert_eq!(
            items(&f.invoke("prop_list", vec![number(1)]).unwrap()).len(),
            1
        );
        f.invoke(
            "prop_add",
            vec![
                number(1),
                number(1),
                dict(&[("type", string("T")), ("id", number(5))]),
            ],
        )
        .unwrap();
        assert_eq!(
            f.number(
                "prop_remove",
                vec![dict(&[("type", string("T")), ("all", number(1))])]
            ),
            2
        );
        assert!(items(&f.invoke("prop_list", vec![number(1)]).unwrap()).is_empty());
    }

    #[test]
    fn prop_errors_have_vim_codes() {
        let mut f = Fixture::new();
        add_type(&mut f, "T");
        assert_eq!(
            f.invoke("prop_add", vec![number(1), number(1), dict(&[])])
                .unwrap_err()
                .code,
            "E965"
        );
        assert_eq!(
            f.invoke(
                "prop_add",
                vec![number(1), number(1), dict(&[("type", string("Missing"))])]
            )
            .unwrap_err()
            .code,
            "E971"
        );
        assert_eq!(
            f.invoke(
                "prop_add",
                vec![number(1), number(0), dict(&[("type", string("T"))])]
            )
            .unwrap_err()
            .code,
            "E964"
        );
        assert_eq!(
            f.invoke(
                "prop_add",
                vec![number(9), number(1), dict(&[("type", string("T"))])]
            )
            .unwrap_err()
            .code,
            "E966"
        );
        assert_eq!(
            f.invoke(
                "prop_add",
                vec![
                    number(1),
                    number(1),
                    dict(&[("type", string("T")), ("bufnr", number(99))])
                ]
            )
            .unwrap_err()
            .code,
            "E158"
        );
        assert_eq!(
            f.invoke("prop_list", vec![number(99)]).unwrap_err().code,
            "E16"
        );
    }

    // --- signs ----------------------------------------------------------

    fn define_sign(f: &mut Fixture, name: &str) {
        f.invoke("sign_define", vec![string(name), dict(&[])])
            .unwrap();
    }

    #[test]
    fn sign_define_and_getdefined() {
        let mut f = Fixture::new();
        assert_eq!(
            f.number(
                "sign_define",
                vec![
                    string("S"),
                    dict(&[("text", string(">>")), ("linehl", string("Error"))])
                ]
            ),
            0
        );
        let defined = items(&f.invoke("sign_getdefined", vec![string("S")]).unwrap());
        assert_eq!(defined.len(), 1);
        assert_eq!(field_str(&defined[0], "name"), "S");
        assert_eq!(field_str(&defined[0], "text"), ">>");
        assert_eq!(field_str(&defined[0], "linehl"), "Error");
        // Single-cell text is padded with a space.
        assert_eq!(
            f.number(
                "sign_define",
                vec![string("P"), dict(&[("text", string("x"))])]
            ),
            0
        );
        let defined = items(&f.invoke("sign_getdefined", vec![string("P")]).unwrap());
        assert_eq!(field_str(&defined[0], "text"), "x ");
        assert!(
            items(
                &f.invoke("sign_getdefined", vec![string("Missing")])
                    .unwrap()
            )
            .is_empty()
        );
        assert_eq!(
            f.invoke(
                "sign_define",
                vec![string("Bad"), dict(&[("text", string("abc"))])]
            )
            .unwrap_err()
            .code,
            "E239"
        );
        // List form defines several signs and returns one status per sign.
        let results = f
            .invoke(
                "sign_define",
                vec![list(vec![
                    dict(&[("name", string("A")), ("text", string("AA"))]),
                    dict(&[("name", string("B"))]),
                ])],
            )
            .unwrap();
        assert_eq!(results.to_string_value(), "[0, 0]");
        assert_eq!(
            f.invoke("sign_getdefined", vec![])
                .unwrap()
                .to_string_value()
                .matches("'name'")
                .count(),
            4
        );
        assert_eq!(
            f.invoke("sign_define", vec![string("X"), number(1)])
                .unwrap_err()
                .code,
            "E715"
        );
    }

    #[test]
    fn sign_place_and_getplaced() {
        let mut f = Fixture::new();
        define_sign(&mut f, "S");
        assert_eq!(
            f.number(
                "sign_place",
                vec![
                    number(5),
                    string(""),
                    string("S"),
                    number(1),
                    dict(&[("lnum", number(2))])
                ]
            ),
            5
        );
        let placed = items(&f.invoke("sign_getplaced", vec![number(1)]).unwrap());
        assert_eq!(placed.len(), 1);
        assert_eq!(field_num(&placed[0], "bufnr"), 1);
        let signs = items(&get_field(&placed[0], "signs"));
        assert_eq!(signs.len(), 1);
        assert_eq!(field_num(&signs[0], "id"), 5);
        assert_eq!(field_str(&signs[0], "group"), "");
        assert_eq!(field_num(&signs[0], "lnum"), 2);
        assert_eq!(field_str(&signs[0], "name"), "S");
        assert_eq!(field_num(&signs[0], "priority"), SIGN_DEF_PRIO);
        // Filtering by line and id.
        assert_eq!(
            items(
                &f.invoke(
                    "sign_getplaced",
                    vec![number(1), dict(&[("lnum", number(2))])]
                )
                .unwrap()
            )[0]
            .to_string_value()
            .matches("'id': 5")
            .count(),
            1
        );
        assert_eq!(
            items(&get_field(
                &items(
                    &f.invoke(
                        "sign_getplaced",
                        vec![number(1), dict(&[("id", number(99))])]
                    )
                    .unwrap()
                )[0],
                "signs"
            ))
            .len(),
            0
        );
    }

    #[test]
    fn sign_place_allocates_ids_per_group() {
        let mut f = Fixture::new();
        define_sign(&mut f, "S");
        let first = f.number(
            "sign_place",
            vec![
                number(0),
                string(""),
                string("S"),
                number(1),
                dict(&[("lnum", number(1))]),
            ],
        );
        let second = f.number(
            "sign_place",
            vec![
                number(0),
                string(""),
                string("S"),
                number(1),
                dict(&[("lnum", number(2))]),
            ],
        );
        assert_eq!((first, second), (1, 2));
        let grouped = f.number(
            "sign_place",
            vec![
                number(0),
                string("g"),
                string("S"),
                number(1),
                dict(&[("lnum", number(1))]),
            ],
        );
        assert_eq!(grouped, 1);
        let all = items(&get_field(
            &items(
                &f.invoke(
                    "sign_getplaced",
                    vec![number(1), dict(&[("group", string("*"))])],
                )
                .unwrap(),
            )[0],
            "signs",
        ));
        assert_eq!(all.len(), 3);
        let global = items(&get_field(
            &items(&f.invoke("sign_getplaced", vec![number(1)]).unwrap())[0],
            "signs",
        ));
        assert_eq!(global.len(), 2);
        let in_group = items(&get_field(
            &items(
                &f.invoke(
                    "sign_getplaced",
                    vec![number(1), dict(&[("group", string("g"))])],
                )
                .unwrap(),
            )[0],
            "signs",
        ));
        assert_eq!(in_group.len(), 1);
    }

    #[test]
    fn sign_placelist_and_unplacelist() {
        let mut f = Fixture::new();
        define_sign(&mut f, "S");
        let placed = f
            .invoke(
                "sign_placelist",
                vec![list(vec![
                    dict(&[
                        ("name", string("S")),
                        ("buffer", number(1)),
                        ("lnum", number(1)),
                    ]),
                    dict(&[
                        ("name", string("S")),
                        ("buffer", number(1)),
                        ("lnum", number(3)),
                    ]),
                ])],
            )
            .unwrap();
        assert_eq!(placed.to_string_value(), "[1, 2]");
        let removed = f
            .invoke(
                "sign_unplacelist",
                vec![list(vec![
                    dict(&[("id", number(1)), ("buffer", number(1))]),
                    dict(&[("id", number(99)), ("buffer", number(1))]),
                ])],
            )
            .unwrap();
        assert_eq!(removed.to_string_value(), "[0, -1]");
        assert_eq!(
            items(&get_field(
                &items(&f.invoke("sign_getplaced", vec![number(1)]).unwrap())[0],
                "signs"
            ))
            .len(),
            1
        );
    }

    #[test]
    fn sign_jump_moves_the_cursor() {
        let mut f = Fixture::new();
        define_sign(&mut f, "S");
        f.number(
            "sign_place",
            vec![
                number(7),
                string(""),
                string("S"),
                number(1),
                dict(&[("lnum", number(2))]),
            ],
        );
        f.ed.set_cursor(Pos::new(2, 0));
        assert_eq!(
            f.number("sign_jump", vec![number(7), string(""), number(1)]),
            2
        );
        assert_eq!(f.ed.cursor().line, 1);
        assert_eq!(
            f.invoke("sign_jump", vec![number(99), string(""), number(1)])
                .unwrap_err()
                .code,
            "E157"
        );
        assert_eq!(
            f.invoke("sign_jump", vec![number(0), string(""), number(1)])
                .unwrap_err()
                .code,
            "E474"
        );
    }

    #[test]
    fn sign_unplace_removes_matching_signs() {
        let mut f = Fixture::new();
        define_sign(&mut f, "S");
        for (id, lnum) in [(5, 1), (6, 2)] {
            f.number(
                "sign_place",
                vec![
                    number(id),
                    string(""),
                    string("S"),
                    number(1),
                    dict(&[("lnum", number(lnum))]),
                ],
            );
        }
        assert_eq!(
            f.number("sign_unplace", vec![string(""), dict(&[("id", number(5))])]),
            0
        );
        // Buffer still has sign 6, so removing 5 again fails.
        assert_eq!(
            f.number("sign_unplace", vec![string(""), dict(&[("id", number(5))])]),
            -1
        );
        assert_eq!(
            f.number(
                "sign_unplace",
                vec![string("*"), dict(&[("buffer", number(1))])]
            ),
            0
        );
        assert!(
            items(&get_field(
                &items(&f.invoke("sign_getplaced", vec![number(1)]).unwrap())[0],
                "signs"
            ))
            .is_empty()
        );
        assert_eq!(
            f.invoke("sign_unplace", vec![number(1)]).unwrap_err().code,
            "E474"
        );
    }

    #[test]
    fn sign_undefine_removes_definitions() {
        let mut f = Fixture::new();
        define_sign(&mut f, "S");
        define_sign(&mut f, "T");
        assert_eq!(f.number("sign_undefine", vec![string("S")]), 0);
        assert!(items(&f.invoke("sign_getdefined", vec![string("S")]).unwrap()).is_empty());
        assert_eq!(
            f.invoke("sign_undefine", vec![string("S")])
                .unwrap_err()
                .code,
            "E155"
        );
        // List form returns one result per name.
        assert_eq!(
            f.invoke(
                "sign_undefine",
                vec![list(vec![string("T"), string("Missing")])]
            )
            .unwrap()
            .to_string_value(),
            "[0, -1]"
        );
        assert!(items(&f.invoke("sign_getdefined", vec![]).unwrap()).is_empty());
    }

    #[test]
    fn sign_place_validates_arguments() {
        let mut f = Fixture::new();
        define_sign(&mut f, "S");
        assert_eq!(
            f.invoke(
                "sign_place",
                vec![
                    number(1),
                    string(""),
                    string("Missing"),
                    number(1),
                    dict(&[("lnum", number(1))])
                ]
            )
            .unwrap_err()
            .code,
            "E155"
        );
        assert_eq!(
            f.invoke(
                "sign_place",
                vec![
                    number(1),
                    string(""),
                    string("S"),
                    number(99),
                    dict(&[("lnum", number(1))])
                ]
            )
            .unwrap_err()
            .code,
            "E158"
        );
        // `*` is reserved for sign groups: silent failure like Vim.
        assert_eq!(
            f.number(
                "sign_place",
                vec![
                    number(1),
                    string("*"),
                    string("S"),
                    number(1),
                    dict(&[("lnum", number(1))])
                ]
            ),
            -1
        );
        assert_eq!(
            f.invoke(
                "sign_place",
                vec![number(1), string(""), string("S"), number(1), number(3)]
            )
            .unwrap_err()
            .code,
            "E715"
        );
    }

    // --- matches --------------------------------------------------------

    #[test]
    fn matchadd_getmatches_matchdelete() {
        let mut f = Fixture::new();
        assert_eq!(
            f.number("matchadd", vec![string("Search"), string("foo")]),
            1
        );
        let matches = items(&f.invoke("getmatches", vec![]).unwrap());
        assert_eq!(matches.len(), 1);
        assert_eq!(field_str(&matches[0], "pattern"), "foo");
        assert_eq!(field_str(&matches[0], "group"), "Search");
        assert_eq!(field_num(&matches[0], "priority"), 10);
        assert_eq!(field_num(&matches[0], "id"), 1);
        assert_eq!(
            f.number("matchadd", vec![string("Search"), string("bar")]),
            2
        );
        assert_eq!(f.number("matchdelete", vec![number(1)]), 0);
        assert_eq!(
            f.invoke("matchdelete", vec![number(1)]).unwrap_err().code,
            "E803"
        );
        assert_eq!(
            f.invoke("matchdelete", vec![number(0)]).unwrap_err().code,
            "E802"
        );
        assert_eq!(f.number("clearmatches", vec![]), 0);
        assert!(items(&f.invoke("getmatches", vec![]).unwrap()).is_empty());
        // Empty group or pattern fails silently with -1.
        assert_eq!(f.number("matchadd", vec![string(""), string("x")]), -1);
        assert_eq!(f.number("matchadd", vec![string("Search"), string("")]), -1);
    }

    #[test]
    fn matchadd_priorities_and_reserved_ids() {
        let mut f = Fixture::new();
        f.number(
            "matchadd",
            vec![string("Search"), string("late"), number(20)],
        );
        f.number(
            "matchadd",
            vec![string("Search"), string("early"), number(5)],
        );
        let matches = items(&f.invoke("getmatches", vec![]).unwrap());
        assert_eq!(field_str(&matches[0], "pattern"), "early");
        assert_eq!(field_str(&matches[1], "pattern"), "late");
        assert_eq!(
            f.invoke(
                "matchadd",
                vec![string("Search"), string("x"), number(10), number(2)]
            )
            .unwrap_err()
            .code,
            "E798"
        );
        assert_eq!(
            f.invoke(
                "matchadd",
                vec![string("Search"), string("x"), number(10), number(0)]
            )
            .unwrap_err()
            .code,
            "E799"
        );
        // An explicitly taken id is rejected with E801.
        f.number(
            "matchadd",
            vec![string("Search"), string("y"), number(10), number(7)],
        );
        assert_eq!(
            f.invoke(
                "matchadd",
                vec![string("Search"), string("z"), number(10), number(7)]
            )
            .unwrap_err()
            .code,
            "E801"
        );
    }

    #[test]
    fn matchaddpos_stores_positions_and_matcharg_reads_it() {
        let mut f = Fixture::new();
        let id = f.number(
            "matchaddpos",
            vec![
                string("Search"),
                list(vec![number(1), list(vec![number(2), number(3), number(4)])]),
            ],
        );
        assert_eq!(id, 1);
        let matches = items(&f.invoke("getmatches", vec![]).unwrap());
        assert_eq!(items(&get_field(&matches[0], "pos1")).len(), 1);
        let pos2 = items(&get_field(&matches[0], "pos2"));
        assert_eq!(
            pos2.iter().map(Value::to_number).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        let arg = f.invoke("matcharg", vec![number(1)]).unwrap();
        let arg = items(&arg);
        assert_eq!(arg[0].to_string_value(), "Search");
        assert_eq!(arg[1].to_string_value(), "");
        // matcharg() only reports the three :match ids.
        // matcharg() reports all three :match slots; unset ones are ["", ""].
        assert!(items(&f.invoke("matcharg", vec![number(4)]).unwrap()).is_empty());
        let unset = items(&f.invoke("matcharg", vec![number(3)]).unwrap());
        assert_eq!(unset.len(), 2);
        assert_eq!(unset[0].to_string_value(), "");
        assert_eq!(
            f.invoke("matchaddpos", vec![string("Search"), number(1)])
                .unwrap_err()
                .code,
            "E686"
        );
        assert_eq!(
            f.invoke(
                "matchaddpos",
                vec![string("Search"), list(vec![]), number(10), number(1)]
            )
            .unwrap_err()
            .code,
            "E798"
        );
    }

    #[test]
    fn setmatches_round_trips_getmatches() {
        let mut f = Fixture::new();
        f.number("matchadd", vec![string("Search"), string("foo")]);
        let saved = f.invoke("getmatches", vec![]).unwrap();
        f.number("clearmatches", vec![]);
        assert_eq!(f.number("setmatches", vec![saved.clone()]), 0);
        assert_eq!(
            f.invoke("getmatches", vec![]).unwrap().to_string_value(),
            saved.to_string_value()
        );
        // Invalid entries are rejected with E474.
        assert_eq!(
            f.invoke("setmatches", vec![list(vec![dict(&[])])])
                .unwrap_err()
                .code,
            "E474"
        );
        assert_eq!(
            f.invoke("setmatches", vec![string("x")]).unwrap_err().code,
            "E714"
        );
        assert_eq!(
            f.invoke("matchdelete", vec![number(1), number(3)])
                .unwrap_err()
                .code,
            "E957"
        );
    }

    // --- history --------------------------------------------------------

    #[test]
    fn history_add_get_number_delete() {
        let mut f = Fixture::new();
        assert_eq!(
            f.number("histadd", vec![string("cmd"), string("echo 1")]),
            1
        );
        assert_eq!(
            f.number("histadd", vec![string("cmd"), string("echo 2")]),
            1
        );
        assert_eq!(f.number("histnr", vec![string("cmd")]), 2);
        assert_eq!(
            f.invoke("histget", vec![string("cmd")])
                .unwrap()
                .to_string_value(),
            "echo 2"
        );
        assert_eq!(
            f.invoke("histget", vec![string("cmd"), number(1)])
                .unwrap()
                .to_string_value(),
            "echo 1"
        );
        assert_eq!(
            f.invoke("histget", vec![string("cmd"), number(-1)])
                .unwrap()
                .to_string_value(),
            "echo 2"
        );
        assert_eq!(
            f.invoke("histget", vec![string(":"), number(-2)])
                .unwrap()
                .to_string_value(),
            "echo 1"
        );
        assert_eq!(
            f.invoke("histget", vec![string("cmd"), number(0)])
                .unwrap()
                .to_string_value(),
            ""
        );
        assert_eq!(
            f.invoke("histget", vec![string("cmd"), number(9)])
                .unwrap()
                .to_string_value(),
            ""
        );
        // An empty entry is not added.
        assert_eq!(f.number("histadd", vec![string("cmd"), string("")]), 0);
        // Duplicates move to the newest position.
        f.number("histadd", vec![string("cmd"), string("echo 1")]);
        assert_eq!(
            f.invoke("histget", vec![string("cmd")])
                .unwrap()
                .to_string_value(),
            "echo 1"
        );
        assert_eq!(f.number("histnr", vec![string("cmd")]), 2);
        assert_eq!(f.number("histdel", vec![string("cmd"), number(1)]), 1);
        assert_eq!(
            f.invoke("histget", vec![string("cmd")])
                .unwrap()
                .to_string_value(),
            "echo 1"
        );
        assert_eq!(f.number("histnr", vec![string("cmd")]), 1);
        assert_eq!(f.number("histdel", vec![string("cmd")]), 1);
        assert_eq!(f.number("histnr", vec![string("cmd")]), 0);
        assert_eq!(
            f.invoke("histget", vec![string("cmd")])
                .unwrap()
                .to_string_value(),
            ""
        );
    }

    #[test]
    fn history_delete_by_pattern_and_types_are_separate() {
        let mut f = Fixture::new();
        for entry in ["alpha", "beta", "gamma"] {
            f.number("histadd", vec![string("cmd"), string(entry)]);
        }
        assert_eq!(f.number("histdel", vec![string("cmd"), string("bet")]), 1);
        assert_eq!(f.number("histnr", vec![string("cmd")]), 2);
        assert_eq!(
            f.number("histadd", vec![string("search"), string("needle")]),
            1
        );
        assert_eq!(f.number("histnr", vec![string("search")]), 1);
        assert_eq!(f.number("histnr", vec![string("cmd")]), 2);
        assert_eq!(
            f.invoke("histget", vec![string("/")])
                .unwrap()
                .to_string_value(),
            "needle"
        );
        // Unknown history types are ignored by histadd/histdel and reported as
        // an empty history.
        assert_eq!(
            f.number("histadd", vec![string("nonsense"), string("x")]),
            0
        );
        assert_eq!(f.number("histnr", vec![string("nonsense")]), -1);
        assert_eq!(f.number("histdel", vec![string("nonsense")]), 0);
    }

    // --- balloons -------------------------------------------------------

    #[test]
    fn balloon_show_and_gettext_have_no_ui() {
        let mut f = Fixture::new();
        assert_eq!(
            f.invoke("balloon_gettext", vec![])
                .unwrap()
                .to_string_value(),
            ""
        );
        assert_eq!(f.number("balloon_show", vec![string("hello")]), 0);
        assert_eq!(
            f.number("balloon_show", vec![list(vec![string("a"), string("b")])]),
            0
        );
    }

    #[test]
    fn balloon_split_wraps_at_half_the_screen_width() {
        let mut f = Fixture::new();
        let lines = items(
            &f.invoke("balloon_split", vec![string("hello world")])
                .unwrap(),
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string_value(), "hello world");
        let long = "a".repeat(100);
        let lines = items(&f.invoke("balloon_split", vec![string(long)]).unwrap());
        let widths: Vec<usize> = lines
            .iter()
            .map(|line| line.to_string_value().chars().count())
            .collect();
        assert_eq!(widths, vec![40, 40, 20]);
        let words = "0123456789 0123456789 0123456789 0123456789 0123456789";
        let lines = items(&f.invoke("balloon_split", vec![string(words)]).unwrap());
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].to_string_value(),
            "0123456789 0123456789 0123456789"
        );
        assert_eq!(lines[1].to_string_value(), "0123456789 0123456789");
        // Explicit newlines are preserved and an empty message yields no lines.
        let lines = items(&f.invoke("balloon_split", vec![string("ab\ncd")]).unwrap());
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].to_string_value(), "cd");
        assert!(items(&f.invoke("balloon_split", vec![string("")]).unwrap()).is_empty());
    }

    // --- dispatch bookkeeping ------------------------------------------

    #[test]
    fn funcs_list_matches_dispatch_and_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for name in FUNCS {
            assert!(seen.insert(*name), "duplicate FUNCS entry: {name}");
        }
        assert_eq!(FUNCS.len(), 52);
        for name in FUNCS {
            assert!(
                rvim_viml::FUNCTIONS.iter().any(|f| f.name == *name),
                "{name} is missing from the baseline function table"
            );
            let mut f = Fixture::new();
            // Every listed function must be dispatched; probing with the wrong
            // arity must yield E119 rather than falling through.
            let result = call(name, &[], &mut f.ed, &mut f.env, &mut f.state);
            assert!(
                result.is_some(),
                "{name} is listed in FUNCS but not dispatched"
            );
        }
    }

    #[test]
    fn dispatch_falls_through_for_unknown_names() {
        let mut f = Fixture::new();
        assert!(call("getline", &[], &mut f.ed, &mut f.env, &mut f.state).is_none());
        assert!(call("no_such_function", &[], &mut f.ed, &mut f.env, &mut f.state).is_none());
    }
}
