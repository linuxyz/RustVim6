//! Window and tabpage Ex commands (reference: ex_docmd.c: ex_splitview,
//! ex_close, ex_resize, ex_wincmd, ex_tabnext, ex_tabmove, ex_doautoall).
//!
//! Every command here drives [`rvim_editor::Windows`] and fires the matching
//! autocommands (WinNew/WinEnter/WinLeave, TabNew/TabEnter/TabLeave) so that
//! filetype plugins and user hooks see the same event stream vim produces.

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;

use crate::autodispatch;
use crate::exec::ExOutcome;
use crate::parser::ParsedEx;
use crate::script::ScriptState;
use rvim_viml::Env;

/// Fire one window/tab event with the current buffer as the pattern.
fn fire(ed: &mut Editor, env: &mut Env, state: &mut ScriptState, event: &str) {
    let pattern = autodispatch::buffer_event_name(ed);
    autodispatch::fire(ed, env, state, event, &pattern);
}

/// `:split` / `:vsplit` / `:sview` / `:new` / `:vnew`.
///
/// `name` is the canonical command name, because `:sview` and `:new` differ from
/// a plain split only in what happens after it.
pub fn cmd_splitview(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    let vertical = matches!(name, "vsplit" | "vnew") || std::mem::take(&mut ed.ex_vertical);
    let open_path = p.args.trim();
    let fresh = matches!(name, "new" | "vnew");
    fire(ed, env, state, "WinLeave");
    let id = ed.split_window(vertical)?;
    fire(ed, env, state, "WinNew");
    if fresh {
        let bufnr = ed.new_buffer();
        autodispatch::switch_buffer(ed, env, state, bufnr)?;
    } else if !open_path.is_empty() {
        let path = std::path::PathBuf::from(open_path);
        autodispatch::edit_file(ed, env, state, &path)?;
    } else {
        let bufnr = ed.current_bufnr();
        let _ = id;
        autodispatch::switch_buffer(ed, env, state, bufnr)?;
    }
    if name == "sview" {
        ed.set_option(
            "readonly",
            rvim_editor::OptValue::Bool(true),
            rvim_editor::SetScope::Local,
        )?;
    }
    fire(ed, env, state, "WinEnter");
    Ok(ExOutcome::Continue)
}

/// `:close` / `:hide`.
pub fn cmd_close(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    let hide = name == "hide";
    if hide && ed.window_count() == 1 {
        // ":hide" on the last window hides the buffer instead of closing the
        // window; a fresh empty buffer is the closest equivalent here.
        let bufnr = ed.new_buffer();
        autodispatch::switch_buffer(ed, env, state, bufnr)?;
        return Ok(ExOutcome::Continue);
    }
    fire(ed, env, state, "WinLeave");
    ed.close_current_window(p.bang)?;
    let bufnr = ed.current_bufnr();
    autodispatch::switch_buffer(ed, env, state, bufnr)?;
    fire(ed, env, state, "WinEnter");
    Ok(ExOutcome::Continue)
}

/// `:only`: close every other window of the current tabpage.
pub fn cmd_only(ed: &mut Editor, env: &mut Env, state: &mut ScriptState) -> VimResult<ExOutcome> {
    ed.only_current_window()?;
    fire(ed, env, state, "WinEnter");
    Ok(ExOutcome::Continue)
}

/// `:resize [+-]N` / `:vertical resize N`.
pub fn cmd_resize(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let vertical = std::mem::take(&mut ed.ex_vertical);
    let arg = p.args.trim();
    if arg.is_empty() {
        // A bare ":resize" reports the height instead of changing it.
        let win = ed.windows.current();
        let value = if vertical { win.width } else { win.height };
        ed.echo(format!("{}", value));
        return Ok(ExOutcome::Continue);
    }
    let (delta, absolute) = match arg.strip_prefix('+') {
        Some(rest) => (parse_count(rest, 1)? as i64, None),
        None => match arg.strip_prefix('-') {
            Some(rest) => (-(parse_count(rest, 1)? as i64), None),
            None => (0, Some(parse_count(arg, 1)?)),
        },
    };
    match absolute {
        Some(size) => ed.windows.set_size(size, vertical)?,
        None => ed.windows.resize(delta, vertical)?,
    }
    Ok(ExOutcome::Continue)
}

fn parse_count(text: &str, default: usize) -> VimResult<usize> {
    if text.is_empty() {
        return Ok(default);
    }
    text.trim()
        .parse::<usize>()
        .map_err(|_| VimError::new("E474", format!("Invalid argument: {text}")))
}

/// `:vertical {cmd}`: run `cmd` with the vertical flag set.
pub fn cmd_vertical(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    if p.args.trim().is_empty() {
        return Err(VimError::new("E471", "Argument required"));
    }
    ed.ex_vertical = true;
    let out = crate::exec::run_line_with(ed, env, state, &p.args)?;
    ed.ex_vertical = false;
    Ok(out)
}

