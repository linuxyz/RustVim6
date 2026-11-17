//! M2 feature tests: Ex command families, the extended parser, script
//! ecosystem (autocmd/map/command/source) and frequency-gate drivers.
//!
//! Each test drives the same `rvim-app::Session` entry point as `rvim -es`.

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

fn temp_path(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("rvim_m2_{tag}"));
    let _ = std::fs::remove_file(&p);
    p
}

// ------------------------------------------------------- :global / :vglobal

#[test]
fn global_deletes_matching_lines() {
    let mut s = Session::new();
    seed(&mut s, &["alpha", "beta", "alnum"]);
    s.ex("g/al/d").unwrap();
    assert_eq!(text(&s), "beta");
}

#[test]
fn vglobal_deletes_non_matching_lines() {
    // `:v/pat/d` runs `:d` on the lines that do NOT match, i.e. it keeps the
    // matches (ex_cmds.h: CMD_vglobal calls ex_global with the inverted test).
    let mut s = Session::new();
    seed(&mut s, &["alpha", "beta", "alnum"]);
    s.ex("v/al/d").unwrap();
    assert_eq!(text(&s), "alpha\nalnum");
}

#[test]
fn vglobal_abbreviation_matches_full_name() {
    // Regression: handlers used to read the typed word, so `:v` lost the
    // inversion that `:vglobal` had.
    let run = |cmd: &str| {
        let mut s = Session::new();
        seed(&mut s, &["alpha", "beta", "alnum"]);
        s.ex(cmd).unwrap();
        text(&s)
    };
    assert_eq!(run("v/al/d"), run("vglobal/al/d"));
    assert_eq!(run("g/al/d"), "beta");
}

#[test]
fn global_runs_substitute_on_matching_lines_only() {
    let mut s = Session::new();
    seed(&mut s, &["foo Bar", "baz", "foo qux"]);
    s.ex("g/foo/s/foo/FOO/").unwrap();
    assert_eq!(text(&s), "FOO Bar\nbaz\nFOO qux");
}

#[test]
fn global_without_match_reports_pattern_not_found_without_error() {
    // Vim uses smsg() here, not emsg(): no E486, exit code unaffected
    // (ex_cmds.c:5026-5042).
    let mut s = Session::new();
    seed(&mut s, &["one"]);
    s.ex("g/zzz/d").unwrap();
    assert_eq!(s.editor.error_count, 0);
    assert!(
        s.editor
            .messages
            .history()
            .iter()
            .any(|m| m.contains("Pattern not found: zzz"))
    );
    assert_eq!(text(&s), "one");
}

// ---------------------------------------------------------------- :sort

#[test]
fn sort_lexicographic_numeric_reverse_and_unique() {
    let mut s = Session::new();
    seed(&mut s, &["banana", "apple", "cherry"]);
    s.ex("sort").unwrap();
    assert_eq!(text(&s), "apple\nbanana\ncherry");

    let mut s = Session::new();
    seed(&mut s, &["10", "9", "100"]);
    s.ex("sort n").unwrap();
    assert_eq!(text(&s), "9\n10\n100");

    // "!" is the reverse flag; "r" sorts on the last search pattern, so with
    // no previous pattern it sorts normally (ex_cmds.c:406-480,604-611).
    let mut s = Session::new();
    seed(&mut s, &["b", "a", "c"]);
    s.ex("sort!").unwrap();
    assert_eq!(text(&s), "c\nb\na");

    let mut s = Session::new();
    seed(&mut s, &["b", "a", "c"]);
    s.ex("sort r").unwrap();
    assert_eq!(text(&s), "a\nb\nc");

    let mut s = Session::new();
    seed(&mut s, &["b", "a", "b", "a"]);
    s.ex("sort u").unwrap();
    assert_eq!(text(&s), "a\nb");
}

// ---------------------------------------------------------------- :join

#[test]
fn join_with_and_without_space() {
    let mut s = Session::new();
    seed(&mut s, &["one", "two", "three"]);
    s.ex("1,3j").unwrap();
    assert_eq!(text(&s), "one two three");

    let mut s = Session::new();
    seed(&mut s, &["a ", "b"]);
    s.ex("1,2j!").unwrap();
    assert_eq!(text(&s), "a b");
}

// -------------------------------------------------- shift / align / retab

#[test]
fn shift_right_and_left_by_shiftwidth() {
    let mut s = Session::new();
    seed(&mut s, &["x"]);
    s.ex("set sw=4 et").unwrap();
    s.ex("1>").unwrap();
    assert_eq!(text(&s), "    x");
    s.ex("1<").unwrap();
    assert_eq!(text(&s), "x");
}

