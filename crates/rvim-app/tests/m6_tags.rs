//! M6 tag-subsystem tests: ctags files, :tag/:tselect/:pop, the tag stack and
//! the CTRL-] / CTRL-T TUI keys.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_core::Pos;
use rvim_editor::OptValue;
use rvim_term::ScriptedKeys;
use rvim_viml::value::Value;

/// One temporary directory with a tags file and its target files.
struct Fixture {
    dir: PathBuf,
    tags: PathBuf,
    a: PathBuf,
    b: PathBuf,
    main: PathBuf,
    punctuation: PathBuf,
}

impl Fixture {
    fn open(&self) -> Session {
        session_with_tags(self, &self.main)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "rvim_m6_tags_{}_{unique}_{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(tag: &str) -> Fixture {
    let dir = unique_dir(tag);
    let a = dir.join("a.txt");
    std::fs::write(&a, "first line\nsecond line\nthird line\n").unwrap();
    let b = dir.join("b.txt");
    std::fs::write(&b, "b one\nb two\nb three\nb four\n").unwrap();
    let main = dir.join("main.txt");
    std::fs::write(&main, "alpha beta\nzeta gamma\n").unwrap();
    let punctuation = dir.join("punctuation.txt");
    std::fs::write(&punctuation, "...\n").unwrap();
    let tags = dir.join("tags");
    // Relative file names (resolved against the tags file's directory), pseudo
    // tags, a /pattern/ address and a plain (delimiter-less) search pattern.
    let text = [
        "!_TAG_FILE_FORMAT\t2\t/extended format/",
        "!_TAG_FILE_SORTED\t1\t/0=unsorted, 1=sorted/",
        "alpha\ta.txt\t1",
        "alpha\tb.txt\t3",
        "beta\ta.txt\t/^second line$/",
        "delta\ta.txt\tthird line",
        "gamma\tb.txt\t2",
    ]
    .join("\n")
        + "\n";
    std::fs::write(&tags, text).unwrap();
    Fixture {
        dir,
        tags,
        a,
        b,
        main,
        punctuation,
    }
}

fn session_with_tags(fixture: &Fixture, source: &Path) -> Session {
    let mut session = Session::new();
    session
        .editor
        .options
        .set("tags", OptValue::Str(fixture.tags.display().to_string()))
        .unwrap();
    session.open_file(source).unwrap();
    session
}

fn buffer_name(session: &Session) -> Option<PathBuf> {
    session.editor.current_buffer().name.clone()
}

fn stack_dict(session: &mut Session, variable: &str) -> Value {
    session
        .ex(&format!("let g:{variable} = gettagstack()"))
        .unwrap();
    session
        .viml
        .env
        .get_var(&format!("g:{variable}"))
        .expect("gettagstack() returned a value")
}

fn number_field(dict: &Value, key: &str) -> i64 {
    let Value::Dict(dict) = dict else {
        panic!("dictionary expected");
    };
    let dict = dict.borrow();
    dict.get(key).expect("field present").to_number()
}

// ---------------------------------------------------------------- :tag jumps

#[test]
fn tag_opens_the_target_file_and_line() {
    let fixture = fixture("jump");
    let mut session = fixture.open();

    session.ex("tag alpha").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.a.as_path()));
    assert_eq!(session.editor.cursor().line, 0);

    // ":2tag" selects the second match of the same name.
    session.ex("2tag alpha").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.b.as_path()));
    assert_eq!(session.editor.cursor().line, 2);

    // A "/pattern/" command field searches the target file.
    session.ex("tag beta").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.a.as_path()));
    assert_eq!(session.editor.cursor().line, 1, "matched 'second line'");
    assert_eq!(session.editor.cursor().col, 0, "first non-blank");

    // A bare pattern (no delimiters) is a search pattern too.
    session.ex("tag delta").unwrap();
    assert_eq!(session.editor.cursor().line, 2);

    let error = session.ex("tag nosuchtag").unwrap_err();
    assert_eq!(error.code, "E426");
    assert!(error.message.contains("nosuchtag"), "{}", error.message);
}

