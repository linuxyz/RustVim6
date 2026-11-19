//! M4 TUI tests: key loop, modes, command line, mappings and rendering.
//!
//! Each test drives [`Tui`] with scripted keys (the same path the interactive
//! binary uses) and asserts on the rendered plain-text screen, so the front-end
//! is verified without a terminal.

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_term::{KeySource as _, ScriptedKeys};

/// Seed a buffer from a temp file so it starts unmodified (like `vim file`).
fn open(lines: &[&str]) -> Session {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("rvim_tui_{unique}.txt"));
    std::fs::write(&path, lines.join("\n")).unwrap();
    let mut session = Session::new();
    session.open_file(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    session
}

fn open_file(name: &str, contents: &str) -> (Session, std::path::PathBuf) {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, contents).unwrap();
    let mut session = Session::new();
    session.open_file(&path).unwrap();
    (session, path)
}

/// Feed key notation and return the final screen (one string per row).
fn screen(tui: &mut Tui, notation: &str, cols: usize, rows: usize) -> Vec<String> {
    let mut keys = ScriptedKeys::from_notation(notation);
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, cols, rows).unwrap();
    tui.screen_text(cols, rows)
        .split('\n')
        .map(str::to_string)
        .collect()
}

// ------------------------------------------------------------- rendering

#[test]
fn initial_screen_shows_buffer_statusline_and_end_of_buffer() {
    let session = open(&["alpha", "beta"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "", 44, 6);
    assert_eq!(rows[0], "alpha");
    assert_eq!(rows[1], "beta");
    assert_eq!(rows[2], "~", "lines past EOF show ~");
    assert!(rows[4].contains("1,1"), "statusline: {:?}", rows[4]);
    assert!(rows[4].contains("NORMAL"), "mode indicator: {:?}", rows[4]);
}

#[test]
fn number_option_adds_a_gutter() {
    let session = open(&["alpha", "beta"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":set number<CR>", 30, 6);
    assert_eq!(rows[0], "1 alpha");
    assert_eq!(rows[1], "2 beta");
}

#[test]
fn long_lines_scroll_horizontally_to_keep_the_cursor_visible() {
    let text = "0123456789abcdefghij";
    let session = open(&[text]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "$", 10, 4);
    assert!(rows[0].chars().count() <= 10, "never wider than the window");
    assert!(
        text.ends_with(rows[0].as_str()) || text.contains(rows[0].as_str()),
        "shows a window near the end: {:?}",
        rows[0]
    );
    assert!(tui.session.editor.cursor().col > 10, "cursor moved right");
}

#[test]
fn vertical_scroll_follows_the_cursor() {
    let lines: Vec<String> = (1..=20).map(|n| format!("line{n}")).collect();
    let mut session = Session::new();
    session.editor.replace_lines(0, 1, lines.clone()).unwrap();
    let mut tui = Tui::new(session);
    // Window of 6 rows => 4 text rows. "G" jumps to the last line.
    let rows = screen(&mut tui, "G", 30, 6);
    assert_eq!(rows[3], "line20", "top scrolled so the cursor is visible");
}

// ---------------------------------------------------------------- modes

#[test]
fn insert_mode_types_text_and_escape_returns_to_normal() {
    let session = open(&["alpha"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "ihello <Esc>", 40, 5);
    assert_eq!(rows[0], "hello alpha");
    assert_eq!(tui.session.editor.mode, rvim_editor::Mode::Normal);
    assert!(rows[3].contains("NORMAL"), "statusline: {:?}", rows[3]);
}

#[test]
fn insert_mode_backspace_and_enter_split_lines() {
    let session = open(&["ab"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "A!<BS><CR>x<Esc>", 20, 5);
    assert_eq!(rows[0], "ab");
    assert_eq!(rows[1], "x");
}

#[test]
fn dd_and_yy_via_two_key_commands() {
    let session = open(&["one", "two", "three"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "dd", 30, 6);
    assert_eq!(rows[0], "two");
    assert_eq!(rows[1], "three");
    assert!(
        tui.session.editor.current_buffer().is_changed(),
        "buffer is modified"
    );
}

// ------------------------------------------------------------ command line

#[test]
fn command_line_runs_ex_commands() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let _rows = screen(&mut tui, ":set tabstop=4<CR>", 30, 5);
    assert_eq!(tui.session.editor.options.num_or("tabstop", 0), 4);
    assert_eq!(tui.session.editor.mode, rvim_editor::Mode::Normal);
}

#[test]
fn command_line_is_rendered_while_editing() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":set nu", 30, 5);
    assert_eq!(
        rows[rows.len() - 1],
        ":set nu",
        "prompt row shows what is typed"
    );
}

#[test]
fn escape_cancels_the_command_line() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":set nu<Esc>", 30, 5);
    assert!(
        !tui.session.editor.options.bool_or("number", false),
        "the command never ran"
    );
    assert_eq!(tui.session.editor.mode, rvim_editor::Mode::Normal);
    assert!(!rows[rows.len() - 1].starts_with(':'), "prompt cleared");
}

