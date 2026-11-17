//! File reading/writing and the unix/dos/mac line-ending model.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use rvim_core::{VimError, VimResult};

use crate::buffer::{Buffer, FileFormat};

/// UTF-8 byte-order mark (`U+FEFF` encoded as EF BB BF).
const UTF8_BOM: char = '\u{feff}';

/// Split an optional leading UTF-8 BOM off the raw file text.
///
/// Vim detects the BOM at the start of the file, removes it from the text and
/// remembers it in 'bomb' so `:write` can put it back (fileio.c:1482-1493,
/// `check_for_bom` in fileio.c:3264).
pub fn split_bom(raw: &str) -> (bool, &str) {
    match raw.strip_prefix(UTF8_BOM) {
        Some(rest) => (true, rest),
        None => (false, raw),
    }
}

/// Inspect raw text, return the detected format, normalised LF text and
/// whether the input ended with a line terminator.
pub fn normalise_input(raw: &str) -> (FileFormat, String, bool) {
    let has_crlf = raw.contains("\r\n");
    let has_lone_cr = raw.contains('\r') && !has_crlf;
    let format = if has_crlf {
        FileFormat::Dos
    } else if has_lone_cr {
        FileFormat::Mac
    } else {
        FileFormat::Unix
    };
    let normalised = if has_crlf {
        raw.replace("\r\n", "\n")
    } else if has_lone_cr {
        raw.replace('\r', "\n")
    } else {
        raw.to_string()
    };
    let final_nl = normalised.ends_with('\n');
    (format, normalised, final_nl)
}

/// Read a file as UTF-8. `Ok(None)` means the file does not exist, which Vim
/// treats as a new (empty) buffer rather than an error (fileio.c:500-512).
pub fn read_file_opt(path: &Path) -> VimResult<Option<String>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(VimError::new(
                "E484",
                format!("Can't open file {}: {e}", path.display()),
            ));
        }
    };
    // M1 assumes UTF-8; the encoding subsystem (M6) will plug in here.
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|e| VimError::new("E474", format!("Invalid UTF-8 in {}: {e}", path.display())))
}

/// Read a file as UTF-8, reporting a missing file as E484.
pub fn read_file(path: &Path) -> VimResult<String> {
    read_file_opt(path)?.ok_or_else(|| {
        VimError::new(
            "E484",
            format!("Can't open file {}: file not found", path.display()),
        )
    })
}

/// Serialise the buffer to disk using its 'fileformat'/'bomb' state.
///
/// An empty buffer (Vim's `ML_EMPTY`) is written as a zero-byte file, not as a
/// lone newline; a BOM is still emitted for such a buffer when 'bomb' is set
/// (bufwrite.c:1637, :1967).
///
/// The last line is always terminated: Vim only omits the final EOL when
/// `'binary'` or (`'nofixeol'` *and* `'noeol'`) are in effect
/// (bufwrite.c:2054-2062); RustVim does not model those options, so the
/// default `'fixendofline'` behaviour applies — a file read without a trailing
/// newline gains one when written back.
pub fn write_file(path: &Path, buffer: &Buffer) -> VimResult<()> {
    let mut body = String::new();
    if buffer.is_bomb() {
        body.push(UTF8_BOM);
    }
    if !buffer.is_empty_buffer() {
        let format = buffer.fileformat();
        for line in buffer.lines() {
            body.push_str(line.as_str());
            match format {
                FileFormat::Unix => body.push('\n'),
                FileFormat::Dos => body.push_str("\r\n"),
                FileFormat::Mac => body.push('\r'),
            }
        }
    }
    fs::write(path, body).map_err(|e| {
        VimError::new(
            "E212",
            format!("Can't open file for writing: {}: {e}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_formats() {
        assert_eq!(normalise_input("a\r\nb\r\n").0, FileFormat::Dos);
        assert_eq!(normalise_input("a\rb\r").0, FileFormat::Mac);
        assert_eq!(normalise_input("a\nb\n").0, FileFormat::Unix);
        let (_, text, nl) = normalise_input("a\r\nb");
        assert_eq!(text, "a\nb");
        assert!(!nl);
    }

    #[test]
    fn roundtrip_formats() {
        for ff in [FileFormat::Unix, FileFormat::Dos, FileFormat::Mac] {
            let mut buf = Buffer::from_text(1, "one\ntwo");
            buf.set_fileformat(ff);
            let path = std::env::temp_dir().join(format!("rvim_test_{}.txt", ff.vim_name()));
            write_file(&path, &buf).unwrap();
            let raw = read_file(&path).unwrap();
            let expected = match ff {
                FileFormat::Unix => "one\ntwo\n",
                FileFormat::Dos => "one\r\ntwo\r\n",
                FileFormat::Mac => "one\rtwo\r",
            };
            assert_eq!(raw, expected, "format {}", ff.vim_name());
            let _ = fs::remove_file(&path);
        }
    }

    #[test]
    fn write_adds_final_eol_when_source_had_none() {
        // Vim's default 'fixendofline' is on: a buffer read from a file without
        // a trailing newline is written back *with* one (bufwrite.c:2054-2062).
        let buf = Buffer::from_text(1, "alpha\nbeta");
        assert!(!buf.has_final_newline(), "source really had no final EOL");
        let path = std::env::temp_dir().join("rvim_fixeol.txt");
        write_file(&path, &buf).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"alpha\nbeta\n");
        // ... and reading it back now reports a final EOL.
        assert!(read_file(&path).unwrap().ends_with('\n'));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn bom_is_split_off_and_reemitted() {
        let (bomb, rest) = split_bom("\u{feff}alpha\nbeta\n");
        assert!(bomb);
        assert_eq!(rest, "alpha\nbeta\n");
        assert_eq!(split_bom("alpha"), (false, "alpha"));

        let buf = Buffer::from_text(1, "\u{feff}alpha\nbeta\n");
        assert!(buf.is_bomb());
        assert_eq!(buf.getline(1), Some("alpha".to_string()));
        let path = std::env::temp_dir().join("rvim_bom_roundtrip.txt");
        write_file(&path, &buf).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        assert_eq!(&bytes[3..], b"alpha\nbeta\n");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn empty_buffer_writes_zero_bytes_but_bomb_still_written() {
        let path = std::env::temp_dir().join("rvim_empty_write.txt");
        let buf = Buffer::new(1);
        write_file(&path, &buf).unwrap();
        assert_eq!(fs::read(&path).unwrap(), Vec::<u8>::new());

        let mut bomb = Buffer::new(1);
        bomb.set_bomb(true);
        write_file(&path, &bomb).unwrap();
        assert_eq!(fs::read(&path).unwrap(), vec![0xEF, 0xBB, 0xBF]);
        let _ = fs::remove_file(&path);
    }
}