#[test]
fn tag_without_argument_uses_the_keyword_under_the_cursor() {
    let fixture = fixture("cword");
    let mut session = fixture.open();

    session.editor.set_cursor(Pos::new(0, 6)); // on "beta"
    session.ex("tag").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.a.as_path()));
    assert_eq!(session.editor.cursor().line, 1);

    // No keyword under the cursor is E349.
    let mut punct = session_with_tags(&fixture, &fixture.punctuation);
    punct.editor.set_cursor(Pos::new(0, 0));
    let error = punct.ex("tag").unwrap_err();
    assert_eq!(error.code, "E349");
}

#[test]
fn tag_honours_ignorecase() {
    let fixture = fixture("ignorecase");
    let mut session = fixture.open();

    // Without 'ignorecase' the name must match exactly.
    let error = session.ex("tag ALPHA").unwrap_err();
    assert_eq!(error.code, "E426");

    session.ex("set ignorecase").unwrap();
    session.ex("tag ALPHA").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.a.as_path()));
    assert_eq!(session.editor.cursor().line, 0);
}

// ------------------------------------------------------- :tselect and :tnext

#[test]
fn tselect_lists_matches_and_tnext_cycles() {
    let fixture = fixture("select");
    let mut session = fixture.open();

    session.ex("tselect alpha").unwrap();
    let listing = session.message_history().join("\n");
    assert!(listing.contains("  # pri kind tag"), "{listing}");
    assert!(listing.contains("a.txt"), "{listing}");
    assert!(listing.contains("b.txt"), "{listing}");
    // Listing alone does not jump.
    assert_eq!(
        buffer_name(&session).as_deref(),
        Some(fixture.main.as_path())
    );

    // :tnext starts at the first remembered match and wraps.
    session.ex("tnext").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.a.as_path()));
    assert_eq!(session.editor.cursor().line, 0);
    session.ex("tnext").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.b.as_path()));
    assert_eq!(session.editor.cursor().line, 2);
    session.ex("tnext").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.a.as_path()));

    session.ex("tprevious").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.b.as_path()));
    session.ex("tfirst").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.a.as_path()));
    session.ex("tlast").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.b.as_path()));
    session.ex("trewind").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.a.as_path()));
}

#[test]
fn tjump_jumps_single_matches_and_lists_multiple_ones() {
    let fixture = fixture("tjump");
    let mut session = fixture.open();

    // "gamma" has exactly one match.
    session.ex("tjump gamma").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.b.as_path()));
    assert_eq!(session.editor.cursor().line, 1);

    // "alpha" has two: :tjump lists instead of jumping.
    let before = session.message_history().len();
    session.ex("tjump alpha").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.b.as_path()));
    let listed = session.message_history()[before..].join("\n");
    assert!(listed.contains("  # pri kind tag"), "{listed}");
    assert!(listed.contains("a.txt"), "{listed}");
}

// ------------------------------------------------------------- the tag stack

