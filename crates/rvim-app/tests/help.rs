//! Offline Vim runtime help, topic navigation and relocatable installations.
use rvim_app::{Session, tui::Tui};
use rvim_core::Pos;
use rvim_editor::{OptValue, maps::Key};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rvim_help_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let doc = root.join("runtime/doc");
        std::fs::create_dir_all(&doc).unwrap();
        std::fs::write(
            doc.join("help.txt"),
            "*help.txt* Offline help\nRead |CTRL-W_s| and |'number'|.\n",
        )
        .unwrap();
        std::fs::write(
            doc.join("windows.txt"),
            "*CTRL-W_s* Split windows\n*:split* Ex split\n",
        )
        .unwrap();
        std::fs::write(
            doc.join("options.txt"),
            "*'number'* Show line numbers\n*map()* Transform values\n*!* Shell command\n",
        )
        .unwrap();
        rvim_ex::handlers_help::generate_tags(&doc).unwrap();
        Self { root }
    }
    fn doc(&self) -> PathBuf {
        self.root.join("runtime/doc")
    }
    fn session(&self) -> Session {
        let mut s = Session::new();
        s.editor
            .options
            .set(
                "runtimepath",
                OptValue::Str(self.root.join("runtime").display().to_string()),
            )
            .unwrap();
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let (Ok(path), Ok(base)) = (
            self.root.canonicalize(),
            std::env::temp_dir().canonicalize(),
        ) {
            if path != base && path.starts_with(base) {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
}

#[test]
fn help_opens_a_split_without_replacing_the_unsaved_source() {
    let f = Fixture::new();
    let mut s = f.session();
    s.editor.insert_text(Pos::new(0, 0), "unsaved").unwrap();
    let source = s.editor.current_bufnr();
    s.ex("help").unwrap();
    assert_eq!(s.editor.window_count(), 2);
    assert_eq!(
        s.editor.current_buffer().name.as_deref(),
        Some(f.doc().join("help.txt").canonicalize().unwrap().as_path())
    );
    assert_eq!(s.editor.current_line(), "*help.txt* Offline help");
    assert_eq!(s.editor.buffer(source).unwrap().full_text(), "unsaved");
    assert_eq!(s.editor.wopt_str("buftype", ""), "help");
    assert_eq!(s.editor.wopt_str("filetype", ""), "help");
    s.ex("call assert_equal('help', &buftype)").unwrap();
    s.ex("call assert_equal('help', &l:filetype)").unwrap();
    s.ex("call assert_equal('', &g:buftype)").unwrap();
    assert!(
        s.viml
            .env
            .get_var("v:errors")
            .is_none_or(|v| v.to_string_value() == "[]")
    );
}

#[test]
fn help_topics_reuse_the_pane_and_resolve_options_commands_and_functions() {
    let f = Fixture::new();
    let mut s = f.session();
    for (topic, marker) in [
        ("number", "*'number'*"),
        ("'number'", "*'number'*"),
        ("split", "*:split*"),
        ("map", "*map()*"),
        ("ctrl-w_s", "*CTRL-W_s*"),
        ("!", "*!*"),
    ] {
        s.ex(&format!("help {topic}")).unwrap();
        assert!(s.editor.current_line().contains(marker), "{topic}");
        assert_eq!(s.editor.window_count(), 2);
    }
}

#[test]
fn help_links_and_tag_pop_preserve_punctuation_and_positions() {
    let f = Fixture::new();
    let mut s = f.session();
    s.ex("help").unwrap();
    s.editor.set_cursor(Pos::new(1, 7));
    let original = s.editor.cursor();
    let mut tui = Tui::new(s);
    tui.feed_key(Key::Special("C-]".to_string())).unwrap();
    assert!(tui.session.editor.current_line().contains("*CTRL-W_s*"));
    tui.feed_key(Key::Special("C-T".to_string())).unwrap();
    assert!(tui.session.editor.current_line().contains("|CTRL-W_s|"));
    assert_eq!(tui.session.editor.cursor(), original);
}

#[test]
fn helpclose_returns_to_the_source_even_when_invoked_from_another_pane() {
    let f = Fixture::new();
    let mut s = f.session();
    let source = s.editor.windows.current_id();
    s.ex("help").unwrap();
    s.editor.windows.goto(source).unwrap();
    s.ex("helpclose").unwrap();
    assert_eq!(s.editor.window_count(), 1);
    assert!(!s.editor.current_buffer().is_help());
    assert_ne!(s.editor.wopt_str("buftype", ""), "help");
    s.editor.insert_text(Pos::new(0, 0), "editable").unwrap();
    s.ex("helpclose").unwrap();
}

#[test]
fn unknown_or_stale_topics_do_not_change_windows() {
    let f = Fixture::new();
    let mut s = f.session();
    assert_eq!(s.ex("help no-such-topic").unwrap_err().code, "E149");
    assert_eq!(s.editor.window_count(), 1);
    std::fs::write(f.doc().join("help.txt"), "no marker").unwrap();
    assert_eq!(s.ex("help").unwrap_err().code, "E434");
    assert_eq!(s.editor.window_count(), 1);
}

#[test]
fn help_is_nonmodifiable_and_readonly_without_affecting_other_buffers() {
    let f = Fixture::new();
    let mut s = f.session();
    s.ex("help").unwrap();
    let original = s.editor.current_buffer().full_text();
    assert_eq!(
        s.editor
            .insert_text(Pos::new(0, 0), "bad")
            .unwrap_err()
            .code,
        "E21"
    );
    assert_eq!(s.ex("%s/help/BAD/g").unwrap_err().code, "E21");
    assert_eq!(s.editor.current_buffer().full_text(), original);
    s.ex("setlocal modifiable").unwrap();
    s.editor.insert_text(Pos::new(0, 0), "edit").unwrap();
    assert_eq!(s.ex("write").unwrap_err().code, "E45");
    assert!(
        !std::fs::read_to_string(f.doc().join("help.txt"))
            .unwrap()
            .starts_with("edit")
    );
}

#[test]
fn help_topic_completion_uses_the_runtime_index() {
    let f = Fixture::new();
    let mut tui = Tui::new(f.session());
    let mut keys = rvim_term::ScriptedKeys::from_notation(":help CTRL-W_<Tab><CR>");
    tui.run(&mut keys, &mut Vec::new(), 80, 24).unwrap();
    assert!(tui.session.editor.current_line().contains("*CTRL-W_s*"));
}

#[test]
fn helptags_indexes_nested_files_and_sorts_the_result() {
    let f = Fixture::new();
    let mut s = f.session();
    let nested = f.doc().join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("extra.txt"), "*z-topic* Another topic\n").unwrap();
    s.ex(&format!("helptags {}", f.doc().display())).unwrap();
    let text = std::fs::read_to_string(f.doc().join("tags")).unwrap();
    assert!(text.contains("z-topic\tnested/extra.txt\t/*z-topic*"));
    assert!(
        text.lines()
            .collect::<Vec<_>>()
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    );
    s.ex("help z-topic").unwrap();
    assert!(s.editor.current_line().contains("*z-topic*"));
}

#[test]
fn duplicate_helptags_report_an_error_without_clobbering_the_index() {
    let f = Fixture::new();
    let mut s = f.session();
    let before = std::fs::read(f.doc().join("tags")).unwrap();
    std::fs::write(f.doc().join("duplicate.txt"), "*help.txt* Duplicate\n").unwrap();
    assert_eq!(
        s.ex(&format!("helptags {}", f.doc().display()))
            .unwrap_err()
            .code,
        "E154"
    );
    assert_eq!(std::fs::read(f.doc().join("tags")).unwrap(), before);
}

#[test]
fn installed_runtime_discovery_handles_spaces_and_unrelated_working_directories() {
    let f = Fixture::new();
    let install = f.root.join("Program Files/RustVim");
    std::fs::create_dir_all(install.join("runtime/doc")).unwrap();
    std::fs::write(install.join("runtime/doc/help.txt"), "help").unwrap();
    assert_eq!(
        rvim_app::runtime_near_executable(&install.join("rvim.exe")),
        Some(install.join("runtime").canonicalize().unwrap())
    );
}

#[test]
fn real_bundled_vim_topics_open_offline() {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/vim/runtime");
    if !runtime.join("doc/tags").is_file() {
        eprintln!("reference runtime unavailable");
        return;
    }
    let mut s = Session::new();
    s.editor
        .options
        .set("runtimepath", OptValue::Str(runtime.display().to_string()))
        .unwrap();
    for topic in [
        "help.txt",
        "'number'",
        "CTRL-W_s",
        ":help",
        "usr_01.txt",
        "map()",
        "!",
        "!!",
    ] {
        s.ex(&format!("help {topic}"))
            .unwrap_or_else(|err| panic!("{topic}: {err}"));
        assert!(s.editor.current_buffer().is_help());
        assert!(s.editor.current_line().contains(&format!("*{topic}*")));
    }
}
