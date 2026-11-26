//! GUI pointer scrolling keeps an independent viewport until keyboard input.

use rvim_app::{Session, tui::Tui};
use rvim_core::Pos;
use rvim_editor::maps::Key;

fn editor() -> Tui {
    let mut session = Session::new();
    let text = (0..100)
        .map(|line| format!("line {line:03} -- long enough for horizontal scrolling"))
        .collect::<Vec<_>>()
        .join("\n");
    session.editor.insert_text(Pos::new(0, 0), &text).unwrap();
    session.editor.set_cursor(Pos::new(0, 0));
    let mut tui = Tui::new(session);
    tui.gui_grid(80, 12);
    tui
}

#[test]
fn pointer_scroll_does_not_move_the_cursor_or_snap_back_on_redraw() {
    let mut tui = editor();
    let id = tui.session.editor.windows.current_id();
    assert!(tui.gui_scroll_to(id, 40));
    for _ in 0..3 {
        assert_eq!(tui.gui_grid(80, 12).cursor(), None);
        assert_eq!(tui.session.editor.windows.current().top_line, 40);
        assert_eq!(tui.session.editor.cursor(), Pos::new(0, 0));
    }
}

#[test]
fn keyboard_input_resumes_following_the_edit_cursor() {
    let mut tui = editor();
    let id = tui.session.editor.windows.current_id();
    tui.gui_scroll_to(id, 40);
    tui.gui_grid(80, 12);
    tui.feed_key(Key::Char('j')).unwrap();
    assert!(tui.gui_grid(80, 12).cursor().is_some());
    assert_eq!(tui.session.editor.cursor(), Pos::new(1, 0));
    assert_eq!(tui.session.editor.windows.current().top_line, 1);
}

#[test]
fn bounds_and_resize_are_clamped_without_moving_the_cursor() {
    let mut tui = editor();
    let id = tui.session.editor.windows.current_id();
    tui.gui_scroll_to(id, usize::MAX);
    tui.gui_grid(80, 12);
    let visible = tui.session.editor.windows.current().rect.text_rows();
    assert_eq!(tui.session.editor.windows.current().top_line, 100 - visible);
    tui.gui_grid(80, 30);
    let visible = tui.session.editor.windows.current().rect.text_rows();
    assert_eq!(tui.session.editor.windows.current().top_line, 100 - visible);
    assert_eq!(tui.session.editor.cursor(), Pos::new(0, 0));
    assert!(!tui.gui_scroll_to(-123, 0));
}

#[test]
fn split_panes_scroll_independently_without_changing_focus() {
    let mut tui = editor();
    tui.session.ex("vsplit").unwrap();
    tui.gui_grid(80, 12);
    let active = tui.session.editor.windows.current_id();
    let other = tui
        .session
        .editor
        .windows
        .ids()
        .into_iter()
        .find(|id| *id != active)
        .unwrap();
    tui.gui_scroll_to(other, 30);
    assert!(tui.gui_grid(80, 12).cursor().is_some());
    assert_eq!(tui.session.editor.windows.get(other).unwrap().top_line, 30);
    assert_eq!(tui.session.editor.windows.get(active).unwrap().top_line, 0);
    assert_eq!(tui.session.editor.windows.current_id(), active);
    tui.session.ex("only").unwrap();
    tui.gui_grid(80, 12);
}

#[test]
fn changed_cursor_and_changed_buffer_release_the_viewport_anchor() {
    let mut tui = editor();
    let id = tui.session.editor.windows.current_id();
    tui.gui_scroll_to(id, 40);
    tui.session.editor.set_cursor(Pos::new(90, 0));
    assert!(tui.gui_grid(80, 12).cursor().is_some());
    assert!(tui.session.editor.windows.current().top_line > 40);
    tui.gui_scroll_to(id, 40);
    tui.session.ex("enew!").unwrap();
    tui.gui_grid(80, 12);
    assert_eq!(tui.session.editor.windows.current().top_line, 0);
}

#[test]
fn pointer_scroll_snaps_closed_folds_to_their_header() {
    let mut tui = editor();
    tui.session.ex("10,30fold").unwrap();
    tui.session.editor.set_cursor(Pos::new(0, 0));
    tui.gui_grid(80, 12);
    let id = tui.session.editor.windows.current_id();
    tui.gui_scroll_to(id, 20);
    tui.gui_grid(80, 12);
    assert_eq!(tui.session.editor.windows.current().top_line, 9);
}

#[test]
fn empty_buffers_remain_at_the_first_line() {
    let mut tui = Tui::new(Session::new());
    tui.gui_grid(80, 12);
    let id = tui.session.editor.windows.current_id();
    assert!(!tui.gui_scroll_to(id, usize::MAX));
    assert_eq!(tui.gui_grid(80, 12).cursor(), Some((0, 0)));
}
