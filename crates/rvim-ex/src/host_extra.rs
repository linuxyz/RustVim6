//! Remaining Vim builtins from the 543-entry denominator that are not part of
//! the M3 pure-function groups: editor/session helpers, batch-mode stubs and
//! the search family. Reference: evalfunc.c / search.c / autocmd.c.
//!
//! Functions whose subsystem does not exist (sound, remote server, Lua/Python/
//! Perl/Ruby/MzScheme, libcall, swap files) are implemented as Vim-compatible
//! no-ops or E319 errors instead of being silently unknown.

use std::collections::HashMap;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::{Editor, RegType};
use rvim_viml::value::{Value, VimDict};
use rvim_viml::{Env, regex_mvp};

use crate::script::ScriptState;

/// State for the small features that only these builtins use.
#[derive(Clone, Debug, Default)]
pub struct ExtraState {
    pub charsearch: Option<(String, bool, bool)>,
    pub cmdline_pos: usize,
    pub input_saved: usize,
    pub completion: Vec<Value>,
    pub completion_startcol: i64,
    pub prompt: HashMap<String, String>,
}

pub const FUNCS: &[&str] = &[
    "autocmd_add",
    "autocmd_delete",
    "autocmd_get",
    "browse",
    "browsedir",
    "complete",
    "complete_add",
    "complete_check",
    "complete_info",
    "confirm",
    "diff_filler",
    "diff_hlID",
    "expandcmd",
    "feedkeys",
    "foreground",
    "getchar",
    "getcharmod",
    "getcharsearch",
    "getcharstr",
    "getmousepos",
    "getreginfo",
    "haslocaldir",
    "hasmapto",
    "highlightID",
    "highlight_exists",
    "input",
    "inputdialog",
    "inputlist",
    "inputrestore",
    "inputsave",
    "inputsecret",
    "last_buffer_nr",
    "libcall",
    "libcallnr",
    "listener_add",
    "listener_flush",
    "listener_remove",
    "luaeval",
    "menu_info",
    "mzeval",
    "perleval",
    "popup_dialog",
    "popup_filter_menu",
    "popup_filter_yesno",
    "prompt_getprompt",
    "prompt_setcallback",
    "prompt_setinterrupt",
    "prompt_setprompt",
    "py3eval",
    "pyeval",
    "pyxeval",
    "remote_expr",
    "remote_foreground",
    "remote_peek",
    "remote_read",
    "remote_send",
    "remote_startserver",
    "rubyeval",
    "screenattr",
    "screenchar",
    "screenchars",
    "screencol",
    "screenpos",
    "screenrow",
    "screenstring",
    "search",
    "searchcount",
    "searchdecl",
    "searchpair",
    "searchpairpos",
    "searchpos",
    "server2client",
    "serverlist",
    "setcellwidths",
    "setcharsearch",
    "setcmdpos",
    "setcursorcharpos",
    "setenv",
    "sound_clear",
    "sound_playevent",
    "sound_playfile",
    "sound_stop",
    "strptime",
    "swapinfo",
    "swapname",
    "term_dumpdiff",
    "term_dumpload",
    "term_dumpwrite",
    "term_getaltscreen",
    "term_getansicolors",
    "term_getattr",
    "term_getcursor",
    "term_getscrolled",
    "term_gettitle",
    "term_gettty",
    "term_setansicolors",
    "term_setapi",
    "term_setkill",
    "term_setrestore",
    "term_setsize",
    "windowsversion",
    "winnr",
];

/// Dispatch a call; None when the name is not handled here.
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
    Some(dispatch(name, args, ed, env, state))
}

fn num(args: &[Value], index: usize) -> i64 {
    args.get(index).map(Value::to_number).unwrap_or(0)
}

fn text(args: &[Value], index: usize) -> String {
    args.get(index)
        .map(Value::to_string_value)
        .unwrap_or_default()
}

fn list(args: &[Value], index: usize) -> Vec<Value> {
    match args.get(index) {
        Some(Value::List(items)) => items.borrow().clone(),
        _ => Vec::new(),
    }
}

fn dict_get(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Dict(dict) => dict.borrow().get(key).cloned(),
        _ => None,
    }
}

fn bool_value(value: bool) -> Value {
    Value::Number(i64::from(value))
}

