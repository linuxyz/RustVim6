//! M6 spell checking tests: plain word lists, the Vim binary .spl reader,
//! spellbadword()/spellsuggest()/soundfold(), bad_word_spans() and the
//! :spellgood/:spellwrong/:spellrare/:spellundo/:spellinfo/:spelldump/
//! :spellrepall/:mkspell commands.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rvim_app::Session;
use rvim_core::Pos;
use rvim_editor::OptValue;
use rvim_editor::spell;
use rvim_viml::value::Value;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// Temporary directory: honour TEMP explicitly (the test instructions set it
/// to target/tmp) rather than std::env::temp_dir(), which prefers TMP on
/// Windows.
fn temp_dir() -> PathBuf {
    let dir = std::env::var_os("TEMP")
        .or_else(|| std::env::var_os("TMP"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn temp_file(stem: &str, ext: &str) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    temp_dir().join(format!(
        "rvim_m6_spell_{}_{seq}_{}.{ext}",
        std::process::id(),
        stem
    ))
}

fn write_file(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("write temporary spell file");
}

/// A session with 'spell' on, 'spelllang'=zz and 'spellfile' pointing at a
/// plain word list.
fn session_with_words(words: &str) -> (Session, PathBuf) {
    let path = temp_file("words", "txt");
    write_file(&path, words);
    let mut session = Session::new();
    session
        .editor
        .options
        .set("spellfile", OptValue::Str(path.display().to_string()))
        .unwrap();
    session
        .editor
        .options
        .set("spelllang", OptValue::Str("zz".to_string()))
        .unwrap();
    session
        .editor
        .options
        .set("spell", OptValue::Bool(true))
        .unwrap();
    (session, path)
}

fn call_builtin(session: &mut Session, name: &str, args: &[Value]) -> Value {
    rvim_ex::host_spell::call(
        name,
        args,
        &mut session.editor,
        &mut session.viml.env,
        &mut session.viml.state,
    )
    .expect("spell builtin is handled")
    .expect("spell builtin succeeds")
}

fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::List(items) => items.borrow().iter().map(Value::to_string_value).collect(),
        other => panic!("expected a List, got {other:?}"),
    }
}

fn spellbadword(session: &mut Session, text: Option<&str>) -> Vec<String> {
    let args: Vec<Value> = text
        .map(|text| vec![Value::string(text)])
        .unwrap_or_default();
    strings(&call_builtin(session, "spellbadword", &args))
}

fn spellsuggest(session: &mut Session, word: &str, max: i64) -> Vec<String> {
    let args = [Value::string(word), Value::Number(max)];
    strings(&call_builtin(session, "spellsuggest", &args))
}

fn good_word() -> Vec<String> {
    vec![String::new(), String::new()]
}

// ------------------------------------------------------------- word lists

#[test]
fn plain_word_list_drives_case_aware_checking() {
    let (mut session, path) = session_with_words(
        "# a comment\n/encoding=utf-8\nhello\nworld\ncolour\nrareword/?\nbadword/!\n",
    );

    assert_eq!(spellbadword(&mut session, Some("hello")), good_word());
    // A capitalized or all-capital spelling of a known word is fine.
    assert_eq!(spellbadword(&mut session, Some("Hello")), good_word());
    assert_eq!(spellbadword(&mut session, Some("HELLO")), good_word());
    // Unknown words and words flagged /! are bad, /? is rare.
    assert_eq!(
        spellbadword(&mut session, Some("hellx")),
        vec!["hellx", "bad"]
    );
    assert_eq!(
        spellbadword(&mut session, Some("badword")),
        vec!["badword", "bad"]
    );
    assert_eq!(
        spellbadword(&mut session, Some("Rareword")),
        vec!["Rareword", "rare"]
    );
    // A lowercase word after a sentence end needs a capital.
    assert_eq!(
        spellbadword(&mut session, Some("Hello. world")),
        vec!["world", "caps"]
    );
    // The first good words do not hide a later bad one.
    assert_eq!(
        spellbadword(&mut session, Some("hello world hellx")),
        vec!["hellx", "bad"]
    );

    assert!(!spell::is_bad_word(&session.editor, "hello"));
    assert!(spell::is_bad_word(&session.editor, "hellx"));

    // Rendering spans come back in byte offsets with the Vim group names.
    let spans = spell::bad_word_spans(&session.editor, "hello hellx Rareword badword");
    assert_eq!(
        spans,
        vec![
            (6, 11, "SpellBad".to_string()),
            (12, 20, "SpellRare".to_string()),
            (21, 28, "SpellBad".to_string()),
        ]
    );
    // A lowercase word after a sentence end is a capitalization error.
    let spans = spell::bad_word_spans(&session.editor, "Hello. world");
    assert_eq!(spans, vec![(7, 12, "SpellCap".to_string())]);

    // 'spell' off disables buffer checking but not spellbadword().
    session
        .editor
        .options
        .set("spell", OptValue::Bool(false))
        .unwrap();
    assert!(spell::bad_word_spans(&session.editor, "hellx").is_empty());
    assert!(!spell::is_bad_word(&session.editor, "hellx"));
    assert_eq!(
        spellbadword(&mut session, Some("hellx")),
        vec!["hellx", "bad"]
    );

    assert_eq!(
        spell::split_words("don't well-known foo_bar 42"),
        vec![
            (0, 5, "don't".to_string()),
            (6, 16, "well-known".to_string()),
            (17, 24, "foo_bar".to_string()),
        ]
    );

    let _ = std::fs::remove_file(path);
}