#[test]
fn error_messages_are_shown_and_set_the_exit_code() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let mut keys = ScriptedKeys::from_notation(":boguscmd<CR>");
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, 40, 5).unwrap();
    let text = tui.screen_text(40, 5);
    assert!(text.contains("E492"), "screen: {text}");
    assert_eq!(tui.session.report.error_count, 1);
}

#[test]
fn tab_completion_opens_the_wildmenu() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":se<TAB>", 40, 5);
    let menu_row = &rows[rows.len() - 2];
    assert!(menu_row.contains("set"), "wildmenu: {menu_row:?}");
    assert_eq!(rows[rows.len() - 1], ":set", "the match is inserted");
}

// ---------------------------------------------------------------- search

#[test]
fn forward_search_moves_the_cursor() {
    let session = open(&["alpha", "beta", "gamma"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "/beta<CR>", 30, 6);
    assert_eq!(tui.session.editor.cursor().line, 1);
    assert!(rows[4].contains("2,1"), "statusline: {:?}", rows[4]);
}

#[test]
fn search_wraps_and_reports_when_absent() {
    let session = open(&["alpha", "beta"]);
    let mut tui = Tui::new(session);
    // Start on line 2 and search upwards past the end: wraps to line 1.
    let rows = screen(&mut tui, "j/alpha<CR>", 30, 6);
    assert_eq!(tui.session.editor.cursor().line, 0);
    assert_eq!(rows[0], "alpha");

    let session = open(&["alpha"]);
    let mut tui = Tui::new(session);
    let mut keys = ScriptedKeys::from_notation("/zzz<CR>");
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, 30, 5).unwrap();
    assert!(tui.screen_text(30, 5).contains("E486"));
}

// --------------------------------------------------------------- mappings

#[test]
fn mappings_expand_in_normal_mode() {
    let session = open(&["one", "two", "three"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":nmap Q dd<CR>Q", 30, 6);
    assert_eq!(rows[0], "two", "Q ran dd");
    assert_eq!(rows[1], "three");
}

#[test]
fn noremap_is_not_expanded_recursively() {
    let session = open(&["one", "two"]);
    let mut tui = Tui::new(session);
    // "Q" -> "x" and "x" -> "dd": noremap must stop after the first expansion.
    let rows = screen(&mut tui, ":nmap x dd<CR>:nnoremap Q x<CR>Q", 30, 6);
    assert_eq!(rows[0], "ne", "only one character was deleted (x, not dd)");
    assert_eq!(rows[1], "two");
}

// ----------------------------------------------------------- file handling

#[test]
fn writing_from_the_tui_updates_the_file() {
    let (session, path) = open_file("rvim_tui_write.txt", "alpha\nbeta\n");
    let mut tui = Tui::new(session);
    let _ = screen(&mut tui, "dd:w<CR>", 30, 5);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "beta\n");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn quit_and_force_quit_set_the_exit_state() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let mut keys = ScriptedKeys::from_notation(":q<CR>");
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, 30, 5).unwrap();
    assert!(tui.quit, "clean buffer quits");
    assert_eq!(tui.exit_code, 0);

    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let mut keys = ScriptedKeys::from_notation("x:q<CR>");
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, 30, 5).unwrap();
    assert!(!tui.quit, "modified buffer refuses to quit");
    assert!(tui.screen_text(40, 5).contains("E37"));
}