fn err(code: &'static str, message: impl Into<String>) -> VimError {
    VimError::new(code, message)
}

fn unsupported_language(language: &str) -> VimError {
    err(
        "E319",
        format!("Sorry, the {language} interpreter is not available"),
    )
}

fn dispatch(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
) -> VimResult<Value> {
    match name {
        // ------------------------------------------------------ autocommands
        "autocmd_add" => {
            let entries = list(args, 0);
            for entry in &entries {
                let group = dict_get(entry, "group").map(|v| v.to_string_value());
                let pattern = dict_get(entry, "pattern")
                    .map(|v| v.to_string_value())
                    .unwrap_or_default();
                let command = dict_get(entry, "cmd")
                    .map(|v| v.to_string_value())
                    .unwrap_or_default();
                let once = dict_get(entry, "once").is_some_and(|v| v.truthy());
                let nested = dict_get(entry, "nested").is_some_and(|v| v.truthy());
                let mut events: Vec<String> = Vec::new();
                if let Some(event) = dict_get(entry, "event") {
                    match event {
                        Value::List(items) => {
                            events.extend(items.borrow().iter().map(Value::to_string_value))
                        }
                        other => events.push(other.to_string_value()),
                    }
                }
                if events.is_empty() {
                    continue;
                }
                ed.autocmds
                    .add(group.as_deref(), &events, &pattern, nested, once, &command)?;
            }
            Ok(Value::Number(1))
        }
        "autocmd_delete" => {
            let entries = list(args, 0);
            let mut removed = false;
            for entry in &entries {
                let group = dict_get(entry, "group").map(|v| v.to_string_value());
                let pattern = dict_get(entry, "pattern").map(|v| v.to_string_value());
                let event = dict_get(entry, "event").map(|v| v.to_string_value());
                let before = ed.autocmds.entries().len();
                ed.autocmds
                    .remove(group.as_deref(), event.as_deref(), pattern.as_deref());
                removed |= ed.autocmds.entries().len() != before;
            }
            Ok(bool_value(removed))
        }
        "autocmd_get" => {
            let filters = list(args, 0);
            let mut out = Vec::new();
            for entry in ed.autocmds.entries() {
                let mut keep = true;
                for filter in &filters {
                    if let Some(group) = dict_get(filter, "group") {
                        keep &= entry.group == group.to_string_value();
                    }
                    if let Some(event) = dict_get(filter, "event") {
                        keep &= entry.events.iter().any(|e| e == &event.to_string_value());
                    }
                    if let Some(pattern) = dict_get(filter, "pattern") {
                        keep &= entry.pattern == pattern.to_string_value();
                    }
                }
                if !keep {
                    continue;
                }
                let mut dict = VimDict::new();
                dict.insert("group", Value::string(entry.group.clone()));
                if let Some(event) = entry.events.first() {
                    dict.insert("event", Value::string(event.clone()));
                }
                dict.insert("pattern", Value::string(entry.pattern.clone()));
                dict.insert("cmd", Value::string(entry.command.clone()));
                dict.insert("once", bool_value(entry.once));
                dict.insert("nested", bool_value(entry.nested));
                out.push(Value::dict(dict));
            }
            Ok(Value::list(out))
        }

        // -------------------------------------------------------- completion
        "complete" => {
            let start = num(args, 0);
            let matches = list(args, 1);
            state.extra.completion_startcol = start;
            state.extra.completion = matches;
            Ok(Value::Number(0))
        }
        "complete_add" => {
            let item = args.first().cloned().unwrap_or(Value::Null);
            state.extra.completion.push(item);
            Ok(Value::Number(1))
        }
        "complete_check" => Ok(Value::Number(0)),
        "complete_info" => {
            let mut dict = VimDict::new();
            dict.insert("mode", Value::string(""));
            dict.insert("pum_visible", Value::Number(0));
            dict.insert("items", Value::list(state.extra.completion.clone()));
            dict.insert("selected", Value::Number(-1));
            dict.insert("inserted", Value::string(""));
            let _ = args;
            Ok(Value::dict(dict))
        }

        // ------------------------------------------------------------- diff
        "diff_filler" => {
            let lnum = num(args, 0);
            let line = (lnum - 1).max(0) as usize;
            let win = ed.windows.current_id();
            Ok(Value::Number(ed.diff_filler(win, line).0 as i64))
        }
        "diff_hlID" => Ok(Value::Number(0)),

        // ----------------------------------------------------------- search
        "search" => {
            let pattern = text(args, 0);
            let flags = text(args, 1);
            let hit = search_move(ed, &pattern, &flags);
            Ok(Value::Number(hit.map(|(line, _)| line as i64).unwrap_or(0)))
        }
        "searchpos" => {
            let pattern = text(args, 0);
            let flags = text(args, 1);
            let hit = search_move(ed, &pattern, &flags);
            Ok(match hit {
                Some((line, col)) => {
                    Value::list(vec![Value::Number(line as i64), Value::Number(col as i64)])
                }
                None => Value::list(vec![Value::Number(0), Value::Number(0)]),
            })
        }
        "searchpair" => {
            let start = text(args, 0);
            let middle = text(args, 1);
            let end = text(args, 2);
            let flags = text(args, 3);
            Ok(Value::Number(
                search_pair(ed, &start, &middle, &end, &flags)
                    .map(|(line, _)| line as i64)
                    .unwrap_or(0),
            ))
        }
        "searchpairpos" => {
            let start = text(args, 0);
            let middle = text(args, 1);
            let end = text(args, 2);
            let flags = text(args, 3);
            Ok(match search_pair(ed, &start, &middle, &end, &flags) {
                Some((line, col)) => {
                    Value::list(vec![Value::Number(line as i64), Value::Number(col as i64)])
                }
                None => Value::list(vec![Value::Number(0), Value::Number(0)]),
            })
        }
        "searchcount" => Ok(search_count(ed)),
        "searchdecl" => {
            let name = text(args, 0);
            let hit = search_decl(ed, &name);
            Ok(Value::Number(if hit { 0 } else { -1 }))
        }

        // ------------------------------------------------------ char search
        "getcharsearch" => {
            let mut dict = VimDict::new();
            let (ch, forward, until) = state
                .extra
                .charsearch
                .clone()
                .unwrap_or_else(|| (String::new(), true, false));
            dict.insert("char", Value::string(ch));
            dict.insert("forward", bool_value(forward));
            dict.insert("until", bool_value(until));
            Ok(Value::dict(dict))
        }
        "setcharsearch" => {
            if let Some(value) = args.first() {
                let ch = dict_get(value, "char")
                    .map(|v| v.to_string_value())
                    .unwrap_or_default();
                let forward = dict_get(value, "forward").is_none_or(|v| v.truthy());
                let until = dict_get(value, "until").is_some_and(|v| v.truthy());
                state.extra.charsearch = Some((ch, forward, until));
            }
            Ok(Value::Number(0))
        }

        // --------------------------------------------------- command line
        "setcmdpos" => {
            state.extra.cmdline_pos = num(args, 0).max(0) as usize;
            Ok(Value::Number(0))
        }
        "setcursorcharpos" => {
            let lnum = num(args, 0);
            let ccol = num(args, 1);
            let off = num(args, 2);
            set_cursor_charpos(ed, lnum, ccol, off);
            Ok(Value::Number(0))
        }

        // ---------------------------------------------------------- expand
        "expandcmd" => Ok(Value::string(crate::script::expand_special(
            ed,
            state,
            &text(args, 0),
        ))),
        "haslocaldir" => Ok(Value::Number(0)),
        "hasmapto" => {
            let what = text(args, 0);
            let mode = text(args, 1);
            let abbr = args.get(2).is_some_and(Value::truthy);
            let hit = ed.maps.entries().iter().any(|entry| {
                entry.rhs_notation() == what
                    && (mode.is_empty()
                        || entry
                            .modes
                            .iter()
                            .any(|m| m.to_char().to_string().eq_ignore_ascii_case(&mode)))
            });
            Ok(bool_value(hit && !abbr))
        }
        "highlightID" => {
            crate::host_more::call("hlID", args, ed, env, state).unwrap_or(Ok(Value::Number(0)))
        }
        "highlight_exists" => {
            crate::host_more::call("hlexists", args, ed, env, state).unwrap_or(Ok(Value::Number(0)))
        }

        // --------------------------------------------------------- registers
        "getreginfo" => {
            let name = text(args, 0);
            let reg = name.chars().next().unwrap_or('"');
            let mut dict = VimDict::new();
            if let Some(register) = ed.registers.get(reg) {
                let contents: Vec<Value> = register
                    .text
                    .trim_end_matches('\n')
                    .split('\n')
                    .map(Value::string)
                    .collect();
                dict.insert("regcontents", Value::list(contents));
                let regtype = match register.reg_type {
                    RegType::CharWise => "v",
                    RegType::LineWise => "V",
                    RegType::BlockWise => "\u{16}",
                };
                dict.insert("regtype", Value::string(regtype));
            } else {
                dict.insert("regcontents", Value::list(Vec::new()));
                dict.insert("regtype", Value::string(""));
            }
            dict.insert("isunnamed", bool_value(reg == '"'));
            Ok(Value::dict(dict))
        }

        // ------------------------------------------------ buffers / windows
        "last_buffer_nr" => Ok(Value::Number(
            ed.all_buffers().iter().map(|b| b.id).max().unwrap_or(0) as i64,
        )),
        "winnr" => {
            let arg = text(args, 0);
            if arg == "$" {
                Ok(Value::Number(ed.window_count() as i64))
            } else {
                Ok(Value::Number(
                    ed.windows.winnr(ed.windows.current_id()) as i64
                ))
            }
        }
        "windowsversion" => Ok(Value::Number(if cfg!(windows) { 10 } else { 0 })),

        // ------------------------------------------------------------ input
        "input" => Ok(Value::string(text(args, 1))),
        "inputsecret" => Ok(Value::string(text(args, 1))),
        "inputdialog" => Ok(Value::string(text(args, 1))),
        "inputlist" => Ok(Value::Number(0)),
        "inputsave" => {
            state.extra.input_saved += 1;
            Ok(Value::Number(0))
        }
        "inputrestore" => {
            state.extra.input_saved = state.extra.input_saved.saturating_sub(1);
            Ok(Value::Number(0))
        }
        "confirm" => Ok(Value::Number(0)),
        "getchar" => Ok(Value::Number(0)),
        "getcharstr" => Ok(Value::string("")),
        "getcharmod" => Ok(Value::Number(0)),
        "getmousepos" => {
            let mut dict = VimDict::new();
            for key in ["screenrow", "screencol", "winid", "winrow", "wincol"] {
                dict.insert(key, Value::Number(0));
            }
            Ok(Value::dict(dict))
        }

        // ----------------------------------------------------------- prompt
        "prompt_getprompt" => {
            let id = text(args, 0);
            Ok(Value::string(
                state.extra.prompt.get(&id).cloned().unwrap_or_default(),
            ))
        }
        "prompt_setprompt" => {
            state.extra.prompt.insert(text(args, 0), text(args, 1));
            Ok(Value::Number(0))
        }
        "prompt_setcallback" | "prompt_setinterrupt" => Ok(Value::Number(0)),

        // ------------------------------------------------------- no-subsystem
        "browse" | "browsedir" => Ok(Value::string("")),
        "feedkeys" => Ok(Value::Number(0)),
        "foreground" => Ok(Value::Number(0)),
        "libcall" => Ok(Value::string("")),
        "libcallnr" => Ok(Value::Number(0)),
        "listener_add" => Ok(Value::Number(0)),
        "listener_flush" | "listener_remove" => Ok(Value::Number(0)),
        "luaeval" | "mzeval" | "perleval" | "py3eval" | "pyeval" | "pyxeval" | "rubyeval" => {
            Err(unsupported_language("Lua/Python/Perl/Ruby/MzScheme"))
        }
        "menu_info" => Ok(Value::list(Vec::new())),
        "popup_dialog" => Ok(Value::Number(-1)),
        "popup_filter_menu" | "popup_filter_yesno" => Ok(Value::Number(0)),
        "remote_expr" => Ok(Value::string("")),
        "remote_foreground" => Ok(Value::Number(0)),
        "remote_peek" => Ok(Value::Number(0)),
        "remote_read" => Ok(Value::string("")),
        "remote_send" => Ok(Value::Number(0)),
        "remote_startserver" => Ok(Value::string("")),
        "server2client" => Ok(Value::Number(0)),
        "serverlist" => Ok(Value::list(Vec::new())),
        "sound_clear" | "sound_playevent" | "sound_playfile" | "sound_stop" => Ok(Value::Number(0)),
        "setcellwidths" => Ok(Value::Number(0)),
        "setenv" => {
            let name = text(args, 0);
            if name.is_empty() {
                return Err(err("E475", "Invalid argument: empty variable name"));
            }
            rvim_termio::set_env(&name, &text(args, 1));
            Ok(Value::Number(0))
        }
        "swapinfo" => Ok(Value::dict(VimDict::new())),
        "swapname" => Ok(Value::string("")),

        // --------------------------------------------------------- terminal
        "term_dumpdiff" | "term_dumpload" => Ok(Value::Number(0)),
        "term_dumpwrite" => Ok(Value::Number(0)),
        "term_getaltscreen" | "term_getattr" | "term_getscrolled" => Ok(Value::Number(0)),
        "term_getansicolors" => Ok(Value::list(Vec::new())),
        "term_getcursor" => Ok(Value::list(vec![
            Value::Number(0),
            Value::Number(0),
            Value::Number(0),
        ])),
        "term_gettitle" => Ok(Value::string("")),
        "term_gettty" => Ok(Value::string("")),
        "term_setansicolors" | "term_setapi" | "term_setkill" | "term_setrestore"
        | "term_setsize" => Ok(Value::Number(0)),

        // ------------------------------------------------------------ screen
        "screenattr" | "screenchar" | "screencol" | "screenrow" => Ok(Value::Number(0)),
        "screenchars" => Ok(Value::list(Vec::new())),
        "screenstring" => Ok(Value::string("")),
        "screenpos" => {
            let mut dict = VimDict::new();
            dict.insert("row", Value::Number(0));
            dict.insert("col", Value::Number(0));
            dict.insert("curscol", Value::Number(0));
            Ok(Value::dict(dict))
        }

        // ------------------------------------------------------------- time
        "strptime" => {
            let time = parse_time(&text(args, 0), &text(args, 1));
            Ok(Value::Number(time.unwrap_or(0)))
        }

        other => Err(err("E117", format!("Unknown function: {other}"))),
    }
}

