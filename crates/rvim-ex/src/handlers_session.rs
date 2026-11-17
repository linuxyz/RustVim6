//! Session-level Ex commands: the jump list, the change list and viminfo
//! (reference: ex_docmd.c ex_jumps/ex_changes, viminfo.c, mark.c).
//!
//! A jump list entry is recorded by the front-end before a jump motion (the TUI
//! calls [Editor::push_jump]) and by every buffer switch, so how far back
//! CTRL-O reaches depends on those hooks rather than on a full mark.c.

use std::path::{Path, PathBuf};

use rvim_core::VimResult;
use rvim_editor::viminfo::{VimInfo, VimInfoMark, VimInfoRegister};
use rvim_editor::{Editor, RegType, Register};

use crate::exec::ExOutcome;
use crate::parser::ParsedEx;
use crate::script::ScriptState;
use rvim_viml::Env;

/// Default viminfo file name: vim uses "~/.viminfo" (or the "viminfo" file in
/// the home directory on Windows).
pub fn default_viminfo_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".viminfo")
}

/// The path from the option (when set) or the platform default.
pub fn viminfo_path(ed: &Editor) -> PathBuf {
    let value = ed.wopt_str("viminfo", "");
    // 'viminfo' is "N..." option syntax in vim; a plain path is what RustVim
    // accepts (documented in compat_deviations).
    if value.is_empty() || value.starts_with(|c: char| c.is_ascii_digit()) {
        default_viminfo_path()
    } else {
        PathBuf::from(value)
    }
}

/// Fill a [VimInfo] from the editor state.
pub fn collect(ed: &Editor, state: &ScriptState) -> VimInfo {
    let mut info = VimInfo::default();
    for bufnr in ed.all_buffers().iter().map(|b| b.id) {
        let Some(buffer) = ed.buffer(bufnr).ok() else {
            continue;
        };
        info.buffers.push(buffer.display_name().to_string());
        for name in 'a'..='z' {
            if let Some(pos) = ed.marks.get_local(bufnr, name) {
                info.marks.push(VimInfoMark {
                    name,
                    file: buffer.display_name().to_string(),
                    lnum: pos.line as i64 + 1,
                    col: pos.col as i64,
                });
            }
        }
    }
    for name in 'A'..='Z' {
        if let Some(mark) = ed.marks.get_global(name) {
            if let Ok(buffer) = ed.buffer(mark.bufnr) {
                info.marks.push(VimInfoMark {
                    name,
                    file: buffer.display_name().to_string(),
                    lnum: mark.pos.line as i64 + 1,
                    col: mark.pos.col as i64,
                });
            }
        }
    }
    for name in ('a'..='z').chain([
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '-', '.', ':',
    ]) {
        if let Some(reg) = ed.registers.get(name) {
            info.registers.push(VimInfoRegister {
                name,
                text: reg.text.clone(),
                linewise: reg.reg_type == RegType::LineWise,
            });
        }
    }
    if let Some(reg) = ed.registers.get('/') {
        info.last_search = Some(reg.text.clone());
    }
    info.cmd_history = state.meta.cmd_history.clone();
    info.search_history = state.meta.search_history.clone();
    info.cwd = Some(ed.cwd.display().to_string());
    info
}

/// Restore marks, registers and history from a viminfo file.
pub fn apply(ed: &mut Editor, state: &mut ScriptState, info: &VimInfo) {
    for mark in &info.marks {
        let target = ed
            .all_buffers()
            .iter()
            .find(|b| b.display_name() == mark.file)
            .map(|b| b.id);
        let Some(bufnr) = target else {
            continue;
        };
        let pos = rvim_core::Pos::new((mark.lnum - 1).max(0) as usize, mark.col.max(0) as usize);
        if mark.name.is_ascii_uppercase() {
            ed.marks.set_global(mark.name, bufnr, pos);
        } else {
            ed.marks.set_local(bufnr, mark.name, pos);
        }
    }
    for reg in &info.registers {
        let data = Register {
            text: reg.text.clone(),
            reg_type: if reg.linewise {
                RegType::LineWise
            } else {
                RegType::CharWise
            },
        };
        ed.registers.yank(Some(reg.name), data);
    }
    if let Some(pattern) = &info.last_search {
        ed.search_pattern = pattern.clone();
        ed.registers.yank(
            Some('/'),
            Register {
                text: pattern.clone(),
                reg_type: RegType::CharWise,
            },
        );
    }
    if !info.cmd_history.is_empty() {
        state.meta.cmd_history = info.cmd_history.clone();
    }
    if !info.search_history.is_empty() {
        state.meta.search_history = info.search_history.clone();
    }
    if let Some(cwd) = &info.cwd {
        if Path::new(cwd).is_dir() {
            let _ = std::env::set_current_dir(cwd);
            ed.cwd = PathBuf::from(cwd);
        }
    }
}

