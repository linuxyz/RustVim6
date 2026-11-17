//! Insert-mode entry and text entry helpers (headless; the TUI drives the same
//! API with real key events).

use rvim_core::{Pos, VimResult};
use rvim_editor::{Editor, Mode};

/// Enter insert mode for the i/a/A/o/O/I entry points.
pub fn enter_insert(ed: &mut Editor, key: char) -> VimResult<()> {
    let mut pos = ed.cursor();
    match key {
        'i' => {}
        'a' => {
            let line = ed.current_line();
            pos.col = rvim_core::text::move_chars(&line, pos.col, 1);
        }
        'A' => {
            pos.col = ed.current_buffer().line(pos.line)?.len_bytes();
        }
        'I' => {
            pos.col = ed.current_buffer().first_nonblank_byte(pos.line)?;
        }
        'o' => {
            // open below: insert an empty line and land on it
            let at = pos.line + 1;
            ed.replace_lines(at, at, vec![String::new()])?;
            pos = Pos::new(at, 0);
        }
        'O' => {
            let at = pos.line;
            ed.replace_lines(at, at, vec![String::new()])?;
            pos = Pos::new(at, 0);
        }
        _ => {}
    }
    ed.set_cursor(pos);
    ed.mode = Mode::Insert;
    Ok(())
}

/// Type one character while in insert mode. A newline splits the line.
pub fn type_char(ed: &mut Editor, engine: &mut crate::NormalEngine, ch: char) -> VimResult<()> {
    let pos = ed.cursor();
    let text = ch.to_string();
    ed.insert_text(pos, &text)?;
    engine.note_insert(&text);
    if ch == '\n' {
        ed.set_cursor(Pos::new(pos.line + 1, 0));
    } else {
        let line = ed.current_line();
        ed.set_cursor(Pos::new(
            pos.line,
            rvim_core::text::move_chars(&line, pos.col, 1),
        ));
    }
    Ok(())
}

/// Backspace in insert mode: delete the char before the cursor, joining the
/// previous line at line start.
pub fn backspace(ed: &mut Editor, engine: &mut crate::NormalEngine) -> VimResult<()> {
    let pos = ed.cursor();
    if pos.col > 0 {
        let line = ed.current_line();
        let target = Pos::new(pos.line, rvim_core::text::move_chars(&line, pos.col, -1));
        let removed = ed.delete_chars(target, 1)?;
        engine.note_insert(&removed);
        ed.set_cursor(target);
    } else if pos.line > 0 {
        let prev_len = ed.current_buffer().line_str(pos.line - 1)?.len();
        let prev = ed.current_buffer().line_str(pos.line - 1)?.to_string();
        let cur = ed.current_buffer().line_str(pos.line)?.to_string();
        let merged = format!("{prev}{cur}");
        ed.replace_lines(pos.line - 1, pos.line + 1, vec![merged])?;
        ed.set_cursor(Pos::new(pos.line - 1, prev_len));
    }
    Ok(())
}
