//! Normal-mode window commands, through the same key dispatcher as the GUI.
use rvim_app::{Session, tui::Tui};
use rvim_core::Pos;
use rvim_editor::Mode;
use rvim_term::ScriptedKeys;

fn tui() -> Tui {
    let mut tui = Tui::new(Session::new());
    tui.gui_grid(100, 40);
    tui
}
fn keys(tui: &mut Tui, notation: &str) {
    let mut input = ScriptedKeys::from_notation(notation);
    tui.run(&mut input, &mut Vec::new(), 100, 40).unwrap();
}

#[test]
fn split_commands_accept_printable_and_control_key_aliases() {
    for (command, vertical) in [
        ("<C-w>s", false),
        ("<C-w>S", false),
        ("<C-w><C-s>", false),
        ("<C-w>v", true),
        ("<C-w><C-v>", true),
    ] {
        let mut tui = tui();
        keys(&mut tui, command);
        assert_eq!(tui.session.editor.window_count(), 2, "{command}");
        let rects = tui.session.editor.windows.rects();
        let rects = rects.values().collect::<Vec<_>>();
        assert_eq!(rects[0].row == rects[1].row, vertical, "{command}");
        assert_eq!(tui.session.editor.mode, Mode::Normal);
    }
}

#[test]
fn mixed_splits_work_from_both_the_original_and_new_panes() {
    for command in ["<C-w>s<C-w>j<C-w>v", "<C-w>v<C-w>l<C-w>s"] {
        let mut tui = tui();
        keys(&mut tui, command);
        assert_eq!(tui.session.editor.window_count(), 3, "{command}");
        assert_eq!(tui.session.report.error_count, 0, "{command}");
    }
}

#[test]
fn split_counts_before_and_after_ctrl_w_set_the_new_height() {
    for command in ["8<C-w>s", "<C-w>8s", "2<C-w>4s"] {
        let mut tui = tui();
        keys(&mut tui, command);
        assert_eq!(tui.session.editor.window_count(), 2, "{command}");
        assert_eq!(
            tui.session.editor.windows.current().rect.text_rows(),
            8,
            "{command}"
        );
    }
}

#[test]
fn counted_vertical_split_sets_the_requested_width() {
    let mut tui = tui();
    keys(&mut tui, "<C-w>25<C-v>");
    assert_eq!(tui.session.editor.window_count(), 2);
    assert_eq!(tui.session.editor.windows.current().rect.cols, 25);
}

#[test]
fn held_control_keys_switch_windows_and_repeat_ctrl_w_cycles() {
    let mut tui = tui();
    keys(&mut tui, "<C-w>s");
    let first = tui.session.editor.windows.current_id();
    keys(&mut tui, "<C-w><C-j>");
    let second = tui.session.editor.windows.current_id();
    assert_ne!(first, second);
    keys(&mut tui, "<C-w><C-k>");
    assert_eq!(tui.session.editor.windows.current_id(), first);
    keys(&mut tui, "<C-w><C-w>");
    assert_eq!(tui.session.editor.windows.current_id(), second);
}

#[test]
fn counted_window_cycle_selects_the_numbered_window() {
    let mut tui = tui();
    keys(&mut tui, "<C-w>s<C-w>s");
    let ids = tui.session.editor.windows.ids();
    keys(&mut tui, "<C-w>b<C-w>1w");
    assert_eq!(tui.session.editor.windows.current_id(), ids[0]);
    keys(&mut tui, "2<C-w>w");
    assert_eq!(tui.session.editor.windows.current_id(), ids[1]);
}

#[test]
fn default_maximise_and_explicit_resize_counts_are_distinct() {
    let mut tui = tui();
    keys(&mut tui, "<C-w>s<C-w>_");
    let max_height = tui.session.editor.windows.current().rect.text_rows();
    assert!(max_height > 25);
    keys(&mut tui, "<C-w>6_");
    assert_eq!(tui.session.editor.windows.current().rect.text_rows(), 6);
    keys(&mut tui, "<C-w>3+");
    assert_eq!(tui.session.editor.windows.current().rect.text_rows(), 9);
}

