//! Golden/feature tests driving the whole stack through rvim-app::Session,
//! the same entry point the "rvim -es" binary uses. Each case is a small
//! Vim-script-like command sequence with asserted observable outcomes — the
//! pattern that tests/vim_compat will later replay against real vim.
//!
//! Coverage groups (PLAN M1 exit criteria: >= 20 golden cases):
//!   options / editing+undo / registers / ranges / substitute / persistence /
//!   buffers / normal-mode engine / error semantics.

use rvim_app::Session;
use rvim_editor::{Mode, Register};

fn run(cmds: &[&str]) -> Session {
    let mut s = Session::new();
    s.run_script(&cmds.iter().map(|c| c.to_string()).collect::<Vec<_>>());
    s
}

/// Fill the current buffer with `lines`, starting from the empty default line.
fn seed(s: &mut Session, lines: Vec<&str>) {
    s.editor
        .replace_lines(0, 1, lines.into_iter().map(String::from).collect())
        .unwrap();
}

/// Unique temp path for a persistence test; the file is removed first.
fn temp_path(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("rvim_golden_{tag}.txt"));
    let _ = std::fs::remove_file(&p);
    p
}

// ---------------------------------------------------------------- options

#[test]
fn set_options_and_query_them() {
    let s = run(&["set number tabstop=4 expandtab", "set ts?"]);
    assert!(s.editor.options.bool_or("number", false));
    assert!(s.editor.options.bool_or("expandtab", false));
    assert_eq!(s.editor.options.num_or("tabstop", 0), 4);
    assert!(s.message_history().iter().any(|m| m == "tabstop=4"));
}

#[test]
fn nocompatible_is_the_only_state() {
    let s = run(&["set nocompatible"]);
    assert!(!s.editor.options.bool_or("compatible", true));
    // Trying to enable vi-compatible mode is rejected, not silently accepted.
    let mut s2 = Session::new();
    let err = s2.ex("set compatible").unwrap_err();
    assert_eq!(err.code, "ERV010");
}

#[test]
fn set_reset_and_short_name_forms() {
    let s = run(&[
        "set ts=4",
        "set ts&", // reset to default (8)
        "set nonumber",
        "set et?",
    ]);
    assert_eq!(s.editor.options.num_or("tabstop", 0), 8);
    assert!(!s.editor.options.bool_or("number", true));
    // et was never set -> renders as noexpandtab
    assert!(
        s.message_history().iter().any(|m| m == "noexpandtab"),
        "history: {:?}",
        s.message_history()
    );
}

#[test]
fn unknown_option_reports_vim_error() {
    let mut s = Session::new();
    let err = s.ex("set bogusoption").unwrap_err();
    assert_eq!(err.code, "E518");
    assert_eq!(s.report.error_count, 1);
}

// ------------------------------------------------------- editing and undo

#[test]
fn line_edits_with_undo_redo() {
    let mut s = Session::new();
    s.ex("set ff=unix").unwrap();
    s.editor.set_line_text(0, "one".to_string()).unwrap();
    s.editor
        .replace_lines(1, 1, vec!["two".to_string(), "three".to_string()])
        .unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "one\ntwo\nthree");
    s.ex("2d").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "one\nthree");
    s.ex("u").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "one\ntwo\nthree");
    s.ex("redo").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "one\nthree");
}

#[test]
fn deleting_every_line_keeps_one_empty_line() {
    let mut s = Session::new();
    seed(&mut s, vec!["a", "b"]);
    s.ex("1,$d").unwrap();
    assert_eq!(s.editor.line_count(), 1);
    assert_eq!(s.editor.current_buffer().full_text(), "");
    // The deletion is still undoable.
    s.ex("u").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "a\nb");
}

#[test]
fn copy_and_move_range_commands() {
    let mut s = Session::new();
    seed(&mut s, vec!["a", "b", "c"]);
    // :1co3 duplicates line 1 after line 3.
    s.ex("1co3").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "a\nb\nc\na");
    // :2m4 moves line 2 to after line 4.
    let mut s2 = Session::new();
    seed(&mut s2, vec!["a", "b", "c"]);
    s2.ex("2m3").unwrap();
    assert_eq!(s2.editor.current_buffer().full_text(), "a\nc\nb");
}

