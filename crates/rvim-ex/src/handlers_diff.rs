//! Diff-mode Ex commands (reference: `diff.c` `ex_diffthis()`,
//! `ex_diffoff()`, `ex_diffsplit()`, `ex_diffupdate()`,
//! `ex_diffgetput()`).
//!
//! The window and buffer work is done by `rvim_editor::diff`; this module adds
//! the Ex surface and the autocommands vim fires around it:
//!
//! | command       | effect                                                      |
//! | ------------- | ----------------------------------------------------------- |
//! | `:diffthis`   | put the current window and buffer in diff mode              |
//! | `:diff`       | alias of `:diffthis` (see the deviations below)             |
//! | `:diffoff`    | leave diff mode (`!`: every window of every tabpage)        |
//! | `:diffsplit`  | split, edit the file (if any) and diff both windows         |
//! | `:diffupdate` | recompute every diff window's changed lines                 |
//! | `:diffget`    | copy the changed block under the cursor from another window |
//! | `:diffput`    | copy the changed block under the cursor to another window   |
//!
//! `:diffsplit` honours the `:vertical` modifier and fires
//! `WinLeave`/`WinNew`/`WinEnter`; `:diffupdate`, `:diffget` and
//! `:diffput` fire `DiffUpdated` for the current buffer, like vim's
//! `apply_autocmds(EVENT_DIFFUPDATED, ...)`.
//!
//! # Deviations from Vim 8.2
//!
//! * `:diff` is an alias of `:diffthis` as requested for RustVim; real vim
//!   resolves the prefix `:diff` to `:diffupdate` because that entry comes
//!   first in its command table. The dispatcher has to pass the typed word
//!   (see `crates/rvim-ex/src/exec.rs`), otherwise this arm is unreachable.
//! * `:diffget`/`:diffput` ignore a range: the whole changed block under the
//!   cursor is copied, there is no partial-block copy.
//! * Without an argument `:diffget`/`:diffput` use the *first* counterpart;
//!   vim refuses with E101 when three or more buffers are in diff mode.
//! * `:diffput` does not check 'modifiable' (that option does not exist yet, so
//!   E793 cannot be reported); `:diffget` does not check it either.
//! * `:diffget` has no register argument in vim; the register support lives in
//!   `Editor::diff_get`/`diff_put` and is not reachable from Ex.
//! * `:diffsplit` does not move the cursor to the line corresponding to the old
//!   window (`diff_get_corresponding_line()`) and does not scroll to the same
//!   fraction, because there is no cursor-correspondence helper yet.
//! * `:diffpatch` invokes the external `patch` tool on a temporary copy; the
//!   patched file opens in a split window and both windows enter diff mode.

use std::path::{Path, PathBuf};
use std::process::Command;

use rvim_core::{VimError, VimResult};
use rvim_editor::{Editor, WindowId};

use crate::autodispatch;
use crate::exec::ExOutcome;
use crate::parser::ParsedEx;
use crate::script::ScriptState;
use rvim_viml::Env;

/// Dispatch one of the diff-mode Ex commands.
///
/// `name` is the command name to run: the canonical table name
/// (`diffthis`, `diffoff`, `diffsplit`, `diffupdate`, `diffget`,
/// `diffput`) or the `diff` alias. Unknown names are ignored so that a
/// future dispatch arm cannot panic.
pub fn cmd_diff(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    match name {
        "diffthis" | "diff" => ed.diff_this()?,
        "diffoff" => ed.diff_off(p.bang)?,
        "diffsplit" => return cmd_diffsplit(ed, env, state, p),
        "diffupdate" => {
            ed.diff_update()?;
            if ed.in_diff_mode(ed.current_win_id()) {
                fire_diff_updated(ed, env, state);
            }
        }
        "diffget" => return cmd_diffgetput(ed, env, state, p, false),
        "diffput" => return cmd_diffgetput(ed, env, state, p, true),
        "diffpatch" => return cmd_diffpatch(ed, env, state, p),
        _ => {}
    }
    Ok(ExOutcome::Continue)
}

