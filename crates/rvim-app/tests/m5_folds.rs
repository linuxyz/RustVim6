//! M5 tests: folds (manual, indent, marker) and their TUI rendering.

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_term::ScriptedKeys;

fn open(lines: &[&str]) -> Session {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("rvim_m5fold_{unique}.txt"));
    std::fs::write(&path, lines.join("\n")).unwrap();
    let mut session = Session::new();
    session.open_file(&path).unwrap();
    let _ = std::fs::remove_file(&path);
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
fn manual_fold_hides_lines_and_renders_a_summary() {
    let mut tui = Tui::new(open(&["one", "two", "three", "four"]));
    let rows = screen(&mut tui, "Vjzf", 40, 8);
    assert_eq!(tui.session.editor.folds().len(), 1);
    let label = rows[0].clone();
    assert!(label.starts_with('+'), "fold summary: {label:?}");
    assert!(label.contains("2 lines"), "fold summary: {label:?}");
    assert!(
        rows[1].contains("three"),
        "the hidden lines are skipped: {:?}",
        rows
    );
}

#[test]
fn zo_and_zc_open_and_close_a_fold() {
    let mut tui = Tui::new(open(&["one", "two", "three", "four"]));
    screen(&mut tui, "Vjzf", 40, 8);
    screen(&mut tui, "zo", 40, 8);
    let rows = screen(&mut tui, "", 40, 8);
    assert!(rows[1].contains("two"), "zo shows the body: {:?}", rows);
    screen(&mut tui, "zc", 40, 8);
    assert!(tui.session.editor.folds()[0].closed);
}

#[test]
fn zr_and_zm_open_and_close_every_fold() {
    let mut tui = Tui::new(open(&["a", "b", "c", "d", "e", "f"]));
    screen(&mut tui, "Vjzf", 40, 10);
    screen(&mut tui, "jjVjzf", 40, 10);
    assert_eq!(tui.session.editor.folds().len(), 2);
    screen(&mut tui, "zR", 40, 10);
    assert!(tui.session.editor.folds().iter().all(|f| !f.closed));
    screen(&mut tui, "zM", 40, 10);
    assert!(tui.session.editor.folds().iter().all(|f| f.closed));
    screen(&mut tui, "zd", 40, 10);
    assert_eq!(tui.session.editor.folds().len(), 1, "zd deletes one fold");
}

#[test]
fn foldmethod_indent_folds_by_shiftwidth() {
    let mut tui = Tui::new(open(&[
        "fn main() {",
        "    let a = 1;",
        "    let b = 2;",
        "}",
    ]));
    screen(
        &mut tui,
        ":set foldmethod=indent shiftwidth=4<CR>:set foldlevel=0<CR>",
        40,
        8,
    );
    let folds = tui.session.editor.folds();
    assert!(!folds.is_empty(), "indent folds are computed: {folds:?}");
}

#[test]
fn foldmethod_marker_uses_the_default_markers() {
    let mut tui = Tui::new(open(&["text", "# {{{1", "inner", "# }}}", "tail"]));
    screen(&mut tui, ":set foldmethod=marker<CR>", 40, 10);
    let folds = tui.session.editor.folds();
    assert_eq!(folds.len(), 1, "marker fold: {folds:?}");
    assert_eq!(folds[0].start, 1);
    assert!(folds[0].end >= 3);
}

#[test]
fn fold_text_is_vims_default_summary() {
    let mut tui = Tui::new(open(&["alpha", "beta", "gamma"]));
    screen(&mut tui, "Vjzf", 40, 8);
    let rows = screen(&mut tui, "", 40, 8);
    assert!(
        rows[0].contains("alpha"),
        "the summary keeps the first line: {:?}",
        rows[0]
    );
    assert!(
        rows[0].contains("--"),
        "vim pads with dashes: {:?}",
        rows[0]
    );
}

#[test]
fn foldcolumn_shows_the_fold_marker() {
    let mut tui = Tui::new(open(&["one", "two", "three"]));
    screen(&mut tui, ":set foldcolumn=1<CR>", 40, 8);
    let rows = screen(&mut tui, "Vjzf", 40, 8);
    assert!(
        rows[0].starts_with('+'),
        "a closed fold shows + in the fold column: {:?}",
        rows[0]
    );
}

#[test]
fn foldclosed_reports_the_range() {
    let mut tui = Tui::new(open(&["one", "two", "three", "four"]));
    screen(&mut tui, "Vjzf", 40, 8);
    let ed = &tui.session.editor;
    assert_eq!(ed.closed_fold_at(0), Some((0, 1)));
    assert_eq!(ed.fold_level_at(0), 1);
    assert_eq!(ed.closed_fold_at(2), None);
}

#[test]
fn fold_count_is_reported_by_ex_command() {
    let mut tui = Tui::new(open(&["one", "two", "three"]));
    screen(&mut tui, "Vjzf", 40, 8);
    screen(&mut tui, ":2fold<CR>", 40, 8);
    assert!(tui.session.editor.folds().len() >= 2);
}