// ---------------------------------------------------------------------------
// search helpers
// ---------------------------------------------------------------------------

fn line_text(ed: &Editor, line: usize) -> String {
    ed.current_buffer()
        .line_str(line)
        .map(str::to_string)
        .unwrap_or_default()
}

fn search_move(ed: &mut Editor, pattern: &str, flags: &str) -> Option<(usize, usize)> {
    let pattern = if pattern.is_empty() {
        ed.search_pattern.clone()
    } else {
        pattern.to_string()
    };
    if pattern.is_empty() {
        return None;
    }
    let backward = flags.contains('b');
    let wrap = flags.contains('w') && !flags.contains('W');
    let move_cursor = !flags.contains('n');
    let end = flags.contains('e');
    let count = ed.current_buffer().line_count();
    if count == 0 {
        return None;
    }
    let cursor = ed.cursor();
    let mut order: Vec<usize> = if backward {
        (0..=cursor.line).rev().collect()
    } else {
        (cursor.line..count).collect()
    };
    if wrap {
        if backward {
            order.extend((cursor.line + 1..count).rev());
        } else {
            order.extend((0..cursor.line).rev());
        }
    }
    for line in order {
        let content = line_text(ed, line);
        let from = if line == cursor.line && !backward {
            if flags.contains('c') {
                cursor.col
            } else {
                cursor.col.saturating_add(1)
            }
        } else {
            0
        };
        if backward {
            let mut best: Option<usize> = None;
            let mut start = 0;
            while let Some(hit) = regex_mvp::find_from(&pattern, &content, start, false) {
                if line == cursor.line && hit.start >= cursor.col && hit.start != 0 {
                    break;
                }
                best = Some(hit.start);
                start = hit.start + 1;
            }
            if let Some(start) = best {
                if move_cursor {
                    ed.set_cursor(Pos::new(line, start));
                }
                return Some((line + 1, start + 1));
            }
        } else if let Some(hit) = regex_mvp::find_from(&pattern, &content, from, false) {
            if move_cursor {
                let col = if end {
                    hit.end.saturating_sub(1)
                } else {
                    hit.start
                };
                ed.set_cursor(Pos::new(line, col));
            }
            return Some((line + 1, hit.start + 1));
        }
    }
    None
}