/// :diffpatch {patchfile}: apply the patch to a temp copy, open it in a split
/// window and diff it against the current buffer (diff.c ex_diffpatch).
fn cmd_diffpatch(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let patchfile = PathBuf::from(p.args.trim());
    if patchfile.as_os_str().is_empty() {
        return Err(VimError::new("E471", "Argument required"));
    }
    if !patchfile.is_file() {
        return Err(VimError::new(
            "E484",
            format!("Can't open file {}", patchfile.display()),
        ));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir();
    let original = dir.join(format!("rvim_patch_{}_{stamp}.orig", std::process::id()));
    let patched = dir.join(format!("rvim_patch_{}_{stamp}.new", std::process::id()));
    let body = rvim_buffer::fileio::serialize_buffer(ed.current_buffer());
    std::fs::write(&original, body)
        .map_err(|e| VimError::new("E482", format!("Can't write {}: {e}", original.display())))?;
    // GNU patch also accepts `-o`, but BusyBox patch (bundled with some
    // Windows toolchains) only patches ORIGFILE in place.  Patch the temp
    // original and then open it; the user's file is never touched.
    let status = Command::new("patch")
        .arg("-p0")
        .arg("-i")
        .arg(&patchfile)
        .arg(&original)
        .output();
    let output_ok = matches!(&status, Ok(out) if out.status.success())
        && original.metadata().map(|m| m.len() > 0).unwrap_or(false);
    if !output_ok {
        let _ = std::fs::remove_file(&original);
        let _ = std::fs::remove_file(original.with_extension("rej"));
        let _ = std::fs::remove_file(&patched);
        return Err(VimError::new(
            "E816",
            "Cannot read patch output (is patch installed?)",
        ));
    }
    let command = format!("vsplit {}", original.display());
    crate::exec::run_line_with(ed, env, state, &command)?;
    crate::exec::run_line_with(ed, env, state, "windo diffthis")?;
    // The split has already read the file; the temp copy can go now.
    let _ = std::fs::remove_file(&original);
    let _ = std::fs::remove_file(original.with_extension("rej"));
    let _ = std::fs::remove_file(&patched);
    Ok(ExOutcome::Continue)
}

/// `:diffsplit [file]` - split the window, edit the file and diff both windows.
fn cmd_diffsplit(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let vertical = std::mem::take(&mut ed.ex_vertical);
    let old = ed.current_win_id();
    fire(ed, env, state, "WinLeave");
    let new = ed.split_window(vertical)?;
    fire(ed, env, state, "WinNew");
    let path = p.args.trim();
    if !path.is_empty() {
        // Reuse the buffer that already holds the file, exactly like `:split`.
        autodispatch::edit_file(ed, env, state, &PathBuf::from(path))?;
    }
    ed.diff_this()?;
    if ed.windows.get(old).is_some() {
        // The window that was split is diffed too, so both views line up.
        ed.goto_window(old)?;
        ed.diff_this()?;
        ed.goto_window(new)?;
    }
    fire(ed, env, state, "WinEnter");
    Ok(ExOutcome::Continue)
}

/// `:diffget` (`put == false`) and `:diffput` (`put == true`).
fn cmd_diffgetput(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    put: bool,
) -> VimResult<ExOutcome> {
    if let Some(target) = resolve_target(ed, p.args.trim())? {
        if put {
            ed.diff_put(target, None)?;
        } else {
            ed.diff_get(target, None)?;
        }
        fire_diff_updated(ed, env, state);
    }
    // No target means the argument named the current buffer: vim does nothing.
    Ok(ExOutcome::Continue)
}

/// Resolve the window `:diffget`/`:diffput` work on.
///
/// Without an argument the first counterpart is used (deviation: vim reports
/// E101 instead of guessing when there are three or more diff buffers).
/// `Ok(None)` means "nothing to do": the argument names the current buffer.
fn resolve_target(ed: &Editor, arg: &str) -> VimResult<Option<WindowId>> {
    let cur = ed.current_win_id();
    if !ed.in_diff_mode(cur) {
        return Err(VimError::new("E99", "Current buffer is not in diff mode"));
    }
    if arg.is_empty() {
        return match ed.diff_counterparts(cur).first() {
            Some(win) => Ok(Some(*win)),
            None => Err(VimError::new("E100", "No other buffer in diff mode")),
        };
    }
    let bufnr = find_buffer(ed, arg)?;
    if bufnr == ed.current_bufnr() {
        return Ok(None);
    }
    let target = ed
        .diff_counterparts(cur)
        .into_iter()
        .find(|win| ed.windows.get(*win).map(|window| window.bufnr) == Some(bufnr));
    match target {
        Some(win) => Ok(Some(win)),
        None => Err(VimError::new(
            "E103",
            format!("Buffer \"{arg}\" is not in diff mode"),
        )),
    }
}

/// Resolve a `:diffget {buf}` argument: a buffer number or a buffer name.
///
/// The name may be a full path, a file name or a unique substring, which covers
/// the useful part of vim's `buflist_findpat()`.
fn find_buffer(ed: &Editor, arg: &str) -> VimResult<i32> {
    if arg.chars().all(|c| c.is_ascii_digit()) {
        let bufnr: i32 = arg.parse().map_err(|_| buffer_not_found(arg))?;
        if ed.buffer(bufnr).is_ok() {
            return Ok(bufnr);
        }
        return Err(buffer_not_found(arg));
    }
    let wanted = Path::new(arg);
    let mut partial: Option<i32> = None;
    for buffer in ed.all_buffers() {
        let Some(name) = buffer.name.as_deref() else {
            continue;
        };
        if name == wanted || name.file_name() == Some(std::ffi::OsStr::new(arg)) {
            return Ok(buffer.id);
        }
        if partial.is_none() && name.to_string_lossy().contains(arg) {
            partial = Some(buffer.id);
        }
    }
    partial.ok_or_else(|| buffer_not_found(arg))
}

/// `E102: Can't find buffer "..."`.
fn buffer_not_found(arg: &str) -> VimError {
    VimError::new("E102", format!("Can't find buffer \"{arg}\""))
}

/// Fire one window event with the current buffer as the pattern.
fn fire(ed: &mut Editor, env: &mut Env, state: &mut ScriptState, event: &str) {
    let pattern = autodispatch::buffer_event_name(ed);
    autodispatch::fire(ed, env, state, event, &pattern);
}

/// Fire `DiffUpdated` for the current buffer.
fn fire_diff_updated(ed: &mut Editor, env: &mut Env, state: &mut ScriptState) {
    fire(ed, env, state, "DiffUpdated");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_editor::OptValue;

    /// Replace the whole text of the current buffer.
    fn set_text(ed: &mut Editor, lines: &[&str]) {
        let after: Vec<String> = lines.iter().map(|text| (*text).to_string()).collect();
        let total = ed.current_buffer().line_count();
        ed.replace_lines(0, total, after).unwrap();
    }

    fn lines_of(ed: &Editor, bufnr: i32) -> Vec<String> {
        ed.buffer(bufnr)
            .unwrap()
            .lines()
            .iter()
            .map(|line| line.as_str().to_string())
            .collect()
    }

    /// Run one diff command with a plain parsed argument list.
    fn run(
        ed: &mut Editor,
        env: &mut Env,
        state: &mut ScriptState,
        name: &str,
        args: &str,
    ) -> VimResult<ExOutcome> {
        cmd_diff(ed, env, state, &ParsedEx::simple(name, args), name)
    }

    /// A temporary file with `text`, named after the test.
    fn temp_file(tag: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rvim_ex_diff_{}_{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}.txt"));
        std::fs::write(&path, text).unwrap();
        path
    }

    /// Two windows on two different buffers, both in diff mode.
    ///
    /// Returns (editor, window showing `a`, window showing `b`).
    fn two_way(
        env: &mut Env,
        state: &mut ScriptState,
        a: &[&str],
        b: &[&str],
    ) -> (Editor, WindowId, WindowId) {
        let mut ed = Editor::new();
        set_text(&mut ed, a);
        let left = ed.current_win_id();
        let right = ed.split_window(false).unwrap();
        let bufnr = ed.new_buffer();
        ed.switch_buffer(bufnr).unwrap();
        set_text(&mut ed, b);
        run(&mut ed, env, state, "diffthis", "").unwrap();
        ed.goto_window(left).unwrap();
        run(&mut ed, env, state, "diffthis", "").unwrap();
        (ed, left, right)
    }

    #[test]
    fn diffthis_and_the_diff_alias_enable_diff_mode() {
        let (mut ed, mut env, mut state) = (Editor::new(), Env::new(), ScriptState::default());
        let win = ed.current_win_id();
        assert!(!ed.in_diff_mode(win));
        let out = run(&mut ed, &mut env, &mut state, "diffthis", "").unwrap();
        assert!(matches!(out, ExOutcome::Continue));
        assert!(ed.in_diff_mode(win));

        // ":diff" is the documented alias of ":diffthis".
        let mut alias = Editor::new();
        run(&mut alias, &mut env, &mut state, "diff", "").unwrap();
        assert!(alias.in_diff_mode(alias.current_win_id()));
    }

    #[test]
    fn diffsplit_opens_the_file_and_diffs_both_windows() {
        let (mut ed, mut env, mut state) = (Editor::new(), Env::new(), ScriptState::default());
        set_text(&mut ed, &["one", "two"]);
        let first = ed.current_win_id();
        let path = temp_file("split", "one\ntwo\n");
        let out = run(
            &mut ed,
            &mut env,
            &mut state,
            "diffsplit",
            &path.display().to_string(),
        )
        .unwrap();
        assert!(matches!(out, ExOutcome::Continue));
        assert_eq!(ed.window_count(), 2);
        let second = ed.current_win_id();
        assert_ne!(first, second);
        assert!(ed.in_diff_mode(first));
        assert!(ed.in_diff_mode(second));
        assert_eq!(
            ed.current_buffer().name.as_deref(),
            Some(path.as_path()),
            "the new window shows the file"
        );
        // The two windows show the same text, so nothing is changed.
        assert_eq!(ed.diff_filler(second, 0), (0, false));
        assert!(ed.diff_counterparts(second).contains(&first));
    }

    #[test]
    fn diffsplit_honours_the_vertical_modifier() {
        let (mut ed, mut env, mut state) = (Editor::new(), Env::new(), ScriptState::default());
        set_text(&mut ed, &["one"]);
        let path = temp_file("vertical", "one\n");
        ed.ex_vertical = true;
        run(
            &mut ed,
            &mut env,
            &mut state,
            "diffsplit",
            &path.display().to_string(),
        )
        .unwrap();
        ed.layout_windows(20, 80);
        let rows: Vec<usize> = ed
            .windows
            .ids()
            .into_iter()
            .filter_map(|id| ed.windows.get(id).map(|window| window.rect.row))
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], rows[1], "a vertical split keeps one top row");
        assert!(!ed.ex_vertical, "the modifier is used up");
    }

    #[test]
    fn diffoff_leaves_one_or_every_window() {
        let (mut ed, mut env, mut state) = (Editor::new(), Env::new(), ScriptState::default());
        set_text(&mut ed, &["one"]);
        let first = ed.current_win_id();
        let path = temp_file("off", "one\n");
        run(
            &mut ed,
            &mut env,
            &mut state,
            "diffsplit",
            &path.display().to_string(),
        )
        .unwrap();
        let second = ed.current_win_id();
        run(&mut ed, &mut env, &mut state, "diffoff", "").unwrap();
        assert!(!ed.in_diff_mode(second));
        assert!(ed.in_diff_mode(first), "only the current window was left");

        let mut bang = ParsedEx::simple("diffoff", "");
        bang.bang = true;
        cmd_diff(&mut ed, &mut env, &mut state, &bang, "diffoff").unwrap();
        assert!(!ed.in_diff_mode(first), ":diffoff! leaves every window");
    }

    #[test]
    fn diffupdate_fires_diffupdated() {
        let (mut ed, mut env, mut state) = (Editor::new(), Env::new(), ScriptState::default());
        set_text(&mut ed, &["one"]);
        run(&mut ed, &mut env, &mut state, "diffthis", "").unwrap();
        ed.autocmds
            .add(
                None,
                &["DiffUpdated".to_string()],
                "*",
                false,
                false,
                "let g:updated = 1",
            )
            .unwrap();
        run(&mut ed, &mut env, &mut state, "diffupdate", "").unwrap();
        assert!(env.get_var("g:updated").is_some());
    }

    #[test]
    fn diffget_copies_the_block_under_the_cursor() {
        let (mut env, mut state) = (Env::new(), ScriptState::default());
        let (mut ed, left, right) = two_way(
            &mut env,
            &mut state,
            &["1", "2", "3"],
            &["1", "x", "y", "2", "3"],
        );
        let left_buf = ed.windows.get(left).unwrap().bufnr;
        ed.goto_window(left).unwrap();
        ed.set_cursor(rvim_core::Pos::new(1, 0));
        run(&mut ed, &mut env, &mut state, "diffget", "").unwrap();
        assert_eq!(lines_of(&ed, left_buf), vec!["1", "x", "y", "2", "3"]);
        assert_eq!(ed.current_win_id(), left);
        let _ = right;
    }

    #[test]
    fn diffget_accepts_a_buffer_number_or_name() {
        let (mut env, mut state) = (Env::new(), ScriptState::default());
        let (mut ed, left, right) = two_way(
            &mut env,
            &mut state,
            &["1", "2", "3"],
            &["1", "x", "2", "3"],
        );
        let left_buf = ed.windows.get(left).unwrap().bufnr;
        let right_buf = ed.windows.get(right).unwrap().bufnr;
        ed.goto_window(left).unwrap();
        ed.set_cursor(rvim_core::Pos::new(1, 0));
        run(
            &mut ed,
            &mut env,
            &mut state,
            "diffget",
            &right_buf.to_string(),
        )
        .unwrap();
        assert_eq!(lines_of(&ed, left_buf), vec!["1", "x", "2", "3"]);
        // Naming the current buffer is a silent no-op, like in vim.
        run(
            &mut ed,
            &mut env,
            &mut state,
            "diffget",
            &left_buf.to_string(),
        )
        .unwrap();
    }

    #[test]
    fn diffget_reports_missing_and_undiffed_buffers() {
        let (mut env, mut state) = (Env::new(), ScriptState::default());
        let (mut ed, left, _right) = two_way(
            &mut env,
            &mut state,
            &["1", "2", "3"],
            &["1", "x", "2", "3"],
        );
        ed.goto_window(left).unwrap();
        let err = run(&mut ed, &mut env, &mut state, "diffget", "99").unwrap_err();
        assert_eq!(err.code, "E102", "{err}");
        // A buffer that exists but is not diffing.
        let plain = ed.new_buffer();
        let path = temp_file("plain", "1\n");
        ed.buffer_mut(plain).unwrap().name = Some(path.clone());
        let err = run(
            &mut ed,
            &mut env,
            &mut state,
            "diffget",
            &path.display().to_string(),
        )
        .unwrap_err();
        assert_eq!(err.code, "E103", "{err}");
        // Not in diff mode at all.
        let mut plain_ed = Editor::new();
        let win = plain_ed.current_win_id();
        let err = run(&mut plain_ed, &mut env, &mut state, "diffget", "").unwrap_err();
        assert_eq!(err.code, "E99", "{err}");
        let _ = win;
    }

    #[test]
    fn diffput_writes_the_counterpart_without_moving_the_cursor() {
        let (mut env, mut state) = (Env::new(), ScriptState::default());
        let (mut ed, left, right) = two_way(
            &mut env,
            &mut state,
            &["1", "2", "3"],
            &["1", "x", "2", "3"],
        );
        let left_buf = ed.windows.get(left).unwrap().bufnr;
        let right_buf = ed.windows.get(right).unwrap().bufnr;
        ed.goto_window(right).unwrap();
        ed.set_cursor(rvim_core::Pos::new(1, 0));
        run(&mut ed, &mut env, &mut state, "diffput", "").unwrap();
        assert_eq!(lines_of(&ed, left_buf), vec!["1", "x", "2", "3"]);
        assert_eq!(lines_of(&ed, right_buf), vec!["1", "x", "2", "3"]);
        assert_eq!(ed.current_win_id(), right);
        assert_eq!(ed.cursor().line, 1);
    }

    #[test]
    fn diffput_without_a_counterpart_is_e100() {
        let (mut ed, mut env, mut state) = (Editor::new(), Env::new(), ScriptState::default());
        run(&mut ed, &mut env, &mut state, "diffthis", "").unwrap();
        let err = run(&mut ed, &mut env, &mut state, "diffput", "").unwrap_err();
        assert_eq!(err.code, "E100", "{err}");
    }

    #[test]
    fn unknown_names_are_ignored() {
        let (mut ed, mut env, mut state) = (Editor::new(), Env::new(), ScriptState::default());
        let out = run(&mut ed, &mut env, &mut state, "diffbogus", "patch.diff").unwrap();
        assert!(matches!(out, ExOutcome::Continue));
        assert!(!ed.in_diff_mode(ed.current_win_id()));
    }

    #[test]
    fn diff_colours_follow_the_window_not_the_global_option() {
        // A plain ":setglobal diff" must not turn every window into a diff window.
        let (mut ed, mut env, mut state) = (Editor::new(), Env::new(), ScriptState::default());
        ed.set_option("diff", OptValue::Bool(true), rvim_editor::SetScope::Global)
            .unwrap();
        assert!(!ed.in_diff_mode(ed.current_win_id()));
        let err = run(&mut ed, &mut env, &mut state, "diffget", "").unwrap_err();
        assert_eq!(err.code, "E99");
    }
}
