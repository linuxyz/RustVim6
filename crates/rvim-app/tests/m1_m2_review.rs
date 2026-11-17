//! Regression tests for the M1/M2 review pass.
//!
//! Each case mirrors a concrete vim behaviour that the first M1/M2 delivery got
//! wrong; the reference location is quoted next to the test.

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

fn messages(s: &Session) -> Vec<String> {
    s.message_history().to_vec()
}

// --------------------------------------------------- error accounting (M1)

#[test]
fn script_errors_are_counted_even_when_no_command_fails() {
    // vim's emsg() sets ex_exitval = 1 (message.c:729). Errors raised inside a
    // sourced script continue the script, so they never surface as `Err` from
    // Session::ex — the editor-wide counter must still see them.
    let path = std::env::temp_dir().join("rvim_review_bad_script.vim");
    std::fs::write(&path, "let g:ok = 1\nbogus_command_here\nlet g:after = 2\n").unwrap();

    let mut s = Session::new();
    let outcome = s.source(&path);
    assert!(outcome.is_ok(), "sourcing keeps running past a bad line");
    assert_eq!(s.report.error_count, 1);
    assert_eq!(s.editor.error_count, 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn clean_script_leaves_error_count_zero() {
    let path = std::env::temp_dir().join("rvim_review_good_script.vim");
    std::fs::write(&path, "let g:ok = 1\nlet g:after = 2\n").unwrap();

    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(s.report.error_count, 0);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn errors_are_counted_once_per_failure() {
    // The session used to count an error in Session::ex *and* again in the
    // binary's start-up code.
    let mut s = Session::new();
    assert!(s.ex("boguscmd").is_err());
    assert_eq!(s.report.error_count, 1);
    assert_eq!(s.editor.error_count, 1);
}

#[test]
fn silent_bang_swallows_the_error() {
    // :silent! sets emsg_silent, and emsg() returns before ex_exitval = 1
    // (message.c:700-729): a suppressed error must not affect the exit status.
    let mut s = Session::new();
    let out = s.ex("silent! boguscmd").unwrap();
    assert!(matches!(out, rvim_ex::ExOutcome::Continue));
    assert_eq!(s.report.error_count, 0);

    // Plain :silent still reports the error and counts it.
    assert!(s.ex("silent boguscmd").is_err());
    assert_eq!(s.report.error_count, 1);
}

// ------------------------------------------- ranges without a command (M1/M2)

#[test]
fn bare_line_number_jumps() {
    // ":3" jumps to line 3 (ex_docmd.c:1958-1971, ex_range_without_command).
    let mut s = Session::new();
    seed(&mut s, &["one", "two", "three"]);
    s.ex("3").unwrap();
    assert_eq!(s.editor.cursor().line, 2);
    assert_eq!(s.editor.cursor().col, 0);
}

#[test]
fn line_range_prints_only_in_ex_mode() {
    // vim prints for `:1,2` only when exmode_active; otherwise it jumps.
    let mut s = Session::new();
    seed(&mut s, &["one", "two", "three"]);

    s.ex("1,2").unwrap();
    assert!(messages(&s).is_empty(), "no printing outside Ex mode");
    assert_eq!(s.editor.cursor().line, 1);

    s.editor.exmode = true;
    s.ex("1,2").unwrap();
    assert_eq!(messages(&s), vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn trailing_bar_prints_then_runs_next_command() {
    let mut s = Session::new();
    seed(&mut s, &["one", "two", "three"]);
    s.ex("3|p").unwrap();
    // ":3|p" prints line 3 for the range, then ":p" prints the current line.
    assert_eq!(messages(&s), vec!["three".to_string(), "three".to_string()]);
    assert_eq!(s.editor.cursor().line, 2);
}

#[test]
fn line_number_past_end_is_clamped() {
    let mut s = Session::new();
    seed(&mut s, &["one", "two"]);
    s.ex("99").unwrap();
    assert_eq!(s.editor.cursor().line, 1);
}

// ------------------------------------------------- expression bars (M1/M2)

#[test]
fn expression_commands_split_at_unquoted_bar() {
    // `:echo` has EX_EXPR_ARG but no EX_TRLBAR: the expression ends at the bar.
    let mut s = Session::new();
    s.ex("echo 'one' | echo 'two'").unwrap();
    assert_eq!(messages(&s), vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn bar_inside_strings_is_not_split() {
    let mut s = Session::new();
    s.ex("echo 'a|b' | echo 'c'").unwrap();
    assert_eq!(messages(&s), vec!["a|b".to_string(), "c".to_string()]);
}

#[test]
fn let_allows_a_trailing_command() {
    let mut s = Session::new();
    s.ex("let g:x = 1 | echo g:x").unwrap();
    assert_eq!(messages(&s), vec!["1".to_string()]);
}

// ------------------------------------------------- canonical command names

#[test]
fn abbreviated_commands_keep_their_family_semantics() {
    // Handlers used to compare the typed word, so abbreviations lost behaviour
    // that the full name had (`:v` vs `:vglobal`, `:le` vs `:left`).
    let run = |cmd: &str| {
        let mut s = Session::new();
        seed(&mut s, &["alpha", "beta", "alnum"]);
        s.ex(cmd).unwrap();
        text(&s)
    };
    assert_eq!(run("v/al/d"), run("vglobal/al/d"));
    assert_eq!(run("v/al/d"), "alpha\nalnum");

    let mut s = Session::new();
    seed(&mut s, &["   indented"]);
    s.ex("le").unwrap();
    assert_eq!(text(&s), "indented", ":le is :left, not a centre-align");
}

// ------------------------------------------------------------ script runner

#[test]
fn source_keeps_bars_in_autocmd_bodies() {
    // CMD_autocmd has EX_NOTRLCOM but no EX_TRLBAR (ex_cmds.h:170), so the bar
    // belongs to the autocommand text and is expanded when it runs.
    let mut s = Session::new();
    s.ex("autocmd BufEnter * echo 'one'|echo 'two'").unwrap();
    s.ex("doautocmd BufEnter").unwrap();
    assert_eq!(messages(&s), vec!["one".to_string(), "two".to_string()]);
}
