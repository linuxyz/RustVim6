//! GUI font option plumbing (the native dialog itself is Windows-only; this
//! test exercises the Ex hook and option storage on all platforms).

use rvim_app::Session;
use std::sync::Mutex;

static HOOK_LOCK: Mutex<()> = Mutex::new(());

fn choose_test_font(_current: &str, wide: bool) -> Option<String> {
    Some(if wide {
        "MSGothic:h14".to_string()
    } else {
        "Consolas:h14".to_string()
    })
}

#[test]
fn bare_set_guifont_and_guifontwide_use_the_gui_hook() {
    let _guard = HOOK_LOCK.lock().unwrap();
    rvim_ex::gui_font::set_font_dialog_hook(Some(choose_test_font));
    let mut session = Session::new();
    session.ex("set guifont").unwrap();
    session.ex("set guifontwide").unwrap();
    assert_eq!(session.editor.options.str_or("guifont", ""), "Consolas:h14");
    assert_eq!(
        session.editor.options.str_or("guifontwide", ""),
        "MSGothic:h14"
    );
    rvim_ex::gui_font::set_font_dialog_hook(None);
}

#[test]
fn star_also_opens_the_font_hook() {
    let _guard = HOOK_LOCK.lock().unwrap();
    rvim_ex::gui_font::set_font_dialog_hook(Some(choose_test_font));
    let mut session = Session::new();
    session.ex("set guifont=*").unwrap();
    assert_eq!(session.editor.options.str_or("guifont", ""), "Consolas:h14");
    rvim_ex::gui_font::set_font_dialog_hook(None);
}
