//! Cursor motions (reference: move.c, textobject.c).

use rvim_core::{Pos, VimError, VimResult, charclass};
use rvim_editor::Editor;

pub fn move_cursor(ed: &Editor, key: char, count: u64) -> VimResult<Pos> {
    let pos = ed.cursor();
    let buf = ed.current_buffer();
    let line_text = buf.line_str(pos.line)?;
    let mut next = pos;
    for _ in 0..count {
        match key {
            'h' => {
                next.col = rvim_core::text::move_chars(line_text, next.col, -1);
            }
            'l' | ' ' => {
                next.col = rvim_core::text::move_chars(line_text, next.col, 1);
            }
            'j' => {
                if next.line + 1 < buf.line_count() {
                    next.line += 1;
                }
            }
            'k' => {
                next.line = next.line.saturating_sub(1);
            }
            _ => {}
        }
    }
    let mut clamped = buf.clamp_pos(next);
    // In Normal mode the cursor rests *on* a character: `l`/` ` stop at the last
    // character instead of moving one past the end of the line (vim clamps
    // unless 'virtualedit' allows otherwise). `h` uses the same bound so it can
    // step back from the end-of-line position that `$` produces.
    if matches!(key, 'l' | ' ' | 'h') && !line_text.is_empty() {
        let last = line_text.len() - 1;
        clamped.col = clamped.col.min(last);
    }
    Ok(clamped)
}

pub fn line_end(ed: &Editor, count: u64) -> VimResult<Pos> {
    let buf = ed.current_buffer();
    let mut pos = ed.cursor();
    pos.line = (pos.line + count as usize - 1).min(buf.line_count() - 1);
    pos.col = buf.line(pos.line)?.len_bytes();
    Ok(pos)
}

/// "gg" (last=false) goes to line "count" (default 1); "G" (last=true)
/// defaults to the final line. Column lands on the first non-blank.
pub fn goto_line(ed: &Editor, count: u64, last: bool) -> VimResult<Pos> {
    let buf = ed.current_buffer();
    let target = if last && count == 1 {
        buf.line_count() - 1
    } else {
        (count.max(1) as usize - 1).min(buf.line_count() - 1)
    };
    let col = buf.first_nonblank_byte(target)?;
    Ok(Pos::new(target, col))
}

/// Word motions w/b/e using the default keyword class (iskeyword option hook
/// lands in M2).
pub fn word_motion(ed: &Editor, key: char, count: u64) -> VimResult<Pos> {
    let buf = ed.current_buffer();
    let mut pos = ed.cursor();
    for _ in 0..count {
        let text = buf.line_str(pos.line)?;
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let cur_col = rvim_core::text::byte_to_char_col(text, pos.col);
        match key {
            'w' | 'W' => {
                if let Some(found) = next_word_start(&chars, cur_col) {
                    pos.col = found;
                } else if pos.line + 1 < buf.line_count() {
                    pos.line += 1;
                    pos.col = buf.first_nonblank_byte(pos.line)?;
                }
            }
            'b' | 'B' => {
                if let Some(found) = prev_word_start(&chars, cur_col) {
                    pos.col = found;
                } else if pos.line > 0 {
                    pos.line -= 1;
                    pos.col = 0;
                }
            }
            'e' | 'E' => {
                if let Some(found) = next_word_end(&chars, cur_col) {
                    pos.col = found;
                } else if pos.line + 1 < buf.line_count() {
                    pos.line += 1;
                    let t = buf.line_str(pos.line)?;
                    let cc: Vec<(usize, char)> = t.char_indices().collect();
                    pos.col = next_word_end(&cc, 0).unwrap_or(0);
                }
            }
            _ => return Err(VimError::internal("unknown word motion")),
        }
    }
    Ok(buf.clamp_pos(pos))
}

fn next_word_start(chars: &[(usize, char)], char_col: usize) -> Option<usize> {
    let n = chars.len();
    let mut i = char_col;
    while i < n && charclass::is_word_char(chars[i].1) {
        i += 1;
    }
    while i < n && !charclass::is_word_char(chars[i].1) {
        i += 1;
    }
    chars.get(i).map(|(byte, _)| *byte)
}

fn prev_word_start(chars: &[(usize, char)], char_col: usize) -> Option<usize> {
    let mut i = char_col.min(chars.len());
    if i == 0 {
        return None;
    }
    i -= 1;
    while i > 0 && !charclass::is_word_char(chars[i].1) {
        i -= 1;
    }
    while i > 0 && charclass::is_word_char(chars[i - 1].1) {
        i -= 1;
    }
    chars.get(i).map(|(byte, _)| *byte)
}

fn next_word_end(chars: &[(usize, char)], char_col: usize) -> Option<usize> {
    let n = chars.len();
    let mut i = char_col;
    while i < n && !charclass::is_word_char(chars[i].1) {
        i += 1;
    }
    while i + 1 < n && charclass::is_word_char(chars[i + 1].1) {
        i += 1;
    }
    chars.get(i).map(|(byte, _)| *byte)
}
