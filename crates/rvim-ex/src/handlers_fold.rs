//! Ex commands for folding (reference: ex_docmd.c ex_fold, ex_foldopen,
//! ex_folddo; fold.c foldCreate, opFoldRange, setManualFold, hasFolding).
//!
//! Supported commands:
//!
//! * :[range]fold - create a manual fold over the range (foldCreate). Only
//!   allowed for 'foldmethod' manual or marker: otherwise E350.
//! * :[range]foldopen[!] / :[range]foldclose[!] - open or close folds in the
//!   range. Without "!" one level is changed at a time, with "!" every fold
//!   under each line is changed recursively.
//! * :[range]folddoopen {cmd} / :[range]folddoclosed {cmd} - run {cmd} on the
//!   lines that are not (folddoopen) or are (folddoclosed) inside a closed
//!   fold. Without a range the whole buffer is used, like Vim's :global.
//!
//! # Deviations from Vim
//!
//! * :folddoopen/:folddoclosed collect the target lines first, then run the
//!   command from the bottom line up so that edits made by the command cannot
//!   shift the pending line numbers. Vim's :global marks adjust instead and
//!   therefore runs top-down; only commands whose execution order is visible
//!   (":m", ":g/.../d") can tell the difference.
//! * A command run by :folddoopen that deletes lines below its cursor keeps the
//!   already collected (now stale) target numbers; targets past the end of the
//!   buffer are skipped.
//! * :fold with arguments is E474; Vim silently ignores trailing arguments for
//!   :fold. The task's Ex contract asks for the stricter behaviour.
//! * :foldopen/:foldclose outside manual/marker methods act on whatever fold
//!   list exists instead of refusing like foldManualAllowed() does.

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::{Editor, OptValue};
use rvim_viml::Env;

use crate::exec::{self, ExOutcome};
use crate::parser::ParsedEx;
use crate::script::ScriptState;

/// Invalid argument helper (E474, option.c/do_one_cmd convention).
fn invalid_argument(text: &str) -> VimError {
    VimError::new("E474", format!("Invalid argument: {text}"))
}

/// True when manual folds may be created or deleted ('foldmethod' manual or
/// marker), mirroring fold.c foldManualAllowed().
fn manual_folds_allowed(ed: &Editor) -> bool {
    matches!(
        ed.wopt_str("foldmethod", "manual").as_str(),
        "manual" | "marker"
    )
}

/// Create a manual fold and make sure it is visible ("zf" also sets this).
fn create_manual_fold(ed: &mut Editor, start: usize, end: usize) -> VimResult<()> {
    if !manual_folds_allowed(ed) {
        return Err(VimError::new(
            "E350",
            "Cannot create fold with current 'foldmethod'",
        ));
    }
    if ed.line_count() == 0 {
        return Ok(());
    }
    let last = ed.line_count() - 1;
    let (start, end) = if start <= end {
        (start.min(last), end.min(last))
    } else {
        (end.min(last), start.min(last))
    };
    ed.add_fold(start, end)?;
    ed.windows
        .current_mut()
        .set_local("foldenable", OptValue::Bool(true));
    Ok(())
}

/// Open or close folds over the resolved half-open range [start, end).
fn open_close_range(ed: &mut Editor, start: usize, end: usize, opening: bool, recurse: bool) {
    let mut did = false;
    let mut line = start;
    while line < end {
        let mut skip_to = line;
        if opening && !recurse {
            // Opening one level only: continue after the fold that gets opened.
            if let Some((_, fold_end)) = ed.closed_fold_at(line) {
                skip_to = fold_end;
            }
        }
        if recurse {
            if opening {
                while ed.open_fold_at(line) {
                    did = true;
                }
            } else {
                did |= ed.close_folds_recursive(line);
            }
        } else if opening {
            did |= ed.open_fold_at(line);
        } else {
            did |= ed.close_fold_at(line);
        }
        if !opening && !recurse {
            // Closing one level only: continue after the fold just closed.
            if let Some((_, fold_end)) = ed.closed_fold_at(line) {
                skip_to = skip_to.max(fold_end);
            }
        }
        line = skip_to.saturating_add(1);
    }
    if !did {
        ed.emsg("E490: No fold found");
    }
}

