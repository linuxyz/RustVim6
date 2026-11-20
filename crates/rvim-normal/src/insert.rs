//! Insert-mode entry and text entry helpers (headless; the TUI drives the same
//! API with real key events).

use rvim_core::{Pos, VimResult};
use rvim_editor::indent::{cindent, indent_string, new_line_indent};
use rvim_editor::maps::Key;
use rvim_editor::{Editor, Mode, RegType};

use crate::NormalEngine;

/// Leading white space of a line: what 'autoindent' copies (edit.c get_indent).
fn indent_of(line: &str) -> String {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect::<String>()
}

/// Leading white space of `line`, for callers outside this module (cc/S keep
/// the indent of the first changed line with 'autoindent').
pub fn indent_of_line(line: &str) -> String {
    indent_of(line)
}

/// The indent a new line inherits from the current one, when 'autoindent' is on
/// (edit.c ins_eol() / open_line() with p_ai).
fn autoindent(ed: &Editor) -> String {
    if !ed.bool_option("autoindent", false) {
        return String::new();
    }
    indent_of(&ed.current_line())
}

/// Byte length of the leading white space of `line`.
fn indent_len(line: &str) -> usize {
    indent_of(line).len()
}

/// Screen columns of the leading white space of `line`.
fn indent_cols(line: &str, tabstop: usize) -> usize {
    let ts = tabstop.max(1);
    let mut col = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col += ts - col % ts,
            _ => break,
        }
    }
    col
}

/// Typing '}' or '#' at the start of a line reindents it, like Vim's
/// 'cinkeys' ("0}", "0#") and 'smartindent'.
fn reindent_start_char(ed: &mut Editor, ch: char) -> VimResult<()> {
    if ch != '}' && ch != '#' {
        return Ok(());
    }
    let cindent_on = ed.bool_option("cindent", false);
    let smart_on = ed.bool_option("smartindent", false);
    if !cindent_on && !smart_on {
        return Ok(());
    }
    let pos = ed.cursor();
    let line = ed.current_line();
    let body = line.trim_start_matches([' ', '\t']);
    if !body.starts_with(ch) {
        return Ok(());
    }
    let indent_bytes = line.len() - body.len();
    if pos.col != indent_bytes + ch.len_utf8() {
        return Ok(());
    }
    let tabstop = ed.options.num_or("tabstop", 8).max(1) as usize;
    let sw = {
        let sw = ed.options.num_or("shiftwidth", 8).max(0) as usize;
        if sw == 0 { tabstop } else { sw }
    };
    let expandtab = ed.options.bool_or("expandtab", false);
    let shiftround = ed.options.bool_or("shiftround", false);
    let cols = if ch == '#' {
        0
    } else if cindent_on {
        cindent(ed, pos.line as i64 + 1, 0)
    } else {
        let mut cols = indent_cols(&line[..indent_bytes], tabstop).saturating_sub(sw);
        if shiftround {
            cols -= cols % sw;
        }
        cols as i64
    };
    let new_indent = indent_string(cols, expandtab, tabstop as i64);
    if new_indent == line[..indent_bytes] {
        return Ok(());
    }
    let new_text = format!("{new_indent}{}", &line[indent_bytes..]);
    ed.set_line_text(pos.line, new_text)?;
    ed.set_cursor(Pos::new(pos.line, new_indent.len() + ch.len_utf8()));
    Ok(())
}

/// Enter insert mode for the i/a/A/o/O/I entry points.
pub fn enter_insert(ed: &mut Editor, key: char) -> VimResult<()> {
    let mut pos = ed.cursor();
    // 'o'/'O' copy the indent of the line they were opened from with
    // 'autoindent', so it has to be read before the new empty line replaces
    // it.  'cindent'/'smartindent' compute it from the line above instead.
    let auto = autoindent(ed);
    let smart_indent = ed.bool_option("cindent", false) || ed.bool_option("smartindent", false);
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
            // open below: insert one line with the computed indent
            let at = pos.line + 1;
            let indent = if smart_indent {
                new_line_indent(ed, pos.line).unwrap_or_default()
            } else {
                auto
            };
            ed.replace_lines(at, at, vec![indent.clone()])?;
            pos = Pos::new(at, indent.len());
        }
        'O' => {
            let at = pos.line;
            let indent = if smart_indent {
                if at == 0 {
                    String::new()
                } else {
                    new_line_indent(ed, at - 1).unwrap_or_default()
                }
            } else {
                auto
            };
            ed.replace_lines(at, at, vec![indent.clone()])?;
            pos = Pos::new(at, indent.len());
        }
        _ => {}
    }
    ed.set_cursor(pos);
    ed.mode = Mode::Insert;
    Ok(())
}

