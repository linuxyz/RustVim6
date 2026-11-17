//! Additional host builtins: argument-list access, character positions, manual
//! folds, highlight-group metadata and Ex command-line queries.
//!
//! These are implemented against the current headless/single-window model:
//! "no folds", "no command line being edited" and a single highlight-group
//! registry. States that cannot be observed yet are reported as empty rather
//! than invented.

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::Editor;
use rvim_viml::Env;
use rvim_viml::value::{Value, VimDict};

use crate::script::ScriptState;

/// State for this module: manual fold ranges (per buffer) and highlight groups.
#[derive(Debug, Default)]
pub struct MoreState {
    /// (bufnr, start_lnum, end_lnum, level, closed)
    pub folds: Vec<(i64, i64, i64, i64, bool)>,
    /// Highlight groups: (name, id, properties).
    pub highlight_groups: Vec<(String, i64, VimDict)>,
}

/// Names implemented in this module.
pub const FUNCS: &[&str] = &[
    "argc",
    "argidx",
    "arglistid",
    "argv",
    "bufload",
    "bufwinid",
    "bufwinnr",
    "charcol",
    "cursor",
    "deletebufline",
    "foldclosed",
    "foldclosedend",
    "foldlevel",
    "foldtext",
    "foldtextresult",
    "fullcommand",
    "getcharpos",
    "getcmdcompltype",
    "getcmdline",
    "getcmdpos",
    "getcmdscreenpos",
    "getcmdtype",
    "getcmdwintype",
    "getcompletion",
    "getcursorcharpos",
    "hlexists",
    "hlget",
    "hlID",
    "hlset",
    "setcharpos",
];

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Null)
}

fn sarg(args: &[Value], i: usize) -> String {
    arg(args, i).to_string_value()
}

fn narg(args: &[Value], i: usize) -> i64 {
    arg(args, i).to_number()
}

/// Dispatch a call; `None` when the name does not belong to this module.
pub fn call(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    _env: &mut Env,
    state: &mut ScriptState,
) -> Option<VimResult<Value>> {
    if !FUNCS.contains(&name) {
        return None;
    }
    Some(dispatch(name, args, ed, state))
}