#[test]
fn cancelled_or_unknown_window_commands_do_not_leak_counts() {
    let mut tui = tui();
    for command in ["3<C-w><Esc>", "3<C-w><F12>"] {
        keys(&mut tui, command);
        keys(&mut tui, "<C-w>s");
        assert!(tui.session.editor.windows.current().rect.text_rows() > 3);
        keys(&mut tui, "<C-w>o");
    }
}

#[test]
fn closing_a_modified_shared_buffer_keeps_the_other_split() {
    let mut tui = tui();
    tui.session
        .editor
        .insert_text(Pos::new(0, 0), "unsaved")
        .unwrap();
    keys(&mut tui, "<C-w>s<C-w>c");
    assert_eq!(tui.session.editor.window_count(), 1);
    assert_eq!(tui.session.editor.current_buffer().full_text(), "unsaved");
}

#[test]
fn width_resizing_uses_the_rendered_geometry() {
    let mut tui = tui();
    keys(&mut tui, "<C-w>v<C-w>25|");
    assert_eq!(tui.session.editor.windows.current().rect.cols, 25);
    keys(&mut tui, "<C-w>3>");
    assert_eq!(tui.session.editor.windows.current().rect.cols, 28);
    keys(&mut tui, "<C-w>2<");
    assert_eq!(tui.session.editor.windows.current().rect.cols, 26);
    keys(&mut tui, "<C-w>|");
    assert!(tui.session.editor.windows.current().rect.cols > 75);
}

#[test]
fn nested_resizing_keeps_frames_inside_the_screen_without_overlap() {
    let mut tui = tui();
    keys(&mut tui, "<C-w>v<C-w>l<C-w>s<C-w>8_<C-w>15|");
    assert_eq!(tui.session.editor.window_count(), 3);
    assert_eq!(tui.session.editor.windows.current().rect.text_rows(), 8);
    let rects = tui
        .session
        .editor
        .windows
        .rects()
        .values()
        .copied()
        .collect::<Vec<_>>();
    for (index, rect) in rects.iter().enumerate() {
        assert!(rect.row + rect.rows <= 39);
        assert!(rect.col + rect.cols <= 100);
        for other in &rects[index + 1..] {
            assert!(
                rect.row + rect.rows <= other.row
                    || other.row + other.rows <= rect.row
                    || rect.col + rect.cols <= other.col
                    || other.col + other.cols <= rect.col
            );
        }
    }
}

#[test]
fn unrecognised_ctrl_w_suffix_does_not_affect_the_next_motion() {
    let mut tui = tui();
    tui.session
        .editor
        .insert_text(Pos::new(0, 0), "abcdef")
        .unwrap();
    tui.session.editor.set_cursor(Pos::new(0, 0));
    keys(&mut tui, "3<C-w><F12>l");
    assert_eq!(tui.session.editor.cursor(), Pos::new(0, 1));
}

#[test]
fn ctrl_w_new_alias_opens_an_empty_buffer() {
    let mut tui = tui();
    let original = tui.session.editor.current_bufnr();
    keys(&mut tui, "<C-w><C-n>");
    assert_eq!(tui.session.editor.window_count(), 2);
    assert_ne!(tui.session.editor.current_bufnr(), original);
    assert_eq!(tui.session.editor.current_buffer().full_text(), "");
}

#[test]
fn ctrl_w_suffix_is_not_expanded_by_a_normal_mode_mapping() {
    let mut tui = tui();
    tui.session.ex("nnoremap s iBAD<Esc>").unwrap();
    keys(&mut tui, "<C-w>s");
    assert_eq!(tui.session.editor.window_count(), 2);
    assert_eq!(tui.session.editor.current_buffer().full_text(), "");
}

#[test]
fn close_refuses_to_hide_a_unique_modified_buffer() {
    let mut tui = tui();
    keys(&mut tui, "<C-w>n");
    tui.session
        .editor
        .insert_text(Pos::new(0, 0), "unsaved")
        .unwrap();
    keys(&mut tui, "<C-w>c");
    assert_eq!(tui.session.editor.window_count(), 2);
    assert_eq!(tui.session.editor.current_buffer().full_text(), "unsaved");
    assert_eq!(tui.session.report.error_count, 1);
}

#[test]
fn ctrl_w_q_can_quit_the_last_unmodified_window() {
    let mut tui = tui();
    keys(&mut tui, "<C-w>q");
    assert!(tui.quit);
}