// --------------------------------------------------- review: counts and gg

#[test]
fn gg_jumps_back_to_the_first_line() {
    let lines: Vec<String> = (1..=20).map(|n| format!("line{n}")).collect();
    let mut session = Session::new();
    session.editor.replace_lines(0, 1, lines).unwrap();
    // Start from line 5; `g` alone must not leave the editor in OP-PENDING.
    let mut tui = Tui::new(session);
    let x = rvim_editor::maps::Key::Char('5');
    let g = rvim_editor::maps::Key::Char('G');
    tui.feed_key(x).unwrap();
    tui.feed_key(g).unwrap();
    assert_eq!(tui.session.editor.cursor().line, 4, "5G");
    let mut keys = ScriptedKeys::from_notation("gg");
    while let Some(key) = keys.next_key().unwrap() {
        tui.feed_key(key).unwrap();
    }
    assert_eq!(tui.session.editor.cursor().line, 0, "gg returns to line 1");
    assert_eq!(tui.session.editor.mode, rvim_editor::Mode::Normal);
}

#[test]
fn counts_repeat_operators_and_gg() {
    let lines: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
    let mut session = Session::new();
    session.editor.replace_lines(0, 1, lines).unwrap();
    let mut tui = Tui::new(session);
    // 3dd deletes three lines.
    let rows = screen(&mut tui, "3dd", 20, 7);
    assert_eq!(rows[0], "line4", "three lines were deleted");
    assert_eq!(rows[1], "line5");

    // 2x deletes two characters.
    let session = open(&["abcdef"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "2x", 20, 5);
    assert_eq!(rows[0], "cdef");

    // 3gg jumps to line 3.
    let lines: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
    let mut session = Session::new();
    session.editor.replace_lines(0, 1, lines).unwrap();
    let mut tui = Tui::new(session);
    screen(&mut tui, "3gg", 20, 6);
    assert_eq!(tui.session.editor.cursor().line, 2);
}

// --------------------------------------------------------------- resizing

#[test]
fn resizing_the_terminal_changes_the_screen_geometry() {
    use std::collections::VecDeque;
    use std::io;

    struct ResizingSource {
        keys: VecDeque<rvim_editor::maps::Key>,
        sizes: VecDeque<(usize, usize)>,
    }
    impl rvim_term::KeySource for ResizingSource {
        fn next_key(&mut self) -> io::Result<Option<rvim_editor::maps::Key>> {
            Ok(self.keys.pop_front())
        }
        fn size(&mut self) -> Option<(usize, usize)> {
            // Report the next size on every poll so the loop sees a change.
            if self.sizes.len() > 1 {
                self.sizes.pop_front()
            } else {
                self.sizes.front().copied()
            }
        }
    }

    let session = open(&["alpha", "beta"]);
    let mut tui = Tui::new(session);
    let j = rvim_editor::maps::Key::Char('j');
    let mut source = ResizingSource {
        keys: VecDeque::from(vec![j.clone(), j]),
        sizes: VecDeque::from(vec![(40, 10), (20, 6)]),
    };
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut source, &mut sink, 40, 10).unwrap();
    assert_eq!(tui.screen_size, (20, 6), "the TUI followed the resize");
    let screen = tui.screen_text(20, 6);
    // `split` (not `lines`) so a trailing empty row still counts.
    assert_eq!(
        screen.split('\n').count(),
        6,
        "frame shrunk to the new height"
    );
    // A full repaint was requested after the resize (screen clear + repaint).
    let painted = String::from_utf8_lossy(&sink);
    assert!(painted.contains("\u{1b}[2J"), "old cells cleared");
}

// ------------------------------------------- vim key notation (:help notation)

#[test]
fn special_keys_work_in_mappings() {
    let session = open(&["one", "two", "three"]);
    let mut tui = Tui::new(session);
    // <C-a>, <Char-120> (x) and <F5> are all valid mapping keys.
    let rows = screen(
        &mut tui,
        ":nmap <lt>C-a> dd<CR>:nmap <lt>Char-120> yy<CR><C-a>",
        30,
        6,
    );
    assert_eq!(rows[0], "two", "<C-a> ran dd");
    assert_eq!(rows[1], "three");
}

