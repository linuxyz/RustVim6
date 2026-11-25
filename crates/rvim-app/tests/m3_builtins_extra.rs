//! M3 builtin completion tests: the editor-facing builtins that used to be
//! missing (search family, register info, autocmds, strptime, batch stubs).

use rvim_app::Session;
use rvim_viml::Value;

fn global(s: &Session, name: &str) -> Value {
    s.viml
        .env
        .globals
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("missing g:{name}"))
}

#[test]
fn search_and_searchpos_move_the_cursor() {
    let mut s = Session::new();
    s.editor
        .replace_lines(0, 1, vec!["alpha".into(), "beta".into(), "gamma".into()])
        .unwrap();
    s.ex("call search('beta')").unwrap();
    s.ex("let g:line = line('.')").unwrap();
    assert_eq!(global(&s, "line").to_number(), 2);
    s.ex("let g:pos = searchpos('gamma', 'W')").unwrap();
    assert_eq!(global(&s, "pos").to_string_value(), "[3, 1]");
}

#[test]
fn getreginfo_reports_the_register() {
    let mut s = Session::new();
    s.ex("let @a = \"one\\ntwo\"").unwrap();
    s.ex("let g:info = getreginfo('a')").unwrap();
    let info = global(&s, "info");
    assert!(
        info.to_string_value().contains("'one'"),
        "regcontents missing: {info:?}"
    );
}

#[test]
fn autocmd_add_get_delete_round_trip() {
    let mut s = Session::new();
    s.ex("call autocmd_add([{'event': 'BufRead', 'pattern': '*.zz', 'cmd': 'echo 1'}])")
        .unwrap();
    s.ex("let g:list = autocmd_get()").unwrap();
    let list = global(&s, "list");
    assert!(list.to_string_value().contains("*.zz"));
    s.ex("call autocmd_delete([{'event': 'BufRead', 'pattern': '*.zz'}])")
        .unwrap();
    s.ex("let g:after = autocmd_get()").unwrap();
    assert!(!global(&s, "after").to_string_value().contains("*.zz"));
}

#[test]
fn strptime_parses_dates() {
    let mut s = Session::new();
    s.ex("let g:ts = strptime('2000-01-01', '%F')").unwrap();
    assert_eq!(global(&s, "ts").to_number(), 946_684_800);
}

#[test]
fn setenv_and_getenv_round_trip() {
    let mut s = Session::new();
    s.ex("call setenv('RVIM_M3_BUILTIN_TEST', 'ok')").unwrap();
    s.ex("let g:env = getenv('RVIM_M3_BUILTIN_TEST')").unwrap();
    assert_eq!(global(&s, "env").to_string_value(), "ok");
}

#[test]
fn unsupported_language_eval_reports_e319() {
    let mut s = Session::new();
    let error = s.ex("call pyeval('1 + 1')").unwrap_err();
    assert_eq!(error.code, "E319");
}
