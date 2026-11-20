//! End-to-end checks that the vimregex.com patterns work through `:s` and
//! `:g` (the engine-level tests live in `crates/rvim-viml/tests/vimregex.rs`).

use rvim_app::Session;

fn seed(s: &mut Session, lines: &[&str]) {
    s.editor
        .replace_lines(
            0,
            1,
            lines.iter().map(|l| l.to_string()).collect::<Vec<String>>(),
        )
        .unwrap();
}

fn text(s: &Session) -> String {
    s.editor.current_buffer().full_text()
}

#[test]
fn substitute_respects_word_boundaries() {
    let mut s = Session::new();
    seed(&mut s, &["vi is vi", "a navigator"]);
    s.ex(r#"s/\<vi\>/VIM/g"#).unwrap();
    assert_eq!(text(&s), "VIM is VIM\na navigator");
}

#[test]
fn substitute_non_greedy_matches_the_first_quoted_string() {
    let mut s = Session::new();
    seed(&mut s, &["a \"one\" b \"two\" c"]);
    s.ex(r#"s/".\{-\}"/X/"#).unwrap();
    assert_eq!(text(&s), "a X b \"two\" c");
}

#[test]
fn global_deletes_blank_lines_with_a_class() {
    let mut s = Session::new();
    seed(&mut s, &["one", "", "  ", "two"]);
    s.ex(r"g/^\s*$/d").unwrap();
    assert_eq!(text(&s), "one\ntwo");
}

#[test]
fn substitute_can_use_the_extended_classes() {
    let mut s = Session::new();
    seed(&mut s, &["abc 123 def", "abc xyz def"]);
    s.ex(r"s/\d\+/N/g").unwrap();
    assert_eq!(text(&s), "abc N def\nabc xyz def");
    let mut s = Session::new();
    // \x is the hex-digit class: E, ff and 17 are hex digits, X, H and the
    // letters of "oct" are not.
    seed(&mut s, &["HEX ff, oct 17"]);
    s.ex(r"s/\x\+/X/g").unwrap();
    assert_eq!(text(&s), "HXX X, oXt X");
}