/// :rviminfo {file}: read one viminfo file.
pub fn read_viminfo(ed: &mut Editor, state: &mut ScriptState, path: &Path) -> VimResult<()> {
    let info = VimInfo::read(path)?;
    apply(ed, state, &info);
    Ok(())
}

/// :wviminfo {file}: write one viminfo file.
pub fn write_viminfo(ed: &Editor, state: &ScriptState, path: &Path) -> VimResult<()> {
    let info = collect(ed, state);
    info.write(path)
}

/// :jumps, :changes, :rviminfo, :wviminfo.
pub fn cmd_session(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    let _ = env;
    match name {
        "jumps" => {
            for line in ed.jumps_report() {
                ed.messages.info(line);
            }
        }
        "changes" => {
            for line in ed.changes_report() {
                ed.messages.info(line);
            }
        }
        "rviminfo" => {
            let path = if p.args.trim().is_empty() {
                viminfo_path(ed)
            } else {
                PathBuf::from(p.args.trim())
            };
            read_viminfo(ed, state, &path)?;
        }
        "wviminfo" => {
            let path = if p.args.trim().is_empty() {
                viminfo_path(ed)
            } else {
                PathBuf::from(p.args.trim())
            };
            write_viminfo(ed, state, &path)?;
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
    fn jump_list_records_and_moves() {
        let mut ed = Editor::new();
        ed.replace_lines(0, 1, vec!["a".into(), "b".into(), "c".into(), "d".into()])
            .unwrap();
        ed.set_cursor(rvim_core::Pos::new(0, 0));
        ed.push_jump();
        ed.set_cursor(rvim_core::Pos::new(3, 0));
        ed.push_jump();
        assert_eq!(ed.jumplist.len(), 2);
        ed.jump_back().unwrap();
        assert_eq!(ed.cursor().line, 0);
        ed.jump_forward().unwrap();
        assert_eq!(ed.cursor().line, 3);
        assert_eq!(run_line(&mut ed, "jumps").unwrap(), ExOutcome::Continue);
        assert!(
            ed.messages.history().iter().any(|m| m.starts_with('>')),
            "the current entry is marked: {:?}",
            ed.messages.history()
        );
    }

    #[test]
    fn change_list_tracks_edits() {
        let mut ed = Editor::new();
        ed.set_line_text(0, "one".to_string()).unwrap();
        ed.set_line_text(0, "two".to_string()).unwrap();
        assert_eq!(ed.changelist.len(), 2);
        assert_eq!(run_line(&mut ed, "changes").unwrap(), ExOutcome::Continue);
    }

    #[test]
    fn viminfo_round_trip_through_the_ex_commands() {
        let path = std::env::temp_dir().join("rvim_session_viminfo.txt");
        let _ = std::fs::remove_file(&path);
        let mut ed = Editor::new();
        let mut state = ScriptState::default();
        ed.registers.yank(
            Some('a'),
            Register {
                text: "hello".to_string(),
                reg_type: RegType::LineWise,
            },
        );
        state.meta.cmd_history.push("set number".to_string());
        write_viminfo(&ed, &state, &path).unwrap();
        let mut restored = Editor::new();
        let mut restored_state = ScriptState::default();
        read_viminfo(&mut restored, &mut restored_state, &path).unwrap();
        assert_eq!(
            restored.registers.get('a').map(|r| r.text.clone()),
            Some("hello".to_string())
        );
        assert_eq!(
            restored_state.meta.cmd_history,
            vec!["set number".to_string()]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn default_path_is_a_file_name() {
        assert!(
            viminfo_path(&Editor::new())
                .to_string_lossy()
                .contains("viminfo")
        );
    }
}
