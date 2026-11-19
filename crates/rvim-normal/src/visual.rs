//! Visual mode: char/line/block selections and the operators that consume
//! them (reference: normal.c nv_visual/do_pending_operator, ops.c).
//!
//! The selection lives on the editor as [`VisualState`]: the anchor is fixed
//! and the cursor keeps moving with the normal motions, so `d` in visual mode is
//! simply "apply the operator to anchor..cursor".
//!
//! Block selections (`<C-v>`) are handled here directly because the line-wise
//! and character-wise operators in [`crate::operators`] do not model a rectangle.

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::{Editor, Mode, RegType, Register, VisualState};

use crate::addsub;
use crate::operators::{self, OpRange};
use crate::textobjects::{self, Which};

/// Start visual mode with the given flavour, anchoring at the cursor.
pub fn start(ed: &mut Editor, mode: Mode) -> VimResult<()> {
    ed.block_insert = None;
    let pos = ed.cursor();
    ed.visual = Some(VisualState { mode, anchor: pos });
    ed.mode = mode;
    Ok(())
}

/// Leave visual mode: record `'<`/`'>` and restore Normal (normal.c end_visual_mode).
pub fn stop(ed: &mut Editor) {
    // Leaving visual mode cancels a block insert that never reached insert mode.
    if !ed.mode.is_insert_like() {
        ed.block_insert = None;
    }
    if let Some(state) = ed.visual.take() {
        let anchor = state.anchor;
        let last_cursor = ed.cursor();
        let (start, end) = (state.anchor, ed.cursor());
        let (first, last) = if start.line <= end.line {
            (start, end)
        } else {
            (end, start)
        };
        let bufnr = ed.current_bufnr();
        ed.marks.set_local(bufnr, '<', first);
        ed.marks.set_local(bufnr, '>', last);
        ed.last_visual = Some((
            VisualState {
                mode: state.mode,
                anchor,
            },
            last_cursor,
        ));
    }
    ed.mode = Mode::Normal;
}

/// The active selection as an operator range.
pub fn range(ed: &Editor) -> Option<OpRange> {
    let state = ed.visual?;
    let cursor = ed.cursor();
    let total = ed.line_count();
    match state.mode {
        Mode::VisualLine => Some(operators::line_range(
            state.anchor.line.min(cursor.line),
            state.anchor.line.max(cursor.line),
            total,
        )),
        _ => {
            let (start, end) = if (state.anchor.line, state.anchor.col) <= (cursor.line, cursor.col)
            {
                (state.anchor, cursor)
            } else {
                (cursor, state.anchor)
            };
            let text = ed.current_buffer().line_str(end.line).ok()?.to_string();
            let end_col = if end.col >= text.len() {
                text.len()
            } else {
                next_boundary(&text, end.col)
            };
            Some(OpRange {
                start,
                end: Pos::new(end.line, end_col),
                linewise: false,
            })
        }
    }
}

fn next_boundary(text: &str, byte: usize) -> usize {
    let start = rvim_core::text::snap_byte(text, byte);
    match text[start..].chars().next() {
        Some(ch) => start + ch.len_utf8(),
        None => start,
    }
}

/// Swap the two ends (`o`/`O` in visual mode).
pub fn swap_ends(ed: &mut Editor) -> VimResult<()> {
    let Some(state) = ed.visual else {
        return Ok(());
    };
    let cursor = ed.cursor();
    ed.set_cursor(state.anchor);
    ed.visual = Some(VisualState {
        mode: state.mode,
        anchor: cursor,
    });
    Ok(())
}

/// Block geometry: (first line, last line, first byte column, end byte column
/// exclusive) for the current block selection.
pub fn block_range(ed: &Editor) -> Option<(usize, usize, usize, usize)> {
    let state = ed.visual?;
    if state.mode != Mode::VisualBlock {
        return None;
    }
    let cursor = ed.cursor();
    let (first, last) = (
        state.anchor.line.min(cursor.line),
        state.anchor.line.max(cursor.line),
    );
    let (left, right) = (
        state.anchor.col.min(cursor.col),
        state.anchor.col.max(cursor.col),
    );
    Some((first, last, left, right))
}

/// Text of the block selection, one line per buffer line (vim's block register).
fn block_text(
    ed: &Editor,
    first: usize,
    last: usize,
    left: usize,
    right: usize,
) -> VimResult<String> {
    let mut out = Vec::new();
    for line in first..=last {
        let text = ed.current_buffer().line_str(line)?.to_string();
        let start = rvim_core::text::snap_byte(&text, left.min(text.len()));
        let end = if right >= text.len() {
            text.len()
        } else {
            next_boundary(&text, right)
        };
        out.push(text[start..end.max(start)].to_string());
    }
    Ok(out.join("\n"))
}