/// Type one character while in insert mode. A newline splits the line.
pub fn type_char(ed: &mut Editor, engine: &mut NormalEngine, ch: char) -> VimResult<()> {
    let pos = ed.cursor();
    let text = ch.to_string();
    ed.insert_text(pos, &text)?;
    engine.note_insert(&text);
    engine.note_key(Key::Char(ch));
    if ch == '\n' {
        ed.set_cursor(Pos::new(pos.line + 1, 0));
    } else {
        let line = ed.current_line();
        ed.set_cursor(Pos::new(
            pos.line,
            rvim_core::text::move_chars(&line, pos.col, 1),
        ));
        reindent_start_char(ed, ch)?;
    }
    Ok(())
}

/// `<CR>`: split the line and give the new line the indent chosen by
/// 'cindent', 'smartindent' or 'autoindent' (edit.c ins_eol() -> open_line()).
pub fn newline(ed: &mut Editor, engine: &mut NormalEngine) -> VimResult<()> {
    // The indent is computed from the line being left, before the split.
    let indent = new_line_indent(ed, ed.cursor().line);
    type_char(ed, engine, '\n')?;
    if let Some(indent) = indent {
        for ch in indent.chars() {
            type_char(ed, engine, ch)?;
        }
    }
    Ok(())
}

/// True when the 'backspace' option contains `what`.
fn backspace_allows(ed: &Editor, what: &str) -> bool {
    ed.options
        .str_or("backspace", "")
        .split(',')
        .any(|part| part == what)
}

/// Backspace in insert mode: delete the char before the cursor, joining the
/// previous line at line start.
pub fn backspace(ed: &mut Editor, engine: &mut NormalEngine) -> VimResult<()> {
    let pos = ed.cursor();
    if pos.col > 0 {
        let line = ed.current_line();
        let before = &line[..pos.col.min(line.len())];
        // 'backspace' containing "indent" removes the whole indent in one step
        // when the cursor sits at its end (edit.c ins_bs() BS_INDENT).
        if backspace_allows(ed, "indent")
            && !before.is_empty()
            && before.chars().all(|c| c == ' ' || c == '\t')
        {
            let count = before.chars().count();
            let _ = ed.delete_chars(Pos::new(pos.line, 0), count)?;
            ed.set_cursor(Pos::new(pos.line, 0));
            engine.note_key(Key::Special("BS".to_string()));
            return Ok(());
        }
        let target = Pos::new(pos.line, rvim_core::text::move_chars(&line, pos.col, -1));
        let _ = ed.delete_chars(target, 1)?;
        ed.set_cursor(target);
    } else if pos.line > 0 && backspace_allows(ed, "eol") {
        let prev_len = ed.current_buffer().line_str(pos.line - 1)?.len();
        let prev = ed.current_buffer().line_str(pos.line - 1)?.to_string();
        let cur = ed.current_buffer().line_str(pos.line)?.to_string();
        let merged = format!("{prev}{cur}");
        ed.replace_lines(pos.line - 1, pos.line + 1, vec![merged])?;
        ed.set_cursor(Pos::new(pos.line - 1, prev_len));
    }
    engine.note_key(Key::Special("BS".to_string()));
    Ok(())
}