#[test]
fn pop_restores_the_previous_buffer_and_line() {
    let fixture = fixture("pop");
    let mut session = fixture.open();
    session.editor.set_cursor(Pos::new(1, 3));
    let main_bufnr = session.editor.current_bufnr();

    session.ex("2tag alpha").unwrap();
    assert_eq!(buffer_name(&session).as_deref(), Some(fixture.b.as_path()));
    assert_eq!(session.editor.cursor().line, 2);

    // gettagstack(): one entry, curidx points one past it (Vim's model).
    let stack = stack_dict(&mut session, "stack");
    assert_eq!(number_field(&stack, "length"), 1);
    assert_eq!(number_field(&stack, "curidx"), 2);
    let Value::Dict(dict) = &stack else {
        panic!("dictionary expected");
    };
    let dict = dict.borrow();
    let Some(Value::List(items)) = dict.get("items") else {
        panic!("items list expected");
    };
    let items = items.borrow();
    let Value::Dict(item) = &items[0] else {
        panic!("item dictionary expected");
    };
    let item = item.borrow();
    assert_eq!(
        item.get("tagname").unwrap().to_string_value(),
        "alpha".to_string()
    );
    assert_eq!(item.get("matchnr").unwrap().to_number(), 2);
    assert_eq!(item.get("bufnr").unwrap().to_number(), main_bufnr as i64);
    let Some(Value::List(from)) = item.get("from") else {
        panic!("from list expected");
    };
    let from = from.borrow();
    assert_eq!(from[0].to_number(), main_bufnr as i64);
    assert_eq!(from[1].to_number(), 2, "1-based line of the saved position");
    assert_eq!(from[2].to_number(), 4, "1-based column");

    // :pop restores buffer and cursor and moves curidx back to 1.
    session.ex("pop").unwrap();
    assert_eq!(
        buffer_name(&session).as_deref(),
        Some(fixture.main.as_path())
    );
    assert_eq!(session.editor.cursor().line, 1);
    assert_eq!(session.editor.cursor().col, 3);
    let stack = stack_dict(&mut session, "popped");
    assert_eq!(number_field(&stack, "length"), 1);
    assert_eq!(number_field(&stack, "curidx"), 1);

    // Nothing left to pop.
    let error = session.ex("pop").unwrap_err();
    assert_eq!(error.code, "E73");
    assert!(
        error.message.contains("Tag stack empty"),
        "{}",
        error.message
    );
}

#[test]
fn tags_command_prints_the_stack() {
    let fixture = fixture("stack");
    let mut session = fixture.open();
    session.ex("tag alpha").unwrap();
    session.ex("tag gamma").unwrap();

    session.ex("tags").unwrap();
    let report = session.message_history().join("\n");
    assert!(report.contains("  # TO tag"), "{report}");
    assert!(report.contains("alpha"), "{report}");
    assert!(report.contains("gamma"), "{report}");
    // "> " marks the current entry.
    assert!(
        session
            .message_history()
            .iter()
            .any(|line| line.starts_with('>') && line.contains("gamma")),
        "{report}"
    );
}

// ------------------------------------------------------------------ the TUI

fn feed(tui: &mut Tui, notation: &str) {
    let mut keys = ScriptedKeys::from_notation(notation);
    let mut sink: Vec<u8> = Vec::new();
    tui.run(&mut keys, &mut sink, 40, 8).unwrap();
}

#[test]
fn tui_ctrl_bracket_jumps_and_ctrl_t_pops() {
    let fixture = fixture("tui");
    let mut tui = Tui::new(fixture.open());

    feed(&mut tui, "<C-]>");
    assert_eq!(
        tui.session.editor.current_buffer().name.as_deref(),
        Some(fixture.a.as_path()),
        "CTRL-] jumped to the 'alpha' tag"
    );
    assert_eq!(tui.session.editor.cursor().line, 0);
    assert_eq!(
        tui.session.viml.state.win.tag_stack.len(),
        1,
        "the jump pushed one tag stack entry"
    );
    assert_eq!(tui.session.viml.state.win.tag_stack_idx, 1);

    feed(&mut tui, "<C-T>");
    assert_eq!(
        tui.session.editor.current_buffer().name.as_deref(),
        Some(fixture.main.as_path()),
        "CTRL-T popped back"
    );
    assert_eq!(tui.session.editor.cursor().line, 0);
    assert_eq!(
        tui.session.viml.state.win.tag_stack_idx, 0,
        "the pop moved the tag stack index down"
    );
}

#[test]
fn tui_ctrl_bracket_reports_a_missing_tag() {
    let fixture = fixture("tui_missing");
    let mut tui = Tui::new(fixture.open());
    // "zeta" has no tag: the error lands in the message history.
    tui.session.editor.set_cursor(Pos::new(1, 0));
    feed(&mut tui, "<C-]>");
    let messages = tui.session.message_history().join("\n");
    assert!(messages.contains("E426"), "{messages}");
    assert_eq!(
        tui.session.viml.state.win.tag_stack.len(),
        0,
        "a failed lookup pushes nothing"
    );
}