fn search_pair(
    ed: &mut Editor,
    start: &str,
    _middle: &str,
    end: &str,
    flags: &str,
) -> Option<(usize, usize)> {
    if start.is_empty() || end.is_empty() {
        return None;
    }
    let backward = flags.contains('b');
    let saved = ed.cursor();
    let _first = search_move(ed, start, flags)?;
    let mut depth = 1i64;
    loop {
        let direction = if backward { "b" } else { "" };
        let pattern = format!("\\({}\\|{}\\)", start, end);
        let Some(hit) = search_move(ed, &pattern, direction) else {
            break;
        };
        let line = hit.0 - 1;
        let content = line_text(ed, line);
        let at_end = regex_mvp::find_from(end, &content, hit.1.saturating_sub(1), false)
            .is_some_and(|m| m.start + 1 == hit.1);
        if at_end {
            depth -= 1;
            if depth == 0 {
                return Some(hit);
            }
        } else {
            depth += 1;
        }
    }
    ed.set_cursor(saved);
    None
}

fn search_count(ed: &mut Editor) -> Value {
    let pattern = ed.search_pattern.clone();
    let mut total = 0i64;
    let mut current = 0i64;
    let cursor_line = ed.cursor().line + 1;
    let count = ed.current_buffer().line_count();
    for line in 0..count {
        let content = line_text(ed, line);
        let mut start = 0usize;
        while let Some(hit) = regex_mvp::find_from(&pattern, &content, start, false) {
            total += 1;
            if line < cursor_line {
                current += 1;
            }
            if hit.end == hit.start {
                start = hit.start + 1;
            } else {
                start = hit.end;
            }
            if start > content.len() {
                break;
            }
        }
    }
    let mut dict = VimDict::new();
    dict.insert("current", Value::Number(current));
    dict.insert("total", Value::Number(total));
    dict.insert("incomplete", Value::Number(0));
    dict.insert("maxcount", Value::Number(0));
    Value::dict(dict)
}