/// `:wincmd {arg}`: the argument is an optional count followed by one key.
pub fn cmd_wincmd(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let arg = p.args.trim();
    let (count, rest) = split_count(arg);
    let Some(key) = rest.trim().chars().next() else {
        return Err(VimError::new("E474", "Argument required"));
    };
    ed.wincmd(key, count)?;
    Ok(ExOutcome::Continue)
}

/// Split a leading decimal count off an argument (`3w` → `(3, "w")`).
fn split_count(arg: &str) -> (u64, &str) {
    let digits: String = arg.chars().take_while(char::is_ascii_digit).collect();
    let count = digits.parse::<u64>().unwrap_or(1).max(1);
    (count, &arg[digits.len()..])
}

/// `:tabnew [file]` / `:tabedit [file]`.
pub fn cmd_tabnew(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    let path = p.args.trim();
    fire(ed, env, state, "TabLeave");
    if name == "tabedit" && !path.is_empty() {
        // Reuse the buffer that already holds the file when there is one.
        let wanted = std::path::PathBuf::from(path);
        let existing = ed
            .all_buffers()
            .iter()
            .find(|b| b.name.as_deref() == Some(wanted.as_path()))
            .map(|b| b.id);
        match existing {
            Some(id) => {
                ed.new_tabpage(Some(id))?;
            }
            None => {
                let id = ed.new_buffer();
                ed.new_tabpage(Some(id))?;
                autodispatch::edit_file(ed, env, state, &wanted)?;
            }
        }
    } else if name == "tabedit" {
        let id = ed.new_buffer();
        ed.new_tabpage(Some(id))?;
    } else if path.is_empty() {
        ed.new_tabpage(None)?;
    } else {
        let id = ed.new_buffer();
        ed.new_tabpage(Some(id))?;
        let wanted = std::path::PathBuf::from(path);
        autodispatch::edit_file(ed, env, state, &wanted)?;
    }
    fire(ed, env, state, "TabNew");
    fire(ed, env, state, "WinEnter");
    fire(ed, env, state, "TabEnter");
    Ok(ExOutcome::Continue)
}

/// `:tabclose [N]`, `:tabonly`, `:tabnext [N]`, `:tabprevious`, `:tabfirst`,
/// `:tablast`, `:tabmove [N]`.
pub fn cmd_tab(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    match name {
        "tabclose" => {
            fire(ed, env, state, "TabLeave");
            match p.args.trim() {
                "" => ed.close_current_tabpage(p.bang)?,
                arg => {
                    let n = parse_count(arg, 1)?;
                    ed.windows.close_tabpage(n)?;
                }
            }
            fire(ed, env, state, "TabClosed");
            fire(ed, env, state, "TabEnter");
        }
        "tabonly" => {
            ed.tabonly()?;
            fire(ed, env, state, "TabEnter");
        }
        "tabnext" | "tabprevious" => {
            let forward = name == "tabnext";
            match p.args.trim() {
                "" => ed.cycle_tabpage(forward, 1)?,
                arg => {
                    let n = parse_count(arg, 1)?;
                    ed.goto_tabpage(n)?;
                }
            }
            fire(ed, env, state, "TabLeave");
            fire(ed, env, state, "TabEnter");
        }
        "tabfirst" | "tablast" => {
            let n = if name == "tabfirst" {
                1
            } else {
                ed.tabpage_count()
            };
            ed.goto_tabpage(n)?;
            fire(ed, env, state, "TabLeave");
            fire(ed, env, state, "TabEnter");
        }
        "tabmove" => {
            let arg = p.args.trim();
            let n = if arg.is_empty() {
                0
            } else {
                parse_count(arg, 0)?
            };
            ed.move_tabpage(n)?;
        }
        _ => {}
    }
    Ok(ExOutcome::Continue)
}

/// `:tabs`: list the tabpages, their windows and buffers.
pub fn cmd_tabs(ed: &mut Editor) -> VimResult<ExOutcome> {
    let current = ed.windows.current_tab_index();
    let mut lines = Vec::new();
    for (index, tab) in ed.windows.tabs.iter().enumerate() {
        let ids = tab.layout.leaves();
        let winnr = ids
            .iter()
            .position(|id| *id == tab.current)
            .map(|i| i + 1)
            .unwrap_or(1);
        let marker = if index == current { ">" } else { " " };
        let names: Vec<String> = ids
            .iter()
            .map(|id| {
                ed.windows
                    .get(*id)
                    .and_then(|w| ed.buffer(w.bufnr).ok())
                    .map(|b| b.display_name().to_string())
                    .unwrap_or_default()
            })
            .collect();
        lines.push(format!(
            "{marker} Tab page {} \"{}\" ({} windows)",
            index + 1,
            names.get(winnr - 1).cloned().unwrap_or_default(),
            ids.len()
        ));
    }
    for line in lines {
        ed.messages.info(line);
    }
    Ok(ExOutcome::Continue)
}