#[test]
fn shift_multiple_lines_with_count() {
    let mut s = Session::new();
    seed(&mut s, &["x", "y", "z"]);
    s.ex("set sw=2 et").unwrap();
    // ":1>2" shifts TWO lines (the count is a line count), one shiftwidth each.
    s.ex("1>2").unwrap();
    assert_eq!(text(&s), "  x\n  y\nz");
}

#[test]
fn left_right_and_center_align_text() {
    let mut s = Session::new();
    seed(&mut s, &["   hello"]);
    s.ex("left").unwrap();
    assert_eq!(text(&s), "hello");
    s.ex("right 10").unwrap();
    assert_eq!(text(&s), "     hello");
    s.ex("center 10").unwrap();
    // Centering within 10 columns puts half the pad on the left.
    assert!(text(&s).starts_with("  ") && text(&s).ends_with("hello"));
}

#[test]
fn retab_converts_tabs_to_spaces_with_expandtab() {
    let mut s = Session::new();
    seed(&mut s, &["\tx"]);
    s.ex("set ts=4 et").unwrap();
    s.ex("retab").unwrap();
    assert_eq!(text(&s), "    x");
}

// ---------------------------------------------------------------- :# / :& / :~

#[test]
fn hash_prints_numbered_line() {
    let mut s = Session::new();
    seed(&mut s, &["alpha"]);
    s.ex("#").unwrap();
    assert!(s.message_history().iter().any(|m| m == "     1\talpha"));
}

#[test]
fn amp_repeats_last_substitute() {
    let mut s = Session::new();
    seed(&mut s, &["ax", "bx", "cx2"]);
    s.ex("1s/x/y/").unwrap();
    assert_eq!(text(&s), "ay\nbx\ncx2");
    s.ex("2&").unwrap();
    assert_eq!(text(&s), "ay\nby\ncx2");
    s.ex("3&").unwrap();
    assert_eq!(text(&s), "ay\nby\ncy2");
    // Repeating on a line with no match reports E486 like vim.
    assert_eq!(s.ex("1&").unwrap_err().code, "E486");
}

// ----------------------------------------------------- addresses and marks

#[test]
fn search_address_delete() {
    let mut s = Session::new();
    seed(&mut s, &["start", "mid", "end"]);
    s.ex("/end/d").unwrap();
    assert_eq!(text(&s), "start\nmid");
}

#[test]
fn mark_address_delete() {
    use rvim_normal::NormalEngine;
    let mut s = Session::new();
    seed(&mut s, &["a", "b", "c"]);
    let mut eng = NormalEngine::new();
    eng.feed(&mut s.editor, 'G').unwrap();
    eng.feed(&mut s.editor, 'm').unwrap();
    eng.complete_mark(&mut s.editor, 'c').unwrap();
    s.ex("'cd").unwrap();
    assert_eq!(text(&s), "a\nb");
}

#[test]
fn invalid_search_and_mark_are_vim_errors() {
    let mut s = Session::new();
    seed(&mut s, &["only"]);
    assert_eq!(s.ex("/nope/d").unwrap_err().code, "E486");
    assert_eq!(s.ex("'zd").unwrap_err().code, "E20");
}

// ------------------------------------------------------------- mappings

#[test]
fn nmap_registers_and_expands() {
    use rvim_editor::MapMode;
    use rvim_editor::maps::{keys_to_notation, parse_keys};
    let mut s = Session::new();
    s.ex("nmap x dd").unwrap();
    let out = s
        .editor
        .maps
        .expand(MapMode::Normal, &parse_keys("x"))
        .unwrap();
    assert_eq!(keys_to_notation(&out), "dd");
    // Mappings are visible in `:nmap` listing.
    s.ex("nmap").unwrap();
    assert!(s.message_history().iter().any(|m| m.contains("dd")));
}

#[test]
fn noremap_does_not_recurse_but_map_does() {
    use rvim_editor::MapMode;
    use rvim_editor::maps::{keys_to_notation, parse_keys};
    let mut s = Session::new();
    s.ex("map x y").unwrap();
    s.ex("map y z").unwrap();
    let out = s
        .editor
        .maps
        .expand(MapMode::Normal, &parse_keys("x"))
        .unwrap();
    assert_eq!(keys_to_notation(&out), "z");

    let mut s2 = Session::new();
    s2.ex("noremap x y").unwrap();
    s2.ex("map y z").unwrap();
    let out = s2
        .editor
        .maps
        .expand(MapMode::Normal, &parse_keys("x"))
        .unwrap();
    assert_eq!(keys_to_notation(&out), "y");
}

