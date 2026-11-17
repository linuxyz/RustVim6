//! M5 tests: quickfix and location lists (`:cexpr`, `:copen`, `:cnext`, ...).

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_term::ScriptedKeys;

fn screen(tui: &mut Tui, notation: &str, cols: usize, rows: usize) -> Vec<String> {
    let mut keys = ScriptedKeys::from_notation(notation);
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, cols, rows).unwrap();
    tui.screen_text(cols, rows)
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// A file with three lines plus its errorfile-style path.
fn fixture(name: &str, body: &str) -> (Session, std::path::PathBuf) {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, body).unwrap();
    let mut session = Session::new();
    session.editor.new_buffer();
    (session, path)
}

#[test]
fn cexpr_with_a_list_fills_the_quickfix_list() {
    let (session, path) = fixture("rvim_m5qf1.txt", "one\ntwo\nthree");
    let mut tui = Tui::new(session);
    let list = format!(
        "['{}:2:1: bad', '{}:3:1: worse']",
        path.display(),
        path.display()
    )
    .replace('\\', "/");
    let keys = format!(":cexpr {list}<CR>");
    screen(&mut tui, &keys, 80, 10);
    let list = tui.session.editor.qf_state().current_list().cloned();
    assert!(list.is_some(), ":cexpr built a quickfix list");
    assert_eq!(list.unwrap().entries.len(), 2);
}

#[test]
fn copen_creates_a_quickfix_window_with_the_entries() {
    let (session, path) = fixture("rvim_m5qf2.txt", "one\ntwo\nthree");
    let mut tui = Tui::new(session);
    let list = format!("['{}:2:1: bad']", path.display());
    let keys = format!(":cexpr {list}<CR>:copen<CR>");
    let rows = screen(&mut tui, &keys, 80, 14);
    assert!(
        tui.session.editor.in_quickfix_window(),
        "the qf window is current"
    );
    let text = rows.join("\n");
    assert!(text.contains("bad"), "the entry text is shown: {text}");
    assert!(tui.session.editor.window_count() >= 2);
}

#[test]
fn cnext_jumps_to_the_error_line() {
    let (session, path) = fixture("rvim_m5qf3.txt", "one\ntwo\nthree");
    let mut tui = Tui::new(session);
    let list = format!("['{}:3:1: bad']", path.display());
    let keys = format!(":cexpr {list}<CR>:cnext<CR>");
    screen(&mut tui, &keys, 80, 12);
    assert_eq!(tui.session.editor.cursor().line, 2, "cnext moved to line 3");
    assert_eq!(
        tui.session.editor.current_buffer().line_str(2).unwrap(),
        "three"
    );
}

#[test]
fn cclose_removes_the_quickfix_window() {
    let (session, path) = fixture("rvim_m5qf4.txt", "one");
    let mut tui = Tui::new(session);
    let list = format!("['{}:1:1: bad']", path.display());
    let keys = format!(":cexpr {list}<CR>:copen<CR>");
    screen(&mut tui, &keys, 80, 12);
    let before = tui.session.editor.window_count();
    screen(&mut tui, ":cclose<CR>", 80, 12);
    assert!(tui.session.editor.window_count() < before);
}

#[test]
fn setqflist_and_getqflist_round_trip() {
    let (session, _path) = fixture("rvim_m5qf5.txt", "one\ntwo");
    let mut tui = Tui::new(session);
    screen(
        &mut tui,
        ":call setqflist([{'filename': 'a.txt', 'lnum': 7, 'text': 'boom'}])<CR>",
        80,
        10,
    );
    screen(&mut tui, ":let g:n = len(getqflist())<CR>", 80, 10);
    let n = tui
        .session
        .viml
        .env
        .get_var("g:n")
        .map(|v| v.to_number())
        .unwrap_or(-1);
    assert_eq!(n, 1, "getqflist() reported the entry");
}

#[test]
fn location_list_is_per_window() {
    let (session, path) = fixture("rvim_m5qf6.txt", "one\ntwo");
    let mut tui = Tui::new(session);
    let list = format!("['{}:2:1: loc']", path.display());
    let keys = format!(":lexpr {list}<CR>");
    screen(&mut tui, &keys, 80, 10);
    let win = tui.session.editor.windows.current_id();
    assert!(
        tui.session.editor.loclist(win).is_some(),
        "the location list belongs to the window"
    );
    assert!(
        !tui.session.editor.loclist(win).unwrap().entries.is_empty(),
        "the entry is stored"
    );
}

#[test]
fn clist_prints_the_entries() {
    let (session, path) = fixture("rvim_m5qf7.txt", "one\ntwo");
    let mut tui = Tui::new(session);
    let list = format!("['{}:2:1: listed']", path.display());
    let keys = format!(":cexpr {list}<CR>:clist<CR>");
    screen(&mut tui, &keys, 80, 10);
    assert!(
        tui.session
            .message_history()
            .iter()
            .any(|m| m.contains("listed")),
        ":clist lists the entry: {:?}",
        tui.session.message_history()
    );
}
