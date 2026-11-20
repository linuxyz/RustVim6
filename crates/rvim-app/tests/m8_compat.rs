//! M8 compatibility: :redir and the has() feature list.

use rvim_app::Session;

#[test]
fn redir_captures_echo_into_a_variable() {
    let mut s = Session::new();
    s.ex("redir => g:out").unwrap();
    s.ex("echo 'hello'").unwrap();
    s.ex("redir END").unwrap();
    let value = s.viml.env.get_var("g:out").unwrap();
    assert_eq!(value.to_string_value(), "hello\n");
}

#[test]
fn redir_captures_into_a_register() {
    let mut s = Session::new();
    s.ex("redir @a").unwrap();
    s.ex("echo 'register text'").unwrap();
    s.ex("redir END").unwrap();
    assert_eq!(
        s.editor.registers.get('a').map(|r| r.text.clone()),
        Some("register text\n".to_string())
    );
}

#[test]
fn redir_writes_a_file() {
    let path = std::env::temp_dir().join("rvim_m8_redir.txt");
    let _ = std::fs::remove_file(&path);
    let mut s = Session::new();
    s.ex(&format!("redir > {}", path.display())).unwrap();
    s.ex("echo 'to file'").unwrap();
    s.ex("redir END").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "to file\n");
    let _ = std::fs::remove_file(path);
}

#[test]
fn mksession_and_mkview_write_sourceable_files() {
    let dir = std::env::temp_dir().join("rvim_m8_session");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let session = dir.join("Session.vim");
    let view = dir.join("View.vim");
    let mut s = Session::new();
    s.open_file(&file).unwrap();
    s.editor.set_cursor(rvim_core::Pos::new(2, 0));
    s.ex(&format!("mksession {}", session.display())).unwrap();
    s.ex(&format!("mkview {}", view.display())).unwrap();
    let session_text = std::fs::read_to_string(&session).unwrap();
    assert!(session_text.contains("a.txt"), "{session_text}");
    assert!(session_text.contains("edit "), "{session_text}");
    let view_text = std::fs::read_to_string(&view).unwrap();
    assert!(view_text.contains("3G"), "{view_text}");

    let mut restored = Session::new();
    let _ = restored.source(&session);
    assert!(
        restored
            .editor
            .current_buffer()
            .display_name()
            .contains("a.txt")
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn has_reports_the_m8_features() {
    let mut s = Session::new();
    for feature in [
        "crypt-blowfish2",
        "crypt-blowfish",
        "crypt-zip",
        "spell",
        "syntax",
        "quickfix",
        "folding",
        "diff",
        "windows",
        "tabpages",
        "eval",
        "multi_byte",
    ] {
        let script = format!(
            "call writefile([string(has('{feature}'))], '{}')",
            std::env::temp_dir().join("rvim_m8_has.txt").display()
        );
        s.ex(&script).unwrap();
        let text = std::fs::read_to_string(std::env::temp_dir().join("rvim_m8_has.txt"))
            .unwrap_or_default();
        assert_eq!(text.trim(), "1", "has('{feature}')");
    }
    let patch = std::env::temp_dir().join("rvim_m8_patch.txt");
    s.ex(&format!(
        "call writefile([string(has('patch-8.2.5172')), string(has('patch-8.2.5173')), string(has('patch-9.0.1'))], '{}')",
        patch.display()
    ))
    .unwrap();
    let text = std::fs::read_to_string(&patch).unwrap_or_default();
    assert_eq!(text.lines().collect::<Vec<_>>(), vec!["1", "0", "0"]);
    let _ = std::fs::remove_file(patch);
    let script = format!(
        "call writefile([string(has('terminal')), string(has('job')), string(has('channel'))], '{}')",
        std::env::temp_dir().join("rvim_m8_has.txt").display()
    );
    s.ex(&script).unwrap();
    let text =
        std::fs::read_to_string(std::env::temp_dir().join("rvim_m8_has.txt")).unwrap_or_default();
    assert_eq!(text.lines().collect::<Vec<_>>(), vec!["1", "1", "1"]);
    let _ = std::fs::remove_file(std::env::temp_dir().join("rvim_m8_has.txt"));
}