/// Apply one operator to the selection.
///
/// Returns `Ok(true)` when the operator left the editor in insert mode (`c` and
/// block `I`/`A`), so the caller keeps feeding keys as text.
pub fn apply_operator(ed: &mut Editor, op: char, reg: Option<char>) -> VimResult<bool> {
    let Some(state) = ed.visual else {
        return Ok(false);
    };
    if state.mode == Mode::VisualBlock && matches!(op, 'd' | 'y' | 'c' | 'I' | 'A') {
        return apply_block(ed, op, reg);
    }
    if state.mode == Mode::VisualBlock && matches!(op, '>' | '<') {
        // Block shifting indents the touched lines, like vim.
        let Some((first, last, _, _)) = block_range(ed) else {
            return Ok(false);
        };
        let range = operators::line_range(first, last, ed.line_count());
        shift(ed, range, op)?;
        stop(ed);
        return Ok(false);
    }
    let Some(range) = range(ed) else {
        return Ok(false);
    };
    match op {
        'd' => {
            operators::delete_range(ed, range, reg)?;
            stop(ed);
        }
        'y' => {
            operators::yank_range(ed, range, reg)?;
            stop(ed);
        }
        'c' => {
            ed.begin_undo_block();
            operators::change_range(ed, range, reg)?;
            ed.visual = None;
            ed.mode = Mode::Insert;
            return Ok(true);
        }
        '>' | '<' => {
            shift(ed, range, op)?;
            stop(ed);
        }
        '~' | 'u' | 'U' => {
            case_change(ed, range, op)?;
            stop(ed);
        }
        'J' | 'g' => {
            join_lines(ed, range, op == 'J')?;
            stop(ed);
        }
        _ => {}
    }
    Ok(false)
}

