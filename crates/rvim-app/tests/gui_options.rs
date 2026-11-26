//! guioptions assignments, completion and the actual GUI grid entry point.

use rvim_app::{Session, tui::Tui};

fn flags(session: &Session) -> String {
    session.editor.options.str_or("guioptions", "")
}

#[test]
fn remove_default_tabline_flag_and_add_it_back() {
    let mut session = Session::new();
    session.ex("set go-=e").unwrap();
    assert_eq!(flags(&session), "gmrLtT");
    session.ex("set guioptions+=e").unwrap();
    assert_eq!(flags(&session), "gmrLtTe");
    session.ex("set go+=e").unwrap();
    assert_eq!(flags(&session), "gmrLtTe");
    assert!(!flags(&session).contains(','));
}

#[test]
fn prepend_preserves_flags_and_deduplicates_like_vim() {
    let mut session = Session::new();
    session.ex("set go=mr").unwrap();
    session.ex("set go^=e").unwrap();
    assert_eq!(flags(&session), "emr");
    session.ex("set go^=e").unwrap();
    assert_eq!(flags(&session), "emr");
    session.ex("set go=eegmre").unwrap();
    assert_eq!(flags(&session), "gmre");
}

#[test]
fn removing_multiple_flags_preserves_vim_substring_semantics() {
    let mut session = Session::new();
    session.ex("set go=egmr").unwrap();
    session.ex("set go-=gr").unwrap();
    assert_eq!(flags(&session), "egmr");
    session.ex("set go-=gm").unwrap();
    assert_eq!(flags(&session), "er");
    session.ex("set go-=Z").unwrap();
    assert_eq!(flags(&session), "er");
}

#[test]
fn invalid_guioptions_preserve_the_previous_value() {
    let mut session = Session::new();
    let original = flags(&session);
    for command in ["set go=Z", "set go+=,", "set go^=中文", "let &go = 'e,e'"] {
        let error = session.ex(command).unwrap_err();
        assert_eq!(error.code, "E539", "{command}");
        assert_eq!(flags(&session), original, "{command}");
    }
}

#[test]
fn empty_values_and_reset_to_defaults_work() {
    let mut session = Session::new();
    session.ex("set go=").unwrap();
    assert_eq!(flags(&session), "");
    session.ex("set go+=e").unwrap();
    assert_eq!(flags(&session), "e");
    session.ex("set go-=e").unwrap();
    assert_eq!(flags(&session), "");
    session.ex("set go&").unwrap();
    assert_eq!(flags(&session), "egmrLtT");
}

#[test]
fn gui_redraw_applies_changes_and_reclaims_the_tabline_row() {
    let mut session = Session::new();
    session.ex("tabnew").unwrap();
    let mut tui = Tui::new(session);
    assert_eq!(tui.gui_grid(80, 10).cursor(), Some((1, 0)));
    tui.session.ex("set go-=e").unwrap();
    assert_eq!(tui.gui_grid(80, 10).cursor(), Some((0, 0)));
    tui.session.ex("set go+=e").unwrap();
    assert_eq!(tui.gui_grid(80, 10).cursor(), Some((1, 0)));
    tui.session.ex("tabclose").unwrap();
    assert_eq!(tui.gui_grid(80, 10).cursor(), Some((0, 0)));
}

#[test]
fn terminal_grid_does_not_obey_gui_only_flags() {
    let mut session = Session::new();
    session.ex("tabnew").unwrap();
    session.ex("set go=").unwrap();
    let mut tui = Tui::new(session);
    assert_eq!(tui.grid(80, 10).cursor(), Some((1, 0)));
}

#[test]
fn clipboard_and_multiplatform_flags_remain_recognised() {
    let mut session = Session::new();
    session.ex("set go=LPR").unwrap();
    assert_eq!(flags(&session), "LPR");
}

#[test]
fn completed_guioption_commands_toggle_only_the_requested_flag() {
    let mut tui = Tui::new(Session::new());
    let mut keys = rvim_term::ScriptedKeys::from_notation(":set go-=e<Tab><CR>");
    tui.run(&mut keys, &mut Vec::new(), 80, 10).unwrap();
    assert_eq!(flags(&tui.session), "gmrLtT");
    let mut keys = rvim_term::ScriptedKeys::from_notation(":set go+=e<Tab><CR>");
    tui.run(&mut keys, &mut Vec::new(), 80, 10).unwrap();
    assert_eq!(flags(&tui.session), "gmrLtTe");
    assert_eq!(tui.session.report.error_count, 0);
}