#[test]
fn map_modes_are_isolated_and_unmapped() {
    use rvim_editor::MapMode;
    use rvim_editor::maps::{keys_to_notation, parse_keys};
    let mut s = Session::new();
    s.ex("inoremap jk <Esc>").unwrap();
    // Insert-mode mapping does not affect normal mode.
    let normal = s
        .editor
        .maps
        .expand(MapMode::Normal, &parse_keys("jk"))
        .unwrap();
    assert_eq!(keys_to_notation(&normal), "jk");
    let insert = s
        .editor
        .maps
        .expand(MapMode::Insert, &parse_keys("jk"))
        .unwrap();
    assert_eq!(keys_to_notation(&insert), "<Esc>", "vim casing");
    s.ex("iunmap jk").unwrap();
    assert!(s.editor.maps.entries_for(MapMode::Insert).is_empty());
}

#[test]
fn map_options_are_stored() {
    use rvim_editor::MapMode;
    let mut s = Session::new();
    s.ex("nnoremap <silent> <leader>w :write<CR>").unwrap();
    let entry = s.editor.maps.entries_for(MapMode::Normal)[0];
    assert!(entry.opts.silent);
    assert!(!entry.opts.recursive);
}

#[test]
fn mapclear_removes_mappings() {
    use rvim_editor::MapMode;
    let mut s = Session::new();
    s.ex("nnoremap a b").unwrap();
    s.ex("nnoremap c d").unwrap();
    s.ex("nmapclear").unwrap();
    assert!(s.editor.maps.entries_for(MapMode::Normal).is_empty());
}

// --------------------------------------------------------- abbreviations

#[test]
fn iabbrev_expands_insert_word() {
    use rvim_editor::AbbrevMode;
    let mut s = Session::new();
    s.ex("iabbrev teh the").unwrap();
    assert_eq!(
        s.editor.abbrevs.expand(AbbrevMode::Insert, "I typed teh"),
        Some((3, "the".to_string()))
    );
    s.ex("unabbreviate teh").unwrap();
    assert!(s.editor.abbrevs.expand(AbbrevMode::Insert, "teh").is_none());
}

// -------------------------------------------------------------- autocmd

#[test]
fn augroup_autocmd_registration_and_listing() {
    let mut s = Session::new();
    s.ex("augroup MyGrp").unwrap();
    s.ex("autocmd BufRead *.rs setlocal expandtab").unwrap();
    s.ex("augroup END").unwrap();
    assert_eq!(s.editor.autocmds.groups(), &vec!["MyGrp".to_string()]);
    let entry = &s.editor.autocmds.entries()[0];
    assert_eq!(entry.group, "MyGrp");
    assert_eq!(entry.events, vec!["BufRead".to_string()]);
    assert_eq!(entry.pattern, "*.rs");
    assert_eq!(entry.command, "setlocal expandtab");
    // Listing shows it.
    s.ex("autocmd").unwrap();
    assert!(s.message_history().iter().any(|m| m.contains("MyGrp")));
}

#[test]
fn doautocmd_executes_matching_command() {
    let mut s = Session::new();
    s.ex("autocmd BufRead *.rs setlocal expandtab").unwrap();
    s.ex("doautocmd BufRead main.rs").unwrap();
    assert!(s.editor.options.bool_or("expandtab", false));
}

#[test]
fn autocmd_bang_removes_entries() {
    let mut s = Session::new();
    s.ex("augroup G").unwrap();
    s.ex("autocmd BufRead * set number").unwrap();
    assert_eq!(s.editor.autocmds.entries().len(), 1);
    s.ex("autocmd!").unwrap();
    assert!(s.editor.autocmds.entries().is_empty());
}

#[test]
fn invalid_event_reports_e216() {
    let mut s = Session::new();
    assert_eq!(
        s.ex("autocmd BogusEvent * echo 1").unwrap_err().code,
        "E216"
    );
}

#[test]
fn autocmd_command_text_keeps_bars() {
    let mut s = Session::new();
    // :autocmd has EX_NOTRLCOM, so the whole tail is the command text.
    s.ex("autocmd BufRead *.txt set number|set wrap").unwrap();
    assert_eq!(
        s.editor.autocmds.entries()[0].command,
        "set number|set wrap"
    );
}

// ------------------------------------------------------- user commands

#[test]
fn user_command_define_and_execute() {
    let mut s = Session::new();
    s.ex("command -nargs=1 Greet echomsg '<args>'").unwrap();
    s.ex("Greet hello").unwrap();
    assert!(s.message_history().iter().any(|m| m == "hello"));
}

