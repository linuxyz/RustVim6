//! :vimgrep / :lvimgrep populate the quickfix and location lists.

use rvim_app::Session;

#[test]
fn vimgrep_fills_the_quickfix_list_and_lvimgrep_the_location_list() {
    let dir = std::env::temp_dir().join("rvim_m6_vimgrep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    std::fs::write(&a, "one foo\ntwo\nfoo again\n").unwrap();
    std::fs::write(&b, "nothing\nfoo here\n").unwrap();
    let mut s = Session::new();
    s.ex(&format!("vimgrep /foo/j {}", dir.join("*.txt").display()))
        .unwrap();
    let history = s.editor.messages.history().join("\n");
    assert!(history.contains("3 matches in 2 files"), "{history}");
    let qf = s
        .ex("call writefile([string(len(getqflist()))], 'C:/BUILD/RustVim6/target/tmp/vimgrep.out')")
        .unwrap();
    let _ = qf;
    let text =
        std::fs::read_to_string("C:/BUILD/RustVim6/target/tmp/vimgrep.out").unwrap_or_default();
    assert_eq!(text.trim(), "3", "{text}");
    s.ex(&format!("lvimgrep /again/ {}", a.display())).unwrap();
    let history = s.editor.messages.history().join("\n");
    assert!(history.contains("1 match in 1 file"), "{history}");
    let _ = std::fs::remove_dir_all(dir);
}
