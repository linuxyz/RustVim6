//! M5 tests: windows, tabpages, visual mode, text objects and `.` repeat.
//!
//! Everything is driven through [`Tui`] with scripted keys, the same path the
//! interactive binary uses, and asserted on the rendered plain-text screen.

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_term::ScriptedKeys;

fn open(lines: &[&str]) -> Session {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("rvim_m5_{unique}.txt"));
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

fn buffer_text(tui: &Tui) -> String {
    tui.session.editor.current_buffer().full_text()
}

// ------------------------------------------------------------------ windows

#[test]
fn split_shows_two_windows_each_with_a_status_line() {
    let mut tui = Tui::new(open(&["alpha", "beta"]));
    let rows = screen(&mut tui, ":split<CR>", 40, 8);
    assert_eq!(tui.session.editor.window_count(), 2);
    // Two windows: rows 0-2 (text+status) and 3-6 (text+status), message at 7.
    assert_eq!(rows[0], "alpha", "top window text: {:?}", rows);
    assert!(rows[2].contains("NORMAL"), "top status: {:?}", rows[2]);
    assert!(rows[6].contains("NORMAL"), "bottom status: {:?}", rows[6]);
}

#[test]
fn vsplit_draws_a_separator_column() {
    let mut tui = Tui::new(open(&["alpha"]));
    let rows = screen(&mut tui, ":vsplit<CR>", 41, 6);
    assert_eq!(tui.session.editor.window_count(), 2);
    assert!(rows[0].contains('|'), "separator in row 0: {:?}", rows[0]);
    let rects = tui.session.editor.windows.rects().clone();
    assert_eq!(rects.len(), 2);
}

#[test]
fn ctrl_w_moves_the_cursor_between_windows() {
    let mut tui = Tui::new(open(&["alpha", "beta", "gamma", "delta"]));
    screen(&mut tui, ":split<CR>", 40, 10);
    let top = tui.session.editor.windows.current_id();
    screen(&mut tui, "<C-w>j", 40, 10);
    let bottom = tui.session.editor.windows.current_id();
    assert_ne!(top, bottom, "<C-w>j moves to the other window");
    screen(&mut tui, "<C-w>k", 40, 10);
    assert_eq!(tui.session.editor.windows.current_id(), top);
}

#[test]
fn each_window_keeps_its_own_cursor() {
    let mut tui = Tui::new(open(&["one", "two", "three", "four", "five"]));
    screen(&mut tui, ":split<CR>", 40, 10);
    let top = tui.session.editor.windows.current_id();
    screen(&mut tui, "jj", 40, 10);
    let top_cursor = tui.session.editor.windows.get(top).unwrap().cursor.line;
    screen(&mut tui, "<C-w>j", 40, 10);
    let bottom = tui.session.editor.windows.current_id();
    screen(&mut tui, "G", 40, 10);
    assert_eq!(
        tui.session.editor.windows.get(top).unwrap().cursor.line,
        top_cursor,
        "the other window keeps its cursor"
    );
    assert_eq!(
        tui.session.editor.windows.get(bottom).unwrap().cursor.line,
        4,
        "G moved only the active window"
    );
}

#[test]
fn window_local_number_option_does_not_leak_into_the_other_window() {
    let mut tui = Tui::new(open(&["alpha"]));
    screen(&mut tui, ":split<CR>:setlocal number<CR>", 40, 8);
    let ids = tui.session.editor.windows.ids();
    let with_number = ids
        .iter()
        .filter(|id| tui.session.editor.wopt_for_bool(**id, "number", false))
        .count();
    assert_eq!(with_number, 1, ":setlocal only affects the current window");
}

#[test]
fn tabnew_adds_a_tabline_and_tabnext_switches_back() {
    let mut tui = Tui::new(open(&["alpha"]));
    let rows = screen(&mut tui, ":tabnew<CR>", 120, 6);
    assert_eq!(tui.session.editor.tabpage_count(), 2);
    assert!(rows[0].contains("1:"), "tabline: {:?}", rows[0]);
    assert!(rows[0].contains("2:"), "tabline: {:?}", rows[0]);
    screen(&mut tui, ":tabnext 1<CR>", 40, 6);
    assert_eq!(tui.session.editor.windows.current_tab_index(), 0);
    screen(&mut tui, ":tabclose<CR>", 40, 6);
    assert_eq!(tui.session.editor.tabpage_count(), 1);
}

// ------------------------------------------------------------ visual mode

#[test]
fn visual_character_selection_deletes_the_selection() {
    let mut tui = Tui::new(open(&["abcdef"]));
    screen(&mut tui, "vll d", 20, 4);
    assert_eq!(buffer_text(&tui), "def", "vlld removes three characters");
}

