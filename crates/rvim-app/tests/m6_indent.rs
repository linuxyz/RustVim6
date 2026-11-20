//! M6 tests: C indenting (cindent) and smart indenting.
//!
//! cindent() values are compared directly (the same numbers :echo cindent(n)
//! prints, checked against the real Vim binary for the common constructs) and
//! the insert path is driven through the TUI so <CR>, o and O use the shared
//! new_line_indent() implementation.

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_editor::indent::{cindent, new_line_indent};
use rvim_term::ScriptedKeys;

fn session(lines: &[&str]) -> Session {
    let mut session = Session::new();
    session
        .editor
        .replace_lines(
            0,
            1,
            lines
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<String>>(),
        )
        .unwrap();
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

fn buffer_lines(tui: &Tui) -> Vec<String> {
    tui.session
        .editor
        .current_buffer()
        .full_text()
        .split('\n')
        .map(str::to_string)
        .collect()
}

fn cindent_session(lines: &[&str]) -> Session {
    let mut session = session(lines);
    session.ex("set cindent shiftwidth=4 expandtab").unwrap();
    session
}

// ------------------------------------------------------------- cindent()

#[test]
fn cindent_reports_brace_levels_and_rejects_bad_lines() {
    let session = cindent_session(&["int main() {", "    x;", "}", "if (a) {", "    y;", "}"]);
    let ed = &session.editor;
    assert_eq!(cindent(ed, 1, 0), 0, "first line");
    assert_eq!(cindent(ed, 2, 0), 4, "body of the function");
    assert_eq!(cindent(ed, 3, 0), 0, "closing brace dedents");
    assert_eq!(cindent(ed, 4, 0), 0, "if at the same level");
    assert_eq!(cindent(ed, 5, 0), 4);
    assert_eq!(cindent(ed, 6, 0), 0);
    assert_eq!(cindent(ed, 0, 0), -1, "line 0 is invalid");
    assert_eq!(cindent(ed, 7, 0), -1, "past the end is invalid");
}

#[test]
fn cindent_builtin_is_wired() {
    let mut session = cindent_session(&["int main() {", "    x;"]);
    session.ex("echo cindent(2)").unwrap();
    assert_eq!(session.editor.messages.take_status().as_deref(), Some("4"));
}

#[test]
fn cindent_switch_labels_and_case_bodies() {
    let session = cindent_session(&[
        "switch (v) {",
        "    case 1:",
        "        a;",
        "        break;",
        "    default:",
        "        b;",
        "}",
    ]);
    let ed = &session.editor;
    assert_eq!(cindent(ed, 2, 0), 4, "case label one level in");
    assert_eq!(cindent(ed, 3, 0), 8, "case body another level");
    assert_eq!(cindent(ed, 4, 0), 8, "break stays in the case body");
    assert_eq!(cindent(ed, 5, 0), 4, "default label");
    assert_eq!(cindent(ed, 6, 0), 8);
    assert_eq!(cindent(ed, 7, 0), 0);
}

#[test]
fn cindent_labels_preprocessor_and_access_specifiers() {
    let session = cindent_session(&[
        "int main() {",
        "label:",
        "    x;",
        "#define X 1",
        "  #define Y 2",
        "public:",
        "};",
    ]);
    let ed = &session.editor;
    assert_eq!(cindent(ed, 2, 0), 0, "jump labels go to column 0");
    assert_eq!(
        cindent(ed, 3, 0),
        4,
        "label does not change the block indent"
    );
    assert_eq!(cindent(ed, 4, 0), 0, "preprocessor lines go to column 0");
    assert_eq!(cindent(ed, 5, 0), 0, "indented preprocessor line too");
    assert_eq!(cindent(ed, 6, 0), 0, "access specifier goes to column 0");
    assert_eq!(cindent(ed, 7, 0), 0);
}

#[test]
fn cindent_continuation_lines_after_open_paren_or_bracket() {
    let session = cindent_session(&[
        "int a = foo(x,",
        "    y);",
        "int b = bar(",
        "        z);",
        "int c = g[",
        "        i];",
    ]);
    let ed = &session.editor;
    assert_eq!(cindent(ed, 2, 0), 4, "one shiftwidth after an open paren");
    assert_eq!(
        cindent(ed, 4, 0),
        8,
        "two shiftwidths when the previous line ends with ("
    );
    assert_eq!(cindent(ed, 6, 0), 8, "same for an open bracket");
}

#[test]
fn cindent_else_and_do_while_align() {
    let session = cindent_session(&[
        "int main() {",
        "    if (a) {",
        "        x;",
        "    }",
        "    else",
        "        y;",
        "    do",
        "        z;",
        "    while (q);",
        "}",
    ]);
    let ed = &session.editor;
    assert_eq!(cindent(ed, 5, 0), 4, "else lines up with its if");
    assert_eq!(cindent(ed, 6, 0), 8, "statement after an unbraced else");
    assert_eq!(cindent(ed, 9, 0), 4, "while of a do lines up with the do");
}

#[test]
fn cindent_respects_common_cinoptions() {
    // '>N' and '}N'
    let mut s = session(&["int main() {", "    x;", "}"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=>3,}2")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 3);
    assert_eq!(cindent(&s.editor, 3, 0), 2);

    // 's' suffix scales by shiftwidth: 2s = 8 columns.
    let mut s = session(&["int main() {", "    x;"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=>2s")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 8);

    // 'cino=(0' lines the continuation up with the character after the '('
    // (Vim: cindent() == 12 for "int b = bar(").
    let mut s = session(&["int b = bar(", "        z);"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=(0")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 12);

    // '+N' sets the ordinary continuation amount (2s = 8 columns; the 's'
    // spelling also keeps :set from parsing the value as a number).
    let mut s = session(&["int a = foo(x,", "    y);"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=+2s")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 8);

    // ':N' and '=N' move case labels and their bodies.
    let mut s = session(&["switch (v) {", "    case 1:", "        a;"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=:0,=2")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 0);
    assert_eq!(cindent(&s.editor, 3, 0), 2);

    // '#N' indents preprocessor lines by N.
    let mut s = session(&["# define X 1"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=#1")
        .unwrap();
    assert_eq!(cindent(&s.editor, 1, 0), 1);

    // 'L' shifts jump labels left.
    let mut s = session(&["int main() {", "label:"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=L2")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 2);

    // 'g0' keeps access specifiers at the class indent (0).
    let mut s = session(&["class A {", "public:", "    int x;"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=g0")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 0);
    assert_eq!(cindent(&s.editor, 3, 0), 4);

    // 'N' adds indent inside a namespace.
    let mut s = session(&["namespace ns {", "    x;"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=N2")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 6);

    // '^N' positions the body of a brace in column 0.
    let mut s = session(&["{", "x;"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=^2")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 6);

    // 't' indents a function type specifier line.
    let mut s = session(&["static", "foo(void)"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=t2")
        .unwrap();
    assert_eq!(cindent(&s.editor, 1, 0), 2);

    // 'p' indents K&R parameter lines.
    let mut s = session(&["int main()", "int argc;"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=p2")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 2);

    // 'U' makes every unclosed paren use the unclosed amount: with (8 the
    // ordinary continuation amount wins (4), with U1 the 8 columns do.
    let mut s = session(&["int a = foo(x,", "    y);"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=(8")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 4);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=(8,U1")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 8);

    // 'u' adds the nested-paren amount for each extra unclosed paren.
    let mut s = session(&["int a = f(g(", "        z);"]);
    s.ex("set cindent shiftwidth=4 expandtab cinoptions=u0")
        .unwrap();
    assert_eq!(cindent(&s.editor, 2, 0), 8);
}

// ---------------------------------------------------- insert-mode wiring

#[test]
fn cindent_indents_a_new_line_opened_with_o() {
    let mut tui = Tui::new(session(&["int main() {"]));
    screen(
        &mut tui,
        ":set cindent shiftwidth=4 expandtab<CR>o<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "    ");
}

#[test]
fn cindent_indents_a_line_split_with_cr() {
    let mut tui = Tui::new(session(&["int main() {"]));
    screen(
        &mut tui,
        ":set cindent shiftwidth=4 expandtab<CR>A<CR>x<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "    x");
}

#[test]
fn cindent_dedents_a_typed_closing_brace_and_preprocessor_hash() {
    let mut tui = Tui::new(session(&["int main() {", "    x;"]));
    screen(
        &mut tui,
        ":set cindent shiftwidth=4 expandtab<CR>o}<Esc>",
        40,
        8,
    );
    assert_eq!(
        buffer_lines(&tui)[1],
        "}",
        "typed closing brace is dedented"
    );

    let mut tui = Tui::new(session(&["int main() {"]));
    screen(
        &mut tui,
        ":set cindent shiftwidth=4 expandtab<CR>o#define X<Esc>",
        40,
        8,
    );
    assert_eq!(
        buffer_lines(&tui)[1],
        "#define X",
        "typed # goes to column 0"
    );
}

#[test]
fn cindent_uses_tabs_when_expandtab_is_off() {
    let mut tui = Tui::new(session(&["int main() {"]));
    screen(
        &mut tui,
        ":set cindent shiftwidth=8 noexpandtab tabstop=8<CR>o<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "\t");
}

#[test]
fn cindent_with_shiftwidth_zero_uses_tabstop() {
    let mut tui = Tui::new(session(&["int main() {"]));
    screen(
        &mut tui,
        ":set cindent shiftwidth=0 tabstop=4 expandtab<CR>o<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "    ");
}

// ---------------------------------------------------------- smartindent

#[test]
fn smartindent_after_open_brace_and_cinwords() {
    let mut tui = Tui::new(session(&["if (x) {"]));
    screen(
        &mut tui,
        ":set smartindent shiftwidth=4 expandtab<CR>o<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "    ");

    let mut tui = Tui::new(session(&["if (x)"]));
    screen(
        &mut tui,
        ":set smartindent shiftwidth=4 expandtab<CR>o<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "    ", "first word is a cinword");
}

#[test]
fn smartindent_dedents_brace_and_left_aligns_hash() {
    let mut tui = Tui::new(session(&["int main() {", "    x;"]));
    screen(
        &mut tui,
        ":set smartindent shiftwidth=4 expandtab<CR>o}<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "}");

    let mut tui = Tui::new(session(&["if (x) {"]));
    screen(
        &mut tui,
        ":set smartindent shiftwidth=4 expandtab<CR>o#define X<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "#define X");
}

#[test]
fn smartindent_copies_the_previous_indent_when_nothing_opens_a_block() {
    let mut tui = Tui::new(session(&["    indented"]));
    screen(
        &mut tui,
        ":set smartindent shiftwidth=4 expandtab<CR>o<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "    ");

    // A cinword that ends the statement does not indent the next line.
    let mut tui = Tui::new(session(&["if (x);"]));
    screen(
        &mut tui,
        ":set smartindent shiftwidth=4 expandtab<CR>o<Esc>",
        40,
        8,
    );
    assert_eq!(buffer_lines(&tui)[1], "");
}

// ---------------------------------------------------------- autoindent

#[test]
fn autoindent_keeps_the_previous_indent() {
    let mut tui = Tui::new(session(&["    indented"]));
    screen(&mut tui, ":set autoindent<CR>o<Esc>", 40, 8);
    assert_eq!(buffer_lines(&tui)[1], "    ");

    let mut tui = Tui::new(session(&["    indented"]));
    screen(&mut tui, ":set autoindent<CR>ohello<Esc>", 40, 8);
    assert_eq!(buffer_lines(&tui)[1], "    hello");
}

#[test]
fn no_indent_option_returns_none() {
    let session = session(&["x"]);
    assert_eq!(new_line_indent(&session.editor, 0), None);
}
