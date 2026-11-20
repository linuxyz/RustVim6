//! :diffpatch applies an external patch and shows the result in a diff split.

use rvim_app::Session;

#[test]
fn diffpatch_opens_the_patched_copy_in_diff_mode() {
    let dir = std::env::temp_dir().join("rvim_m5_diffpatch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "one\ntwo\n").unwrap();
    let patch = dir.join("change.patch");
    std::fs::write(
        &patch,
        "--- a.txt\n+++ a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n",
    )
    .unwrap();
    let mut s = Session::new();
    s.open_file(&file).unwrap();
    s.ex("set fileformat=unix").unwrap();
    s.ex(&format!("diffpatch {}", patch.display())).unwrap();
    let text = s.editor.current_buffer().full_text();
    assert!(text.contains("TWO"), "patched buffer: {text}");
    assert!(s.editor.windows.ids().len() >= 2, "a split was created");
    assert!(s.editor.in_diff_mode(s.editor.current_win_id()));
    let _ = std::fs::remove_dir_all(dir);
}
