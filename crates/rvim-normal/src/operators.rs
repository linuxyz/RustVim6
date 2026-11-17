//! Normal-mode operators (reference: ops.c, normal.c do_pending_operator).

use rvim_core::{Pos, VimResult};
use rvim_editor::{Editor, RegType, Register};

use crate::motions;

pub fn delete_char(ed: &mut Editor, count: u64, reg: Option<char>) -> VimResult<()> {
    let pos = ed.cursor();
    let removed = ed.delete_chars(pos, count as usize)?;
    ed.registers.delete(
        reg,
        Register {
            text: removed,
            reg_type: RegType::CharWise,
        },
    );
    Ok(())
}

pub fn delete_lines(ed: &mut Editor, count: u64, reg: Option<char>) -> VimResult<()> {
    let start = ed.cursor().line;
    let end = (start + count as usize).min(ed.line_count());
    ed.delete_lines(start, end, reg)?;
    ed.set_cursor(Pos::new(start.min(ed.line_count() - 1), 0));
    Ok(())
}

pub fn yank_lines(ed: &mut Editor, count: u64, reg: Option<char>) -> VimResult<()> {
    let start = ed.cursor().line;
    let end = (start + count as usize).min(ed.line_count());
    ed.yank_lines(start, end, reg)
}

/// "p" pastes after the cursor line (line-wise) or after the cursor column
/// (char-wise); "P" pastes before.
pub fn paste(ed: &mut Editor, key: char, reg: Option<char>) -> VimResult<()> {
    let name = reg.unwrap_or('"');
    let data = ed
        .registers
        .get(name)
        .or_else(|| ed.registers.unnamed())
        .cloned()
        .ok_or_else(|| rvim_core::VimError::new("E353", "Nothing to put".to_string()))?;
    match data.reg_type {
        RegType::LineWise => {
            let lines: Vec<String> = data
                .text
                .trim_end_matches('\n')
                .lines()
                .map(str::to_string)
                .collect();
            let at = if key == 'p' {
                ed.cursor().line + 1
            } else {
                ed.cursor().line
            };
            let at = at.min(ed.line_count());
            ed.replace_lines(at, at, lines)?;
            ed.set_cursor(Pos::new(at, 0));
        }
        RegType::CharWise | RegType::BlockWise => {
            let pos = ed.cursor();
            let col = if key == 'p' {
                rvim_core::text::move_chars(&ed.current_line(), pos.col, 1)
            } else {
                pos.col
            };
            ed.insert_text(Pos::new(pos.line, col), &data.text)?;
        }
    }
    Ok(())
}
/// Target range of a pending operator (d, y, c) resolved from its motion key
/// (normal.c do_pending_operator()).
///
/// With linewise set, the range covers whole lines: [start.line, end.line).
/// Otherwise it is a character-wise range of bytes [start, end), which may span
/// several lines (end.line is inclusive there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpRange {
    pub start: Pos,
    pub end: Pos,
    pub linewise: bool,
}

/// Byte offset just past the character starting at byte (a character boundary).
fn next_boundary(text: &str, byte: usize) -> usize {
    let start = rvim_core::text::snap_byte(text, byte);
    match text[start..].chars().next() {
        Some(ch) => start + ch.len_utf8(),
        None => start,
    }
}

/// A whole-line range covering first..=last (both 0-based).
pub fn line_range(first: usize, last: usize, total: usize) -> OpRange {
    let start = first.min(total.saturating_sub(1));
    let end = (last + 1).max(start + 1).min(total);
    OpRange {
        start: Pos::new(start, 0),
        end: Pos::new(end, 0),
        linewise: true,
    }
}

/// "gg" as an operator motion: from line 1 (or count) to the cursor line.
pub fn gg_range(ed: &Editor, count: u64) -> VimResult<OpRange> {
    let total = ed.line_count();
    let target = (count.max(1) as usize - 1).min(total.saturating_sub(1));
    let cur = ed.cursor().line;
    Ok(line_range(cur.min(target), cur.max(target), total))
}

