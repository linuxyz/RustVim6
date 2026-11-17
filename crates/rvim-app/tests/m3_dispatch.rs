//! M3 integration tests: autocommand event dispatch, filetype detection and the
//! expression rules that runtime scripts depend on.
//!
//! Before this pass autocommands were only *declared* (`:autocmd`) and could be
//! fired manually with `:doautocmd`; nothing dispatched events, so a sourced
//! `filetype.vim` (811 autocommands) detected no filetypes at all.

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

fn messages(s: &Session) -> Vec<String> {
    s.message_history().to_vec()
}

fn temp_file(tag: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("rvim_m3_{tag}"));
    std::fs::write(&path, contents).unwrap();
    path
}

// ------------------------------------------------------- event dispatch

#[test]
fn opening_a_file_fires_bufread_and_bufenter() {
    let path = temp_file("dispatch.py", "print(1)\n");
    let mut s = Session::new();
    s.ex("autocmd BufRead * echo 'read'").unwrap();
    s.ex("autocmd BufEnter * echo 'enter'").unwrap();
    s.ex(&format!("e {}", path.display())).unwrap();
    assert_eq!(messages(&s), vec!["read".to_string(), "enter".to_string()]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn writing_a_file_fires_pre_and_post_events() {
    let path = temp_file("dispatch_write.txt", "a\n");
    let mut s = Session::new();
    s.ex("autocmd BufWritePre * echo 'pre'").unwrap();
    s.ex("autocmd BufWritePost * echo 'post'").unwrap();
    s.ex(&format!("e {}", path.display())).unwrap();
    s.ex("1s/a/b/").unwrap();
    s.ex("w").unwrap();
    // The write confirmation follows the two hooks in the message log.
    assert_eq!(&messages(&s)[..2], &["pre".to_string(), "post".to_string()]);
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "b");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn once_autocommand_fires_only_once() {
    let path = temp_file("dispatch_once.py", "x\n");
    let mut s = Session::new();
    s.ex("autocmd BufEnter * ++once echo 'once'").unwrap();
    s.ex(&format!("e {}", path.display())).unwrap();
    s.ex("enew").unwrap();
    assert_eq!(messages(&s), vec!["once".to_string()]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn autocmds_do_not_nest_without_the_nested_flag() {
    // A non-nested autocommand must not trigger further events (autocmd.c).
    let mut s = Session::new();
    s.ex("autocmd BufEnter * echo 'inner'").unwrap();
    s.ex("autocmd User Boom doautocmd BufEnter").unwrap();
    s.ex("doautocmd User Boom").unwrap();
    assert!(messages(&s).is_empty(), "messages: {:?}", messages(&s));

    // With ++nested the inner event is allowed to fire.
    let mut s2 = Session::new();
    s2.ex("autocmd BufEnter * echo 'inner'").unwrap();
    s2.ex("autocmd User Boom ++nested doautocmd BufEnter")
        .unwrap();
    s2.ex("doautocmd User Boom").unwrap();
    assert_eq!(messages(&s2), vec!["inner".to_string()]);
}

// ------------------------------------------------- filetype detection

#[test]
fn setfiletype_fires_the_filetype_event() {
    let mut s = Session::new();
    s.ex("au BufRead *.py setf python").unwrap();
    s.ex("au FileType python echo 'ft:' . &ft").unwrap();
    let path = temp_file("detect.py", "print(1)\n");
    s.ex(&format!("e {}", path.display())).unwrap();
    assert_eq!(messages(&s), vec!["ft:python".to_string()]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn filetype_vim_detects_a_python_file() {
    // End-to-end: `:filetype on` sources the real runtime filetype.vim, whose
    // `au BufRead *.py setf python` rule now actually runs.
    //
    // The baseline checkout is not versioned (scripts/fetch-reference.ps1), so
    // this test skips itself when it is missing instead of failing.
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference/vim/runtime/filetype.vim");
    if !script.is_file() {
        eprintln!("skipping: reference/vim/runtime missing (run scripts/fetch-reference.ps1)");
        return;
    }
    let path = temp_file("real_detect.py", "print(1)\n");
    let mut s = Session::new();
    s.ex("filetype on").unwrap();
    s.ex(&format!("e {}", path.display())).unwrap();
    assert_eq!(s.editor.options.str_or("filetype", ""), "python");
    assert!(s.report.error_count == 0, "no errors expected");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn did_filetype_tracks_the_filetype_option() {
    let mut s = Session::new();
    s.ex("let g:before = did_filetype()").unwrap();
    s.ex("setf c").unwrap();
    s.ex("let g:after = did_filetype()").unwrap();
    assert_eq!(
        s.viml.env.get_var("g:before"),
        Some(rvim_viml::value::Value::Number(0))
    );
    assert_eq!(
        s.viml.env.get_var("g:after"),
        Some(rvim_viml::value::Value::Number(1))
    );
}

#[test]
fn vim9_scripts_are_rejected_with_one_clear_error() {
    // Only a few v8.2.5172 runtime files are Vim9 (`autoload/dist/ft.vim`);
    // sourcing one must fail once with ERV338 instead of cascading errors.
    let path = temp_file(
        "vim9.vim",
        "vim9script\nvar x = 1\nexport def Foo()\nenddef\n",
    );
    let mut s = Session::new();
    let err = s.ex(&format!("source {}", path.display())).unwrap_err();
    assert_eq!(err.code, "ERV338");
    assert_eq!(s.report.error_count, 1);
    let _ = std::fs::remove_file(&path);
}

// --------------------------------------------------- expression rules

#[test]
fn logical_or_is_not_a_command_separator() {
    // `||` must stay inside the expression: vim parses it before looking for a
    // trailing command separator.
    let mut s = Session::new();
    s.ex("echo 1 || 0").unwrap();
    s.ex("if (1 || 0) | echo 'branch' | endif").unwrap();
    assert_eq!(messages(&s), vec!["1".to_string(), "branch".to_string()]);
}

#[test]
fn dot_concatenation_with_a_function_call() {
    // vim decides between `dict.key` and string concatenation by the evaluated
    // left value (eval.c:6420); a function call on the right is concatenation.
    let mut s = Session::new();
    s.ex("echo 'a' . strlen('ab')").unwrap();
    s.ex("echo 'x' . toupper('y')").unwrap();
    assert_eq!(messages(&s), vec!["a2".to_string(), "xY".to_string()]);
}

#[test]
fn dict_member_access_still_works() {
    let mut s = Session::new();
    s.ex("let d = {'key': 'value'}").unwrap();
    s.ex("echo d.key").unwrap();
    assert_eq!(messages(&s), vec!["value".to_string()]);
}

#[test]
fn expr_user_command_executes_the_evaluated_text() {
    // `:command -expr` makes the replacement text a VimL expression whose value
    // is executed as an Ex command.
    let mut s = Session::new();
    s.ex("command! -nargs=1 -expr Font 'set ts=' . <q-args>")
        .unwrap();
    s.ex("Font 4").unwrap();
    assert_eq!(s.editor.options.num_or("tabstop", 0), 4);
}

#[test]
fn new_buffers_enter_through_the_same_chain() {
    let mut s = Session::new();
    seed(&mut s, &["one"]);
    let path = temp_file("dispatch_enew.txt", "x\n");
    s.ex("autocmd BufEnter * echo 'entered'").unwrap();
    // ":e" refuses to abandon the modified seed buffer (E37), so override.
    s.ex(&format!("e! {}", path.display())).unwrap();
    assert_eq!(messages(&s), vec!["entered".to_string()]);
    let _ = std::fs::remove_file(&path);
}
