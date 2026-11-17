//! Buffer: one open document.

use std::path::{Path, PathBuf};

use rvim_core::text::{snap_byte, split_lines};
use rvim_core::{Col, Lineno, Pos, VimError, VimResult};

use crate::edit::RangeEdit;
use crate::fileio;
use crate::line::Line;

/// Handle identifying a buffer; equals Vim's `bufnr()`.
pub type BufferId = i32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileFormat {
    #[default]
    Unix,
    Dos,
    Mac,
}

impl FileFormat {
    pub fn vim_name(self) -> &'static str {
        match self {
            FileFormat::Unix => "unix",
            FileFormat::Dos => "dos",
            FileFormat::Mac => "mac",
        }
    }

    /// The default 'fileformat' for a fresh buffer, Vim's `DFLT_FF`
    /// (reference option.h: DOS on Windows, unix everywhere else).
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            FileFormat::Dos
        } else {
            FileFormat::Unix
        }
    }

    pub fn parse(name: &str) -> VimResult<Self> {
        match name.to_ascii_lowercase().as_str() {
            "unix" => Ok(FileFormat::Unix),
            "dos" => Ok(FileFormat::Dos),
            "mac" => Ok(FileFormat::Mac),
            other => Err(VimError::invalid_argument(&format!("fileformat={other}"))),
        }
    }
}

#[derive(Debug)]
pub struct Buffer {
    pub id: BufferId,
    pub name: Option<PathBuf>,
    lines: Vec<Line>,
    changed: bool,
    changedtick: u64,
    fileformat: FileFormat,
    /// Whether the source file ended with a line terminator. Vim's default
    /// `'fixendofline'` means writes always terminate the last line anyway
    /// (bufwrite.c:2054-2062); the flag is informational until `'binary'` /
    /// `'nofixeol'` are modelled.
    final_newline: bool,
    /// UTF-8 byte-order mark was read / must be written ('bomb', fileio.c
    /// `check_for_bom` / bufwrite.c:1967).
    bomb: bool,
    /// Vim's `ML_EMPTY`: an empty buffer is written as a zero-byte file
    /// (bufwrite.c:1637 `if (buf->b_ml.ml_flags & ML_EMPTY) start = end + 1`).
    ml_empty: bool,
    readonly: bool,
    /// Last cursor position remembered for this buffer.
    pub last_cursor: Pos,
}

impl Buffer {
    /// A new, empty unnamed buffer containing a single empty line — Vim
    /// buffers are never line-less.
    pub fn new(id: BufferId) -> Self {
        Self {
            id,
            name: None,
            lines: vec![Line::empty()],
            changed: false,
            changedtick: 0,
            fileformat: FileFormat::platform_default(),
            final_newline: true,
            bomb: false,
            ml_empty: true,
            readonly: false,
            last_cursor: Pos::new(0, 0),
        }
    }

    /// Build a buffer from raw text, normalising line endings and detecting
    /// the source file format. A leading UTF-8 BOM is removed from the text
    /// and remembered on the buffer so `:write` can put it back
    /// (fileio.c:1486-1493 "Remove BOM from the text" / `b_p_bomb = TRUE`).
    pub fn from_text(id: BufferId, text: &str) -> Self {
        let mut buf = Self::new(id);
        let (bomb, text) = fileio::split_bom(text);
        let (format, normalised, final_nl) = fileio::normalise_input(text);
        buf.fileformat = format;
        buf.final_newline = final_nl;
        buf.bomb = bomb;
        // A truly empty file leaves ML_EMPTY set; a file containing one line
        // (even an empty one, i.e. just a newline) does not (memline.c
        // ml_append clears the flag).
        buf.ml_empty = text.is_empty();
        let parsed = split_lines(&normalised);
        buf.lines = parsed.iter().map(|(t, _)| Line::new(t.clone())).collect();
        if buf.lines.is_empty() {
            buf.lines.push(Line::empty());
        }
        buf
    }

    pub fn open(id: BufferId, path: &Path) -> VimResult<Self> {
        match fileio::read_file_opt(path)? {
            Some(raw) => {
                let mut buf = Self::from_text(id, &raw);
                buf.name = Some(path.to_path_buf());
                buf.changed = false;
                Ok(buf)
            }
            None => {
                // ENOENT is not an error: Vim starts a new, empty and
                // unmodified buffer that `:write` will create
                // (fileio.c:500-512, `BF_NEW`). Other read errors stay E484.
                let mut buf = Self::new(id);
                buf.name = Some(path.to_path_buf());
                Ok(buf)
            }
        }
    }