#[test]
fn user_command_requires_bang_to_redefine() {
    let mut s = Session::new();
    s.ex("command Foo echo 1").unwrap();
    assert_eq!(s.ex("command Foo echo 2").unwrap_err().code, "E174");
    assert!(s.ex("command! Foo echo 2").is_ok());
}

#[test]
fn user_command_nargs_validation_and_delcommand() {
    let mut s = Session::new();
    s.ex("command -nargs=0 Zero echo ok").unwrap();
    assert_eq!(s.ex("Zero extra").unwrap_err().code, "E488");
    s.ex("delcommand Zero").unwrap();
    assert_eq!(s.ex("Zero").unwrap_err().code, "E492");
}

#[test]
fn unknown_command_is_e492() {
    let mut s = Session::new();
    assert_eq!(s.ex("Bogus").unwrap_err().code, "E492");
}

// ------------------------------------------------------------- sourcing

#[test]
fn source_runs_script_and_finish_stops_it() {
    let path = temp_path("source.vim");
    std::fs::write(
        &path,
        "\" a small vimrc\nset expandtab\nfinish\nset number\n",
    )
    .unwrap();
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert!(s.editor.options.bool_or("expandtab", false));
    // Commands after :finish are not executed.
    assert!(!s.editor.options.bool_or("number", false));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn source_supports_continuation_lines_and_mappings() {
    let path = temp_path("source2.vim");
    std::fs::write(&path, "set tabstop=4\nnmap ,w :write<CR>\n").unwrap();
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(s.editor.options.num_or("tabstop", 0), 4);
    assert_eq!(
        s.editor
            .maps
            .entries_for(rvim_editor::MapMode::Normal)
            .len(),
        1
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn runtime_sources_from_runtimepath() {
    let dir = std::env::temp_dir().join("rvim_m2_runtime");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("plugin")).unwrap();
    std::fs::write(dir.join("plugin").join("test.vim"), "set shiftwidth=3\n").unwrap();
    let mut s = Session::new();
    s.ex(&format!("set runtimepath={}", dir.display())).unwrap();
    s.ex("runtime plugin/test.vim").unwrap();
    assert_eq!(s.editor.options.num_or("shiftwidth", 0), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn source_tolerates_utf8_bom() {
    // Windows editors (and PowerShell's -Encoding utf8) prepend a BOM; the first
    // command must still run.
    let path = temp_path("bom.vim");
    let mut content = String::from("\u{feff}set tabstop=6\nset number\n");
    std::fs::write(&path, &content).unwrap();
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(s.editor.options.num_or("tabstop", 0), 6);
    assert!(s.editor.options.bool_or("number", false));
    content.clear();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn source_missing_file_reports_e484() {
    let mut s = Session::new();
    let err = s.source(std::path::Path::new("no_such_script_xyz.vim"));
    assert_eq!(err.unwrap_err().code, "E484");
}

// ------------------------------------------------------ file args / write

#[test]
fn write_with_ff_opt_uses_dos_line_endings() {
    let path = temp_path("fileopt.txt");
    let mut s = Session::new();
    seed(&mut s, &["one", "two"]);
    s.ex(&format!("w ++ff=dos {}", path.display())).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\r\ntwo\r\n");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn edit_with_plus_line_command_moves_cursor() {
    let path = temp_path("plus.txt");
    std::fs::write(&path, "a\nb\nc\n").unwrap();
    let mut s = Session::new();
    s.ex(&format!("e +2 {}", path.display())).unwrap();
    assert_eq!(s.editor.cursor().line, 1);
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------- arglist

#[test]
fn args_add_delete_and_list() {
    let mut s = Session::new();
    s.ex("args one.txt two.txt").unwrap();
    assert_eq!(s.editor.arglist.len(), 2);
    s.ex("argadd three.txt").unwrap();
    assert_eq!(s.editor.arglist.len(), 3);
    s.ex("argdelete 1").unwrap();
    assert_eq!(s.editor.arglist.len(), 2);
    assert_eq!(s.editor.arglist[0].to_string_lossy(), "two.txt");
    s.ex("args").unwrap();
    assert!(s.message_history().iter().any(|m| m.contains("two.txt")));
}

// ---------------------------------------------------- :dl list flag / :#

#[test]
fn delete_with_list_flag_prints_deleted_lines() {
    let mut s = Session::new();
    seed(&mut s, &["gone", "kept"]);
    s.ex("dl").unwrap();
    assert_eq!(text(&s), "kept");
    assert!(s.message_history().iter().any(|m| m == "gone$"));
}
