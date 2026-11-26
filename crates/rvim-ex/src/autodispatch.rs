//! Autocommand event dispatch (reference: autocmd.c `do_autocmd()`).
//!
//! `rvim-editor::autocmd` stores the declarations; this module runs the ones
//! that match at the points where vim fires events: reading and writing a file,
//! entering a buffer, `:setf` (→ `FileType`), and so on.
//!
//! This is what makes the runtime scripts meaningful: `filetype.vim` is nothing
//! but a table of `au BufRead,BufNewFile *.c setf c` style autocommands, so
//! without dispatch a sourced `filetype.vim` detects no filetype at all.

use std::path::Path;

use rvim_editor::Editor;
use rvim_viml::Env;

use crate::script::ScriptState;

/// Guard against an autocommand that keeps re-triggering its own event through
/// `++nested` (vim relies on the user not doing this; we fail safe instead).
const MAX_AUTOCMD_DEPTH: usize = 20;

/// Run every autocommand matching `event` for `pattern`.
///
/// `pattern` is a file name for buffer events and the filetype name for
/// `FileType`, mirroring how vim passes the "file name" of the event.
///
/// Errors raised by an autocommand are reported as `emsg()` but never abort the
/// command that fired the event: vim keeps going after a failing autocommand.
pub fn fire(ed: &mut Editor, env: &mut Env, state: &mut ScriptState, event: &str, pattern: &str) {
    fire_impl(ed, env, state, event, pattern, false);
}

/// Fire `event` with vim's `force` flag, which ignores the "busy" suppression.
///
/// `FileType` is applied this way when the 'filetype' option changes
/// (optionstr.c:2551), which is why `au BufRead *.c setf c` can trigger
/// `FileType` from inside the `BufRead` autocommand.
pub fn fire_forced(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    event: &str,
    pattern: &str,
) {
    fire_impl(ed, env, state, event, pattern, true);
}

fn fire_impl(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    event: &str,
    pattern: &str,
    force: bool,
) {
    // Autocommands are not triggered from inside another autocommand unless it
    // was declared `++nested` or the event is applied with "force"
    // (autocmd.c:1948).
    if ed.autocmd_busy && !(force || ed.autocmd_nested) {
        return;
    }
    if ed.autocmd_depth >= MAX_AUTOCMD_DEPTH {
        return;
    }
    if rvim_editor::autocmd::canonical_event(event).is_none() {
        return;
    }
    // Collect first: running a command may redefine autocommands.
    let mut jobs: Vec<(String, bool)> = Vec::new();
    let mut fire_once: Vec<usize> = Vec::new();
    for (idx, entry) in ed.autocmds.entries().iter().enumerate() {
        let event_hit = entry
            .events
            .iter()
            .any(|e| e == "*" || e.eq_ignore_ascii_case(event));
        if !event_hit || !rvim_editor::autocmd::pattern_matches(&entry.pattern, pattern) {
            continue;
        }
        jobs.push((entry.command.clone(), entry.nested));
        if entry.once {
            fire_once.push(idx);
        }
    }
    if jobs.is_empty() {
        return;
    }
    // `++once` autocommands are removed as soon as they fire.
    ed.autocmds.remove_indices(&fire_once);

    for (command, nested) in jobs {
        let outer_match = std::mem::replace(&mut state.amatch, pattern.to_string());
        let outer_busy = ed.autocmd_busy;
        let outer_nested = ed.autocmd_nested;
        ed.autocmd_busy = true;
        ed.autocmd_nested = nested;
        ed.autocmd_depth += 1;
        if let Err(e) = crate::exec::run_line_with(ed, env, state, &command) {
            // Report and continue: a broken autocommand must not lose the write
            // or the file that triggered it.
            let text = e.to_string();
            ed.emsg(text);
        }
        ed.autocmd_depth -= 1;
        ed.autocmd_busy = outer_busy;
        ed.autocmd_nested = outer_nested;
        state.amatch = outer_match;
    }
}

/// Fire an event whose pattern is the current buffer's name.
pub fn fire_for_buffer(ed: &mut Editor, env: &mut Env, state: &mut ScriptState, event: &str) {
    let name = buffer_event_name(ed);
    fire(ed, env, state, event, &name);
}