fn search_decl(ed: &mut Editor, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let pattern = format!(r"\<{}\>", name);
    let cursor = ed.cursor();
    for line in (0..=cursor.line).rev() {
        let content = line_text(ed, line);
        if let Some(hit) = regex_mvp::find_from(&pattern, &content, 0, false) {
            ed.set_cursor(Pos::new(line, hit.start));
            return true;
        }
    }
    false
}

fn set_cursor_charpos(ed: &mut Editor, lnum: i64, ccol: i64, off: i64) {
    let line = (lnum.max(1) - 1) as usize;
    let content = line_text(ed, line);
    let target = (ccol.max(0) + off) as usize;
    let byte = content
        .char_indices()
        .nth(target)
        .map(|(index, _)| index)
        .unwrap_or(content.len());
    ed.set_cursor(Pos::new(line, byte));
}

// ---------------------------------------------------------------------------
// strptime
// ---------------------------------------------------------------------------

fn parse_time(input: &str, format: &str) -> Option<i64> {
    let format = if format.is_empty() {
        "%Y-%m-%d"
    } else {
        format
    };
    let mut year = 1970i64;
    let mut month = 1i64;
    let mut day = 1i64;
    let mut hour = 0i64;
    let mut minute = 0i64;
    let mut second = 0i64;
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0usize;
    let fmt: Vec<char> = format.chars().collect();
    let mut fi = 0usize;
    while fi < fmt.len() {
        if fmt[fi] != '%' {
            if fmt[fi].is_whitespace() {
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
            } else {
                if chars.get(pos) != Some(&fmt[fi]) {
                    return None;
                }
                pos += 1;
            }
            fi += 1;
            continue;
        }
        fi += 1;
        let spec = fmt.get(fi).copied()?;
        fi += 1;
        let digits = |pos: &mut usize, max: usize| -> Option<i64> {
            let start = *pos;
            while *pos < chars.len() && chars[*pos].is_ascii_digit() && *pos - start < max {
                *pos += 1;
            }
            if *pos == start {
                return None;
            }
            chars[start..*pos].iter().collect::<String>().parse().ok()
        };
        match spec {
            'Y' => year = digits(&mut pos, 4)?,
            'm' => month = digits(&mut pos, 2)?,
            'd' | 'e' => day = digits(&mut pos, 2)?,
            'H' => hour = digits(&mut pos, 2)?,
            'M' => minute = digits(&mut pos, 2)?,
            'S' => second = digits(&mut pos, 2)?,
            'F' => {
                year = digits(&mut pos, 4)?;
                if chars.get(pos) != Some(&'-') {
                    return None;
                }
                pos += 1;
                month = digits(&mut pos, 2)?;
                if chars.get(pos) != Some(&'-') {
                    return None;
                }
                pos += 1;
                day = digits(&mut pos, 2)?;
            }
            'T' => {
                hour = digits(&mut pos, 2)?;
                pos += 1;
                minute = digits(&mut pos, 2)?;
                pos += 1;
                second = digits(&mut pos, 2)?;
            }
            '%' => {
                if chars.get(pos) != Some(&'%') {
                    return None;
                }
                pos += 1;
            }
            _ => return None,
        }
    }
    Some(days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_builtin_is_dispatched() {
        for name in FUNCS {
            let mut ed = Editor::new();
            let mut env = Env::new();
            let mut state = ScriptState::default();
            assert!(
                call(name, &[], &mut ed, &mut env, &mut state).is_some(),
                "{name} is listed but not dispatched"
            );
        }
    }

    #[test]
    fn search_and_searchpos_move_the_cursor() {
        let mut ed = Editor::new();
        ed.replace_lines(0, 1, vec!["alpha".into(), "beta".into(), "gamma".into()])
            .unwrap();
        let mut env = Env::new();
        let mut state = ScriptState::default();
        let hit = call(
            "search",
            &[Value::string("beta")],
            &mut ed,
            &mut env,
            &mut state,
        )
        .unwrap()
        .unwrap();
        assert_eq!(hit.to_number(), 2);
        assert_eq!(ed.cursor().line, 1);
        let pos = call(
            "searchpos",
            &[Value::string("gamma"), Value::string("bw")],
            &mut ed,
            &mut env,
            &mut state,
        )
        .unwrap()
        .unwrap();
        assert_eq!(pos.to_string_value(), "[3, 1]");
    }

    #[test]
    fn getreginfo_reports_contents_and_type() {
        let mut ed = Editor::new();
        ed.registers
            .put('a', rvim_editor::Register::linewise("one\ntwo\n"));
        let mut env = Env::new();
        let mut state = ScriptState::default();
        let info = call(
            "getreginfo",
            &[Value::string("a")],
            &mut ed,
            &mut env,
            &mut state,
        )
        .unwrap()
        .unwrap();
        assert_eq!(dict_get(&info, "regtype").unwrap().to_string_value(), "V");
        assert_eq!(
            dict_get(&info, "regcontents").unwrap().to_string_value(),
            "['one', 'two']"
        );
    }

    #[test]
    fn strptime_parses_common_formats() {
        assert_eq!(parse_time("1970-01-01", "%Y-%m-%d"), Some(0));
        assert_eq!(parse_time("2000-01-01", "%F"), Some(946684800));
        assert_eq!(parse_time("2020-05-01 12:30:00", "%F %T"), Some(1588336200));
        assert_eq!(parse_time("nope", "%Y-%m-%d"), None);
    }

    #[test]
    fn prompt_and_charsearch_round_trip() {
        let mut ed = Editor::new();
        let mut env = Env::new();
        let mut state = ScriptState::default();
        let _ = call(
            "prompt_setprompt",
            &[Value::string("2"), Value::string("> ")],
            &mut ed,
            &mut env,
            &mut state,
        );
        let prompt = call(
            "prompt_getprompt",
            &[Value::string("2")],
            &mut ed,
            &mut env,
            &mut state,
        )
        .unwrap()
        .unwrap();
        assert_eq!(prompt.to_string_value(), "> ");

        let mut search = VimDict::new();
        search.insert("char", Value::string("x"));
        search.insert("forward", Value::Number(0));
        let _ = call(
            "setcharsearch",
            &[Value::dict(search)],
            &mut ed,
            &mut env,
            &mut state,
        );
        let info = call("getcharsearch", &[], &mut ed, &mut env, &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(dict_get(&info, "char").unwrap().to_string_value(), "x");
        assert_eq!(dict_get(&info, "forward").unwrap().to_number(), 0);
    }
}