#[test]
fn new_edit_truncates_redo_branch() {
    let mut s = Session::new();
    seed(&mut s, vec!["abc"]);
    s.editor.set_line_text(0, "abcX".to_string()).unwrap();
    s.ex("u").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "abc");
    // A fresh edit after undo discards the redoable future.
    s.editor.set_line_text(0, "abcY".to_string()).unwrap();
    s.ex("redo").unwrap_err();
    assert_eq!(s.editor.current_buffer().full_text(), "abcY");
    s.ex("u").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "abc");
}

// ------------------------------------------------------------- registers

#[test]
fn yank_delete_put_register_flow() {
    let mut s = Session::new();
    seed(&mut s, vec!["a", "b", "c", "d"]);
    s.ex("1,2y").unwrap();
    assert_eq!(s.editor.registers.get('0').unwrap().text, "a\nb\n");
    s.ex("$pu").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "a\nb\nc\nd\na\nb");
    s.ex("1d").unwrap();
    // line-wise delete shifts numbered registers
    assert_eq!(s.editor.registers.get('1').unwrap().text, "a\n");
}

#[test]
fn named_register_isolation_and_black_hole() {
    let mut s = Session::new();
    seed(&mut s, vec!["alpha", "beta", "gamma"]);
    s.ex("1y a").unwrap();
    assert_eq!(
        s.editor.registers.get('a').unwrap().text,
        "alpha\n",
        "named register a holds the yanked line"
    );
    // A named yank leaves register 0 alone (only unnamed yanks fill it).
    assert!(s.editor.registers.get('0').is_none());
    assert_eq!(
        s.editor.registers.unnamed().unwrap().text,
        "alpha\n",
        "a named yank still updates the unnamed register"
    );
    // An unnamed yank fills register 0; a black-hole yank must not.
    s.ex("1y").unwrap();
    assert_eq!(s.editor.registers.get('0').unwrap().text, "alpha\n");
    s.ex("2y _").unwrap();
    assert!(s.editor.registers.get('_').is_none());
    assert_eq!(
        s.editor.registers.get('0').unwrap().text,
        "alpha\n",
        "black-hole yank must not overwrite register 0"
    );
    // Black-hole delete shifts no numbered register and leaves unnamed alone.
    let unnamed_before = s.editor.registers.unnamed().cloned();
    s.ex("1d _").unwrap();
    assert!(s.editor.registers.get('1').is_none());
    assert_eq!(s.editor.registers.unnamed().cloned(), unnamed_before);
    assert_eq!(s.editor.current_buffer().full_text(), "beta\ngamma");
}

#[test]
fn charwise_delete_uses_dash_register() {
    use rvim_normal::NormalEngine;
    let mut s = Session::new();
    seed(&mut s, vec!["abcdef"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, 'x').unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "bcdef");
    // Small (char-wise) delete lands in the '-' register, not numbered ones.
    assert_eq!(s.editor.registers.get('-').unwrap().text, "a");
    assert!(s.editor.registers.get('1').is_none());
}

// --------------------------------------------------------------- ranges

#[test]
fn print_variants_emit_expected_messages() {
    let s = run(&[
        "set ff=unix",
        "call ignored", // error first, proves later commands still run
        "p",
    ]);
    // The bad command is counted once and the following :p still executes,
    // printing the single empty default line into history.
    assert_eq!(s.report.error_count, 1);
    assert!(
        s.message_history().iter().any(|m| m.is_empty()),
        ":p on the empty default buffer records an empty line: {:?}",
        s.message_history()
    );

    let mut s2 = Session::new();
    seed(&mut s2, vec!["one", "two", "three"]);
    s2.ex("2,3nu").unwrap();
    let hist = s2.message_history();
    assert!(hist.iter().any(|m| m.contains("2") && m.contains("two")));
    assert!(hist.iter().any(|m| m.contains("3") && m.contains("three")));
    s2.ex("1l").unwrap();
    assert!(
        s2.message_history().iter().any(|m| m == "one$"),
        ":list appends the trailing $ marker"
    );
}

#[test]
fn whole_buffer_range_percent_and_offsets() {
    let mut s = Session::new();
    seed(&mut s, vec!["a", "b", "c", "d"]);
    // %d clears everything (covered by the empty-line invariant elsewhere);
    // here verify offset ranges address the expected slice via :y.
    s.ex("1,+2y").unwrap();
    assert_eq!(s.editor.registers.get('0').unwrap().text, "a\nb\nc\n");
    s.ex(".,$y").unwrap();
    // Cursor stays on line 1; ". , $" therefore yanks the whole buffer.
    assert_eq!(s.editor.registers.get('0').unwrap().text, "a\nb\nc\nd\n");
}