/// Byte offset of the character that starts the word before `col`.
fn word_start(line: &str, col: usize) -> usize {
    let before = &line[..col.min(line.len())];
    let chars: Vec<char> = before.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut n = chars.len();
    // Vim skips white space first and then the whole word (edit.c ins_bs()).
    while n > 0 && chars[n - 1].is_whitespace() {
        n -= 1;
    }
    if n > 0 {
        if is_word(chars[n - 1]) {
            while n > 0 && is_word(chars[n - 1]) {
                n -= 1;
            }
        } else {
            while n > 0 && !is_word(chars[n - 1]) && !chars[n - 1].is_whitespace() {
                n -= 1;
            }
        }
    }
    chars.iter().take(n).map(|c| c.len_utf8()).sum()
}

/// `<C-w>`: delete the word (and its white space) before the cursor.
pub fn delete_word(ed: &mut Editor, engine: &mut NormalEngine) -> VimResult<()> {
    let pos = ed.cursor();
    if pos.col == 0 {
        // Nothing left on this line: vim joins the previous line.
        return backspace(ed, engine);
    }
    let line = ed.current_line();
    let start = word_start(&line, pos.col);
    if start == pos.col {
        return Ok(());
    }
    let count = line[start..pos.col.min(line.len())].chars().count();
    let _ = ed.delete_chars(Pos::new(pos.line, start), count)?;
    ed.set_cursor(Pos::new(pos.line, start));
    engine.note_key(Key::Special("C-W".to_string()));
    Ok(())
}

/// `<C-u>`: delete everything entered before the cursor on this line
/// (edit.c ins_bs() BS_INS).
pub fn delete_to_start(ed: &mut Editor, engine: &mut NormalEngine) -> VimResult<()> {
    let pos = ed.cursor();
    let line = ed.current_line();
    let start = if pos.line == engine.insert_start.line {
        engine.insert_start.col.min(pos.col)
    } else {
        indent_len(&line).min(pos.col)
    };
    if start >= pos.col {
        return Ok(());
    }
    let count = line[start..pos.col.min(line.len())].chars().count();
    let _ = ed.delete_chars(Pos::new(pos.line, start), count)?;
    ed.set_cursor(Pos::new(pos.line, start));
    engine.note_key(Key::Special("C-U".to_string()));
    Ok(())
}

/// `<C-r>{register}`: insert the contents of a register literally
/// (edit.c ins_reg()).
pub fn insert_register(ed: &mut Editor, engine: &mut NormalEngine, name: char) -> VimResult<()> {
    let reg = if name == '"' {
        ed.registers.unnamed().cloned()
    } else {
        ed.registers.get(name).cloned()
    };
    let Some(reg) = reg else {
        return Ok(());
    };
    let mut text = reg.text;
    if reg.reg_type == RegType::LineWise && !text.ends_with('\n') {
        text.push('\n');
    }
    for ch in text.chars() {
        type_char(ed, engine, ch)?;
    }
    Ok(())
}

/// `<C-a>`: insert the previously inserted text (edit.c ins_ctrl_a()).
pub fn insert_last_inserted(ed: &mut Editor, engine: &mut NormalEngine) -> VimResult<()> {
    let Some(reg) = ed.registers.get('.').cloned() else {
        return Ok(());
    };
    for ch in reg.text.chars() {
        type_char(ed, engine, ch)?;
    }
    Ok(())
}