#[test]
fn mapping_listing_uses_canonical_notation() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":nmap <lt>C-a> dd<CR>:nmap<CR>", 40, 6);
    assert!(
        rows.iter().any(|row| row.contains("<C-A>")),
        "listing shows the canonical name: {rows:?}"
    );
}

#[test]
fn ambiguous_mapping_prefix_flushes_after_timeoutlen() {
    // "zq" is a mapping, so "z" alone waits for the rest; with 'timeoutlen' the
    // pending key becomes literal instead of hanging (vim's timeout behaviour).
    let session = open(&["one", "two"]);
    let mut tui = Tui::new(session);
    let z = rvim_editor::maps::Key::Char('z');
    let q = rvim_editor::maps::Key::Char('q');
    tui.session.ex("nmap zq dd").unwrap();
    tui.session.ex("set timeoutlen=0").unwrap();

    tui.feed_key(z.clone()).unwrap();
    assert!(
        tui.flush_pending_timeout().unwrap(),
        "the buffered prefix was flushed"
    );
    tui.feed_key(q.clone()).unwrap();
    assert_eq!(
        tui.session.editor.current_buffer().full_text(),
        "one\ntwo",
        "z and q stayed literal, so nothing was deleted"
    );

    // Without the timeout the two keys form the mapping.
    let session = open(&["one", "two"]);
    let mut tui = Tui::new(session);
    tui.session.ex("nmap zq dd").unwrap();
    tui.feed_key(z).unwrap();
    tui.feed_key(q).unwrap();
    assert_eq!(tui.session.editor.current_buffer().full_text(), "two");
}

#[test]
fn counts_and_prefixes_still_work_with_special_keys() {
    // <Esc> must clear the pending operator, keeping the count semantics intact.
    let session = open(&["abcdef"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "3d<Esc>x", 20, 5);
    assert_eq!(rows[0], "bcdef", "<Esc> cancelled the operator");
}

// --------------------------------- counted insert and insert undo grouping

#[test]
fn counted_insert_repeats_text_in_the_tui() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "3ihello<Esc>", 40, 5);
    assert_eq!(rows[0], "hellohellohellox");
    // vim leaves the cursor on the last inserted character.
    assert_eq!(tui.session.editor.cursor().col, 14);
}

#[test]
fn counted_insert_repeats_the_typed_line_break() {
    let session = open(&["seed"]);
    let mut tui = Tui::new(session);
    // 3i%<CR><Esc>: vim repeats the whole typed text, newline included, so the
    // result is three "%" lines followed by the original line (regression: the
    // TUI used to split the line itself, so the repeat kept only "%").
    let rows = screen(&mut tui, "3i%<CR><Esc>", 30, 8);
    assert_eq!(rows[0], "%");
    assert_eq!(rows[1], "%");
    assert_eq!(rows[2], "%");
    assert_eq!(rows[3], "seed");
    assert_eq!(tui.session.editor.line_count(), 4);
    // The insert register holds the newline too (it is what "." replays).
    assert!(
        tui.session
            .editor
            .registers
            .get('.')
            .is_some_and(|r| r.text.contains(char::from(10))),
        "the last-insert register keeps the <CR>"
    );
}

#[test]
fn large_counted_insert_with_a_newline_stays_consistent() {
    let session = open(&["seed"]);
    let mut tui = Tui::new(session);
    // The reported repro: 1000i%<CR><Esc> must produce 1000 "%" lines plus the
    // original line, not one line with 1000 percent signs.
    let rows = screen(&mut tui, "1000i%<CR><Esc>", 20, 6);
    assert_eq!(rows[0], "%");
    assert_eq!(tui.session.editor.line_count(), 1001);
    let all_percent = (0..1000).all(|i| {
        tui.session
            .editor
            .current_buffer()
            .line_str(i)
            .map(|l| l == "%")
            .unwrap_or(false)
    });
    assert!(all_percent, "every repeated copy is its own line");
    assert_eq!(
        tui.session.editor.current_buffer().line_str(1000).unwrap(),
        "seed"
    );
}