// ----------------------------------------------------------- substitute

#[test]
fn substitute_respects_range_and_global_flag() {
    let mut s = Session::new();
    seed(&mut s, vec!["foo", "foo foo", "bar"]);
    s.ex("1s/foo/FOO/").unwrap();
    assert_eq!(s.editor.current_buffer().line_str(0).unwrap(), "FOO");
    s.ex("2s/foo/FOO/g").unwrap();
    assert_eq!(s.editor.current_buffer().line_str(1).unwrap(), "FOO FOO");
    // untouched third line
    assert_eq!(s.editor.current_buffer().line_str(2).unwrap(), "bar");
}

#[test]
fn substitute_without_g_replaces_first_match_only() {
    let mut s = Session::new();
    seed(&mut s, vec!["x x x"]);
    s.ex("%s/x/Y/").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "Y x x");
}

#[test]
fn substitute_honours_ignorecase_option() {
    let mut s = Session::new();
    seed(&mut s, vec!["Cat cat CAT"]);
    s.ex("set ignorecase").unwrap();
    s.ex("%s/cat/dog/g").unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "dog dog dog");
}

// ------------------------------------------------------------ persistence

#[test]
fn write_quit_roundtrip_on_disk() {
    let path = temp_path("wq");
    let mut s = Session::new();
    seed(&mut s, vec!["hello", "world"]);
    let out = s.ex(&format!("set ff=unix|wq {}", path.display())).unwrap();
    assert!(matches!(out, rvim_ex::ExOutcome::Quit { .. }));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\nworld\n");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dos_fileformat_writes_crlf() {
    let path = temp_path("crlf");
    let mut s = Session::new();
    seed(&mut s, vec!["one", "two"]);
    s.ex("set ff=dos").unwrap();
    s.ex(&format!("w {}", path.display())).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "one\r\ntwo\r\n",
        "ff=dos must persist CRLF terminators"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_command_inserts_file_after_addressed_line() {
    let path = temp_path("read");
    std::fs::write(&path, "in1\nin2\n").unwrap();
    let mut s = Session::new();
    seed(&mut s, vec!["top", "bottom"]);
    s.ex(&format!("1r {}", path.display())).unwrap();
    assert_eq!(
        s.editor.current_buffer().full_text(),
        "top\nin1\nin2\nbottom"
    );
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------- buffers

#[test]
fn quit_outcome_and_buffer_listing() {
    let mut s = Session::new();
    s.editor.set_line_text(0, "hello".to_string()).unwrap();
    let out = s.ex("ls").unwrap();
    assert_eq!(out, rvim_ex::ExOutcome::Continue);
    assert!(
        s.message_history()
            .iter()
            .any(|m| !m.contains("hello") && m.contains('1'))
    );
    let quit = s.ex("q!").unwrap();
    assert!(matches!(quit, rvim_ex::ExOutcome::Quit { .. }));
}

#[test]
fn multiple_buffers_switch_and_list() {
    let mut s = Session::new();
    let b2 = s.editor.new_buffer();
    s.editor.switch_buffer(b2).unwrap();
    s.editor.set_line_text(0, "second".to_string()).unwrap();
    assert_eq!(s.editor.buffer_count(), 2);
    assert_eq!(s.editor.current_bufnr(), b2);
    // bprevious wraps around the ring of buffers.
    s.ex("bp").unwrap();
    assert_eq!(s.editor.current_bufnr(), 1);
    s.ex("bn").unwrap();
    assert_eq!(s.editor.current_bufnr(), b2);
    s.ex("ls").unwrap();
    let listing = s.message_history().join("\n");
    // The current buffer is marked '%' and both buffer ids appear once.
    assert!(listing.contains('%'));
    assert!(listing.contains("1"));
    assert!(listing.contains(&b2.to_string()));
}

#[test]
fn bdelete_refuses_when_only_buffer_remains() {
    let mut s = Session::new();
    let err = s.ex("bd").unwrap_err();
    assert_eq!(err.code, "E516");
}

// -------------------------------------------------------- normal-mode engine

#[test]
fn mode_machine_via_normal_engine() {
    use rvim_normal::NormalEngine;
    let mut s = Session::new();
    let mut eng = NormalEngine::new();
    s.editor.set_line_text(0, "abcdef".to_string()).unwrap();
    // "x" deletes one char at cursor
    eng.feed(&mut s.editor, 'x').unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "bcdef");
    // count then motion: "3l"
    let mut eng2 = NormalEngine::new();
    eng2.feed(&mut s.editor, '3').unwrap();
    eng2.feed(&mut s.editor, 'l').unwrap();
    assert_eq!(s.editor.cursor().col, 3);
    // enter insert with "o" opens a new line
    let mut eng3 = NormalEngine::new();
    eng3.feed(&mut s.editor, 'o').unwrap();
    assert_eq!(s.editor.mode, Mode::Insert);
    assert_eq!(s.editor.line_count(), 2);
}

#[test]
fn normal_dd_yy_p_linewise_flow() {
    use rvim_normal::NormalEngine;
    let mut s = Session::new();
    seed(&mut s, vec!["a", "b", "c"]);
    let mut eng = NormalEngine::new();
    eng.feed_pair(&mut s.editor, 'y', 'y').unwrap();
    assert_eq!(s.editor.registers.get('0').unwrap().text, "a\n");
    eng.feed_pair(&mut s.editor, 'd', 'd').unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "b\nc");
    // p pastes the unnamed (deleted) line below the cursor line.
    eng.feed(&mut s.editor, 'p').unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "b\na\nc");
}

