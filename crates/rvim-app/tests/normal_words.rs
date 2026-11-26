//! Word boundaries in the Normal/GUI input path, including CJK punctuation.
use rvim_app::{Session, tui::Tui};
use rvim_core::Pos;
use rvim_term::ScriptedKeys;

fn editor(text: &str) -> Tui {
    let mut session = Session::new();
    session.editor.insert_text(Pos::new(0, 0), text).unwrap();
    session.editor.set_cursor(Pos::new(0, 0));
    Tui::new(session)
}
fn keys(tui: &mut Tui, notation: &str) {
    tui.run(
        &mut ScriptedKeys::from_notation(notation),
        &mut Vec::new(),
        80,
        24,
    )
    .unwrap();
}

#[test]
fn w_and_b_stop_at_cjk_punctuation_and_script_boundaries() {
    let mut tui = editor("甲乙，丙丁。ABC！");
    for col in [6, 9, 15, 18, 21, 21] {
        keys(&mut tui, "w");
        assert_eq!(tui.session.editor.cursor(), Pos::new(0, col));
    }
    for col in [18, 15, 9, 6, 0, 0] {
        keys(&mut tui, "b");
        assert_eq!(tui.session.editor.cursor(), Pos::new(0, col));
    }
}

#[test]
fn common_fullwidth_punctuation_is_a_word_boundary() {
    for punctuation in "，。！？；：、（）【】《》“”‘’…—".chars() {
        let mut tui = editor(&format!("中文{punctuation}正文"));
        keys(&mut tui, "w");
        assert_eq!(tui.session.editor.cursor().col, 6, "{punctuation}");
        keys(&mut tui, "w");
        assert_eq!(
            tui.session.editor.cursor().col,
            6 + punctuation.len_utf8(),
            "{punctuation}"
        );
    }
}

#[test]
fn ascii_punctuation_is_not_skipped_with_whitespace() {
    let mut tui = editor("one,two...three; tail");
    for col in [3, 4, 7, 10, 15, 17] {
        keys(&mut tui, "w");
        assert_eq!(tui.session.editor.cursor().col, col);
    }
}

#[test]
fn e_and_counts_advance_through_punctuation_runs() {
    let mut tui = editor("甲乙，丙丁。ABC！");
    for col in [3, 6, 12, 15, 20, 21] {
        keys(&mut tui, "e");
        assert_eq!(tui.session.editor.cursor().col, col);
    }
    tui.session.editor.set_cursor(Pos::new(0, 0));
    keys(&mut tui, "3w");
    assert_eq!(tui.session.editor.cursor().col, 15);
    keys(&mut tui, "2b");
    assert_eq!(tui.session.editor.cursor().col, 6);
}

#[test]
fn big_word_motions_keep_punctuation_inside_the_word() {
    let mut tui = editor("中文，English。 next");
    keys(&mut tui, "W");
    assert_eq!(tui.session.editor.cursor().col, "中文，English。 ".len());
    keys(&mut tui, "B");
    assert_eq!(tui.session.editor.cursor().col, 0);
    keys(&mut tui, "E");
    assert_eq!(tui.session.editor.cursor().col, "中文，English".len());
}

#[test]
fn word_operators_and_text_objects_preserve_cjk_punctuation() {
    let mut tui = editor("甲乙，丙丁");
    keys(&mut tui, "dw");
    assert_eq!(tui.session.editor.current_buffer().full_text(), "，丙丁");
    keys(&mut tui, "dw");
    assert_eq!(tui.session.editor.current_buffer().full_text(), "丙丁");
    for (col, expected) in [(0, "X，乙"), (3, "甲X乙")] {
        let mut tui = editor("甲，乙");
        tui.session.editor.set_cursor(Pos::new(0, col));
        keys(&mut tui, "cwX<Esc>");
        assert_eq!(tui.session.editor.current_buffer().full_text(), expected);
        keys(&mut tui, "u");
        assert_eq!(tui.session.editor.current_buffer().full_text(), "甲，乙");
    }
    let mut tui = editor("甲，。！？乙");
    tui.session.editor.set_cursor(Pos::new(0, 3));
    keys(&mut tui, "diw");
    assert_eq!(tui.session.editor.current_buffer().full_text(), "甲乙");
    let mut tui = editor("中文English");
    keys(&mut tui, "diw");
    assert_eq!(tui.session.editor.current_buffer().full_text(), "English");
    let mut tui = editor("甲　乙");
    keys(&mut tui, "diW");
    assert_eq!(tui.session.editor.current_buffer().full_text(), "　乙");
}

