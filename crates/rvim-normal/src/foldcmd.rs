//! Normal-mode folding commands: the key that follows "z" (reference:
//! normal.c nv_zo/nv_zc/nv_za/nv_zj/nv_zk/nv_zf, fold.c foldCreate,
//! foldMoveTo, setManualFold, clearFolding).
//!
//! The normal-mode engine owns the key sequence, so [feed_z] receives only the
//! second key plus the count. "zf" needs a motion, which this API cannot see:
//! the caller resolves it and calls [create_fold] directly (Visual mode does
//! the same). [feed_z] handles "f"/"F" as "zF", i.e. a fold for [count] lines
//! starting at the cursor.
//!
//! # Deviations from Vim
//!
//! * No error is raised when "zo"/"zc"/"za" find nothing to do; only "zd",
//!   "zE" and "zf" report E490/E351/E352/E350 like Vim.
//! * "zf" with 'foldmethod' other than manual/marker fails with E350, and
//!   "zd"/"zE" fail with E351/E352, matching fold.c foldManualAllowed().
//! * "zE" never edits the buffer, so marker folds are only forgotten for the
//!   current window instead of deleting their {{{/}}} markers.

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::{Editor, OptValue};

/// Run one fold operation repeatedly, stopping at the first no-op.
fn repeat(mut action: impl FnMut() -> bool, count: u64) -> bool {
    let mut any = false;
    for _ in 0..count.max(1) {
        if !action() {
            break;
        }
        any = true;
    }
    any
}

/// Open enough folds to make the cursor line visible ("zv", foldOpenCursor()).
fn view_cursor(ed: &mut Editor) {
    let line = ed.cursor().line;
    while ed.open_fold_at(line) {}
}

/// Set a window-local folding option without touching the global value.
fn set_local(ed: &mut Editor, name: &str, value: OptValue) {
    ed.windows.current_mut().set_local(name, value);
}

fn set_foldenable(ed: &mut Editor, enabled: bool) {
    set_local(ed, "foldenable", OptValue::Bool(enabled));
}

/// True when folds may be created or deleted ('foldmethod' manual or marker).
fn manual_folds_allowed(ed: &Editor) -> bool {
    matches!(
        ed.wopt_str("foldmethod", "manual").as_str(),
        "manual" | "marker"
    )
}

