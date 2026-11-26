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
    if matches!(key, 'j' | 'k') && !ed.mode.is_visual() {
        next.col = rvim_editor::display::byte_col(
            buf.line_str(next.line)?,
            ed.windows.current().curswant,
            ed.options.num_or("tabstop", 8).max(1) as usize,
        );
    }
    let mut clamped = buf.clamp_pos(next);
    // In Normal mode the cursor rests *on* a character: `l`/` ` stop at the last
    // character instead of moving one past the end of the line (vim clamps
    // unless 'virtualedit' allows otherwise). `h` uses the same bound so it can
    // step back from the end-of-line position that `$` produces.
    if matches!(key, 'l' | ' ' | 'h') && !line_text.is_empty() {
        let last = rvim_core::text::last_char_offset(line_text);
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

/// Word motions using Vim's default Unicode classes; uppercase WORD motions
/// group all non-whitespace characters together.
pub fn word_motion(ed: &Editor, key: char, count: u64) -> VimResult<Pos> {
    word_motion_inner(ed, key, count, false)
}

/// cw/cW stops at the current word end even on a one-character word.
pub(crate) fn change_word_end(ed: &Editor, key: char) -> VimResult<Pos> {
    word_motion_inner(ed, key, 1, true)
}

fn word_motion_inner(ed: &Editor, key: char, count: u64, stop: bool) -> VimResult<Pos> {
    let buf = ed.current_buffer();
    let mut pos = ed.cursor();
    let big = key.is_ascii_uppercase();
    for _ in 0..count {
        let text = buf.line_str(pos.line)?;
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let cur_col = rvim_core::text::byte_to_char_col(text, pos.col);
        match key {
            'w' | 'W' => {
                if let Some(found) = next_word_start(&chars, cur_col, big) {
                    pos.col = found;
                } else {
                    loop {
                        if pos.line + 1 == buf.line_count() {
                            pos.col = rvim_core::text::last_char_offset(buf.line_str(pos.line)?);
                            break;
                        }
                        pos.line += 1;
                        let text = buf.line_str(pos.line)?;
                        pos.col = text
                            .char_indices()
                            .find(|(_, ch)| class(*ch, big) != 0)
                            .map(|(byte, _)| byte)
                            .unwrap_or(0);
                        if text.is_empty() || text[pos.col..].chars().any(|ch| class(ch, big) != 0)
                        {
                            break;
                        }
                    }
                }
            }
            'b' | 'B' => {
                if let Some(found) = prev_word_start(&chars, cur_col, big) {
                    pos.col = found;
                } else {
                    pos.col = 0;
                    while pos.line > 0 {
                        pos.line -= 1;
                        pos.col = 0;
                        let chars: Vec<_> = buf.line_str(pos.line)?.char_indices().collect();
                        if let Some(found) = prev_word_start(&chars, chars.len(), big) {
                            pos.col = found;
                            break;
                        }
                        if chars.is_empty() {
                            break;
                        }
                    }
                }
            }
            'e' | 'E' => {
                if let Some(found) = next_word_end(&chars, cur_col, big, stop) {
                    pos.col = found;
                } else {
                    loop {
                        if pos.line + 1 == buf.line_count() {
                            pos.col = rvim_core::text::last_char_offset(buf.line_str(pos.line)?);
                            break;
                        }
                        pos.line += 1;
                        let cc: Vec<_> = buf.line_str(pos.line)?.char_indices().collect();
                        if let Some(found) = next_word_end(&cc, 0, big, true) {
                            pos.col = found;
                            break;
                        }
                    }
                }
            }
            _ => return Err(VimError::internal("unknown word motion")),
        }
    }
    Ok(buf.clamp_pos(pos))
}

fn class(ch: char, big: bool) -> u32 {
    let class = charclass::word_class(ch);
    if big && class != 0 { 1 } else { class }
}

fn next_word_start(chars: &[(usize, char)], char_col: usize, big: bool) -> Option<usize> {
    let n = chars.len();
    let mut i = char_col;
    let initial = chars.get(i).map(|(_, ch)| class(*ch, big)).unwrap_or(0);
    while i < n && initial != 0 && class(chars[i].1, big) == initial {
        i += 1;
    }
    while i < n && class(chars[i].1, big) == 0 {
        i += 1;
    }
    chars.get(i).map(|(byte, _)| *byte)
}

fn prev_word_start(chars: &[(usize, char)], char_col: usize, big: bool) -> Option<usize> {
    let mut i = char_col.min(chars.len());
    if i == 0 {
        return None;
    }
    i -= 1;
    while i > 0 && class(chars[i].1, big) == 0 {
        i -= 1;
    }
    let initial = class(chars[i].1, big);
    if initial == 0 {
        return None;
    }
    while i > 0 && class(chars[i - 1].1, big) == initial {
        i -= 1;
    }
    chars.get(i).map(|(byte, _)| *byte)
}

fn next_word_end(chars: &[(usize, char)], char_col: usize, big: bool, stop: bool) -> Option<usize> {
    let n = chars.len();
    let mut i = char_col + usize::from(!stop);
    while i < n && class(chars[i].1, big) == 0 {
        i += 1;
    }
    let initial = chars.get(i).map(|(_, ch)| class(*ch, big))?;
    while i + 1 < n && class(chars[i + 1].1, big) == initial {
        i += 1;
    }
    chars.get(i).map(|(byte, _)| *byte)
}