/// Resolve a motion key into the range its operator applies to.
///
/// count is the total count (operator count times motion count, so 3d2w is 6).
/// Returns Ok(None) when the key is not a motion this engine supports.
pub fn motion_range(ed: &Editor, motion: char, count: u64) -> VimResult<Option<OpRange>> {
    let count = count.max(1);
    let cur = ed.cursor();
    let text = ed.current_buffer().line_str(cur.line)?.to_string();
    let cur_end = next_boundary(&text, cur.col);
    let total = ed.line_count();
    let charwise = |start: Pos, end: Pos| {
        Some(OpRange {
            start,
            end,
            linewise: false,
        })
    };
    Ok(match motion {
        // h deletes backwards; at column 1 it is a no-op (vim does not wrap).
        // The cursor character is not part of the range.
        'h' => {
            let target = motions::move_cursor(ed, 'h', count)?;
            if target == cur {
                charwise(cur, cur)
            } else {
                charwise(target, cur)
            }
        }
        // l/dl deletes the character under the cursor (like x); a count deletes
        // that many characters, so the end is the target character itself.
        'l' | ' ' => {
            let target = motions::move_cursor(ed, 'l', count)?;
            let end = if target.col == cur.col {
                cur_end
            } else {
                target.col
            };
            charwise(cur, Pos::new(cur.line, end))
        }
        // dw stops at the end of the line instead of swallowing the next one
        // (normal.c keeps an exclusive motion from ending in column 1).
        'w' | 'W' => {
            let target = motions::word_motion(ed, motion, count)?;
            if target.line == cur.line && target != cur {
                charwise(cur, target)
            } else {
                // Last word of the buffer (or a motion that crosses the line
                // end): "dw" stops at the end of this line.
                charwise(cur, Pos::new(cur.line, text.len()))
            }
        }
        // e is inclusive of the last character of the word.
        'e' | 'E' => {
            let target = motions::word_motion(ed, motion, count)?;
            let target_text = ed.current_buffer().line_str(target.line)?;
            charwise(
                cur,
                Pos::new(target.line, next_boundary(target_text, target.col)),
            )
        }
        'b' | 'B' => {
            let target = motions::word_motion(ed, motion, count)?;
            charwise(target, cur)
        }
        // "$" honours a count by moving down that many lines (move.c nv_dollar).
        '$' => {
            let target = motions::line_end(ed, count)?;
            charwise(cur, target)
        }
        '0' => charwise(Pos::new(cur.line, 0), cur),
        '^' | '_' => {
            let col = ed.current_buffer().first_nonblank_byte(cur.line)?;
            charwise(Pos::new(cur.line, col), cur)
        }
        // Line-wise motions: the range is the inclusive span of whole lines.
        'j' => Some(line_range(cur.line, cur.line + count as usize, total)),
        'k' => Some(line_range(
            cur.line.saturating_sub(count as usize),
            cur.line,
            total,
        )),
        'G' => {
            let target = motions::goto_line(ed, count, true)?.line;
            Some(line_range(
                cur.line.min(target),
                cur.line.max(target),
                total,
            ))
        }
        _ => None,
    })
}

/// Text of a character-wise range plus its snapped start position.
fn charwise_text(ed: &Editor, range: OpRange) -> VimResult<(String, Pos, usize)> {
    let text =
        |line: usize| -> VimResult<String> { Ok(ed.current_buffer().line_str(line)?.to_string()) };
    let first = text(range.start.line)?;
    let start_col = rvim_core::text::snap_byte(&first, range.start.col);
    if range.start.line == range.end.line {
        let end_col = rvim_core::text::snap_byte(&first, range.end.col);
        if start_col >= end_col {
            return Ok((
                String::new(),
                Pos::new(range.start.line, start_col),
                end_col,
            ));
        }
        return Ok((
            first[start_col..end_col].to_string(),
            Pos::new(range.start.line, start_col),
            end_col,
        ));
    }
    let last = text(range.end.line)?;
    let end_col = rvim_core::text::snap_byte(&last, range.end.col);
    let mut removed = first[start_col..].to_string();
    for line in range.start.line + 1..range.end.line {
        removed.push('\n');
        removed.push_str(&text(line)?);
    }
    removed.push('\n');
    removed.push_str(&last[..end_col]);
    Ok((removed, Pos::new(range.start.line, start_col), end_col))
}