/// Handle the key that follows 'z' in normal mode.
///
/// Ok(true) = handled, Ok(false) = not a fold command (the caller ignores it).
pub fn feed_z(ed: &mut Editor, key: char, count: u64) -> VimResult<bool> {
    let count = count.max(1);
    let line = ed.cursor().line;
    match key {
        'o' => {
            repeat(|| ed.open_fold_at(line), count);
        }
        'O' => while ed.open_fold_at(line) {},
        'c' => {
            repeat(|| ed.close_fold_at(line), count);
        }
        'C' => {
            ed.close_folds_recursive(line);
        }
        'a' => {
            repeat(|| ed.toggle_fold_at(line), count);
        }
        'R' => ed.open_all_folds(),
        'M' => ed.close_all_folds(),
        'v' => view_cursor(ed),
        'd' => {
            if !manual_folds_allowed(ed) {
                return Err(VimError::new(
                    "E351",
                    "Cannot delete fold with current 'foldmethod'",
                ));
            }
            if !ed.delete_fold_at(line) {
                ed.emsg("E490: No fold found");
            }
        }
        'E' => {
            if !manual_folds_allowed(ed) {
                return Err(VimError::new(
                    "E352",
                    "Cannot erase folds with current 'foldmethod'",
                ));
            }
            ed.delete_all_folds();
        }
        'j' | 'k' => return fold_motion(ed, key, count),
        'f' | 'F' => {
            let start = line;
            let end = line.saturating_add(count as usize - 1);
            create_fold(ed, start, end)?;
        }
        'n' => set_foldenable(ed, false),
        'N' => set_foldenable(ed, true),
        'i' => {
            let enabled = ed.wopt_bool("foldenable", true);
            set_foldenable(ed, !enabled);
        }
        'r' | 'm' => {
            let current = ed.wopt_num("foldlevel", 0).max(0);
            let delta = i64::try_from(count).unwrap_or(i64::MAX);
            let next = if key == 'r' {
                current.saturating_add(delta)
            } else {
                current.saturating_sub(delta).max(0)
            };
            set_local(ed, "foldlevel", OptValue::Num(next));
            ed.apply_foldlevel();
        }
        'x' => {
            ed.recompute_folds()?;
            ed.apply_foldlevel();
            view_cursor(ed);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// 'zf' with an already resolved range (used by visual mode and by 'zF').
///
/// The range is inclusive and zero-based; a backwards range is swapped and an
/// end past the buffer is clamped. Fails with E350 when 'foldmethod' is not
/// manual or marker, and switches 'foldenable' on like Vim's "zf".
pub fn create_fold(ed: &mut Editor, start: usize, end: usize) -> VimResult<()> {
    if !manual_folds_allowed(ed) {
        return Err(VimError::new(
            "E350",
            "Cannot create fold with current 'foldmethod'",
        ));
    }
    let total = ed.line_count();
    if total == 0 {
        return Ok(());
    }
    let (start, end) = if start <= end {
        (start.min(total - 1), end.min(total - 1))
    } else {
        (end.min(total - 1), start.min(total - 1))
    };
    ed.add_fold(start, end)?;
    set_foldenable(ed, true);
    Ok(())
}

/// 'zj'/'zk' as motions; move the cursor and return Ok(true) when handled.
///
/// "zj" moves to the start of the next fold and "zk" to the end (or, when
/// inside a fold, the start) of the previous one; a closed fold counts as one
/// fold. The cursor column is reset to 0 like Vim's foldMoveTo().
pub fn fold_motion(ed: &mut Editor, key: char, count: u64) -> VimResult<bool> {
    let mut line = ed.cursor().line;
    for _ in 0..count.max(1) {
        let next = match key {
            'j' => ed.next_fold(line).filter(|next| *next > line),
            'k' => ed.prev_fold(line).filter(|next| *next < line),
            _ => return Ok(false),
        };
        match next {
            Some(next) => line = next,
            None => break,
        }
    }
    if line != ed.cursor().line {
        ed.set_cursor(Pos::new(line, 0));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_editor::SetScope;

    fn editor_with(lines: &[&str]) -> Editor {
        let mut ed = Editor::new();
        let lines: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
        ed.replace_lines(0, 1, lines).unwrap();
        ed
    }

    fn set_str(ed: &mut Editor, name: &str, value: &str) {
        ed.set_option(name, OptValue::Str(value.to_string()), SetScope::Local)
            .unwrap();
    }

    #[test]
    fn zf_and_zf_with_count_create_manual_folds() {
        let mut ed = editor_with(&["a", "b", "c", "d", "e"]);
        ed.set_cursor(Pos::new(1, 0));
        assert!(feed_z(&mut ed, 'f', 3).unwrap());
        assert_eq!(ed.folds().len(), 1);
        assert_eq!((ed.folds()[0].start, ed.folds()[0].end), (1, 3));
        assert!(ed.folds()[0].closed);
        assert!(ed.wopt_bool("foldenable", false));

        // zF is the same "fold [count] lines" command.
        ed.set_cursor(Pos::new(0, 0));
        assert!(feed_z(&mut ed, 'F', 1).unwrap());
        assert_eq!(ed.folds().len(), 2);
        assert!(
            ed.folds()
                .iter()
                .any(|fold| (fold.start, fold.end) == (0, 0))
        );
    }

    #[test]
    fn create_fold_swaps_and_clamps_ranges() {
        let mut ed = editor_with(&["a", "b", "c"]);
        create_fold(&mut ed, 2, 1).unwrap();
        assert_eq!(ed.folds(), &[rvim_editor::fold::Fold::new(1, 2, 1, true)]);
        create_fold(&mut ed, 0, 99).unwrap();
        assert_eq!(ed.folds().len(), 2);
        assert_eq!((ed.folds()[0].start, ed.folds()[0].end), (0, 2));
    }

    #[test]
    fn open_close_toggle_and_recursive_commands() {
        let mut ed = editor_with(&["a", "b", "c", "d", "e"]);
        create_fold(&mut ed, 0, 3).unwrap();
        create_fold(&mut ed, 1, 2).unwrap();
        ed.set_cursor(Pos::new(2, 0));
        assert_eq!(ed.folds().len(), 2);
        assert_eq!(ed.closed_fold_at(2), Some((0, 3)));

        // zo opens one level (the outermost closed fold).
        assert!(feed_z(&mut ed, 'o', 1).unwrap());
        assert_eq!(ed.closed_fold_at(2), Some((1, 2)));
        // zO opens every fold under the cursor.
        assert!(feed_z(&mut ed, 'O', 1).unwrap());
        assert_eq!(ed.closed_fold_at(2), None);

        // zc closes one level, zC closes recursively.
        assert!(feed_z(&mut ed, 'c', 1).unwrap());
        assert!(ed.folds().iter().any(|fold| fold.closed));
        assert!(feed_z(&mut ed, 'C', 1).unwrap());
        assert!(ed.folds().iter().all(|fold| fold.closed));

        // za toggles the outermost closed fold.
        assert!(feed_z(&mut ed, 'a', 1).unwrap());
        assert_eq!(ed.closed_fold_at(2), Some((1, 2)));
        assert!(feed_z(&mut ed, 'a', 1).unwrap());
        assert_eq!(ed.closed_fold_at(2), None);
    }

    #[test]
    fn open_all_close_all_and_delete() {
        let mut ed = editor_with(&["a", "b", "c", "d"]);
        create_fold(&mut ed, 0, 1).unwrap();
        create_fold(&mut ed, 2, 3).unwrap();
        assert!(feed_z(&mut ed, 'R', 1).unwrap());
        assert!(ed.folds().iter().all(|fold| !fold.closed));
        assert!(feed_z(&mut ed, 'M', 1).unwrap());
        assert!(ed.folds().iter().all(|fold| fold.closed));

        ed.set_cursor(Pos::new(0, 0));
        assert!(feed_z(&mut ed, 'd', 1).unwrap());
        assert_eq!(ed.folds().len(), 1);
        assert!(feed_z(&mut ed, 'E', 1).unwrap());
        assert!(ed.folds().is_empty());
        // Deleting again reports E490 but is still a handled key.
        assert!(feed_z(&mut ed, 'd', 1).unwrap());
        assert_eq!(ed.error_count, 1);
    }

    #[test]
    fn zj_and_zk_move_between_folds() {
        let mut ed = editor_with(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        create_fold(&mut ed, 1, 2).unwrap();
        create_fold(&mut ed, 4, 6).unwrap();
        ed.set_cursor(Pos::new(0, 0));
        assert!(feed_z(&mut ed, 'j', 1).unwrap());
        assert_eq!(ed.cursor().line, 1);
        assert!(feed_z(&mut ed, 'j', 1).unwrap());
        assert_eq!(ed.cursor().line, 4);
        assert!(feed_z(&mut ed, 'j', 1).unwrap());
        assert_eq!(ed.cursor().line, 4, "no next fold: the cursor stays");

        // On a fold's first line Vim's foldMoveTo() has nowhere to go.
        assert!(feed_z(&mut ed, 'k', 1).unwrap());
        assert_eq!(ed.cursor().line, 4);
        ed.set_cursor(Pos::new(5, 0));
        assert!(fold_motion(&mut ed, 'k', 1).unwrap());
        assert_eq!(ed.cursor().line, 4, "start of the containing fold");
        ed.set_cursor(Pos::new(7, 0));
        assert!(fold_motion(&mut ed, 'k', 1).unwrap());
        assert_eq!(ed.cursor().line, 6, "end of the previous fold");
        assert!(!fold_motion(&mut ed, 'x', 1).unwrap());
    }

    #[test]
    fn zv_and_zx_make_the_cursor_visible() {
        let mut ed = editor_with(&["a", "b", "c", "d"]);
        create_fold(&mut ed, 0, 3).unwrap();
        create_fold(&mut ed, 1, 2).unwrap();
        ed.set_cursor(Pos::new(2, 0));
        assert!(feed_z(&mut ed, 'v', 1).unwrap());
        assert_eq!(ed.closed_fold_at(2), None);
        assert!(feed_z(&mut ed, 'M', 1).unwrap());
        assert!(feed_z(&mut ed, 'x', 1).unwrap());
        assert_eq!(ed.closed_fold_at(2), None);
    }

    #[test]
    fn foldenable_commands() {
        let mut ed = editor_with(&["a", "b"]);
        assert!(feed_z(&mut ed, 'n', 1).unwrap());
        assert!(!ed.wopt_bool("foldenable", true));
        assert!(feed_z(&mut ed, 'N', 1).unwrap());
        assert!(ed.wopt_bool("foldenable", false));
        assert!(feed_z(&mut ed, 'i', 1).unwrap());
        assert!(!ed.wopt_bool("foldenable", true));
    }

    #[test]
    fn foldlevel_commands_and_unknown_keys() {
        let mut ed = editor_with(&["a", "b", "c", "d"]);
        ed.add_fold_at_level(0, 3, 1).unwrap();
        ed.add_fold_at_level(1, 2, 2).unwrap();
        assert!(feed_z(&mut ed, 'm', 2).unwrap());
        assert_eq!(ed.wopt_num("foldlevel", -1), 0);
        assert!(ed.folds().iter().all(|fold| fold.closed));
        assert!(feed_z(&mut ed, 'r', 2).unwrap());
        assert_eq!(ed.wopt_num("foldlevel", -1), 2);
        assert!(ed.folds().iter().all(|fold| !fold.closed));
        assert!(!feed_z(&mut ed, 'Q', 1).unwrap());
        assert!(!feed_z(&mut ed, '0', 1).unwrap());
    }

    #[test]
    fn manual_only_commands_report_the_foldmethod() {
        let mut ed = editor_with(&["a", "b"]);
        set_str(&mut ed, "foldmethod", "indent");
        assert_eq!(create_fold(&mut ed, 0, 1).unwrap_err().code, "E350");
        assert_eq!(feed_z(&mut ed, 'd', 1).unwrap_err().code, "E351");
        assert_eq!(feed_z(&mut ed, 'E', 1).unwrap_err().code, "E352");
        // Delete/open commands are still available with the default method.
        set_str(&mut ed, "foldmethod", "manual");
        assert!(feed_z(&mut ed, 'E', 1).unwrap());
    }

    #[test]
    fn commands_never_panic_on_an_empty_buffer_or_past_the_end() {
        let mut ed = Editor::new();
        ed.set_cursor(Pos::new(usize::MAX, usize::MAX));
        for key in [
            'o', 'O', 'c', 'C', 'a', 'R', 'M', 'v', 'd', 'E', 'j', 'k', 'f', 'F',
        ] {
            let _ = feed_z(&mut ed, key, 3);
        }
        assert!(ed.folds().is_empty() || ed.cursor().line < ed.line_count() + 1);
    }
}