fn dispatch(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    state: &mut ScriptState,
) -> VimResult<Value> {
    match name {
        // -------------------------------------------------- argument list
        "argc" => Ok(Value::Number(ed.arglist.len() as i64)),
        "argidx" => Ok(Value::Number(ed.arg_index as i64)),
        "arglistid" => Ok(Value::Number(0)),
        "argv" => {
            if args.is_empty() || narg(args, 0) == 0 {
                let all: Vec<Value> = ed
                    .arglist
                    .iter()
                    .map(|p| Value::String(p.display().to_string()))
                    .collect();
                Ok(Value::list(all))
            } else {
                let idx = narg(args, 0);
                if idx < 0 || idx as usize > ed.arglist.len() {
                    return Ok(Value::Null);
                }
                Ok(Value::String(
                    ed.arglist[idx as usize - 1].display().to_string(),
                ))
            }
        }

        // ------------------------------------------------- buffer loading
        "bufload" => Ok(Value::Number(0)),
        "bufwinid" | "bufwinnr" => {
            let target = narg(args, 0);
            let current = if target == 0 {
                ed.current_bufnr()
            } else {
                target as i32
            };
            let loaded = ed.all_buffers().iter().any(|b| b.id == current);
            Ok(Value::Number(if loaded { 1 } else { -1 }))
        }
        "deletebufline" => {
            let first = narg(args, 1).max(1) as usize;
            let last = if args.len() > 2 {
                narg(args, 2).max(1) as usize
            } else {
                first
            };
            let total = ed.line_count();
            if first > total {
                return Err(VimError::new(
                    "E16",
                    format!("Invalid range: line {first} out of range"),
                ));
            }
            let end = last.min(total);
            if first == 1 && end >= total {
                ed.replace_lines(0, total, vec![String::new()])?;
            } else {
                ed.replace_lines(first - 1, end, Vec::new())?;
            }
            Ok(Value::Number(0))
        }

        // ------------------------------------------------------ positions
        "cursor" => {
            let lnum = narg(args, 0);
            let col = if args.len() > 1 { narg(args, 1) } else { 1 };
            if lnum < 1 || lnum as usize > ed.line_count() {
                return Err(VimError::new(
                    "E16",
                    format!("Invalid range: line {lnum} out of range"),
                ));
            }
            let line = ed.current_buffer().line_str(lnum as usize - 1)?.to_string();
            let col = col.max(1) as usize;
            let byte = line
                .char_indices()
                .nth(col - 1)
                .map(|(b, _)| b)
                .unwrap_or(line.len());
            ed.set_cursor(Pos::new(lnum as usize - 1, byte));
            Ok(Value::Number(0))
        }
        "charcol" => {
            let spec = sarg(args, 0);
            let pos = match spec.as_str() {
                "." | "" => ed.cursor(),
                "$" => Pos::new(ed.line_count().saturating_sub(1), 0),
                mark if mark.starts_with('\'') => {
                    let name = mark.chars().nth(1).unwrap_or('"');
                    ed.marks
                        .resolve(ed.current_bufnr(), name)
                        .unwrap_or_else(|| Pos::new(0, 0))
                }
                _ => ed.cursor(),
            };
            let line = ed.current_buffer().line_str(pos.line)?;
            Ok(Value::Number(
                rvim_core::text::byte_to_char_col(line, pos.col) as i64 + 1,
            ))
        }
        "getcursorcharpos" => {
            let pos = ed.cursor();
            let line = ed.current_buffer().line_str(pos.line)?;
            let col = rvim_core::text::byte_to_char_col(line, pos.col) as i64 + 1;
            Ok(Value::list(vec![
                Value::Number(0),
                Value::Number(pos.line as i64 + 1),
                Value::Number(col),
                Value::Number(0),
                Value::Number(0),
            ]))
        }
        "getcharpos" => {
            let spec = sarg(args, 0);
            let (bufnr, pos) = if spec == "." || spec.is_empty() {
                (ed.current_bufnr() as i64, ed.cursor())
            } else if spec == "$" {
                (
                    ed.current_bufnr() as i64,
                    Pos::new(ed.line_count().saturating_sub(1), 0),
                )
            } else if let Some(mark) = spec.strip_prefix('\'') {
                let name = mark.chars().next().unwrap_or('"');
                (
                    ed.current_bufnr() as i64,
                    ed.marks
                        .resolve(ed.current_bufnr(), name)
                        .unwrap_or_else(|| Pos::new(0, 0)),
                )
            } else {
                (0, Pos::new(0, 0))
            };
            let line = ed.current_buffer().line_str(pos.line)?;
            let col = rvim_core::text::byte_to_char_col(line, pos.col) as i64 + 1;
            Ok(Value::list(vec![
                Value::Number(bufnr),
                Value::Number(pos.line as i64 + 1),
                Value::Number(col),
                Value::Number(0),
            ]))
        }
        "setcharpos" => {
            let spec = sarg(args, 0);
            let list = match args.get(1) {
                Some(Value::List(l)) => l.borrow().clone(),
                _ => return Ok(Value::Number(-1)),
            };
            let lnum = list.get(1).map(Value::to_number).unwrap_or(0);
            let col = list.get(2).map(Value::to_number).unwrap_or(0);
            let pos = Pos::new(
                lnum.saturating_sub(1).max(0) as usize,
                col.saturating_sub(1).max(0) as usize,
            );
            if spec == "." {
                ed.set_cursor(pos);
            } else if let Some(mark) = spec.strip_prefix('\'') {
                let name = mark.chars().next().unwrap_or('"');
                let bufnr = ed.current_bufnr();
                ed.marks.set_local(bufnr, name, pos);
            }
            Ok(Value::Number(0))
        }

        // ---------------------------------------------------------- folds
        "foldlevel" => {
            let lnum = if args.is_empty() {
                ed.cursor().line as i64 + 1
            } else {
                narg(args, 0)
            };
            let bufnr = ed.current_bufnr() as i64;
            let level = state
                .more
                .folds
                .iter()
                .filter(|(b, s, e, _, _)| *b == bufnr && (lnum >= *s && lnum <= *e))
                .map(|(_, _, _, level, _)| *level)
                .max()
                .unwrap_or(0);
            Ok(Value::Number(level))
        }
        "foldclosed" | "foldclosedend" => {
            let lnum = if args.is_empty() {
                ed.cursor().line as i64 + 1
            } else {
                narg(args, 0)
            };
            let bufnr = ed.current_bufnr() as i64;
            let fold =
                state.more.folds.iter().find(|(b, s, e, _, closed)| {
                    *b == bufnr && *closed && lnum >= *s && lnum <= *e
                });
            Ok(Value::Number(match (name, fold) {
                (_, None) => -1,
                ("foldclosed", Some((_, s, _, _, _))) => *s,
                ("foldclosedend", Some((_, _, e, _, _))) => *e,
                _ => -1,
            }))
        }
        "foldtext" => {
            // Default fold text: the first line of the fold.
            let line = ed.current_line();
            Ok(Value::String(line))
        }
        "foldtextresult" => Ok(Value::String(String::new())),

        // ------------------------------------------- Ex command line state
        // No command line is being edited in the headless model.
        "getcmdline" => Ok(Value::String(
            state.meta.cmd_history.last().cloned().unwrap_or_default(),
        )),
        "getcmdtype" => Ok(Value::String(String::new())),
        "getcmdpos" => Ok(Value::Number(0)),
        "getcmdscreenpos" => Ok(Value::Number(0)),
        "getcmdwintype" => Ok(Value::String(String::new())),
        "getcmdcompltype" => Ok(Value::String(String::new())),

        // ---------------------------------------------------- completion
        "getcompletion" => {
            let prefix = sarg(args, 0);
            let kind = sarg(args, 1);
            let mut out: Vec<String> = Vec::new();
            match kind.as_str() {
                "command" => {
                    for cmd in crate::table::COMMANDS.iter() {
                        if cmd.name.starts_with(&prefix) {
                            out.push(cmd.name.to_string());
                        }
                    }
                    for name in state.funcs.keys() {
                        if !name.starts_with("<SNR>") && name.starts_with(&prefix) {
                            out.push(name.clone());
                        }
                    }
                }
                "function" => {
                    for name in rvim_viml::builtins::implemented_funcs() {
                        if name.starts_with(&prefix) {
                            out.push(name.to_string());
                        }
                    }
                    for name in state.funcs.keys() {
                        if !name.starts_with("<SNR>") && name.starts_with(&prefix) {
                            out.push(name.clone());
                        }
                    }
                }
                "option" => {
                    for line in ed.options.render_all() {
                        let name = line
                            .split('=')
                            .next()
                            .unwrap_or("")
                            .trim_start_matches("no");
                        if name.starts_with(&prefix) {
                            out.push(name.to_string());
                        }
                    }
                }
                "file" => {
                    let dir = std::env::current_dir().unwrap_or_default();
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if name.starts_with(&prefix) {
                                out.push(name);
                            }
                        }
                    }
                }
                _ => {}
            }
            out.sort();
            out.dedup();
            Ok(Value::list(out.into_iter().map(Value::String).collect()))
        }

        // -------------------------------------------------- full command
        "fullcommand" => {
            let word = sarg(args, 0);
            Ok(Value::String(
                crate::table::lookup(&word)
                    .map(|c| c.name.to_string())
                    .unwrap_or_default(),
            ))
        }

        // ------------------------------------------ highlight groups
        "hlget" => {
            let filter = if args.is_empty() {
                None
            } else {
                Some(sarg(args, 0))
            };
            let items: Vec<Value> = state
                .more
                .highlight_groups
                .iter()
                .filter(|(n, _, _)| filter.as_ref().is_none_or(|f| n == f))
                .map(|(name, id, props)| {
                    let mut d = props.clone();
                    d.insert("name", Value::String(name.clone()));
                    d.insert("id", Value::Number(*id));
                    Value::dict(d)
                })
                .collect();
            Ok(Value::list(items))
        }
        "hlset" => {
            let list = match args.first() {
                Some(Value::List(l)) => l.borrow().clone(),
                _ => return Err(VimError::new("E714", "List required".to_string())),
            };
            for item in list {
                let Value::Dict(d) = item else {
                    return Err(VimError::new("E715", "Dictionary required".to_string()));
                };
                let d = d.borrow();
                let name = d
                    .get("name")
                    .map(Value::to_string_value)
                    .unwrap_or_default();
                let id = state.more.highlight_groups.len() as i64 + 1;
                let mut props = VimDict::new();
                for (k, v) in d.iter() {
                    if k != "name" {
                        props.insert(k.clone(), v.clone());
                    }
                }
                if let Some(existing) = state
                    .more
                    .highlight_groups
                    .iter_mut()
                    .find(|(n, _, _)| *n == name)
                {
                    existing.2 = props;
                } else {
                    state.more.highlight_groups.push((name, id, props));
                }
            }
            Ok(Value::Number(0))
        }
        "hlID" => {
            let name = sarg(args, 0);
            let hit = state
                .more
                .highlight_groups
                .iter()
                .find(|(n, _, _)| n.eq_ignore_ascii_case(&name))
                .map(|(_, id, _)| *id)
                .unwrap_or(0);
            Ok(Value::Number(hit))
        }
        "hlexists" => {
            let name = sarg(args, 0);
            let hit = state
                .more
                .highlight_groups
                .iter()
                .any(|(n, _, _)| n.eq_ignore_ascii_case(&name));
            Ok(Value::Number(i64::from(hit)))
        }

        other => Err(VimError::new(
            "E117",
            format!("builtin {other} is listed but not dispatched"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_editor::Editor;

    fn state() -> (Editor, Env, ScriptState) {
        (Editor::new(), Env::new(), ScriptState::default())
    }

    fn call_ok(
        name: &str,
        args: &[Value],
        ed: &mut Editor,
        env: &mut Env,
        state: &mut ScriptState,
    ) -> Value {
        call(name, args, ed, env, state)
            .expect("handled")
            .expect("ok")
    }

    #[test]
    fn argument_list_accessors() {
        let (mut ed, mut env, mut st) = state();
        ed.arglist = vec!["a.txt".into(), "b.txt".into()];
        ed.arg_index = 1;
        assert_eq!(
            call_ok("argc", &[], &mut ed, &mut env, &mut st).to_number(),
            2
        );
        assert_eq!(
            call_ok("argidx", &[], &mut ed, &mut env, &mut st).to_number(),
            1
        );
        assert_eq!(
            call_ok("argv", &[Value::Number(0)], &mut ed, &mut env, &mut st).to_string_value(),
            "['a.txt', 'b.txt']"
        );
        assert_eq!(
            call_ok("argv", &[Value::Number(2)], &mut ed, &mut env, &mut st).to_string_value(),
            "b.txt"
        );
        assert_eq!(
            call_ok("arglistid", &[], &mut ed, &mut env, &mut st).to_number(),
            0
        );
    }

    #[test]
    fn buffer_loading_and_window_numbers() {
        let (mut ed, mut env, mut st) = state();
        assert_eq!(
            call_ok("bufload", &[Value::Number(0)], &mut ed, &mut env, &mut st).to_number(),
            0
        );
        assert_eq!(
            call_ok("bufwinnr", &[Value::Number(0)], &mut ed, &mut env, &mut st).to_number(),
            1
        );
        assert_eq!(
            call_ok("bufwinid", &[Value::Number(0)], &mut ed, &mut env, &mut st).to_number(),
            1
        );
        assert_eq!(
            call_ok("bufwinid", &[Value::Number(99)], &mut ed, &mut env, &mut st).to_number(),
            -1
        );
    }

    #[test]
    fn cursor_and_character_positions() {
        let (mut ed, mut env, mut st) = state();
        ed.replace_lines(0, 1, vec!["alpha".into(), "beta".into()])
            .unwrap();
        assert_eq!(
            call_ok(
                "cursor",
                &[Value::Number(2), Value::Number(2)],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            0
        );
        assert_eq!(ed.cursor().line, 1);
        assert_eq!(
            call_ok(
                "charcol",
                &[Value::String(".".into())],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            2
        );
        let pos = call_ok("getcursorcharpos", &[], &mut ed, &mut env, &mut st);
        assert_eq!(pos.to_string_value(), "[0, 2, 2, 0, 0]");
        let pos = call_ok(
            "getcharpos",
            &[Value::String(".".into())],
            &mut ed,
            &mut env,
            &mut st,
        );
        assert_eq!(pos.to_string_value(), "[1, 2, 2, 0]");
        assert_eq!(
            call_ok(
                "setcharpos",
                &[Value::String(".".into()), pos],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            0
        );
    }

    #[test]
    fn deletebufline_removes_lines() {
        let (mut ed, mut env, mut st) = state();
        ed.replace_lines(0, 1, vec!["a".into(), "b".into(), "c".into()])
            .unwrap();
        assert_eq!(
            call_ok(
                "deletebufline",
                &[Value::Number(0), Value::Number(2)],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            0
        );
        assert_eq!(ed.current_buffer().full_text(), "a\nc");
    }

    #[test]
    fn folds_report_no_fold_state() {
        let (mut ed, mut env, mut st) = state();
        assert_eq!(
            call_ok("foldlevel", &[Value::Number(1)], &mut ed, &mut env, &mut st).to_number(),
            0
        );
        assert_eq!(
            call_ok(
                "foldclosed",
                &[Value::Number(1)],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            -1
        );
        assert_eq!(
            call_ok(
                "foldclosedend",
                &[Value::Number(1)],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            -1
        );
        assert_eq!(
            call_ok(
                "foldtextresult",
                &[Value::Number(1)],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_string_value(),
            ""
        );
    }

    #[test]
    fn command_line_state_is_empty_headless() {
        let (mut ed, mut env, mut st) = state();
        assert_eq!(
            call_ok("getcmdline", &[], &mut ed, &mut env, &mut st).to_string_value(),
            ""
        );
        assert_eq!(
            call_ok("getcmdtype", &[], &mut ed, &mut env, &mut st).to_string_value(),
            ""
        );
        assert_eq!(
            call_ok("getcmdpos", &[], &mut ed, &mut env, &mut st).to_number(),
            0
        );
        assert_eq!(
            call_ok("getcmdwintype", &[], &mut ed, &mut env, &mut st).to_string_value(),
            ""
        );
        assert_eq!(
            call_ok("getcmdcompltype", &[], &mut ed, &mut env, &mut st).to_string_value(),
            ""
        );
        assert_eq!(
            call_ok("getcmdscreenpos", &[], &mut ed, &mut env, &mut st).to_number(),
            0
        );
    }

    #[test]
    fn completion_and_fullcommand() {
        let (mut ed, mut env, mut st) = state();
        let cmds = call_ok(
            "getcompletion",
            &[Value::String("wri".into()), Value::String("command".into())],
            &mut ed,
            &mut env,
            &mut st,
        );
        assert!(cmds.to_string_value().contains("write"), "{cmds}");
        let opts = call_ok(
            "getcompletion",
            &[
                Value::String("tabstop".into()),
                Value::String("option".into()),
            ],
            &mut ed,
            &mut env,
            &mut st,
        );
        assert!(opts.to_string_value().contains("tabstop"));
        assert_eq!(
            call_ok(
                "fullcommand",
                &[Value::String("w".into())],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_string_value(),
            "write"
        );
        assert_eq!(
            call_ok(
                "fullcommand",
                &[Value::String("zzz".into())],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_string_value(),
            ""
        );
    }

    #[test]
    fn highlight_group_metadata() {
        let (mut ed, mut env, mut st) = state();
        assert_eq!(
            call_ok(
                "hlID",
                &[Value::String("ErrorMsg".into())],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            0
        );
        let mut d = VimDict::new();
        d.insert("name", Value::String("ErrorMsg".into()));
        d.insert("guifg", Value::String("Red".into()));
        assert_eq!(
            call_ok(
                "hlset",
                &[Value::list(vec![Value::dict(d)])],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            0
        );
        assert_eq!(
            call_ok(
                "hlID",
                &[Value::String("ErrorMsg".into())],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            1
        );
        assert_eq!(
            call_ok(
                "hlexists",
                &[Value::String("errormsg".into())],
                &mut ed,
                &mut env,
                &mut st
            )
            .to_number(),
            1
        );
        let list = call_ok("hlget", &[], &mut ed, &mut env, &mut st);
        assert!(list.to_string_value().contains("ErrorMsg"));
    }
}