/// The name vim matches autocommand patterns against: the path as typed/known
/// to the buffer, empty for a nameless scratch buffer.
pub fn buffer_event_name(ed: &Editor) -> String {
    ed.current_buffer()
        .name
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// Open `path` the way `:edit` does, firing the read-side event chain.
///
/// vim: `BufLeave` for the buffer being left, `BufReadPre`, the read itself,
/// then `BufRead` (existing file) or `BufNewFile` (new file), then `BufEnter`.
/// `FileType` follows from `setf` inside the runtime autocommands.
pub fn edit_file(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    path: &Path,
) -> rvim_core::VimResult<()> {
    let existed = path.exists();
    let target = path.display().to_string();
    if !buffer_event_name(ed).is_empty() {
        fire_for_buffer(ed, env, state, "BufLeave");
    }
    fire(ed, env, state, "BufReadPre", &target);
    ed.open_file(path)?;
    fire(
        ed,
        env,
        state,
        if existed { "BufRead" } else { "BufNewFile" },
        &target,
    );
    fire(ed, env, state, "BufEnter", &target);
    Ok(())
}

/// Write the current buffer the way `:write` does, firing the write-side chain.
pub fn write_file(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    path: Option<&Path>,
    bang: bool,
) -> rvim_core::VimResult<()> {
    fire_for_buffer(ed, env, state, "BufWritePre");
    ed.save_current(path, bang)?;
    fire_for_buffer(ed, env, state, "BufWritePost");
    Ok(())
}

/// Switch to another buffer, firing `BufLeave`/`BufEnter` like `:buffer` does.
pub fn switch_buffer(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    bufnr: i32,
) -> rvim_core::VimResult<()> {
    fire_for_buffer(ed, env, state, "BufLeave");
    ed.switch_buffer(bufnr)?;
    fire_for_buffer(ed, env, state, "BufEnter");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_editor::Editor;

    fn ctx() -> (Editor, Env, ScriptState) {
        (Editor::new(), Env::new(), ScriptState::default())
    }

    #[test]
    fn fires_only_matching_event_and_pattern() {
        let (mut ed, mut env, mut state) = ctx();
        ed.messages.clear();
        ed.autocmds
            .add(
                None,
                &["BufRead".to_string()],
                "*.txt",
                false,
                false,
                "let g:hit = 1",
            )
            .unwrap();
        fire(&mut ed, &mut env, &mut state, "BufRead", "notes.txt");
        assert!(env.get_var("g:hit").is_some(), "g:hit was set");

        // Wrong event / wrong pattern do nothing.
        let mut env2 = Env::new();
        let mut ed2 = Editor::new();
        ed2.autocmds
            .add(
                None,
                &["BufRead".to_string()],
                "*.txt",
                false,
                false,
                "let g:hit = 1",
            )
            .unwrap();
        fire(&mut ed2, &mut env2, &mut state, "BufWritePre", "notes.txt");
        fire(&mut ed2, &mut env2, &mut state, "BufRead", "notes.md");
        assert_eq!(env2.get_var("g:hit"), None);
    }

    #[test]
    fn once_autocommand_is_removed_after_firing() {
        let (mut ed, mut env, mut state) = ctx();
        ed.autocmds
            .add(
                None,
                &["BufEnter".to_string()],
                "*",
                false,
                true,
                "let g:n = 1",
            )
            .unwrap();
        fire(&mut ed, &mut env, &mut state, "BufEnter", "x");
        assert_eq!(ed.autocmds.entries().len(), 0, "++once is consumed");
    }

    #[test]
    fn non_nested_autocommand_cannot_trigger_more_events() {
        let (mut ed, mut env, mut state) = ctx();
        // Simulate being inside a non-nested autocommand: further events are
        // suppressed instead of recursing (autocmd.c autocmd_busy).
        ed.autocmds
            .add(
                None,
                &["BufEnter".to_string()],
                "a",
                false,
                false,
                "let g:inner = 1",
            )
            .unwrap();
        ed.autocmd_busy = true;
        ed.autocmd_nested = false;
        fire(&mut ed, &mut env, &mut state, "BufEnter", "a");
        assert_eq!(env.get_var("g:inner"), None);

        // With ++nested on the running autocommand the event does fire.
        ed.autocmd_nested = true;
        fire(&mut ed, &mut env, &mut state, "BufEnter", "a");
        assert!(env.get_var("g:inner").is_some());
    }
}