/// Position a normal-mode cursor on the last character of a line when col points
/// past the end of it (normal.c leaves the cursor on a character).
fn normal_cursor(ed: &Editor, line: usize, col: usize) -> Pos {
    let text = ed.current_buffer().line_str(line).unwrap_or("");
    let col = rvim_core::text::snap_byte(text, col);
    let col = if !text.is_empty() && col >= text.len() {
        rvim_core::text::last_char_offset(text)
    } else {
        col
    };
    Pos::new(line, col)
}

/// Delete the operator range, storing it in the register (ops.c op_delete()).
pub fn delete_range(ed: &mut Editor, range: OpRange, reg: Option<char>) -> VimResult<()> {
    if range.linewise {
        let total = ed.line_count();
        let start = range.start.line.min(total.saturating_sub(1));
        let end = range.end.line.max(start + 1).min(total);
        ed.delete_lines(start, end, reg)?;
        ed.set_cursor(Pos::new(start.min(ed.line_count() - 1), 0));
        return Ok(());
    }
    let (removed, start, end_col) = charwise_text(ed, range)?;
    if removed.is_empty() {
        return Ok(());
    }
    ed.registers.delete(
        reg,
        Register {
            text: removed,
            reg_type: RegType::CharWise,
        },
    );
    let first = ed.current_buffer().line_str(range.start.line)?.to_string();
    let last = ed.current_buffer().line_str(range.end.line)?.to_string();
    let merged = format!("{}{}", &first[..start.col], &last[end_col..]);
    ed.replace_lines(range.start.line, range.end.line + 1, vec![merged])?;
    ed.set_cursor(normal_cursor(ed, range.start.line, start.col));
    Ok(())
}

/// Yank the operator range without editing (ops.c op_yank()).
pub fn yank_range(ed: &mut Editor, range: OpRange, reg: Option<char>) -> VimResult<()> {
    if range.linewise {
        let total = ed.line_count();
        let start = range.start.line.min(total.saturating_sub(1));
        let end = range.end.line.max(start + 1).min(total);
        ed.yank_lines(start, end, reg)?;
        let col = ed.cursor().col;
        ed.set_cursor(normal_cursor(ed, start, col));
        return Ok(());
    }
    let (text, start, _) = charwise_text(ed, range)?;
    if text.is_empty() {
        return Ok(());
    }
    ed.registers.yank(
        reg,
        Register {
            text,
            reg_type: RegType::CharWise,
        },
    );
    ed.set_cursor(normal_cursor(ed, start.line, start.col));
    Ok(())
}

/// c operator: delete the range and leave the cursor where insert mode starts.
/// A line-wise cc keeps a single (empty) line, as vim does.
pub fn change_range(ed: &mut Editor, range: OpRange, reg: Option<char>) -> VimResult<()> {
    if !range.linewise {
        return delete_range(ed, range, reg);
    }
    let total = ed.line_count();
    let start = range.start.line.min(total.saturating_sub(1));
    let end = range.end.line.max(start + 1).min(total);
    let texts: Vec<String> = ed.current_buffer().lines()[start..end]
        .iter()
        .map(|l| l.as_str().to_string())
        .collect();
    ed.registers.delete(
        reg,
        Register {
            text: texts.join("\n") + "\n",
            reg_type: RegType::LineWise,
        },
    );
    ed.replace_lines(start, end, vec![String::new()])?;
    ed.set_cursor(Pos::new(start, 0));
    Ok(())
}