#[test]
fn word_motions_handle_ideographic_spaces_line_breaks_and_eof() {
    let mut tui = editor("甲，\n  \n\n　乙。丙");
    for pos in [
        Pos::new(0, 3),
        Pos::new(2, 0),
        Pos::new(3, 3),
        Pos::new(3, 6),
        Pos::new(3, 9),
        Pos::new(3, 9),
    ] {
        keys(&mut tui, "w");
        assert_eq!(tui.session.editor.cursor(), pos);
    }
    keys(&mut tui, "3b");
    assert_eq!(tui.session.editor.cursor(), Pos::new(2, 0));
}

#[test]
fn word_motion_matches_reference_vim_when_available() {
    let Some(vim) = std::env::var_os("RVIM_REFERENCE_VIM") else {
        return;
    };
    let fixtures = [
        "甲乙，丙丁。ABC！",
        "one,two...three; tail",
        "中文English１２3中文",
        "αβ中文ひらがなカタカナ한글。",
        "甲，\n  \n\n　乙。丙",
        "abc  \n def!\n",
        "甲😀，𠀀乙！  z",
        "  甲，乙 ",
        "甲\n   ",
    ];
    let root = std::env::temp_dir().join(format!("rvim_words_reference_{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let script_path = root.join("words.vim");
    let result_path = root.join("positions.txt");
    let mut script = String::from("set nocompatible encoding=utf-8\nlet g:positions = []\n");
    let mut expected = Vec::new();
    for text in fixtures {
        let mut tui = editor(text);
        script.push_str("%delete _\ncall setline(1, [");
        script.push_str(
            &text
                .split('\n')
                .map(|line| format!("'{}'", line.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(","),
        );
        script.push_str("])\n");
        for (line, text) in text.split('\n').enumerate() {
            for col in text
                .char_indices()
                .map(|(col, _)| col)
                .chain(text.is_empty().then_some(0))
            {
                for key in ['w', 'b', 'e', 'W', 'B', 'E'] {
                    for count in [1, 2, 3] {
                        tui.session.editor.set_cursor(Pos::new(line, col));
                        let pos =
                            rvim_normal::motions::word_motion(&tui.session.editor, key, count)
                                .unwrap();
                        expected.push((
                            format!("{line}:{col} {count}{key} in {text:?}"),
                            format!("{},{}", pos.line + 1, pos.col + 1),
                        ));
                        script.push_str(&format!("call cursor({}, {})\nnormal! {count}{key}\ncall add(g:positions, printf('%d,%d', line('.'), col('.')))\n", line + 1, col + 1));
                    }
                }
            }
        }
    }
    script.push_str(&format!(
        "call writefile(g:positions, '{}')\nqall!\n",
        result_path
            .display()
            .to_string()
            .replace('\\', "/")
            .replace('\'', "''")
    ));
    std::fs::write(&script_path, script).unwrap();
    let output = std::process::Command::new(vim)
        .args(["-Nu", "NONE", "-i", "NONE", "-n", "-es", "-S"])
        .arg(&script_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = std::fs::read_to_string(&result_path).unwrap();
    assert_eq!(actual.lines().count(), expected.len());
    for ((case, rust), vim) in expected.iter().zip(actual.lines()) {
        assert_eq!(rust, vim, "{case}");
    }
    eprintln!(
        "Compared {} word-motion positions with reference Vim",
        expected.len()
    );
}