#[test]
fn dot_repeats_an_insert_that_contains_a_newline() {
    let session = open(&["seed"]);
    let mut tui = Tui::new(session);
    screen(&mut tui, "i%<CR><Esc>", 30, 8);
    screen(&mut tui, "j.", 30, 8);
    let text = tui.session.editor.current_buffer().full_text();
    assert_eq!(text, format!("%{nl}%{nl}seed", nl = char::from(10)));
}

#[test]
fn counted_open_inserts_multiple_lines() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "3oabc<Esc>", 20, 7);
    assert_eq!(rows[0], "x");
    assert_eq!(rows[1], "abc");
    assert_eq!(rows[2], "abc");
    assert_eq!(rows[3], "abc");
}

#[test]
fn undo_after_insert_session_and_redo() {
    let session = open(&["abc"]);
    let mut tui = Tui::new(session);
    // One `u` undoes the whole session, even with a count.
    let rows = screen(&mut tui, "3iXX<Esc>u", 20, 5);
    assert_eq!(rows[0], "abc");
    assert_eq!(tui.session.editor.current_buffer().full_text(), "abc");

    // <C-r> redoes it.
    let session = open(&["abc"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "3iXX<Esc>u<C-r>", 24, 5);
    assert_eq!(rows[0], "XXXXXXabc");
}

// ------------------------------------- review: operator + motion (M4 round)

#[test]
fn operator_motion_commands_work() {
    // dw deletes to the start of the next word.
    let session = open(&["one two three", "second"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "dw", 30, 6);
    assert_eq!(rows[0], "two three");

    // dj is line-wise: two lines disappear.
    let session = open(&["one", "two", "three", "four"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "dj", 30, 6);
    assert_eq!(rows[0], "three");
    assert_eq!(rows[1], "four");

    // d$ and its one-key form D.
    let session = open(&["one two three"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "wd$", 30, 5);
    assert_eq!(rows[0], "one");

    let session = open(&["one two", "second"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "D", 30, 6);
    assert_eq!(rows[0], "");
    assert_eq!(rows[1], "second");

    // A count makes D reach one line further (d2$).
    let session = open(&["aa", "bb", "cc"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "2D", 20, 6);
    assert_eq!(rows[0], "");
    assert_eq!(rows[1], "cc");
}

#[test]
fn operator_counts_and_backward_motions() {
    // d3l deletes exactly three characters.
    let session = open(&["abcdef"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "d3l", 20, 5);
    assert_eq!(rows[0], "def");

    // dh deletes backwards without taking the cursor character.
    let session = open(&["abcdef"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "ldh", 20, 5);
    assert_eq!(rows[0], "bcdef");

    // A count between operator and motion (d2w).
    let session = open(&["a b c d e"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "d2w", 20, 5);
    assert_eq!(rows[0], "c d e");
}

#[test]
fn yank_and_change_motions() {
    let session = open(&["one two", "second"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "ywP", 30, 6);
    assert_eq!(rows[0], "one one two");

    // cw behaves like ce when the cursor is on a word (vim's special case).
    let session = open(&["one two", "second"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "cwX<Esc>", 30, 6);
    assert_eq!(rows[0], "X two");

    // cc keeps a single empty line to insert into.
    let session = open(&["one", "two", "three"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "ccX<Esc>", 30, 6);
    assert_eq!(rows[0], "X");
    assert_eq!(rows[1], "two");
}

#[test]
fn operator_gg_and_upper_g() {
    let session = open(&["aa", "bb", "cc", "dd"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "Gdgg", 20, 6);
    assert_eq!(rows[0], "", "dgg deleted every line");

    let session = open(&["aa", "bb", "cc", "dd"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "jdG", 20, 6);
    assert_eq!(rows[0], "aa");
    assert_eq!(rows[1], "~", "dG deleted the rest of the buffer");
}

// ---------------------------------------------- review: search behaviour

#[test]
fn search_starts_after_the_cursor_on_the_current_line() {
    // "/" must find a match later on the current line, not only on the next one.
    let session = open(&["alpha beta gamma"]);
    let mut tui = Tui::new(session);
    screen(&mut tui, "0/beta<CR>", 30, 5);
    assert_eq!(tui.session.editor.cursor().col, 6);
}

#[test]
fn n_and_shift_n_repeat_the_search_in_both_directions() {
    let session = open(&["alpha", "beta", "alpha"]);
    let mut tui = Tui::new(session);
    screen(&mut tui, "/alpha<CR>", 30, 6);
    assert_eq!(tui.session.editor.cursor().line, 2);
    screen(&mut tui, "n", 30, 6);
    assert_eq!(tui.session.editor.cursor().line, 0, "n wraps to the top");
    screen(&mut tui, "N", 30, 6);
    assert_eq!(tui.session.editor.cursor().line, 2, "N goes the other way");
}

#[test]
fn backward_search_and_empty_pattern() {
    let session = open(&["alpha", "beta"]);
    let mut tui = Tui::new(session);
    screen(&mut tui, "G?alpha<CR>", 30, 5);
    assert_eq!(tui.session.editor.cursor().line, 0);

    // An empty pattern repeats the last one; without one it is E35.
    let session = open(&["alpha", "beta"]);
    let mut tui = Tui::new(session);
    let mut keys = ScriptedKeys::from_notation("/<CR>");
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, 30, 5).unwrap();
    assert!(
        tui.screen_text(30, 5).contains("E35"),
        "no previous pattern"
    );
}

// ----------------------------------------- review: message line and paging

#[test]
fn echo_output_reaches_the_message_line() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":echo \"hi there\"<CR>", 30, 5);
    assert_eq!(rows[rows.len() - 1], "hi there");
}

#[test]
fn an_error_is_reported_once() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    tui.session.ex("boguscmd").unwrap_err();
    tui.collect_messages();
    assert!(tui.message.contains("E492"));
    assert!(
        tui.message_queue.is_empty(),
        "the error must not be queued twice: {:?}",
        tui.message_queue
    );
}

#[test]
fn queued_messages_page_with_more() {
    let session = open(&["x"]);
    let mut tui = Tui::new(session);
    tui.session.ex("bogusone").unwrap_err();
    tui.collect_messages();
    tui.session.ex("bogustwo").unwrap_err();
    tui.collect_messages();
    assert!(
        tui.message.contains("bogusone"),
        "first message: {:?}",
        tui.message
    );
    let text = tui.screen_text(60, 5);
    assert!(text.contains("-- More --"), "more prompt: {text}");

    // The key that advances the prompt is not a command.
    tui.feed_key(rvim_editor::maps::Key::Special("CR".into()))
        .unwrap();
    assert!(
        tui.message.contains("bogustwo"),
        "next message: {:?}",
        tui.message
    );
    assert!(tui.message_queue.is_empty());
    // The next key clears the last message and is processed normally.
    tui.feed_key(rvim_editor::maps::Key::Char('j')).unwrap();
    assert!(tui.message.is_empty());
}

// --------------------------------------------- review: tabs and the ruler

#[test]
fn tabs_in_the_buffer_are_expanded_to_spaces() {
    let session = open(&["\tabc", "x"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "", 20, 4);
    assert_eq!(rows[0], "        abc", "tab expands to 'tabstop' cells");

    let session = open(&["\tabc"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":set tabstop=4<CR>", 20, 4);
    assert_eq!(rows[0], "    abc", "'tabstop' is honoured");
}

#[test]
fn ruler_shows_all_top_and_bot_like_vim() {
    let session = open(&["a", "b"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "", 30, 6);
    assert!(
        rows[4].contains("All"),
        "whole buffer visible: {:?}",
        rows[4]
    );

    let lines: Vec<String> = (1..=20).map(|n| format!("line{n}")).collect();
    let mut session = Session::new();
    session.editor.replace_lines(0, 1, lines).unwrap();
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "G", 30, 6);
    assert!(rows[4].contains("Bot"), "at the end: {:?}", rows[4]);
}

#[test]
fn horizontal_scroll_leaves_room_for_the_number_gutter() {
    let session = open(&["0123456789abcdefghij"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, ":set number<CR>19l", 12, 4);
    // 12 columns minus the two-cell gutter leaves ten text cells.
    assert_eq!(
        tui.session.editor.windows.current().left,
        10,
        "cursor cell 19, ten text cells"
    );
    assert_eq!(
        rows[0].chars().count(),
        12,
        "row fits the window: {:?}",
        rows[0]
    );
}

// -------------------------------------------- review: cancelling a command

#[test]
fn escape_cancels_a_pending_operator_without_moving_the_cursor() {
    let session = open(&["abcdef"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "llld<Esc>x", 20, 5);
    assert_eq!(rows[0], "abcef", "x deleted the character under the cursor");
    assert_eq!(tui.session.editor.mode, rvim_editor::Mode::Normal);
    assert_eq!(tui.session.editor.cursor().col, 3);
}

// --------------------------------------------- review: paging through a file

#[test]
fn page_down_and_up_scroll_one_screen() {
    let lines: Vec<String> = (1..=40).map(|n| format!("line{n}")).collect();
    let mut session = Session::new();
    session.editor.replace_lines(0, 1, lines).unwrap();
    let mut tui = Tui::new(session);
    // Four text rows on a six-row screen.
    let rows = screen(&mut tui, "<C-f>", 20, 6);
    assert_eq!(rows[0], "line5", "one screen further down");
    assert_eq!(tui.session.editor.cursor().line, 4);

    let rows = screen(&mut tui, "<PageUp>", 20, 6);
    assert_eq!(rows[0], "line1", "back to the top");
    assert_eq!(tui.session.editor.cursor().line, 0);

    // <PageDown> behaves like <C-f>.
    let rows = screen(&mut tui, "<PageDown>", 20, 6);
    assert_eq!(rows[0], "line5");
}

#[test]
fn escape_drops_a_pending_count() {
    let session = open(&["one", "two", "three"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "3<Esc>dd", 20, 6);
    assert_eq!(
        rows[0], "two",
        "the count was cancelled, so dd took one line"
    );
    assert_eq!(rows[1], "three");
}

#[test]
fn a_change_command_is_one_undo_step() {
    // "cw" and the inserted text undo together, like vim's change operator.
    let session = open(&["one two"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "cwX<Esc>u", 20, 4);
    assert_eq!(rows[0], "one two");

    let session = open(&["one", "two"]);
    let mut tui = Tui::new(session);
    let rows = screen(&mut tui, "ccX<Esc>u", 20, 5);
    assert_eq!(rows[0], "one");
    assert_eq!(rows[1], "two");
}

#[test]
fn the_search_prompt_starts_empty() {
    let session = open(&["alpha beta"]);
    let mut tui = Tui::new(session);
    screen(&mut tui, "/alpha<CR>", 30, 5);
    let rows = screen(&mut tui, "/", 30, 5);
    assert_eq!(
        rows[rows.len() - 1],
        "/",
        "vim does not pre-fill the prompt"
    );
}

// ------------------------------------------------------------- showcmd

#[test]
fn showcmd_shows_a_pending_operator_and_count() {
    // Each case starts from a fresh editor so the pending state cannot leak.
    let mut tui = Tui::new(open(&["alpha", "bravo"]));
    let rows = screen(&mut tui, "d", 40, 5);
    assert!(
        rows[4].trim_end().ends_with("d"),
        "pending d: {:?}",
        rows[4]
    );

    let mut tui = Tui::new(open(&["alpha", "bravo"]));
    let rows = screen(&mut tui, "2d", 40, 5);
    assert!(
        rows[4].trim_end().ends_with("2d"),
        "count and operator: {:?}",
        rows[4]
    );

    let mut tui = Tui::new(open(&["alpha", "bravo"]));
    let rows = screen(&mut tui, "d2", 40, 5);
    assert!(
        rows[4].trim_end().ends_with("d2"),
        "motion count after the operator: {:?}",
        rows[4]
    );
}

#[test]
fn showcmd_shows_the_register_prefix_and_window_prefix() {
    let mut tui = Tui::new(open(&["alpha"]));
    let rows = screen(&mut tui, "\"a", 40, 5);
    assert!(rows[4].ends_with("\"a"), "register prefix: {:?}", rows[4]);
    let rows = screen(&mut tui, "<Esc><C-w>", 40, 5);
    assert!(rows[4].ends_with("^W"), "CTRL-W prefix: {:?}", rows[4]);
}

#[test]
fn showcmd_shows_the_visual_selection_size() {
    let mut tui = Tui::new(open(&["alpha", "bravo", "charlie", "delta", "echo"]));
    let rows = screen(&mut tui, "vll", 40, 7);
    assert!(rows[6].ends_with('3'), "three characters: {:?}", rows[6]);

    let mut tui = Tui::new(open(&["alpha", "bravo", "charlie", "delta", "echo"]));
    let rows = screen(&mut tui, "v3j", 40, 7);
    assert!(rows[6].ends_with('4'), "four lines: {:?}", rows[6]);

    let mut tui = Tui::new(open(&["alpha", "bravo", "charlie", "delta", "echo"]));
    let rows = screen(&mut tui, "<C-v>jjll", 40, 7);
    assert!(rows[6].ends_with("3x3"), "block size: {:?}", rows[6]);
}

#[test]
fn showcmd_is_on_by_default_and_can_be_disabled() {
    let mut tui = Tui::new(open(&["alpha"]));
    let rows = screen(&mut tui, ":set showcmd?<CR>", 40, 5);
    assert!(rows[4].contains("showcmd"), "default is on: {:?}", rows[4]);
    let rows = screen(&mut tui, ":set noshowcmd<CR>d", 40, 5);
    assert!(
        rows[4].trim().is_empty(),
        "nothing is shown with 'noshowcmd': {:?}",
        rows[4]
    );
}

#[test]
fn showcmd_is_hidden_while_the_command_line_is_open() {
    let mut tui = Tui::new(open(&["alpha"]));
    let rows = screen(&mut tui, ":", 40, 5);
    assert_eq!(rows[4].trim_end(), ":", "the command line takes the row");
}

// ------------------------------------------------------- CTRL-A / CTRL-X

fn line_text(tui: &Tui) -> String {
    tui.session.editor.current_buffer().full_text()
}

#[test]
fn ctrl_a_increments_and_ctrl_x_decrements_the_number() {
    let mut tui = Tui::new(open(&["value 41"]));
    screen(&mut tui, "<C-a>", 30, 5);
    assert_eq!(line_text(&tui), "value 42");
    screen(&mut tui, "3<C-a>", 30, 5);
    assert_eq!(line_text(&tui), "value 45");
    screen(&mut tui, "2<C-x>", 30, 5);
    assert_eq!(line_text(&tui), "value 43");
}

#[test]
fn ctrl_x_crosses_zero_into_a_negative_number() {
    let mut tui = Tui::new(open(&["0"]));
    screen(&mut tui, "<C-x>", 30, 5);
    assert_eq!(line_text(&tui), "-1");
    screen(&mut tui, "<C-a><C-a>", 30, 5);
    assert_eq!(line_text(&tui), "1");
}

#[test]
fn ctrl_a_keeps_the_number_format() {
    let mut tui = Tui::new(open(&["0x1f"]));
    screen(&mut tui, "<C-a>", 30, 5);
    assert_eq!(line_text(&tui), "0x20");

    let mut tui = Tui::new(open(&["0b101"]));
    screen(&mut tui, "<C-a>", 30, 5);
    assert_eq!(line_text(&tui), "0b110");

    let mut tui = Tui::new(open(&["007"]));
    screen(&mut tui, "<C-a>", 30, 5);
    assert_eq!(
        line_text(&tui),
        "008",
        "a zero padded number keeps its width"
    );
}

#[test]
fn ctrl_a_repeats_with_dot_and_undoes_in_one_step() {
    let mut tui = Tui::new(open(&["41"]));
    screen(&mut tui, "3<C-a>.", 30, 5);
    assert_eq!(line_text(&tui), "47", "the repeat adds another 3");
    screen(&mut tui, "u", 30, 5);
    assert_eq!(line_text(&tui), "44", "one undo step per increment");
}