#[test]
fn visual_line_selection_deletes_whole_lines() {
    let mut tui = Tui::new(open(&["one", "two", "three"]));
    screen(&mut tui, "Vj d", 20, 5);
    assert_eq!(buffer_text(&tui), "three");
}

#[test]
fn visual_block_deletes_a_rectangle() {
    let mut tui = Tui::new(open(&["abcd", "efgh", "ijkl"]));
    screen(&mut tui, "<C-v>jl d", 20, 6);
    assert_eq!(buffer_text(&tui), "cd\ngh\nijkl", "a 2x2 block is removed");
}

#[test]
fn visual_yank_puts_the_selection_in_the_register() {
    let mut tui = Tui::new(open(&["abcdef"]));
    screen(&mut tui, "vlly", 20, 4);
    let reg = tui.session.editor.registers.unnamed().unwrap().text.clone();
    assert_eq!(reg, "abc");
    screen(&mut tui, "$", 20, 4);
    screen(&mut tui, "p", 20, 4);
    assert_eq!(buffer_text(&tui), "abcdefabc");
}

#[test]
fn gv_reselects_the_last_selection() {
    let mut tui = Tui::new(open(&["abcdef"]));
    screen(&mut tui, "vll<Esc>$", 20, 4);
    screen(&mut tui, "gvd", 20, 4);
    assert_eq!(buffer_text(&tui), "def", "gv restored the selection");
}

#[test]
fn visual_mode_swap_ends() {
    let mut tui = Tui::new(open(&["abcdef"]));
    screen(&mut tui, "vll o l d", 20, 4);
    let text = buffer_text(&tui);
    assert!(text.len() < 6, "a wider selection was deleted: {text:?}");
}

#[test]
fn visual_shift_indents_the_selected_lines() {
    let mut tui = Tui::new(open(&["one", "two"]));
    screen(&mut tui, ":set expandtab shiftwidth=2<CR>Vj >", 20, 5);
    assert_eq!(buffer_text(&tui), "  one\n  two");
}

// ----------------------------------------------------------- text objects

#[test]
fn diw_deletes_the_word_under_the_cursor() {
    let mut tui = Tui::new(open(&["alpha beta gamma"]));
    screen(&mut tui, "wdiw", 30, 4);
    assert_eq!(buffer_text(&tui), "alpha  gamma");
}

#[test]
fn dap_deletes_a_whole_paragraph() {
    let mut tui = Tui::new(open(&["one", "two", "", "three"]));
    screen(&mut tui, "dap", 20, 6);
    assert_eq!(
        buffer_text(&tui),
        "three",
        "ap takes the trailing blank line"
    );
}

#[test]
fn ci_quote_changes_inside_double_quotes() {
    let mut tui = Tui::new(open(&["say \"hello\" now"]));
    screen(&mut tui, "4lci\"bye<Esc>", 30, 4);
    assert_eq!(buffer_text(&tui), "say \"bye\" now");
}

#[test]
fn visual_text_object_selects_the_word() {
    let mut tui = Tui::new(open(&["alpha beta"]));
    screen(&mut tui, "viwd", 20, 4);
    assert_eq!(buffer_text(&tui), " beta");
}

// ----------------------------------------------------------------- repeat

#[test]
fn dot_repeats_an_insert_session() {
    let mut tui = Tui::new(open(&["x"]));
    screen(&mut tui, "ihey<Esc>.", 20, 4);
    // vim replays the redo buffer ("ihey") with the cursor on the last inserted
    // character, so the repeat inserts *before* that character.
    assert_eq!(buffer_text(&tui), "heheyyx");
}

#[test]
fn dot_repeats_an_operator_with_its_count() {
    let mut tui = Tui::new(open(&["a", "b", "c", "d"]));
    screen(&mut tui, "2dd.", 20, 6);
    let text = buffer_text(&tui);
    assert_eq!(text, "", "2dd then . deletes the remaining lines: {text:?}");
}

#[test]
fn dot_repeats_x() {
    let mut tui = Tui::new(open(&["abcdef"]));
    screen(&mut tui, "x3.", 20, 4);
    assert_eq!(buffer_text(&tui), "ef", "x then 3. deletes three more");
}

// ------------------------------------------------------------- ex ranges

#[test]
fn colon_from_visual_mode_prefills_the_range() {
    let mut tui = Tui::new(open(&["one", "two", "three"]));
    let mut keys = ScriptedKeys::from_notation("Vj:d<CR>");
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, 40, 6).unwrap();
    assert_eq!(buffer_text(&tui), "three", "the visual range reached :d");
}
