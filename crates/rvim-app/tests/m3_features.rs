//! M3 tests: VimL expressions, statements, functions, exceptions and sourcing
//! the real runtime scripts (`defaults.vim`, `filetype.vim`).

use rvim_app::Session;
use rvim_viml::Value;

fn session() -> Session {
    Session::new()
}

fn msg(s: &Session) -> String {
    s.message_history().join("\n")
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rvim_m3_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Path of a vendored runtime script, or `None` when the baseline checkout is
/// absent (it is not versioned: run `scripts/fetch-reference.ps1`).
fn runtime_script(name: &str) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference/vim/runtime")
        .join(name);
    if !path.is_file() {
        eprintln!(
            "skipping runtime test: {} missing (run scripts/fetch-reference.ps1)",
            path.display()
        );
        return None;
    }
    Some(path)
}

// ------------------------------------------------------------- expressions

#[test]
fn echo_evaluates_expressions() {
    let mut s = session();
    s.ex("let x = 21").unwrap();
    s.ex("echo x * 2").unwrap();
    s.ex("echo 'a' . 'b' . x").unwrap();
    s.ex("echo [1, 2, 3][1]").unwrap();
    s.ex("echo {'k': 'v'}.k").unwrap();
    assert_eq!(msg(&s), "42\nab21\n2\nv");
}

#[test]
fn string_and_number_coercions_follow_vim() {
    let mut s = session();
    s.ex("echo 10 / 3").unwrap();
    s.ex("echo 10 / 3.0").unwrap();
    s.ex("echo '12abc' + 3").unwrap();
    s.ex("echo '5' > 4").unwrap();
    assert_eq!(msg(&s), "3\n3.333333\n15\n1"); // vim '%g' (6 significant digits)
}

#[test]
fn comparison_and_regex_operators() {
    let mut s = session();
    s.ex("echo 'Evim' =~? 'evim'").unwrap();
    s.ex("echo 'Evim' =~# 'evim'").unwrap();
    s.ex("echo 'xterm-256color' =~ '^xterm'").unwrap();
    s.ex("echo 'abc' !~ 'z'").unwrap();
    s.ex("echo 2 == 2 && 3 != 4").unwrap();
    assert_eq!(msg(&s), "1\n0\n1\n1\n1");
}

// ------------------------------------------------------------------ :let

#[test]
fn let_scopes_and_operators() {
    let mut s = session();
    s.ex("let g:foo = 1").unwrap();
    s.ex("let foo += 9").unwrap();
    s.ex("let list = [1, 2]").unwrap();
    s.ex("let list += [3]").unwrap();
    s.ex("let str = 'ab' . 'cd'").unwrap();
    s.ex("echo g:foo foo str list").unwrap();
    // Undefined global becomes a script/global variable; b:/g: prefixes work.
    assert_eq!(msg(&s), "10 10 abcd [1, 2, 3]");
}

#[test]
fn let_option_and_missing_variable_error() {
    let mut s = session();
    s.ex("let &tabstop = 4").unwrap();
    assert_eq!(s.editor.options.num_or("tabstop", 0), 4);
    assert_eq!(s.ex("let nope = undefined_var").unwrap_err().code, "E121");
}

#[test]
fn let_container_elements_and_slices() {
    let mut s = session();
    s.ex("let d = {'a': 1}").unwrap();
    s.ex("let d.b = 2").unwrap();
    s.ex("let l = [1, 2, 3]").unwrap();
    s.ex("let l[0] = 9").unwrap();
    s.ex("echo d l").unwrap();
    assert_eq!(msg(&s), "{'a': 1, 'b': 2} [9, 2, 3]");
}

// ------------------------------------------------------------ control flow

#[test]
fn if_elseif_else_branches() {
    let mut s = session();
    s.ex("let n = 2").unwrap();
    s.ex("if n == 1 | echo 'one' | elseif n == 2 | echo 'two' | else | echo 'many' | endif")
        .unwrap();
    assert_eq!(msg(&s), "two");
}