// ------------------------------------------------------------- suggestions

#[test]
fn suggestions_are_ranked_by_edit_distance_then_alphabetically() {
    let (mut session, path) = session_with_words("hello\nhallo\nhelp\nworld\nyellow\n");
    let suggestions = spellsuggest(&mut session, "hellx", 5);
    assert_eq!(suggestions.first().map(String::as_str), Some("hello"));
    assert!(
        suggestions.contains(&"hallo".to_string()),
        "{suggestions:?}"
    );
    assert!(suggestions.contains(&"help".to_string()), "{suggestions:?}");
    let hallo = suggestions.iter().position(|word| word == "hallo").unwrap();
    let help = suggestions.iter().position(|word| word == "help").unwrap();
    assert!(
        hallo < help,
        "distance ties sort alphabetically: {suggestions:?}"
    );

    assert_eq!(
        spellsuggest(&mut session, "hellx", 2),
        vec!["hello".to_string(), "hallo".to_string()]
    );

    // The third argument keeps only suggestions that start with a capital.
    let args = [Value::string("hellx"), Value::Number(5), Value::Number(1)];
    let capital = strings(&call_builtin(&mut session, "spellsuggest", &args));
    assert!(capital.is_empty(), "{capital:?}");

    let _ = std::fs::remove_file(path);
}

#[test]
fn soundfold_maps_sound_alike_spellings() {
    assert_eq!(spell::soundfold("Phone"), "fone");
    assert_eq!(spell::soundfold("Phone"), spell::soundfold("fone"));
    assert_eq!(spell::soundfold("letter"), "leter");
    assert_eq!(spell::soundfold(""), "");
    assert_eq!(spell::soundfold("456"), "");
}

// -------------------------------------------------------------- commands

