//! M5 tests: signs, text properties and popup windows (their data model and
//! the TUI rendering of the sign column and popup overlays).

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_term::ScriptedKeys;

fn open(lines: &[&str]) -> Session {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("rvim_m5sign_{unique}.txt"));
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
fn sign_define_and_place_store_the_sign() {
    let mut tui = Tui::new(open(&["one", "two"]));
    screen(
        &mut tui,
        ":call sign_define('err', {'text': 'E>'})<CR>:call sign_place(1, '', 'err', bufnr('%'), {'lnum': 2})<CR>",
        40,
        6,
    );
    let placed = tui.session.viml.state.meta.sign_places.clone();
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].lnum, 2);
    assert_eq!(placed[0].name, "err");
}

#[test]
fn sign_column_is_drawn_next_to_the_text() {
    let mut tui = Tui::new(open(&["one", "two"]));
    screen(
        &mut tui,
        ":call sign_define('err', {'text': 'E>'})<CR>:call sign_place(1, '', 'err', bufnr('%'), {'lnum': 2})<CR>",
        40,
        6,
    );
    let rows = screen(&mut tui, "", 40, 6);
    assert!(
        rows[1].contains("E>"),
        "the sign column shows the sign: {:?}",
        rows[1]
    );
    assert!(rows[1].contains("two"));
}

#[test]
fn sign_unplace_removes_the_sign() {
    let mut tui = Tui::new(open(&["one", "two"]));
    screen(
        &mut tui,
        ":call sign_define('err', {'text': 'E>'})<CR>:call sign_place(1, '', 'err', bufnr('%'), {'lnum': 1})<CR>:call sign_unplace('', {'buffer': bufnr('%')})<CR>",
        40,
        6,
    );
    assert!(tui.session.viml.state.meta.sign_places.is_empty());
}

#[test]
fn prop_type_add_and_prop_add_record_a_property() {
    let mut tui = Tui::new(open(&["hello world"]));
    screen(
        &mut tui,
        ":call prop_type_add('T', {})<CR>:call prop_add(1, 1, {'type': 'T', 'length': 5})<CR>",
        40,
        6,
    );
    let props = tui.session.viml.state.meta.textprops.clone();
    assert_eq!(props.len(), 1, "one property entry");
    assert_eq!(props[0].prop_type, "T");
}

#[test]
fn popup_create_is_listed_and_rendered() {
    let mut tui = Tui::new(open(&["one", "two", "three"]));
    let rows = screen(
        &mut tui,
        ":call popup_create('POPUP', {'line': 2, 'col': 3})<CR>",
        40,
        8,
    );
    assert_eq!(tui.session.viml.state.meta.popups.len(), 1);
    assert!(
        rows.iter().any(|r| r.contains("POPUP")),
        "the popup text is drawn over the window: {rows:?}"
    );
}

#[test]
fn popup_close_removes_the_overlay() {
    let mut tui = Tui::new(open(&["one"]));
    screen(
        &mut tui,
        ":call popup_create('X', {})<CR>:call popup_clear()<CR>",
        40,
        6,
    );
    assert!(tui.session.viml.state.meta.popups.is_empty());
}
