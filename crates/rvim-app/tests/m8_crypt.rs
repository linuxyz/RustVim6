//! M8: Vim-compatible file encryption (:set key= + :write).

use rvim_app::Session;

fn temp(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(name);
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn each_crypt_method_round_trips_through_write_and_edit() {
    for method in ["zip", "blowfish", "blowfish2"] {
        let path = temp(&format!("rvim_m8_{method}.txt"));
        let mut writer = Session::new();
        writer.ex("set key=secret").unwrap();
        writer.ex(&format!("set cryptmethod={method}")).unwrap();
        writer
            .editor
            .replace_lines(0, 1, vec!["alpha".to_string(), "beta".to_string()])
            .unwrap();
        writer.ex(&format!("w! {}", path.display())).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let magic: &[u8] = match method {
            "zip" => b"VimCrypt~01!",
            "blowfish" => b"VimCrypt~02!",
            _ => b"VimCrypt~03!",
        };
        assert_eq!(&bytes[..12], magic, "{method} header");
        assert!(!bytes.windows(5).any(|window| window == b"alpha"));

        let mut reader = Session::new();
        reader.ex("set key=secret").unwrap();
        reader.open_file(&path).unwrap();
        assert_eq!(
            reader.editor.current_buffer().full_text(),
            "alpha\nbeta",
            "{method} decrypts"
        );
        assert_eq!(
            reader.editor.options.str_or("cryptmethod", ""),
            method,
            "the file header updates 'cryptmethod'"
        );
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn writing_without_a_key_stays_plaintext() {
    let path = temp("rvim_m8_plain.txt");
    let mut s = Session::new();
    s.ex("set fileformat=unix").unwrap();
    s.editor
        .replace_lines(0, 1, vec!["plain".to_string()])
        .unwrap();
    s.ex(&format!("w! {}", path.display())).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"plain\n");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_wrong_key_does_not_read_the_original_text() {
    let path = temp("rvim_m8_wrong.txt");
    let mut writer = Session::new();
    writer.ex("set key=secret").unwrap();
    writer
        .editor
        .replace_lines(0, 1, vec!["hidden".to_string()])
        .unwrap();
    writer.ex(&format!("w! {}", path.display())).unwrap();

    let mut reader = Session::new();
    reader.ex("set key=wrong").unwrap();
    let _ = reader.open_file(&path);
    let text = reader.editor.current_buffer().full_text();
    assert!(!text.contains("hidden"), "wrong key must not reveal text");
    let _ = std::fs::remove_file(path);
}

#[test]
fn encrypted_read_requires_a_key() {
    let path = temp("rvim_m8_nokey.txt");
    let mut writer = Session::new();
    writer.ex("set key=secret").unwrap();
    writer
        .editor
        .replace_lines(0, 1, vec!["x".to_string()])
        .unwrap();
    writer.ex(&format!("w! {}", path.display())).unwrap();

    let mut reader = Session::new();
    let error = reader.open_file(&path).unwrap_err();
    assert_eq!(error.code, "E576", "{error}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn x_command_sets_the_key() {
    let mut s = Session::new();
    s.ex("X topsecret").unwrap();
    assert_eq!(s.editor.options.str_or("key", ""), "topsecret");
}