/// `<C-t>`/`<C-d>`: shift the current line right/left by 'shiftwidth'
/// (edit.c ins_shift()).
pub fn shift_line(ed: &mut Editor, engine: &mut NormalEngine, right: bool) -> VimResult<()> {
    let line_no = ed.cursor().line;
    let text = ed.current_line();
    let (sw, expandtab) = {
        let sw = ed.options.num_or("shiftwidth", 8).max(0) as usize;
        let sw = if sw == 0 {
            ed.options.num_or("tabstop", 8).max(1) as usize
        } else {
            sw
        };
        (sw, ed.options.bool_or("expandtab", false))
    };
    let old_indent = indent_len(&text);
    let (new_text, new_indent) = if right {
        let pad = if expandtab {
            " ".repeat(sw)
        } else {
            "\t".to_string()
        };
        (format!("{pad}{text}"), old_indent + pad.len())
    } else {
        let mut rest = text.as_str();
        let mut removed = 0usize;
        while removed < sw {
            if let Some(tail) = rest.strip_prefix(' ') {
                rest = tail;
                removed += 1;
            } else if let Some(tail) = rest.strip_prefix('\t') {
                rest = tail;
                removed += 1;
            } else {
                break;
            }
        }
        (rest.to_string(), old_indent.saturating_sub(removed))
    };
    if new_text == text {
        return Ok(());
    }
    ed.set_line_text(line_no, new_text)?;
    let col = ed.cursor().col;
    let col = if new_indent >= old_indent {
        col + (new_indent - old_indent)
    } else {
        col.saturating_sub(old_indent - new_indent).max(new_indent)
    };
    let len = ed.current_line().len();
    ed.set_cursor(Pos::new(line_no, col.min(len)));
    let name = if right { "C-T" } else { "C-D" };
    engine.note_key(Key::Special(name.to_string()));
    Ok(())
}

/// Move the insert-mode cursor with an arrow/Home/End key.
pub fn move_cursor(ed: &mut Editor, name: &str) -> VimResult<()> {
    let pos = ed.cursor();
    let line = ed.current_line();
    let col = match name {
        "LEFT" => rvim_core::text::move_chars(&line, pos.col, -1),
        "RIGHT" => rvim_core::text::move_chars(&line, pos.col, 1),
        "HOME" => 0,
        "END" => line.len(),
        _ => pos.col,
    };
    let line_no = match name {
        "UP" => pos.line.saturating_sub(1),
        "DOWN" => (pos.line + 1).min(ed.line_count() - 1),
        _ => pos.line,
    };
    let col = col.min(ed.current_buffer().line(line_no)?.len_bytes());
    ed.set_cursor(Pos::new(line_no, col));
    Ok(())
}

/// Dispatch one insert-mode key (normal.c edit()).
///
/// Everything the session records goes through this function, so a counted
/// insert (`3ifoo<C-w>bar<Esc>`) and `.` replay the same keys vim keeps in its
/// redo buffer instead of copying the resulting text.
pub fn key(ed: &mut Editor, engine: &mut NormalEngine, key: Key) -> VimResult<bool> {
    // `<C-r>` is waiting for the register name.
    if engine.insert_pending_register {
        if let Key::Char(name) = key {
            insert_register(ed, engine, name)?;
            engine.insert_pending_register = false;
            engine.note_key(Key::Special("C-R".to_string()));
            engine.note_key(Key::Char(name));
            return Ok(true);
        }
        engine.insert_pending_register = false;
        return Ok(false);
    }
    match key {
        Key::Char(ch) => {
            type_char(ed, engine, ch)?;
            Ok(true)
        }
        Key::Special(name) => match name.as_str() {
            "CR" => {
                newline(ed, engine)?;
                Ok(true)
            }
            "TAB" => {
                type_char(ed, engine, '\t')?;
                Ok(true)
            }
            "BS" => {
                backspace(ed, engine)?;
                Ok(true)
            }
            "DELETE" => {
                let pos = ed.cursor();
                let _ = ed.delete_chars(pos, 1)?;
                engine.note_key(Key::Special("DELETE".to_string()));
                Ok(true)
            }
            "C-W" => {
                delete_word(ed, engine)?;
                Ok(true)
            }
            "C-U" => {
                delete_to_start(ed, engine)?;
                Ok(true)
            }
            "C-R" => {
                engine.insert_pending_register = true;
                Ok(true)
            }
            "C-A" => {
                insert_last_inserted(ed, engine)?;
                Ok(true)
            }
            "C-T" => {
                shift_line(ed, engine, true)?;
                Ok(true)
            }
            "C-D" => {
                shift_line(ed, engine, false)?;
                Ok(true)
            }
            "LEFT" | "RIGHT" | "UP" | "DOWN" | "HOME" | "END" => {
                move_cursor(ed, name.as_str())?;
                engine.note_key(Key::Special(name));
                Ok(true)
            }
            _ => Ok(false),
        },
        Key::Nop => Ok(false),
    }
}