/// `:windo` / `:tabdo` / `:bufdo` / `:argdo`: run a command in every context.
pub fn cmd_doall(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    let command = p.args.trim();
    if command.is_empty() {
        return Err(VimError::new("E471", "Argument required"));
    }
    match name {
        "windo" => {
            let ids = ed.windows.ids();
            let saved = ed.windows.current_id();
            for id in ids {
                if ed.windows.get(id).is_none() {
                    continue;
                }
                ed.goto_window(id)?;
                crate::exec::run_line_with(ed, env, state, command)?;
            }
            if ed.windows.get(saved).is_some() {
                ed.goto_window(saved)?;
            }
        }
        "tabdo" => {
            let total = ed.tabpage_count();
            let saved = ed.windows.current_tab_index() + 1;
            for n in 1..=total {
                if n > ed.tabpage_count() {
                    break;
                }
                ed.goto_tabpage(n)?;
                crate::exec::run_line_with(ed, env, state, command)?;
            }
            if saved <= ed.tabpage_count() {
                ed.goto_tabpage(saved)?;
            }
        }
        "bufdo" | "argdo" => {
            let ids: Vec<i32> = ed.all_buffers().iter().map(|b| b.id).collect();
            let saved = ed.current_bufnr();
            for id in ids {
                if ed.buffer(id).is_err() {
                    continue;
                }
                autodispatch::switch_buffer(ed, env, state, id)?;
                crate::exec::run_line_with(ed, env, state, command)?;
            }
            if ed.buffer(saved).is_ok() {
                autodispatch::switch_buffer(ed, env, state, saved)?;
            }
        }
        _ => {}
    }
    Ok(ExOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::run_line;

    #[test]
    fn split_creates_a_second_window_and_close_returns() {
        let mut ed = Editor::new();
        assert_eq!(ed.window_count(), 1);
        run_line(&mut ed, "split").unwrap();
        assert_eq!(ed.window_count(), 2);
        run_line(&mut ed, "vsplit").unwrap();
        assert_eq!(ed.window_count(), 3);
        run_line(&mut ed, "close").unwrap();
        assert_eq!(ed.window_count(), 2);
        run_line(&mut ed, "only").unwrap();
        assert_eq!(ed.window_count(), 1);
        assert_eq!(run_line(&mut ed, "close").unwrap_err().code, "E444");
    }

    #[test]
    fn wincmd_moves_between_windows() {
        let mut ed = Editor::new();
        run_line(&mut ed, "split").unwrap();
        ed.layout_windows(23, 80);
        // ":split" puts the new window on top and switches to it.
        let top = ed.windows.current_id();
        run_line(&mut ed, "wincmd j").unwrap();
        let bottom = ed.windows.current_id();
        assert_ne!(top, bottom);
        run_line(&mut ed, "wincmd k").unwrap();
        assert_eq!(ed.windows.current_id(), top);
        let err = run_line(&mut ed, "wincmd k").unwrap_err();
        assert_eq!(err.code, "E11");
        run_line(&mut ed, "wincmd w").unwrap();
        assert_eq!(ed.windows.current_id(), bottom);
    }

    #[test]
    fn tab_commands_create_and_destroy_tabpages() {
        let mut ed = Editor::new();
        run_line(&mut ed, "tabnew").unwrap();
        assert_eq!(ed.tabpage_count(), 2);
        run_line(&mut ed, "tabnext 1").unwrap();
        assert_eq!(ed.windows.current_tab_index(), 0);
        run_line(&mut ed, "tabnext 2").unwrap();
        assert_eq!(ed.windows.current_tab_index(), 1);
        run_line(&mut ed, "tabclose").unwrap();
        assert_eq!(ed.tabpage_count(), 1);
        assert_eq!(run_line(&mut ed, "tabclose").unwrap_err().code, "E784");
    }

    #[test]
    fn resize_changes_the_window_size() {
        let mut ed = Editor::new();
        run_line(&mut ed, "split").unwrap();
        let before = ed.windows.current().height;
        run_line(&mut ed, "resize +4").unwrap();
        assert_eq!(ed.windows.current().height, before + 4);
        run_line(&mut ed, "resize -2").unwrap();
        assert_eq!(ed.windows.current().height, before + 2);
        run_line(&mut ed, "resize 3").unwrap();
        assert_eq!(ed.windows.current().height, 3);
    }

    #[test]
    fn vertical_split_prefix() {
        let mut ed = Editor::new();
        run_line(&mut ed, "vertical split").unwrap();
        assert_eq!(ed.window_count(), 2);
        ed.layout_windows(20, 80);
        let rects = ed.windows.rects().clone();
        let mut rows: Vec<usize> = rects.values().map(|r| r.row).collect();
        rows.sort_unstable();
        assert_eq!(rows[0], rows[1], "a vertical split keeps the same top row");
        assert_eq!(
            run_line(&mut ed, "vertical").unwrap_err().code,
            "E471",
            ":vertical needs a command"
        );
    }

    #[test]
    fn windo_runs_the_command_in_every_window() {
        let mut ed = Editor::new();
        run_line(&mut ed, "split").unwrap();
        run_line(&mut ed, "windo setlocal number").unwrap();
        for id in ed.windows.ids() {
            assert!(
                ed.windows
                    .get(id)
                    .unwrap()
                    .get_local("number")
                    .is_some_and(|v| v.as_bool() == Some(true)),
                "windo applies to window {id}"
            );
        }
    }
}
