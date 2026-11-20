//! <expr> mappings now go through the VimL evaluator (M4 gap).

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_term::ScriptedKeys;

#[test]
fn expr_mapping_is_evaluated_and_replayed_as_keys() {
    let mut session = Session::new();
    session
        .editor
        .replace_lines(0, 1, vec!["abcdef".to_string()])
        .unwrap();
    session.ex("nnoremap <expr> Q '3l'").unwrap();
    let mut tui = Tui::new(session);
    let mut keys = ScriptedKeys::from_notation("Q");
    let mut sink = Vec::new();
    tui.run(&mut keys, &mut sink, 20, 5).unwrap();
    assert_eq!(tui.session.editor.cursor().col, 3);
}