    // --- basic accessors -------------------------------------------------

    pub fn line_count(&self) -> Lineno {
        self.lines.len()
    }

    pub fn line(&self, idx: Lineno) -> VimResult<&Line> {
        self.lines.get(idx).ok_or_else(|| {
            VimError::internal(format!(
                "line index {idx} out of range ({} lines)",
                self.lines.len()
            ))
        })
    }

    pub fn line_str(&self, idx: Lineno) -> VimResult<&str> {
        Ok(self.line(idx)?.as_str())
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub fn is_changed(&self) -> bool {
        self.changed
    }

    /// Override the modified flag without touching `b:changedtick`, matching
    /// `:set [no]modified` (option.c:3001, where 'modified' *is* `b_changed`).
    /// `changed()` itself (`change.c:145`) does not bump the tick either; the
    /// text change that set the flag already did.
    pub fn set_changed(&mut self, changed: bool) {
        self.changed = changed;
    }

    pub fn changedtick(&self) -> u64 {
        self.changedtick
    }

    pub fn fileformat(&self) -> FileFormat {
        self.fileformat
    }

    pub fn set_fileformat(&mut self, ff: FileFormat) {
        self.fileformat = ff;
    }

    pub fn is_bomb(&self) -> bool {
        self.bomb
    }

    pub fn set_bomb(&mut self, bomb: bool) {
        self.bomb = bomb;
    }

    /// True when this is Vim's "empty buffer" (ML_EMPTY): it has no text at all
    /// and `:write` must produce a zero-byte file (or a bare BOM when 'bomb' is
    /// set), not a newline.
    pub fn is_empty_buffer(&self) -> bool {
        self.ml_empty
    }

    pub fn set_empty_buffer(&mut self, empty: bool) {
        self.ml_empty = empty;
    }

    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    pub fn set_readonly(&mut self, ro: bool) {
        self.readonly = ro;
    }

    pub fn has_final_newline(&self) -> bool {
        self.final_newline
    }

    pub fn set_final_newline(&mut self, v: bool) {
        self.final_newline = v;
    }

    pub fn display_name(&self) -> String {
        self.name
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "[No Name]".to_string())
    }

    // --- whole-document helpers -----------------------------------------

    /// Join all lines with `\n` (logical representation, file format applied
    /// only at write time).
    pub fn full_text(&self) -> String {
        let texts: Vec<&str> = self.lines.iter().map(|l| l.as_str()).collect();
        texts.join(
            "
",
        )
    }

    /// Vim's `getline()`: 1-based line number, `None` out of range.
    pub fn getline(&self, vim_lineno: usize) -> Option<String> {
        let idx = vim_lineno.checked_sub(1)?;
        self.lines.get(idx).map(|l| l.as_str().to_string())
    }

    // --- mutation --------------------------------------------------------

    fn touch(&mut self) {
        self.changed = true;
        self.changedtick += 1;
    }

    /// Apply a universal range edit, returning the recorded edit (which
    /// contains the `before` snapshot needed for undo).
    pub fn apply_edit(&mut self, edit: &RangeEdit) -> VimResult<()> {
        if edit.start > edit.end || edit.end > self.lines.len() {
            return Err(VimError::internal(format!(
                "invalid range edit [{}, {}) for {} lines",
                edit.start,
                edit.end,
                self.lines.len()
            )));
        }
        edit.apply(&mut self.lines);
        // ML_EMPTY tracking (memline.c): deleting the last line sets the flag,
        // while ml_append/ml_replace (inserting or replacing text) clear it.
        if self.lines.len() == 1 && self.lines[0].is_empty() {
            if edit.before.iter().any(|l| !l.is_empty()) {
                self.ml_empty = true;
            }
        } else {
            self.ml_empty = false;
        }
        self.touch();
        Ok(())
    }

    /// Convenience: replace half-open line range with new lines.
    pub fn replace_line_range(
        &mut self,
        start: Lineno,
        end: Lineno,
        after: Vec<String>,
    ) -> VimResult<RangeEdit> {
        let before: Vec<String> = self.lines[start..end.min(self.lines.len())]
            .iter()
            .map(|l| l.as_str().to_string())
            .collect();
        let edit = RangeEdit {
            start,
            end,
            before,
            after,
        };
        self.apply_edit(&edit)?;
        Ok(edit)
    }

