//! M6 syntax tests: filetype detection, the :syntax/:highlight commands and
//! the colours the TUI actually paints.

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_term::{Color, ScriptedKeys};

fn open_named(name: &str, contents: &str) -> (Session, std::path::PathBuf) {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, contents).unwrap();
    let mut session = Session::new();
    session.open_file(&path).unwrap();
    (session, path)
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

// --------------------------------------------------------------- rendering

#[test]
fn syntax_on_colours_a_rust_buffer() {
    let (session, path) = open_named("rvim_m6_a.rs", "fn main() {\n    let x = 1; // hi\n}\n");
    let mut tui = Tui::new(session);
    let _ = screen(&mut tui, ":syntax on<CR>", 40, 8);
    let grid = tui.grid(40, 8);
    // "fn" at (0,0) is the Keyword blue; "main" is not.
    assert_eq!(grid.cell(0, 0).unwrap().style.fg, Color::Rgb(86, 156, 214));
    assert_ne!(
        grid.cell(0, 3).unwrap().style.fg,
        Color::Rgb(86, 156, 214),
        "the function name is not a keyword"
    );
    // The comment on line 2 is green (Comment).
    let comment_col = 15; // the "//" after "    let x = 1; "
    assert_eq!(
        grid.cell(1, comment_col).unwrap().style.fg,
        Color::Rgb(106, 153, 85)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn syntax_off_leaves_the_buffer_in_normal_colors() {
    let (session, path) = open_named("rvim_m6_b.rs", "fn main() {}\n");
    let mut tui = Tui::new(session);
    let _ = screen(&mut tui, ":syntax on<CR>:syntax off<CR>", 40, 6);
    let grid = tui.grid(40, 6);
    assert_eq!(grid.cell(0, 0).unwrap().style, tui.theme.text);
    assert_eq!(grid.cell(0, 3).unwrap().style, tui.theme.text);
    let _ = std::fs::remove_file(path);
}

#[test]
fn highlight_command_changes_the_rendered_colour() {
    let (session, path) = open_named("rvim_m6_c.rs", "fn main() {}\n");
    let mut tui = Tui::new(session);
    let rows = screen(
        &mut tui,
        ":syntax on<CR>:highlight Keyword guifg=#ff0000<CR>",
        40,
        6,
    );
    assert!(rows[0].starts_with("fn"), "text still drawn: {rows:?}");
    let grid = tui.grid(40, 6);
    assert_eq!(grid.cell(0, 0).unwrap().style.fg, Color::Rgb(255, 0, 0));
    let _ = std::fs::remove_file(path);
}

// ------------------------------------------------------------ filetype map

#[test]
fn twenty_filetypes_are_detected_by_extension() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("rs", "fn main() {}", "fn", "Keyword"),
        ("c", "int main(void) {}", "int", "Type"),
        ("cpp", "class A {}", "class", "Keyword"),
        ("py", "def f(): pass", "def", "Keyword"),
        ("vim", "let g:x = 1", "let", "Keyword"),
        ("js", "const x = 1;", "const", "Keyword"),
        ("ts", "interface A {}", "interface", "Keyword"),
        ("json", "{\"a\": true}", "true", "Constant"),
        ("yaml", "key: value", "key:", "Identifier"),
        ("toml", "[table]", "[table]", "Title"),
        ("html", "<div>", "<div", "Statement"),
        ("css", "a { color: red; }", "color:", "Identifier"),
        ("sh", "if true; then", "if", "Keyword"),
        ("mk", "all: build", "all:", "Function"),
        ("md", "# Title", "# Title", "Title"),
        ("lua", "local x = nil", "local", "Keyword"),
        ("java", "public class A {}", "public", "Keyword"),
        ("go", "func main() {}", "func", "Keyword"),
        ("rb", "def f; end", "def", "Keyword"),
        ("sql", "SELECT * FROM t", "SELECT", "Keyword"),
    ];
    assert_eq!(cases.len(), 20);
    for (ext, line, needle, group) in cases {
        let name = format!("rvim_m6_ft.{ext}");
        let (mut session, path) = open_named(&name, line);
        session.ex("syntax on").unwrap();
        let bufnr = session.editor.current_bufnr();
        let spans = session.editor.syntax_spans(bufnr, 0);
        assert!(
            spans
                .iter()
                .any(|(start, end, g)| &line[*start..*end] == *needle && g == group),
            "{ext}: expected {needle} as {group}, got {spans:?}"
        );
        let _ = std::fs::remove_file(path);
    }
}

// ------------------------------------------------------------ :syntax edits

#[test]
fn syntax_keyword_and_region_extend_a_definition() {
    let (mut session, path) = open_named("rvim_m6_d.txt", "alpha \"x\" omega\n");
    session.ex("syntax on").unwrap();
    session.ex("syntax keyword MyWord alpha").unwrap();
    session
        .ex("syntax region MyStr start=/\"/ end=/\"/")
        .unwrap();
    let bufnr = session.editor.current_bufnr();
    let spans = session.editor.syntax_spans(bufnr, 0);
    assert!(
        spans
            .iter()
            .any(|(s, e, g)| &"alpha \"x\" omega"[*s..*e] == "alpha" && g == "MyWord"),
        "{spans:?}"
    );
    assert!(
        spans
            .iter()
            .any(|(s, e, g)| &"alpha \"x\" omega"[*s..*e] == "\"x\"" && g == "MyStr"),
        "{spans:?}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn syntax_list_reports_the_items() {
    let (mut session, path) = open_named("rvim_m6_e.rs", "fn main() {}\n");
    session.ex("syntax on").unwrap();
    session.ex("syntax keyword MyWord alpha").unwrap();
    session.ex("syntax list MyWord").unwrap();
    let history = session.editor.messages.history().join("\n");
    assert!(history.contains("keyword MyWord alpha"), "{history}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn highlight_links_resolve_to_the_target_colour() {
    let (mut session, path) = open_named("rvim_m6_f.rs", "fn main() {}\n");
    session.ex("syntax on").unwrap();
    session.ex("highlight MyAlias guifg=#00ff00").unwrap();
    session.ex("highlight link MyWord MyAlias").unwrap();
    session.ex("syntax keyword MyWord fn").unwrap();
    let attr = session.editor.syntax_highlight("MyWord").expect("resolves");
    assert_eq!(
        attr.fg,
        Some(rvim_syntax::highlight::HlColor::Rgb(0, 255, 0))
    );
    let _ = std::fs::remove_file(path);
}