fn apply_block(ed: &mut Editor, op: char, reg: Option<char>) -> VimResult<bool> {
    let Some((first, last, left, right)) = block_range(ed) else {
        return Ok(false);
    };
    match op {
        'y' => {
            let text = block_text(ed, first, last, left, right)?;
            ed.registers.yank(
                reg,
                Register {
                    text,
                    reg_type: RegType::BlockWise,
                },
            );
            ed.set_cursor(Pos::new(first, left));
            stop(ed);
            Ok(false)
        }
        'd' => {
            let text = block_text(ed, first, last, left, right)?;
            ed.registers.delete(
                reg,
                Register {
                    text,
                    reg_type: RegType::BlockWise,
                },
            );
            ed.begin_undo_block();
            for line in (first..=last).rev() {
                let content = ed.current_buffer().line_str(line)?.to_string();
                let start = rvim_core::text::snap_byte(&content, left.min(content.len()));
                let end = if right >= content.len() {
                    content.len()
                } else {
                    next_boundary(&content, right)
                };
                let count = content[start..end.max(start)].chars().count();
                if count > 0 {
                    ed.delete_chars(Pos::new(line, start), count)?;
                }
            }
            ed.end_undo_block();
            ed.set_cursor(Pos::new(first, 0));
            stop(ed);
            Ok(false)
        }
        'c' => {
            let text = block_text(ed, first, last, left, right)?;
            ed.registers.delete(
                reg,
                Register {
                    text,
                    reg_type: RegType::BlockWise,
                },
            );
            ed.begin_undo_block();
            for line in (first..=last).rev() {
                let content = ed.current_buffer().line_str(line)?.to_string();
                let start = rvim_core::text::snap_byte(&content, left.min(content.len()));
                let end = if right >= content.len() {
                    content.len()
                } else {
                    next_boundary(&content, right)
                };
                let count = content[start..end.max(start)].chars().count();
                if count > 0 {
                    ed.delete_chars(Pos::new(line, start), count)?;
                }
            }
            ed.set_cursor(Pos::new(first, left));
            ed.visual = None;
            ed.mode = Mode::Insert;
            Ok(true)
        }
        // Block insert: type once, and the text is repeated on every other line
        // of the block when insert mode ends (ops.c op_insert/block_insert).
        'I' | 'A' => {
            let first_text = ed.current_buffer().line_str(first)?.to_string();
            let col = if op == 'I' {
                rvim_core::text::snap_byte(&first_text, left.min(first_text.len()))
            } else {
                // "A" appends after the block's rightmost character.
                if right >= first_text.len() {
                    first_text.len()
                } else {
                    next_boundary(&first_text, right)
                }
            };
            ed.set_cursor(Pos::new(first, col));
            ed.visual = None;
            ed.block_insert = Some(rvim_editor::BlockInsert {
                first,
                last,
                col,
                append: op == 'A',
            });
            ed.mode = Mode::Insert;
            ed.begin_undo_block();
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// `>`/`<` over a range: indent/dedent each line by `'shiftwidth'`.
pub fn shift(ed: &mut Editor, range: OpRange, op: char) -> VimResult<()> {
    let shift = {
        let sw = ed.options.num_or("shiftwidth", 8).max(0) as usize;
        if sw == 0 {
            ed.options.num_or("tabstop", 8).max(1) as usize
        } else {
            sw
        }
    };
    let expandtab = ed.options.bool_or("expandtab", false);
    let (first, last) = if range.linewise {
        (range.start.line, range.end.line.saturating_sub(1))
    } else {
        (range.start.line, range.end.line)
    };
    let last = last.min(ed.line_count().saturating_sub(1));
    ed.begin_undo_block();
    for line in first..=last {
        let text = ed.current_buffer().line_str(line)?.to_string();
        let next = if op == '>' {
            if expandtab {
                format!("{}{}", " ".repeat(shift), text)
            } else {
                format!("\t{}", text)
            }
        } else {
            let mut removed = 0usize;
            let mut rest = text.as_str();
            while removed < shift {
                if let Some(tail) = rest.strip_prefix(' ') {
                    rest = tail;
                    removed += 1;
                } else if let Some(tail) = rest.strip_prefix('\t') {
                    rest = tail;
                    removed = shift;
                } else {
                    break;
                }
            }
            rest.to_string()
        };
        ed.set_line_text(line, next)?;
    }
    ed.end_undo_block();
    Ok(())
}

/// `~`/`u`/`U` over a range: toggle/lower/upper case of the selected text.
fn case_change(ed: &mut Editor, range: OpRange, op: char) -> VimResult<()> {
    let (first, last) = (range.start.line, range.end.line.min(ed.line_count() - 1));
    ed.begin_undo_block();
    for line in first..=last {
        let text = ed.current_buffer().line_str(line)?.to_string();
        let (from, to) = if line == range.start.line || line == last {
            let from = if line == range.start.line {
                range.start.col
            } else {
                0
            };
            let to = if line == range.end.line {
                range.end.col
            } else {
                text.len()
            };
            (
                from.min(text.len()),
                to.min(text.len()).max(from.min(text.len())),
            )
        } else {
            (0, text.len())
        };
        let start = rvim_core::text::snap_byte(&text, from);
        let end = rvim_core::text::snap_byte(&text, to).max(start);
        let mid = &text[start..end];
        let changed: String = mid
            .chars()
            .map(|c| match op {
                'u' => c.to_lowercase().collect::<String>(),
                'U' => c.to_uppercase().collect::<String>(),
                _ => {
                    if c.is_uppercase() {
                        c.to_lowercase().collect::<String>()
                    } else {
                        c.to_uppercase().collect::<String>()
                    }
                }
            })
            .collect();
        let next = format!("{}{}{}", &text[..start], changed, &text[end..]);
        ed.set_line_text(line, next)?;
    }
    ed.end_undo_block();
    Ok(())
}

/// `J`/`gJ` over a range.
fn join_lines(ed: &mut Editor, range: OpRange, with_space: bool) -> VimResult<()> {
    let first = range.start.line;
    let last = if range.linewise {
        range.end.line.saturating_sub(1)
    } else {
        range.end.line
    }
    .min(ed.line_count().saturating_sub(1));
    if last <= first {
        return Ok(());
    }
    let mut joined = ed.current_buffer().line_str(first)?.to_string();
    for line in first + 1..=last {
        let text = ed.current_buffer().line_str(line)?.to_string();
        let text = text.trim_start();
        if with_space && !joined.is_empty() && !joined.ends_with(' ') && !text.is_empty() {
            joined.push(' ');
        }
        joined.push_str(text);
    }
    ed.begin_undo_block();
    ed.replace_lines(first, last + 1, vec![joined])?;
    ed.end_undo_block();
    ed.set_cursor(Pos::new(first, 0));
    Ok(())
}

/// `i`/`a` text objects inside visual mode (`viw`, `va(`, ...).
pub fn select_object(ed: &mut Editor, obj: char, which: Which) -> VimResult<bool> {
    let Some(state) = ed.visual else {
        return Ok(false);
    };
    let lines: Vec<String> = (0..ed.line_count())
        .filter_map(|i| ed.current_buffer().line_str(i).ok().map(str::to_string))
        .collect();
    let cursor = ed.cursor();
    match textobjects::text_object(&lines, (cursor.line, cursor.col), obj, which) {
        Some(found) => {
            ed.visual = Some(VisualState {
                mode: state.mode,
                anchor: Pos::new(found.start_line, found.start_col),
            });
            ed.set_cursor(Pos::new(found.end_line, found.end_col));
            Ok(true)
        }
        None => {
            ed.emsg(format!("E475: object not found for {obj}"));
            Ok(false)
        }
    }
}

/// `gv`: restore the last visual selection.
pub fn reselect(ed: &mut Editor) -> bool {
    match ed.last_visual {
        Some((state, cursor)) => {
            ed.visual = Some(state);
            ed.mode = state.mode;
            ed.set_cursor(cursor);
            true
        }
        None => false,
    }
}

/// `:` from visual mode: the range prefix vim inserts (`:'<,'>`).
pub fn ex_range_prefix(ed: &mut Editor) -> String {
    stop(ed);
    "'<,'>".to_string()
}

/// `g<C-a>` / `g<C-x>` in Visual mode: add/subtract a sequence of amounts,
/// one line at a time (ops.c `op_addsub()` with `g_cmd` set).
///
/// The first line gets `count`, and every line that actually contains a number
/// adds another `count` to the amount, so lines without numbers do not consume a
/// step: selecting "0 / x / 0" and pressing `g<C-a>` gives "1 / x / 2".
pub fn visual_addsub(ed: &mut Editor, add: bool, count: u64) -> VimResult<bool> {
    let Some(state) = ed.visual else {
        return Ok(false);
    };
    let cursor = ed.cursor();
    let anchor = state.anchor;
    let (top, bot) = if anchor.line <= cursor.line {
        (anchor.line, cursor.line)
    } else {
        (cursor.line, anchor.line)
    };
    let step = count.max(1) as i128;
    let mut amount: i128 = step;
    let block = if state.mode == Mode::VisualBlock {
        let left = anchor.col.min(cursor.col);
        let right = anchor.col.max(cursor.col);
        Some((left, right))
    } else {
        None
    };
    // The two ends of a charwise selection (inclusive byte columns).
    let (start_pos, end_pos) = if (anchor.line, anchor.col) <= (cursor.line, cursor.col) {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    };
    ed.begin_undo_block();
    let mut changed = false;
    for line in top..=bot {
        let text = ed.current_buffer().line_str(line)?.to_string();
        let len = text.len();
        let (from, to) = match state.mode {
            Mode::VisualLine => (0, len),
            Mode::VisualBlock => {
                let (left, right) = block.expect("block geometry");
                let end = if right >= len {
                    len
                } else {
                    next_boundary(&text, right)
                };
                (left.min(len), end)
            }
            _ => {
                let from = if line == top {
                    start_pos.col.min(len)
                } else {
                    0
                };
                let to = if line == bot {
                    if end_pos.col >= len {
                        len
                    } else {
                        next_boundary(&text, end_pos.col)
                    }
                } else {
                    len
                };
                (from, to)
            }
        };
        if addsub::addsub_region(ed, line, from, to, add, amount.unsigned_abs() as u64)? {
            changed = true;
            amount += step;
        }
    }
    ed.end_undo_block();
    // vim leaves Visual mode and puts the cursor back where it was.
    stop(ed);
    let clamped = ed.current_buffer().clamp_pos(cursor);
    ed.set_cursor(clamped);
    Ok(changed)
}

/// Normal-mode `gq`/`gw` formatting helper: re-wrap the range to 'textwidth'.
pub fn format_range(ed: &mut Editor, range: OpRange) -> VimResult<()> {
    let width = ed.options.num_or("textwidth", 0).max(0) as usize;
    if width == 0 {
        return Err(VimError::new("E474", "Invalid argument: 'textwidth' is 0"));
    }
    let (first, last) = if range.linewise {
        (range.start.line, range.end.line.saturating_sub(1))
    } else {
        (range.start.line, range.end.line)
    };
    let last = last.min(ed.line_count().saturating_sub(1));
    let mut text = String::new();
    for line in first..=last {
        let l = ed.current_buffer().line_str(line)?.trim().to_string();
        if l.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&l);
    }
    let mut wrapped: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            wrapped.push(current);
            current = String::new();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        wrapped.push(current);
    }
    if wrapped.is_empty() {
        return Ok(());
    }
    ed.begin_undo_block();
    ed.replace_lines(first, last + 1, wrapped)?;
    ed.end_undo_block();
    ed.set_cursor(Pos::new(first, 0));
    Ok(())
}