    /// Insert raw text at a byte position (`Pos.col` is a byte offset, see
    /// rvim-core position.rs), splitting on newlines. A column that points
    /// inside a multi-byte character snaps back to that character's first
    /// byte. Returns the resulting range edit.
    pub fn insert_text(&mut self, pos: Pos, text: &str) -> VimResult<RangeEdit> {
        self.assert_line(pos.line)?;
        let line = self.lines[pos.line].as_str().to_string();
        let byte = snap_byte(&line, pos.col);
        let merged = format!("{}{}{}", &line[..byte], text, &line[byte..]);
        let pieces = split_lines(&merged);
        let mut after: Vec<String> = pieces.iter().map(|(t, _)| t.clone()).collect();
        // split_lines() follows the *file* convention, where a trailing newline
        // does not start another line. Inserting text is different: typing
        // <CR> at the end of a line must leave an empty line after the split
        // (edit.c ins_eol), so the trailing empty piece is added back here.
        if merged.ends_with('\n') {
            after.push(String::new());
        }
        self.replace_line_range(pos.line, pos.line + 1, after)
    }

    /// Delete `count` characters starting at a byte position (`Pos.col` is a
    /// byte offset; a mid-character offset snaps back to the char start).
    /// Never crosses past the line end in M1; newline joining is handled by the
    /// normal layer with explicit range edits. Returns the deleted text and the
    /// edit.
    pub fn delete_chars(&mut self, pos: Pos, count: usize) -> VimResult<(String, RangeEdit)> {
        self.assert_line(pos.line)?;
        let line = self.lines[pos.line].as_str().to_string();
        let byte = snap_byte(&line, pos.col);
        let mut removed = String::new();
        let mut end_byte = byte;
        for ch in line[byte..].chars().take(count) {
            removed.push(ch);
            end_byte += ch.len_utf8();
        }
        let merged = format!("{}{}", &line[..byte], &line[end_byte..]);
        let edit = self.replace_line_range(pos.line, pos.line + 1, vec![merged])?;
        Ok((removed, edit))
    }

    /// Replace the text of one line wholesale (used by `:setline` etc.).
    pub fn set_line_text(&mut self, idx: Lineno, text: String) -> VimResult<RangeEdit> {
        self.assert_line(idx)?;
        self.replace_line_range(idx, idx + 1, vec![text])
    }

    pub fn append_lines(
        &mut self,
        after_line: Lineno,
        new_lines: Vec<String>,
    ) -> VimResult<RangeEdit> {
        let at = (after_line + 1).min(self.lines.len());
        self.replace_line_range(at, at, new_lines)
    }

    // --- persistence -----------------------------------------------------

    pub fn save(&mut self, path: &Path, bang: bool) -> VimResult<()> {
        if self.readonly && !bang {
            return Err(VimError::read_only());
        }
        fileio::write_file(path, self)?;
        self.name = Some(path.to_path_buf());
        self.changed = false;
        Ok(())
    }

    /// Save to the buffer's own `name` (`:w` without arguments).
    pub fn save_named(&mut self, bang: bool) -> VimResult<()> {
        let path = self.name.clone().ok_or_else(VimError::no_name)?;
        self.save(&path, bang)
    }

    // --- position validation --------------------------------------------

    pub fn assert_line(&self, line: Lineno) -> VimResult<()> {
        if line >= self.lines.len() {
            return Err(VimError::internal(format!(
                "line {line} out of range ({} lines)",
                self.lines.len()
            )));
        }
        Ok(())
    }

    /// Clamp a cursor position to valid line/column boundaries (snapping a
    /// byte column that lands inside a multi-byte character back to its start).
    pub fn clamp_pos(&self, mut pos: Pos) -> Pos {
        if self.lines.is_empty() {
            return Pos::new(0, 0);
        }
        pos.line = pos.line.min(self.lines.len() - 1);
        let line = self.lines[pos.line].as_str();
        pos.col = snap_byte(line, pos.col);
        pos
    }

    /// Byte offset of the first non-blank character on a line (`^` motion).
    pub fn first_nonblank_byte(&self, line: Lineno) -> VimResult<Col> {
        let text = self.line_str(line)?;
        let idx = text
            .char_indices()
            .find_map(|(i, ch)| (!rvim_core::charclass::is_blank(ch)).then_some(i))
            .unwrap_or(0);
        Ok(idx)
    }
}