#[test]
fn spell_commands_change_the_word_lists_and_the_file() {
    let (mut session, path) = session_with_words("hello\n");
    assert_eq!(
        spellbadword(&mut session, Some("hellx")),
        vec!["hellx", "bad"]
    );

    session.ex("spellgood hellx").unwrap();
    assert_eq!(spellbadword(&mut session, Some("hellx")), good_word());
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.lines().any(|line| line.trim() == "hellx"),
        "{written:?}"
    );

    session.ex("spellwrong hello").unwrap();
    assert_eq!(
        spellbadword(&mut session, Some("hello")),
        vec!["hello", "bad"]
    );
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.lines().any(|line| line.trim() == "hello/!"),
        "{written:?}"
    );

    session.ex("spellundo hellx").unwrap();
    assert_eq!(
        spellbadword(&mut session, Some("hellx")),
        vec!["hellx", "bad"]
    );

    session.ex("spellrare rarew").unwrap();
    assert_eq!(
        spellbadword(&mut session, Some("rarew")),
        vec!["rarew", "rare"]
    );

    session.ex("spellinfo").unwrap();
    let history = session.editor.messages.history().join("\n");
    assert!(history.contains("spell: spelllang=zz"), "{history}");
    assert!(history.contains("words="), "{history}");

    session.ex("spelldump").unwrap();
    let history = session.editor.messages.history().join("\n");
    assert!(history.contains("spelldump:"), "{history}");
    assert!(history.contains("hello"), "{history}");

    let error = session.ex("spellrepall").unwrap_err();
    assert_eq!(error.code, "E753");
    assert!(
        error.message.contains("No previous spell replacement"),
        "{error:?}"
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn internal_word_list_needs_no_spellfile() {
    let mut session = Session::new();
    session
        .editor
        .options
        .set("spelllang", OptValue::Str("zz".to_string()))
        .unwrap();
    session
        .editor
        .options
        .set("spell", OptValue::Bool(true))
        .unwrap();

    let error = session.ex("spellgood plainword").unwrap_err();
    assert_eq!(error.code, "E756");
    assert!(error.message.contains("No spell file name"), "{error:?}");

    session.ex("spellgood! internalword").unwrap();
    assert_eq!(
        spellbadword(&mut session, Some("internalword")),
        good_word()
    );

    session.ex("spellundo! internalword").unwrap();
    assert_eq!(
        spellbadword(&mut session, Some("internalword")),
        vec!["internalword", "bad"]
    );
}

#[test]
fn spell_builtins_are_reachable_from_ex() {
    let (mut session, path) = session_with_words("hello\n");
    session.ex("echo spellbadword('hellx')").unwrap();
    session.ex("echo spellsuggest('hellx', 2)").unwrap();
    session.ex("echo soundfold('Phone')").unwrap();
    let history = session.editor.messages.history().join("\n");
    assert!(history.contains("hellx"), "{history}");
    assert!(history.contains("hello"), "{history}");
    assert!(history.contains("fone"), "{history}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn spellbadword_without_argument_uses_the_cursor_line() {
    let (mut session, path) = session_with_words("hello\n");
    session
        .editor
        .current_buffer_mut()
        .set_line_text(0, "hello hellx".to_string())
        .unwrap();
    session.editor.set_cursor(Pos::new(0, 7));
    assert_eq!(spellbadword(&mut session, None), vec!["hellx", "bad"]);

    // On the good word the search continues to the next bad word.
    session.editor.set_cursor(Pos::new(0, 0));
    assert_eq!(spellbadword(&mut session, None), vec!["hellx", "bad"]);

    // Past the last word nothing is reported.
    session.editor.set_cursor(Pos::new(0, 11));
    assert_eq!(spellbadword(&mut session, None), good_word());

    let _ = std::fs::remove_file(path);
}

#[test]
fn mkspell_writes_a_sorted_plain_word_list() {
    let input = temp_file("input", "txt");
    write_file(&input, "gamma\nalpha\nbeta\nbad/!\n# comment\n");
    let output = temp_file("output", "txt");
    let mut session = Session::new();
    session
        .ex(&format!("mkspell {} {}", output.display(), input.display()))
        .unwrap();
    let written = std::fs::read_to_string(&output).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines, vec!["alpha", "bad/!", "beta", "gamma"]);
    let history = session.editor.messages.history().join("\n");
    assert!(history.contains("mkspell: wrote 4 words"), "{history}");

    // :mkspell! is accepted and overwrites the output.
    session
        .ex(&format!(
            "mkspell! {} {}",
            output.display(),
            input.display()
        ))
        .unwrap();
    assert_eq!(std::fs::read_to_string(&output).unwrap(), written);

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

// ------------------------------------------------- Vim binary spell file

#[test]
fn vim_binary_spell_file_is_read_and_checked() {
    let spl = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference/vim/runtime/spell/en.utf-8.spl");
    if !spl.is_file() {
        eprintln!("skipping: {} is not available", spl.display());
        return;
    }
    let mut session = Session::new();
    session
        .editor
        .options
        .set("spelllang", OptValue::Str("en".to_string()))
        .unwrap();
    session
        .editor
        .options
        .set("spell", OptValue::Bool(true))
        .unwrap();

    assert_eq!(spellbadword(&mut session, Some("hello")), good_word());
    assert_eq!(spellbadword(&mut session, Some("Hello")), good_word());
    assert_eq!(
        spellbadword(&mut session, Some("hellx")),
        vec!["hellx", "bad"]
    );
    assert_eq!(spellbadword(&mut session, Some("the")), good_word());

    let summary = session.editor.spell.summary();
    assert!(
        summary.words > 100_000,
        "expected the full Vim dictionary, got {} words",
        summary.words
    );
    assert!(
        summary
            .sources
            .iter()
            .any(|source| source.contains("en.utf-8.spl")),
        "{:?}",
        summary.sources
    );

    // Suggestions from the real dictionary include the obvious candidate.
    let suggestions = spellsuggest(&mut session, "hellx", 5);
    assert!(
        suggestions.contains(&"hello".to_string()),
        "{suggestions:?}"
    );

    session.ex("spellinfo").unwrap();
    let history = session.editor.messages.history().join("\n");
    assert!(history.contains("words="), "{history}");
}
