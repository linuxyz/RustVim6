//! Integration tests for the buffer document model.

use rvim_buffer::{Buffer, FileFormat};
use rvim_core::Pos;

#[test]
fn empty_buffer_has_one_line() {
    let buf = Buffer::new(1);
    assert_eq!(buf.line_count(), 1);
    assert_eq!(buf.changedtick(), 0);
    assert!(!buf.is_changed());
}

#[test]
fn from_text_counts_lines() {
    let buf = Buffer::from_text(
        2,
        "alpha
beta
gamma",
    );
    assert_eq!(buf.line_count(), 3);
    assert_eq!(buf.getline(1), Some("alpha".to_string()));
    assert_eq!(buf.getline(3), Some("gamma".to_string()));
    assert_eq!(buf.getline(4), None);
}

#[test]
fn edit_bumps_tick_and_sets_changed() {
    let mut buf = Buffer::from_text(
        1,
        "hello
world",
    );
    let edit = buf.set_line_text(0, "HELLO".to_string()).unwrap();
    assert_eq!(edit.before, vec!["hello".to_string()]);
    assert_eq!(edit.after, vec!["HELLO".to_string()]);
    assert_eq!(buf.changedtick(), 1);
    assert!(buf.is_changed());
    assert_eq!(buf.line_str(0).unwrap(), "HELLO");
}

#[test]
fn inverse_edit_restores_text() {
    let mut buf = Buffer::from_text(
        1, "a
b
c",
    );
    let edit = buf.replace_line_range(1, 2, vec!["B".to_string()]).unwrap();
    assert_eq!(
        buf.full_text(),
        "a
B
c"
    );
    let inv = edit.inverse();
    buf.apply_edit(&inv).unwrap();
    assert_eq!(
        buf.full_text(),
        "a
b
c"
    );
    assert_eq!(buf.changedtick(), 2);
}

#[test]
fn insert_text_splits_lines() {
    let mut buf = Buffer::from_text(
        1, "ab
cd",
    );
    buf.insert_text(
        Pos::new(1, 1),
        "X
Y",
    )
    .unwrap();
    assert_eq!(
        buf.full_text(),
        "ab
cX
Yd"
    );
}

#[test]
fn delete_chars_returns_removed() {
    let mut buf = Buffer::from_text(1, "abcdef");
    let (removed, _) = buf.delete_chars(Pos::new(0, 2), 2).unwrap();
    assert_eq!(removed, "cd");
    assert_eq!(buf.full_text(), "abef");
}

#[test]
fn utf8_columns_are_byte_offsets() {
    let mut buf = Buffer::from_text(1, "éx");
    // Pos.col is a byte offset: byte 2 is where 'x' starts.
    buf.insert_text(Pos::new(0, 2), "Z").unwrap();
    assert_eq!(buf.full_text(), "éZx");
    // A mid-character byte offset snaps back to the character start.
    let mut snap = Buffer::from_text(1, "éx");
    snap.insert_text(Pos::new(0, 1), "Z").unwrap();
    assert_eq!(snap.full_text(), "Zéx");
}

#[test]
fn delete_chars_uses_byte_column() {
    // "あb": あ occupies bytes 0..3, b is at byte 3.
    let mut buf = Buffer::from_text(1, "あb");
    let (removed, _) = buf.delete_chars(Pos::new(0, 3), 1).unwrap();
    assert_eq!(removed, "b");
    assert_eq!(buf.full_text(), "あ");
}

#[test]
fn insert_text_uses_byte_column() {
    let mut buf = Buffer::from_text(1, "あb");
    buf.insert_text(Pos::new(0, 3), "X").unwrap();
    assert_eq!(buf.full_text(), "あXb");
}

#[test]
fn open_missing_file_creates_empty_new_buffer() {
    let path = std::env::temp_dir().join("rvim_buffer_missing_created.txt");
    let _ = std::fs::remove_file(&path);
    let mut buf = Buffer::open(9, &path).expect("missing file is a new buffer, not an error");
    assert_eq!(buf.line_count(), 1);
    assert_eq!(buf.getline(1), Some(String::new()));
    assert!(!buf.is_changed());
    assert_eq!(buf.name.as_deref(), Some(path.as_path()));
    // Saving creates the file: an empty buffer is zero bytes.
    buf.save(&path, false).unwrap();
    assert!(path.exists());
    assert_eq!(std::fs::read(&path).unwrap(), Vec::<u8>::new());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn save_and_reopen_roundtrip() {
    let path = std::env::temp_dir().join("rvim_buffer_roundtrip.txt");
    let mut buf = Buffer::from_text(
        1,
        "line1
line2",
    );
    buf.save(&path, false).unwrap();
    assert!(!buf.is_changed());
    let reopened = Buffer::open(9, &path).unwrap();
    assert_eq!(
        reopened.full_text(),
        "line1
line2"
    );
    assert_eq!(reopened.fileformat(), FileFormat::Unix);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn readonly_requires_bang() {
    let path = std::env::temp_dir().join("rvim_readonly.txt");
    let mut buf = Buffer::from_text(1, "x");
    buf.set_readonly(true);
    assert!(buf.save(&path, false).is_err());
    assert!(buf.save(&path, true).is_ok());
    let _ = std::fs::remove_file(&path);
}
#[test]
fn insert_text_newline_at_end_of_line_creates_an_empty_line() {
    let mut buf = Buffer::from_text(1, "ab!");
    buf.insert_text(Pos::new(0, 3), "\n").unwrap();
    assert_eq!(buf.line_count(), 2, "typing <CR> at EOL splits the line");
    assert_eq!(buf.line_str(0).unwrap(), "ab!");
    assert_eq!(buf.line_str(1).unwrap(), "");
    buf.insert_text(Pos::new(0, 1), "\n").unwrap();
    assert_eq!(buf.line_count(), 3, "<CR> in the middle splits too");
    assert_eq!(buf.line_str(0).unwrap(), "a");
    assert_eq!(buf.line_str(1).unwrap(), "b!");
}
