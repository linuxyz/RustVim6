//! M6 substitute replacement syntax: case modifiers, \n, \= and groups.

use rvim_app::Session;

fn run(lines: &[&str], command: &str) -> String {
    let mut s = Session::new();
    s.editor
        .replace_lines(
            0,
            1,
            lines
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<String>>(),
        )
        .unwrap();
    s.ex(command).unwrap();
    s.editor.current_buffer().full_text()
}

#[test]
fn case_modifiers_apply_to_one_character_or_the_rest() {
    assert_eq!(run(&["hello"], r"s/hello/\u&/"), "Hello");
    assert_eq!(run(&["hello"], r"s/hello/\U&/"), "HELLO");
    assert_eq!(run(&["HELLO"], r"s/HELLO/\L&/"), "hello");
    assert_eq!(run(&["HELLO"], r"s/HELLO/\l&/"), "hELLO");
    assert_eq!(run(&["hello"], r"s/hello/\U&\E!/"), "HELLO!");
}

#[test]
fn replacement_newline_splits_the_line() {
    assert_eq!(run(&["a-b"], r"s/-/\n/"), "a\nb");
}

#[test]
fn expression_replacement_is_evaluated() {
    assert_eq!(run(&["abc"], r"s/\(b\)/X/"), "aXc");
    assert_eq!(run(&["abc"], r"s/abc/\=1+1/"), "2");
    assert_eq!(run(&["abc"], r#"s/abc/\='[' . 'x' . ']'/"#), "[x]");
    assert_eq!(run(&["abc"], r"s/\(b\)/\=submatch(1).submatch(1)/"), "abbc");
    assert_eq!(run(&["abc"], r"s/\(b\)/\=submatch(0)/"), "abc");
    assert_eq!(run(&["abc"], r"s/\(b\)/X/"), "aXc");
}

#[test]
fn group_backreferences_and_whole_match() {
    assert_eq!(run(&["ab"], r"s/\(a\)\(b\)/\2\1/"), "ba");
    assert_eq!(run(&["abc"], r"s/b/[&]/"), "a[b]c");
    assert_eq!(run(&["abc"], r"s/b/\0\0/"), "abbc");
}
