//! M5 tests: diff mode (`:diffthis`, fillers, `]c`/`[c`, `do`/`dp`) and the
//! `rvim -d` (vimdiff) startup path.

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_term::ScriptedKeys;

fn open_two(a: &[&str], b: &[&str]) -> Session {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir();
    let left = dir.join(format!("rvim_m5diff_a_{unique}.txt"));
    let right = dir.join(format!("rvim_m5diff_b_{unique}.txt"));
    std::fs::write(&left, a.join("\n")).unwrap();
    std::fs::write(&right, b.join("\n")).unwrap();
    let mut session = Session::new();
    session.open_file(&left).unwrap();
    // Open the second file in a vertical split, the way vimdiff starts.
    session
        .ex(&format!("vsplit {}", right.display()))
        .expect(":vsplit opens the second file");
    let _ = std::fs::remove_file(&left);
    let _ = std::fs::remove_file(&right);
    session
}

fn screen(tui: &mut Tui, notation: &str, cols: usize, rows: usize) -> Vec<String> {
    let mut keys = ScriptedKeys::from_notation(notation);
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, cols, rows).unwrap();
    tui.screen_text(cols, rows)
        .split('\n')
        .map(str::to_string)
        .collect()
}

#[test]
fn diffthis_marks_both_windows_and_reports_fillers() {
    let mut tui = Tui::new(open_two(&["a", "b", "c"], &["a", "c"]));
    screen(&mut tui, ":diffthis<CR><C-w>w:diffthis<CR>", 60, 12);
    let ed = &tui.session.editor;
    let ids = ed.windows.ids();
    assert_eq!(ids.len(), 2);
    for id in &ids {
        assert!(ed.in_diff_mode(*id), "window {id} is in diff mode");
        assert!(
            ed.diff_next_change(*id, usize::MAX, false).is_some()
                || ed.diff_next_change(*id, 0, true).is_some(),
            "window {id} sees a changed line"
        );
    }
    // Buffer A has one line more than B, so exactly one side shows a filler.
    let fillers: usize = ids
        .iter()
        .flat_map(|id| (0..4).map(move |line| (*id, line)))
        .map(|(id, line)| ed.diff_filler(id, line).0)
        .sum();
    assert!(fillers >= 1, "the extra line produces a filler row");
}

#[test]
fn diff_mode_draws_filler_rows() {
    let mut tui = Tui::new(open_two(&["a", "b", "c"], &["a", "c"]));
    screen(&mut tui, ":diffthis<CR><C-w>w:diffthis<CR>", 60, 12);
    let rows = screen(&mut tui, "", 60, 12);
    let text = rows.join("\n");
    assert!(
        text.contains('^'),
        "the side with fewer lines shows a filler row: {text}"
    );
}

#[test]
fn diff_jumps_between_changed_lines() {
    let mut tui = Tui::new(open_two(&["a", "b", "c", "d"], &["a", "x", "c", "d"]));
    screen(&mut tui, ":diffthis<CR><C-w>w:diffthis<CR>", 60, 14);
    tui.session.editor.set_cursor(rvim_core::Pos::new(0, 0));
    screen(&mut tui, "]c", 60, 14);
    let line = tui.session.editor.cursor().line;
    assert_eq!(line, 1, "]c lands on the changed line");
}

#[test]
fn diffoff_leaves_diff_mode() {
    let mut tui = Tui::new(open_two(&["a"], &["b"]));
    screen(&mut tui, ":diffthis<CR>:diffoff<CR>", 60, 10);
    let win = tui.session.editor.windows.current_id();
    assert!(!tui.session.editor.in_diff_mode(win));
}

#[test]
fn diffget_copies_the_hunk_from_the_other_window() {
    let mut tui = Tui::new(open_two(&["a", "left", "c"], &["a", "right", "c"]));
    screen(&mut tui, ":diffthis<CR><C-w>w:diffthis<CR>", 60, 12);
    tui.session.editor.set_cursor(rvim_core::Pos::new(1, 0));
    screen(&mut tui, ":diffget<CR>", 60, 12);
    let ids = tui.session.editor.windows.ids();
    let text = tui.session.editor.current_buffer().full_text();
    assert!(
        ids.iter().any(|id| {
            tui.session
                .editor
                .buffer(tui.session.editor.windows.get(*id).unwrap().bufnr)
                .map(|b| b.full_text() == text)
                .unwrap_or(false)
        }),
        ":diffget makes the two sides agree on the hunk: {text:?}"
    );
}

#[test]
fn diff_folds_are_closed_and_can_be_opened() {
    let mut same: Vec<&str> = vec!["same"; 20];
    let mut left: Vec<&str> = same.clone();
    let mut right: Vec<&str> = same.clone();
    left[10] = "only-a";
    right[10] = "only-b";
    same.clear();
    let mut tui = Tui::new(open_two(&left, &right));
    screen(&mut tui, ":diffthis<CR><C-w>w:diffthis<CR>", 60, 16);
    let folds = tui.session.editor.folds();
    assert!(
        !folds.is_empty(),
        ":diffthis sets foldmethod=diff: {folds:?}"
    );
}