#[test]
fn word_motions_and_line_jumps_clamp_at_boundaries() {
    use rvim_normal::NormalEngine;
    let mut s = Session::new();
    seed(&mut s, vec!["alpha beta", "gamma"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, 'w').unwrap();
    assert_eq!(s.editor.cursor().col, 6); // byte offset of "beta"
    eng.feed(&mut s.editor, 'G').unwrap();
    assert_eq!(s.editor.cursor().line, 1);
    // Motion past the end clamps instead of panicking.
    for _ in 0..10 {
        eng.feed(&mut s.editor, 'l').unwrap();
    }
    assert_eq!(s.editor.cursor().line, 1);
    assert!(s.editor.cursor().col <= 5);
}

#[test]
fn mark_set_and_jump() {
    use rvim_normal::NormalEngine;
    let mut s = Session::new();
    seed(&mut s, vec!["line1", "line2", "line3"]);
    let mut eng = NormalEngine::new();
    // move to line 3 then set mark 'a'
    eng.feed(&mut s.editor, 'G').unwrap();
    assert_eq!(s.editor.cursor().line, 2);
    eng.feed(&mut s.editor, 'm').unwrap();
    eng.complete_mark(&mut s.editor, 'a').unwrap();
    // jump back to line 1 and then to mark
    eng.feed_pair(&mut s.editor, 'g', 'g').unwrap();
    assert_eq!(s.editor.cursor().line, 0);
    eng.jump_mark(&mut s.editor, 'a').unwrap();
    assert_eq!(s.editor.cursor().line, 2);
}

#[test]
fn zero_key_is_column_motion_unless_a_count_is_running() {
    use rvim_normal::NormalEngine;
    let mut s = Session::new();
    s.editor.set_line_text(0, "abcdef".to_string()).unwrap();
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, 'l').unwrap();
    eng.feed(&mut s.editor, 'l').unwrap();
    assert_eq!(s.editor.cursor().col, 2);
    // "0" moves to the first column (vim only reads 0 as a count digit once a
    // count has started).
    eng.feed(&mut s.editor, '0').unwrap();
    assert_eq!(s.editor.cursor().col, 0);
    // "10l" is still a count of ten, and stops on the last character.
    let mut eng2 = NormalEngine::new();
    eng2.feed(&mut s.editor, '1').unwrap();
    eng2.feed(&mut s.editor, '0').unwrap();
    eng2.feed(&mut s.editor, 'l').unwrap();
    assert_eq!(s.editor.cursor().col, 5);
}