#[test]
fn while_loop_with_break_and_continue() {
    let dir = temp_dir("while");
    let path = dir.join("loop.vim");
    std::fs::write(
        &path,
        "let i = 0\n\
         let out = ''\n\
         while i < 10\n\
           let i += 1\n\
           if i == 3\n\
             continue\n\
           endif\n\
           if i == 6\n\
             break\n\
           endif\n\
           let out .= i\n\
         endwhile\n\
         echomsg out\n",
    )
    .unwrap();
    let mut s = session();
    s.source(&path).unwrap();
    assert_eq!(msg(&s), "1245");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn for_loop_over_list_and_range() {
    let dir = temp_dir("for");
    let path = dir.join("for.vim");
    std::fs::write(
        &path,
        "let out = []\n\
         for x in range(1, 3)\n\
           call add(out, x * 2)\n\
         endfor\n\
         echomsg string(out)\n",
    )
    .unwrap();
    let mut s = session();
    s.source(&path).unwrap();
    assert_eq!(msg(&s), "[2, 4, 6]");
    let _ = std::fs::remove_dir_all(&dir);
}

// -------------------------------------------------------------- functions

#[test]
fn functions_with_args_varargs_and_return() {
    let dir = temp_dir("func");
    let path = dir.join("func.vim");
    std::fs::write(
        &path,
        "function! s:Sum(a, b)\n\
           return a:a + a:b\n\
         endfunction\n\
         function! s:Count(first, ...)\n\
           return a:first + a:0\n\
         endfunction\n\
         echomsg s:Sum(2, 3)\n\
         echomsg s:Count(10, 1, 2, 3)\n",
    )
    .unwrap();
    let mut s = session();
    s.source(&path).unwrap();
    assert_eq!(msg(&s), "5\n13");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lambdas_and_functional_builtins() {
    let mut s = session();
    s.ex("echo map([1, 2, 3], {i, v -> v * 10})").unwrap();
    s.ex("echo filter([1, 2, 3, 4], {i, v -> v % 2 == 0})")
        .unwrap();
    s.ex("let Add = {a, b -> a + b}").unwrap();
    s.ex("echo Add(2, 5)").unwrap();
    assert_eq!(msg(&s), "[10, 20, 30]\n[2, 4]\n7");
}

#[test]
fn function_listing_and_deletion() {
    let dir = temp_dir("delfunc");
    let path = dir.join("d.vim");
    std::fs::write(&path, "function! MyHelper()\n  return 1\nendfunction\n").unwrap();
    let mut s = session();
    s.source(&path).unwrap();
    s.ex("function").unwrap();
    assert!(msg(&s).contains("MyHelper"));
    s.ex("delfunction MyHelper").unwrap();
    assert_eq!(s.ex("MyHelper()").unwrap_err().code, "E492");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------- exceptions

#[test]
fn try_catch_throw_and_finally() {
    let dir = temp_dir("try");
    let path = dir.join("t.vim");
    std::fs::write(
        &path,
        "let out = ''\n\
         try\n\
           throw 'boom'\n\
           let out .= 'unreachable'\n\
         catch /boom/\n\
           let out .= 'caught'\n\
         finally\n\
           let out .= '-done'\n\
         endtry\n\
         echomsg out\n",
    )
    .unwrap();
    let mut s = session();
    s.source(&path).unwrap();
    assert_eq!(msg(&s), "caught-done");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn caught_error_inside_try() {
    let dir = temp_dir("try2");
    let path = dir.join("t.vim");
    std::fs::write(
        &path,
        "let out = 'none'\n\
         try\n\
           call undefined_function_xyz()\n\
         catch /E117/\n\
           let out = 'caught'\n\
         endtry\n\
         echomsg out\n",
    )
    .unwrap();
    let mut s = session();
    s.source(&path).unwrap();
    assert_eq!(msg(&s), "caught");
    let _ = std::fs::remove_dir_all(&dir);
}

// -------------------------------------------------------- execute and eval

#[test]
fn execute_builds_and_runs_commands() {
    let mut s = session();
    s.ex("let opt = 'tabstop'").unwrap();
    s.ex("execute 'set ' . opt . '=3'").unwrap();
    assert_eq!(s.editor.options.num_or("tabstop", 0), 3);
    s.ex("echo eval('1 + 2')").unwrap();
    assert_eq!(msg(&s), "3");
}

// ------------------------------------------------------------- autoload

#[test]
fn autoload_function_loads_on_first_call() {
    let dir = temp_dir("autoload");
    let autoload = dir.join("autoload");
    std::fs::create_dir_all(&autoload).unwrap();
    std::fs::write(
        autoload.join("demo.vim"),
        "function! demo#hello(name)\n  return 'hi ' . a:name\nendfunction\n",
    )
    .unwrap();
    let mut s = session();
    s.ex(&format!("set runtimepath={}", dir.display())).unwrap();
    s.ex("echo demo#hello('vim')").unwrap();
    assert_eq!(msg(&s), "hi vim");
    let _ = std::fs::remove_dir_all(&dir);
}

// --------------------------------------------------------- runtime scripts

#[test]
fn sources_defaults_vim_cleanly() {
    let Some(script) = runtime_script("defaults.vim") else {
        return;
    };
    let mut s = session();
    let before = s.message_history().len();
    s.source(&script).unwrap();
    let errors: Vec<&str> = s.message_history()[before..]
        .iter()
        .filter(|m| m.contains("E1") || m.contains("E4") || m.contains("E5"))
        .map(|m| m.as_str())
        .collect();
    assert!(errors.is_empty(), "defaults.vim errors: {errors:?}");
    assert!(s.editor.options.bool_or("ruler", false));
    assert!(s.editor.options.bool_or("showcmd", false));
    assert!(s.editor.options.bool_or("wildmenu", false));
    assert_eq!(s.editor.options.num_or("history", 0), 200);
    assert_eq!(s.editor.options.num_or("scrolloff", 0), 5);
    // `map Q gq` from defaults.vim registered a normal-mode mapping.
    assert!(
        !s.editor
            .maps
            .entries_for(rvim_editor::MapMode::Normal)
            .is_empty()
    );
}

#[test]
fn sources_filetype_vim_cleanly() {
    let Some(script) = runtime_script("filetype.vim") else {
        return;
    };
    let mut s = session();
    let before = s.message_history().len();
    s.source(&script).unwrap();
    let errors: Vec<&str> = s.message_history()[before..]
        .iter()
        .filter(|m| m.contains("E1") || m.contains("E4") || m.contains("E5"))
        .map(|m| m.as_str())
        .collect();
    assert!(errors.is_empty(), "filetype.vim errors: {errors:?}");
    // The script registers hundreds of autocommands and two helper functions.
    assert!(
        s.editor.autocmds.entries().len() > 700,
        "expected >700 autocmds, got {}",
        s.editor.autocmds.entries().len()
    );
    assert!(s.viml.state.funcs.keys().any(|k| k.contains("StarSetf")));
    // `let did_load_filetypes = 1` and `g:ft_ignore_pat` are set.
    assert_eq!(
        s.viml.env.get_var("did_load_filetypes").unwrap(),
        Value::Number(1)
    );
    assert!(s.viml.env.get_var("g:ft_ignore_pat").is_some());
}

#[test]
fn script_scope_is_isolated_per_script() {
    let dir = temp_dir("sid");
    std::fs::write(
        dir.join("a.vim"),
        "let s:value = 'a'\nfunction! s:Get()\n  return s:value\nendfunction\nechomsg s:Get()\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("b.vim"),
        "if exists('s:value')\n  echomsg 'leaked'\nelse\n  echomsg 'isolated'\nendif\n",
    )
    .unwrap();
    let mut s = session();
    s.source(&dir.join("a.vim")).unwrap();
    s.source(&dir.join("b.vim")).unwrap();
    assert_eq!(msg(&s), "a\nisolated");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------- host builtin functions

#[test]
fn buffer_and_line_builtins() {
    let mut s = session();
    s.ex("call setline(1, ['alpha', 'beta', 'gamma'])").unwrap();
    s.ex("echo getline(2)").unwrap();
    s.ex("echo getbufline(1, 1, 2)").unwrap();
    s.ex("call append(0, 'header')").unwrap();
    s.ex("echo getline(1)").unwrap();
    s.ex("call setbufline(1, 2, 'second')").unwrap();
    s.ex("echo getbufline(1, 1, 2)").unwrap();
    assert_eq!(
        msg(&s),
        "beta\n['alpha', 'beta']\nheader\n['header', 'second']"
    );
}

#[test]
fn buffer_query_and_position_builtins() {
    let path = std::env::temp_dir().join("rvim_m3_bufname.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
    let mut s = session();
    s.ex(&format!("edit {}", path.display())).unwrap();
    s.ex("echo bufname('%')").unwrap();
    s.ex("echo bufnr('%')").unwrap();
    s.ex("echo bufexists(bufnr('%'))").unwrap();
    s.ex("call setpos('.', [0, 2, 1, 0])").unwrap();
    s.ex("echo getcurpos()[1:2]").unwrap();
    s.ex("echo getpos('.')[1:2]").unwrap();
    s.ex("echo line2byte(2)").unwrap();
    s.ex("echo byte2line(5)").unwrap();
    s.ex("echo getbufinfo()[0].linecount").unwrap();
    let messages: Vec<&str> = s.message_history().iter().map(String::as_str).collect();
    assert!(
        messages.iter().any(|m| m.contains("rvim_m3_bufname.txt")),
        "bufname: {messages:?}"
    );
    // Buffer is 1, cursor after setpos is line 2 column 1.
    assert_eq!(messages[1], "1"); // the startup [No Name] buffer is reused for the first file
    assert_eq!(messages[2], "1");
    assert_eq!(messages[3], "[2, 1]");
    assert_eq!(messages[4], "[2, 1]");
    assert_eq!(messages[5], "5");
    assert_eq!(messages[6], "2");
    assert_eq!(messages[7], "3");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn register_builtins() {
    let mut s = session();
    s.ex("call setreg('a', ['x', 'y'], 'V')").unwrap();
    s.ex("echo getreg('a')").unwrap();
    s.ex("echo getregtype('a')").unwrap();
    s.ex("call setreg('b', 'text')").unwrap();
    s.ex("echo getregtype('b')").unwrap();
    let text = msg(&s);
    assert!(text.contains('x') && text.contains('y'), "getreg: {text:?}");
    assert!(text.contains("V") && text.contains('v'), "types: {text:?}");
}