/// Implement :folddoopen / :folddoclosed.
fn fold_do(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    want_closed: bool,
) -> VimResult<ExOutcome> {
    let command = p.args.trim();
    if command.is_empty() {
        return Err(VimError::new("E471", "Argument required"));
    }
    let (start, end) = exec::resolve_range_public(ed, &p.range, true)?;
    let mut targets: Vec<usize> = Vec::new();
    for line in start..end {
        let closed = ed.closed_fold_at(line).is_some();
        if closed == want_closed {
            targets.push(line);
        }
    }
    // Bottom-up: edits made by the command cannot shift the pending targets.
    for line in targets.into_iter().rev() {
        if line >= ed.line_count() {
            continue;
        }
        ed.set_cursor(Pos::new(line, 0));
        exec::run_line_with(ed, env, state, command)?;
    }
    Ok(ExOutcome::Continue)
}

/// The Ex entry point for :fold, :foldopen, :foldclose, :folddoopen and
/// :folddoclosed.
///
/// The name argument is the canonical command-table name, so the callers can
/// share one dispatch arm. Ranges are resolved through the normal Ex
/// machinery, which also gives the standard backwards-range and out-of-range
/// errors.
pub fn cmd_fold(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    match name {
        "fold" => {
            if !p.args.trim().is_empty() {
                return Err(invalid_argument(p.args.trim()));
            }
            let (start, end) = exec::resolve_range_public(ed, &p.range, false)?;
            if end > start {
                create_manual_fold(ed, start, end - 1)?;
            }
            Ok(ExOutcome::Continue)
        }
        "foldopen" | "foldclose" => {
            let (start, end) = exec::resolve_range_public(ed, &p.range, false)?;
            open_close_range(ed, start, end, name == "foldopen", p.bang);
            Ok(ExOutcome::Continue)
        }
        "folddoopen" => fold_do(ed, env, state, p, false),
        "folddoclosed" => fold_do(ed, env, state, p, true),
        other => Err(invalid_argument(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Range;
    use rvim_editor::SetScope;

    fn state() -> (Editor, Env, ScriptState) {
        (Editor::new(), Env::new(), ScriptState::default())
    }

    fn editor_with(lines: &[&str]) -> Editor {
        let mut ed = Editor::new();
        let lines: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
        ed.replace_lines(0, 1, lines).unwrap();
        ed
    }

    fn state_with(lines: &[&str]) -> (Editor, Env, ScriptState) {
        (editor_with(lines), Env::new(), ScriptState::default())
    }

    fn ranged(command: &str, start: usize, end: usize) -> ParsedEx {
        let mut parsed = ParsedEx::simple(command, "");
        parsed.range = Range::Lines(start, end);
        parsed.addr_count = 2;
        parsed
    }

    fn run(
        ed: &mut Editor,
        env: &mut Env,
        state: &mut ScriptState,
        p: &ParsedEx,
        name: &str,
    ) -> VimResult<ExOutcome> {
        cmd_fold(ed, env, state, p, name)
    }

    #[test]
    fn fold_creates_a_manual_fold_for_the_range() {
        let (mut ed, mut env, mut st) = state_with(&["a", "b", "c", "d", "e"]);
        let p = ranged("fold", 1, 4);
        assert!(matches!(
            run(&mut ed, &mut env, &mut st, &p, "fold").unwrap(),
            ExOutcome::Continue
        ));
        assert_eq!(ed.folds().len(), 1);
        assert_eq!((ed.folds()[0].start, ed.folds()[0].end), (1, 3));
        assert!(ed.folds()[0].closed);
        assert!(ed.wopt_bool("foldenable", false));

        // No range: the current line.
        ed.set_cursor(Pos::new(0, 0));
        run(
            &mut ed,
            &mut env,
            &mut st,
            &ParsedEx::simple("fold", ""),
            "fold",
        )
        .unwrap();
        assert_eq!(ed.folds().len(), 2);
    }

    #[test]
    fn fold_rejects_arguments_and_non_manual_methods() {
        let (mut ed, mut env, mut st) = state_with(&["a", "b"]);
        let p = ParsedEx::simple("fold", "bogus");
        let err = run(&mut ed, &mut env, &mut st, &p, "fold").unwrap_err();
        assert_eq!(err.code, "E474");
        ed.set_option(
            "foldmethod",
            OptValue::Str("indent".to_string()),
            SetScope::Local,
        )
        .unwrap();
        let err = run(&mut ed, &mut env, &mut st, &ranged("fold", 0, 2), "fold").unwrap_err();
        assert_eq!(err.code, "E350");
    }

    #[test]
    fn foldopen_and_foldclose_open_one_level_or_all() {
        let (mut ed, mut env, mut st) = state_with(&["a", "b", "c", "d"]);
        ed.add_fold(0, 3).unwrap();
        ed.add_fold(1, 2).unwrap();
        assert_eq!(ed.closed_fold_at(1), Some((0, 3)));
        run(
            &mut ed,
            &mut env,
            &mut st,
            &ranged("foldopen", 0, 4),
            "foldopen",
        )
        .unwrap();
        assert_eq!(
            ed.closed_fold_at(1),
            Some((1, 2)),
            "without ! only one level opens"
        );
        run(
            &mut ed,
            &mut env,
            &mut st,
            &ranged("foldopen", 0, 4),
            "foldopen",
        )
        .unwrap();
        assert_eq!(ed.closed_fold_at(1), None);
        // foldopen! opens the whole stack at once.
        ed.close_all_folds();
        let mut bang = ranged("foldopen", 0, 4);
        bang.bang = true;
        run(&mut ed, &mut env, &mut st, &bang, "foldopen").unwrap();
        assert_eq!(ed.closed_fold_at(1), None);

        // foldclose! closes everything under the range.
        let mut bang = ranged("foldclose", 0, 4);
        bang.bang = true;
        run(&mut ed, &mut env, &mut st, &bang, "foldclose").unwrap();
        assert!(ed.folds().iter().all(|fold| fold.closed));
        // foldclose without ! closes a single level.
        ed.open_all_folds();
        run(
            &mut ed,
            &mut env,
            &mut st,
            &ranged("foldclose", 0, 4),
            "foldclose",
        )
        .unwrap();
        assert!(ed.closed_fold_at(1).is_some());
        assert!(!ed.folds().iter().all(|fold| fold.closed));
    }

    #[test]
    fn foldopen_without_folds_reports_e490() {
        let (mut ed, mut env, mut st) = state();
        run(
            &mut ed,
            &mut env,
            &mut st,
            &ParsedEx::simple("foldopen", ""),
            "foldopen",
        )
        .unwrap();
        assert_eq!(ed.error_count, 1);
    }

    #[test]
    fn folddoopen_runs_the_command_on_visible_lines() {
        let (mut ed, mut env, mut st) = state_with(&["aaa", "bbb", "ccc"]);
        ed.add_fold(0, 1).unwrap();
        let p = ParsedEx::simple("folddoopen", "s/^/X/");
        run(&mut ed, &mut env, &mut st, &p, "folddoopen").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "aaa\nbbb\nXccc");
    }

    #[test]
    fn folddoclosed_runs_the_command_on_hidden_lines() {
        let (mut ed, mut env, mut st) = state_with(&["aaa", "bbb", "ccc"]);
        ed.add_fold(0, 1).unwrap();
        let p = ParsedEx::simple("folddoclosed", "s/^/X/");
        run(&mut ed, &mut env, &mut st, &p, "folddoclosed").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "Xaaa\nXbbb\nccc");
    }

    #[test]
    fn folddo_requires_a_command_and_honours_a_range() {
        let (mut ed, mut env, mut st) = state_with(&["aaa", "bbb", "ccc", "ddd"]);
        let err = run(
            &mut ed,
            &mut env,
            &mut st,
            &ParsedEx::simple("folddoopen", ""),
            "folddoopen",
        )
        .unwrap_err();
        assert_eq!(err.code, "E471");
        // Restrict the range to the last two lines.
        ed.add_fold(0, 0).unwrap();
        let mut p = ranged("folddoopen", 2, 4);
        p.args = "s/^/X/".to_string();
        run(&mut ed, &mut env, &mut st, &p, "folddoopen").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "aaa\nbbb\nXccc\nXddd");
    }

    #[test]
    fn fold_doopen_handles_line_deletion_bottom_up() {
        let (mut ed, mut env, mut st) = state_with(&["aaa", "bbb", "ccc", "ddd", "eee"]);
        // Lines 0 and 1 are folded; the command deletes the visible lines.
        ed.add_fold(0, 1).unwrap();
        let p = ParsedEx::simple("folddoopen", "d");
        run(&mut ed, &mut env, &mut st, &p, "folddoopen").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "aaa\nbbb");
    }

    #[test]
    fn unknown_command_names_are_e474() {
        let (mut ed, mut env, mut st) = state();
        let err = run(
            &mut ed,
            &mut env,
            &mut st,
            &ParsedEx::simple("fold", ""),
            "foldbogus",
        )
        .unwrap_err();
        assert_eq!(err.code, "E474");
    }
}