#[test]
fn operator_counts_apply_to_two_key_commands() {
    use rvim_normal::NormalEngine;
    // `3dd` must delete three lines: the count travels through
    // Mode::MotionPending into feed_pair (review fix).
    let mut s = Session::new();
    seed(&mut s, vec!["one", "two", "three", "four"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, '2').unwrap();
    eng.feed(&mut s.editor, 'd').unwrap();
    eng.feed_pair(&mut s.editor, 'd', 'd').unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "three\nfour");

    // `3gg` goes to line 3 (1-based) = index 2.
    let mut s = Session::new();
    seed(&mut s, vec!["a", "b", "c", "d", "e"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, '3').unwrap();
    eng.feed(&mut s.editor, 'g').unwrap();
    eng.feed_pair(&mut s.editor, 'g', 'g').unwrap();
    assert_eq!(s.editor.cursor().line, 2);
}
#[test]
fn counted_insert_repeats_the_typed_text() {
    // `3ihello<Esc>` inserts "hellohellohello" (vim edit.c ins_esc()).
    use rvim_normal::NormalEngine;
    let mut s = Session::new();
    seed(&mut s, vec!["x"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, '3').unwrap();
    eng.feed(&mut s.editor, 'i').unwrap();
    for ch in "hello".chars() {
        rvim_normal::insert::type_char(&mut s.editor, &mut eng, ch).unwrap();
    }
    eng.finish_insert(&mut s.editor);
    assert_eq!(s.editor.current_buffer().full_text(), "hellohellohellox");

    // Counted `o` opens more lines with the same text; `A`/`I` repeat inline.
    let mut s = Session::new();
    seed(&mut s, vec!["x"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, '3').unwrap();
    eng.feed(&mut s.editor, 'o').unwrap();
    for ch in "abc".chars() {
        rvim_normal::insert::type_char(&mut s.editor, &mut eng, ch).unwrap();
    }
    eng.finish_insert(&mut s.editor);
    assert_eq!(s.editor.current_buffer().full_text(), "x\nabc\nabc\nabc");

    let mut s = Session::new();
    seed(&mut s, vec!["x"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, '3').unwrap();
    eng.feed(&mut s.editor, 'A').unwrap();
    rvim_normal::insert::type_char(&mut s.editor, &mut eng, '!').unwrap();
    eng.finish_insert(&mut s.editor);
    assert_eq!(s.editor.current_buffer().full_text(), "x!!!");
}

#[test]
fn one_undo_step_per_insert_session() {
    use rvim_normal::NormalEngine;
    // vim records a single undo block for an insert session, so one `u`
    // restores the whole text (and a counted insert is also one block).
    let mut s = Session::new();
    seed(&mut s, vec!["abc"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, 'i').unwrap();
    for ch in "XY".chars() {
        rvim_normal::insert::type_char(&mut s.editor, &mut eng, ch).unwrap();
    }
    eng.finish_insert(&mut s.editor);
    assert_eq!(s.editor.current_buffer().full_text(), "XYabc");
    s.editor.undo().unwrap();
    assert_eq!(
        s.editor.current_buffer().full_text(),
        "abc",
        "one undo step"
    );

    let mut s = Session::new();
    seed(&mut s, vec!["abc"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, '3').unwrap();
    eng.feed(&mut s.editor, 'i').unwrap();
    rvim_normal::insert::type_char(&mut s.editor, &mut eng, 'X').unwrap();
    eng.finish_insert(&mut s.editor);
    assert_eq!(s.editor.current_buffer().full_text(), "XXXabc");
    s.editor.undo().unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "abc");
    s.editor.redo().unwrap();
    assert_eq!(s.editor.current_buffer().full_text(), "XXXabc", "redo");
}
// --------------------------------------------------------- error semantics

#[test]
fn errors_are_counted_but_do_not_abort_the_script() {
    let mut s = Session::new();
    seed(&mut s, vec!["x"]);
    let report = s.run_script(&[
        "boguscmd".to_string(),      // E492, counted
        "set tabstop=3".to_string(), // still executed afterwards
    ]);
    assert_eq!(report.error_count, 1);
    assert_eq!(s.editor.options.num_or("tabstop", 0), 3);
}

#[test]
fn dirty_buffer_blocks_plain_quit_but_force_quit_wins() {
    let mut s = Session::new();
    seed(&mut s, vec!["original"]);
    s.editor.set_line_text(0, "changed".to_string()).unwrap();
    let err = s.ex("q").unwrap_err();
    assert_eq!(err.code, "E37");
    let forced = s.ex("q!").unwrap();
    assert!(matches!(
        forced,
        rvim_ex::ExOutcome::Quit { bang: true, .. }
    ));
}

#[test]
fn put_with_empty_register_is_an_error_not_a_crash() {
    let mut s = Session::new();
    seed(&mut s, vec!["only"]);
    let err = s.ex("pu").unwrap_err();
    assert_eq!(err.code, "E353");
    assert_eq!(s.editor.current_buffer().full_text(), "only");
}

#[test]
fn paste_helper_register_types_are_kept_distinct() {
    // Sanity check on the register model the Ex/normal paths share.
    let line = Register::linewise("l");
    let ch = Register::charwise("c");
    assert_ne!(line.reg_type, ch.reg_type);
    assert_eq!(line.text, "l");
    assert_eq!(ch.text, "c");
}
