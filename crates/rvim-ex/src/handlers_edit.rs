//! Editing and buffer-family Ex commands added in M2:
//! `:g`/`:v`, `:sort`, `:join`, `:left`/`:right`/`:center`, `:>`/`:<`,
//! `:retab`, `:#`, `:&`/`:~`, `:args`/`:argadd`/`:argdelete`, and the
//! buffer-navigation family.

use std::path::PathBuf;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::Editor;

use crate::exec::{ExOutcome, resolve_range_public, run_line_with};
use crate::parser::ParsedEx;
use crate::script::ScriptState;
use crate::table::lookup;
use rvim_viml::Env;

// ------------------------------------------------------------- :g / :vglobal

/// Split `:global`'s argument into `(pattern, command)`.
fn split_global_args(args: &str) -> VimResult<(String, String)> {
    let trimmed = args.trim_start();
    let mut chars = trimmed.chars();
    let Some(delim) = chars.next() else {
        return Err(VimError::new(
            "E35",
            "No previous regular expression".to_string(),
        ));
    };
    if delim.is_alphanumeric() || delim == '"' || delim == '|' {
        return Err(VimError::new("E146", format!("Invalid argument: {args}")));
    }
    let rest = &trimmed[delim.len_utf8()..];
    let mut escaped = false;
    let mut end = None;
    for (i, c) in rest.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == delim {
            end = Some(i);
            break;
        }
    }
    let Some(end) = end else {
        // No closing delimiter: the rest is the pattern.
        return Ok((rest.to_string(), "print".to_string()));
    };
    let pattern = rest[..end].to_string();
    let command = rest[end + delim.len_utf8()..].trim().to_string();
    Ok((
        pattern,
        if command.is_empty() {
            "print".to_string()
        } else {
            command
        },
    ))
}

/// A buffer edit performed on one `:global` match, used to keep the remaining
/// tracked match positions pointing at the same logical lines.
///
/// Vim implements `:global` in two passes (ex_cmds.c:4907-4910): pass 1 marks
/// every matching line, pass 2 walks the marks in ascending order while
/// `mark_adjust()` makes them follow moved/inserted text. RustVim tracks the
/// positions explicitly and replays the same adjustment for the edits the
/// supported commands can perform.
enum LineShift {
    Delete(usize, usize),
    Move(usize, usize, usize),
    Copy(usize, usize, usize),
}

impl LineShift {
    fn map(&self, p: usize) -> Option<usize> {
        match self {
            LineShift::Delete(s, e) => {
                if p >= *s && p < *e {
                    None
                } else if p >= *e {
                    Some(p - (e - s))
                } else {
                    Some(p)
                }
            }
            LineShift::Move(s, e, at) => {
                let len = e - s;
                if *at <= *s {
                    if p >= *s && p < *e {
                        Some(at + (p - s))
                    } else if p >= *at && p < *s {
                        Some(p + len)
                    } else {
                        Some(p)
                    }
                } else if p >= *s && p < *e {
                    Some(p + at - e)
                } else if p >= *e && p < *at {
                    Some(p - len)
                } else {
                    Some(p)
                }
            }
            LineShift::Copy(s, e, at) => {
                if p >= *at {
                    Some(p + (e - s))
                } else {
                    Some(p)
                }
            }
        }
    }
}

/// Predict the line-count effect of one `:global` sub-command so the tracked
/// match positions can be updated after it runs.
fn plan_line_shift(ed: &Editor, command: &str) -> Option<LineShift> {
    let parsed = crate::parser::parse_line(command, ed.line_count()).ok()?;
    if parsed.command.is_empty() {
        return None;
    }
    let def = lookup(&parsed.command)?;
    let (s, e) = crate::exec::resolve_range_public(ed, &parsed.range, false).ok()?;
    match def.name {
        "delete" => Some(LineShift::Delete(s, e)),
        "move" => {
            let at = crate::exec::resolve_destination_public(ed, &parsed.args).ok()?;
            Some(LineShift::Move(s, e, at))
        }
        "copy" | "t" => {
            let at = crate::exec::resolve_destination_public(ed, &parsed.args).ok()?;
            Some(LineShift::Copy(s, e, at))
        }
        _ => None,
    }
}

pub fn cmd_global(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let inverted = p.command == "vglobal" || p.bang;
    let (pattern_arg, command) = split_global_args(&p.args)?;
    // An empty pattern reuses the last used search pattern
    // (search_regcomp RE_LAST, search.c:140-158).
    let pattern = if pattern_arg.is_empty() {
        ed.search_pattern.clone()
    } else {
        pattern_arg
    };
    if pattern.is_empty() {
        return Err(VimError::new(
            "E35",
            "No previous regular expression".to_string(),
        ));
    }
    // :global saves the pattern as BOTH the last search and last substitute
    // pattern (search_regcomp with RE_BOTH, search.c:174-183).
    ed.search_pattern = pattern.clone();
    if let Some(last) = ed.last_sub.as_mut() {
        last.pattern = pattern.clone();
    }
    let ignorecase = ed.options.bool_or("ignorecase", false);
    let (start, end) = crate::exec::resolve_range_public(ed, &p.range, true)?;
    let mut positions: Vec<usize> = Vec::new();
    for line in start..end {
        let text = ed.current_buffer().line_str(line)?.to_string();
        // The pattern is a Vim regexp (magic); the shared MVP engine covers
        // the subset used by the runtime files (rvim_viml::regex_mvp).
        let hit = rvim_viml::regex_mvp::is_match(&pattern, &text, ignorecase);
        if hit != inverted {
            positions.push(line);
        }
    }
    if positions.is_empty() {
        // vim uses smsg() here, not emsg(): no error, exit code unaffected
        // (ex_cmds.c:5026-5042).
        let text = if inverted {
            format!("Pattern found in every line: {pattern}")
        } else {
            format!("Pattern not found: {pattern}")
        };
        ed.messages.info(text);
        return Ok(ExOutcome::Continue);
    }
    // Pass 2: ascending over the marked lines; marks follow the text as the
    // command deletes/moves/copies lines (ex_cmds.c:5085 global_exe()).
    // A whole ":g" is one undo step (u_sync() runs per Ex command).
    ed.begin_undo_block();
    let mut outcome = ExOutcome::Continue;
    let mut failure = None;
    while !positions.is_empty() {
        let pos = positions.remove(0);
        let total = ed.line_count();
        if pos >= total {
            continue;
        }
        ed.set_cursor(Pos::new(pos, 0));
        let plan = plan_line_shift(ed, &command);
        match run_line_with(ed, env, state, &command) {
            Ok(o) => outcome = o,
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
        if !matches!(outcome, ExOutcome::Continue) {
            break;
        }
        if let Some(plan) = plan {
            let total = ed.line_count();
            let mut next: Vec<usize> = positions
                .iter()
                .filter_map(|&p| plan.map(p))
                .filter(|&p| p < total)
                .collect();
            next.sort_unstable();
            next.dedup();
            positions = next;
        }
    }
    ed.end_undo_block();
    match failure {
        Some(e) => Err(e),
        None => Ok(outcome),
    }
}

// ------------------------------------------------------------------ :sort

/// Which numeric format the `n`/`b`/`o`/`x` flags select (ex_cmds.c:406-487).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SortFormat {
    None,
    Decimal,
    Binary,
    Octal,
    Hex,
    Float,
}

/// Parse the first number in `line` according to `format`; `None` when the line
/// has no number (such lines sort before every number, ex_cmds.c:524-559).
fn sort_number(line: &str, format: SortFormat) -> Option<i64> {
    match format {
        SortFormat::Float => {
            let text = line.trim_start();
            let text = text.strip_prefix('+').unwrap_or(text);
            let end = text
                .char_indices()
                .take_while(|(_, c)| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '-' | '+'))
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            if end == 0 {
                return None;
            }
            text[..end].parse::<f64>().ok().map(|f| f as i64)
        }
        SortFormat::None | SortFormat::Decimal => {
            let digit_at = line.find(|c: char| c.is_ascii_digit());
            let start = digit_at?;
            let start = if start > 0 && line.as_bytes()[start - 1] == b'-' {
                start - 1
            } else {
                start
            };
            let end = line[start..]
                .char_indices()
                .take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && *c == '-'))
                .last()
                .map(|(i, c)| start + i + c.len_utf8())
                .unwrap_or(start);
            line[start..end].parse::<i64>().ok()
        }
        SortFormat::Hex | SortFormat::Binary | SortFormat::Octal => {
            let pred = |c: char| match format {
                SortFormat::Hex => c.is_ascii_hexdigit(),
                SortFormat::Binary => c == '0' || c == '1',
                _ => ('0'..='7').contains(&c),
            };
            let (start, _) = line.char_indices().find(|(_, c)| pred(*c))?;
            let start = if start > 0 && line.as_bytes()[start - 1] == b'-' {
                start - 1
            } else {
                start
            };
            let mut digits = &line[start..];
            let mut negative = false;
            if let Some(rest) = digits.strip_prefix('-') {
                negative = true;
                digits = rest;
            }
            // Vim's STR2NR_HEX/STR2NR_BIN also accept an explicit 0x/0b prefix
            // (ex_cmds.c:539-558 + vim_str2nr).
            let (body, radix) = if format == SortFormat::Hex
                && (digits.starts_with("0x") || digits.starts_with("0X"))
            {
                (&digits[2..], 16u32)
            } else if format == SortFormat::Binary
                && (digits.starts_with("0b") || digits.starts_with("0B"))
            {
                (&digits[2..], 2u32)
            } else {
                (
                    digits,
                    match format {
                        SortFormat::Hex => 16,
                        SortFormat::Binary => 2,
                        _ => 8,
                    },
                )
            };
            let end = body
                .char_indices()
                .take_while(|(_, c)| pred(*c))
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            if end == 0 {
                // A bare 0x prefix is the number zero.
                return Some(0);
            }
            let value = i64::from_str_radix(&body[..end], radix).ok()?;
            Some(if negative { -value } else { value })
        }
    }
}

pub fn cmd_sort(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let (start, end) = resolve_range_public(ed, &p.range, true)?;
    let mut format = SortFormat::None;
    let mut formats_found = 0usize;
    let mut ignore_case = false;
    let mut unique = false;
    let mut sort_rx = false;
    let mut search_pat: Option<String> = None;
    let mut args: &str = p.args.trim();
    // Parse flags; the first non-alphanumeric character starts an explicit sort
    // regex ("/pat/"), see ex_cmds.c:406-480.
    'flags: while let Some(c) = args.chars().next() {
        if c.is_whitespace() {
            args = args.trim_start();
            continue;
        }
        match c {
            'i' | 'l' => ignore_case = true,
            'n' => {
                format = SortFormat::Decimal;
                formats_found += 1;
            }
            'f' => {
                format = SortFormat::Float;
                formats_found += 1;
            }
            'b' => {
                format = SortFormat::Binary;
                formats_found += 1;
            }
            'o' => {
                format = SortFormat::Octal;
                formats_found += 1;
            }
            'x' => {
                format = SortFormat::Hex;
                formats_found += 1;
            }
            'r' => sort_rx = true,
            'u' => unique = true,
            _ if !c.is_ascii_alphanumeric() && c != '"' => {
                // Delimiter for the sort regexp; a bare "/" uses the last
                // search pattern (ex_cmds.c:458-469).
                let delim = c;
                let rest = &args[delim.len_utf8()..];
                let mut escaped = false;
                let mut close = None;
                for (i, ch) in rest.char_indices() {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == delim {
                        close = Some(i);
                        break;
                    }
                }
                let pat = match close {
                    Some(i) => rest[..i].to_string(),
                    None => rest.to_string(),
                };
                search_pat = Some(if pat.is_empty() {
                    let last = ed.search_pattern.clone();
                    if last.is_empty() {
                        return Err(VimError::new(
                            "E35",
                            "No previous regular expression".to_string(),
                        ));
                    }
                    last
                } else {
                    pat
                });
                break 'flags;
            }
            _ => {
                return Err(VimError::new("E474", format!("Invalid argument: {args}")));
            }
        }
        args = &args[c.len_utf8()..];
    }
    // "Can only have one of 'n', 'b', 'o' and 'x'" (ex_cmds.c:482-487).
    if formats_found > 1 {
        return Err(VimError::new("E474", "Invalid argument".to_string()));
    }
    let reverse = p.bang; // ":sort!" sorts in reverse (ex_cmds.c:611)
    if start >= end {
        return Ok(ExOutcome::Continue);
    }
    let ignorecase = ignore_case || ed.options.bool_or("ignorecase", false);
    let key = |line: &str| -> String {
        if format != SortFormat::None {
            return String::new();
        }
        match &search_pat {
            Some(pat) => match rvim_viml::regex_mvp::find(pat, line, ignorecase) {
                // 'r' sorts on the matched text, otherwise on the text after
                // the match (ex_cmds.c:510-518).
                Some((s, e)) => {
                    if sort_rx {
                        line[s..e].to_string()
                    } else {
                        line[e..].to_string()
                    }
                }
                None => String::new(),
            },
            None => {
                if ignorecase {
                    line.to_ascii_lowercase()
                } else {
                    line.to_string()
                }
            }
        }
    };
    let mut lines: Vec<String> = ed.current_buffer().lines()[start..end]
        .iter()
        .map(|l| l.as_str().to_string())
        .collect();
    let original = lines.clone();
    // Vim's sort is stable: equal keys keep the original line order
    // (ex_cmds.c:357-359).
    lines.sort_by(|a, b| {
        if format != SortFormat::None {
            let na = sort_number(a, format);
            let nb = sort_number(b, format);
            return match (na, nb) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => x.cmp(&y),
            };
        }
        key(a).cmp(&key(b))
    });
    // Unique removes adjacent duplicates of the *whole line* (ex_cmds.c:619).
    if unique {
        let mut out: Vec<String> = Vec::new();
        for line in lines {
            let same = out.last().is_some_and(|last: &String| {
                if ignore_case {
                    last.eq_ignore_ascii_case(&line)
                } else {
                    *last == line
                }
            });
            if !same {
                out.push(line);
            }
        }
        lines = out;
    }
    if reverse {
        lines.reverse();
    }
    // An already-sorted range is left untouched (Vim only calls u_save() when
    // a line actually moves, so 'modified' stays off for a no-op sort).
    if lines != original {
        ed.replace_lines(start, end, lines)?;
    }
    // Vim leaves the cursor on the first line of the sorted range
    // (ex_cmds.c:604-611 ex_sort()).
    ed.set_cursor(Pos::new(start.min(ed.line_count().saturating_sub(1)), 0));
    Ok(ExOutcome::Continue)
}

// ------------------------------------------------------------------ :join

pub fn cmd_join(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let (start, end) = resolve_range_public(ed, &p.range, false)?;
    let total = ed.line_count();
    // A trailing count is the number of lines to join starting at the last
    // addressed line (EX_COUNT, ex_docmd.c:2354-2384).
    let count: Option<usize> = if p.args.trim().is_empty() {
        None
    } else {
        Some(
            p.args
                .trim()
                .parse::<usize>()
                .map_err(|_| VimError::invalid_argument(&p.args))?,
        )
    };
    let line1 = start;
    let line2 = match count {
        Some(n) => (line1 + n.saturating_sub(1)).min(total.saturating_sub(1)),
        None => {
            if p.addr_count >= 2 {
                // Explicit range: join exactly the addressed lines; ":2,2j"
                // does nothing (ex_docmd.c:8074-8091).
                end - 1
            } else if line1 + 1 >= total {
                // ":j" on the last line just beeps.
                return Ok(ExOutcome::Continue);
            } else {
                line1 + 1
            }
        }
    };
    if line2 <= line1 {
        return Ok(ExOutcome::Continue);
    }
    let mut joined = String::new();
    for line in line1..=line2 {
        let text = ed.current_buffer().line_str(line)?.to_string();
        let text = if line == line1 {
            text
        } else {
            text.trim_start().to_string()
        };
        if line > line1 {
            if p.bang || joined.trim_end().ends_with(' ') || text.is_empty() {
                joined.push_str(&text);
            } else {
                joined.push(' ');
                joined.push_str(&text);
            }
        } else {
            joined.push_str(&text);
        }
    }
    ed.replace_lines(line1, line2 + 1, vec![joined])?;
    ed.set_cursor(Pos::new(line1, 0));
    Ok(ExOutcome::Continue)
}

// ---------------------------------------------------- :left / :right / :center

fn align_width(args: &str) -> isize {
    let trimmed = args.trim();
    match trimmed.split_whitespace().next() {
        Some(w) => w.parse::<isize>().unwrap_or(0),
        None => 0,
    }
}

/// Render `cols` columns of indent the way vim's `set_indent()` does: tabs to
/// the next tab stop when 'expandtab' is off, plain spaces when it is on.
fn indent_prefix(cols: usize, expandtab: bool, tabstop: usize) -> String {
    if expandtab {
        return " ".repeat(cols);
    }
    let ts = tabstop.max(1);
    let mut out = String::new();
    out.push_str(&"\t".repeat(cols / ts));
    out.push_str(&" ".repeat(cols % ts));
    out
}

/// Screen width of `text` starting at column 0 (tabs advance to tab stops).
fn display_width(text: &str, tabstop: usize) -> usize {
    let ts = tabstop.max(1);
    let mut col = 0usize;
    for c in text.chars() {
        match c {
            '\t' => col += ts - (col % ts),
            _ => col += 1,
        }
    }
    col
}

pub fn cmd_align(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let (start, end) = resolve_range_public(ed, &p.range, false)?;
    let mut width = align_width(&p.args);
    let expandtab = ed.options.bool_or("expandtab", false);
    let tabstop = ed.options.num_or("tabstop", 8).max(1) as usize;
    if p.command != "left" && width <= 0 {
        // ex_cmds.c:165-176: fall back to 'textwidth', then 80.
        width = ed.options.num_or("textwidth", 0) as isize;
        if width <= 0 {
            width = 80;
        }
    }
    // One :left/:right/:center is one undo step.
    ed.begin_undo_block();
    for line in start..end {
        let text = ed.current_buffer().line_str(line)?.to_string();
        // len = linelen() - get_indent(): the width of the text without its
        // leading indent, trailing whitespace ignored (ex_cmds.c:187-196).
        let body_start = text
            .char_indices()
            .find(|(_, c)| !matches!(c, ' ' | '\t'))
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        let body = &text[body_start..];
        let visible_body = body.trim_end();
        let len = display_width(visible_body, tabstop);
        if visible_body.is_empty() && p.command != "left" {
            // Blank lines are skipped by :right/:center.
            continue;
        }
        let new_indent = if p.command == "left" {
            width.max(0) as usize
        } else if p.command == "center" {
            (width - len as isize).max(0) as usize / 2
        } else {
            (width - len as isize).max(0) as usize
        };
        let new_text = format!("{}{}", indent_prefix(new_indent, expandtab, tabstop), body);
        if new_text != text {
            if let Err(e) = ed.set_line_text(line, new_text) {
                ed.end_undo_block();
                return Err(e);
            }
        }
    }
    ed.end_undo_block();
    // Vim's ex_align() always calls u_save() for the range.
    ed.set_modified(true);
    Ok(ExOutcome::Continue)
}

// ------------------------------------------------------------ :> and :<

pub fn cmd_shift(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let (s, e) = resolve_range_public(ed, &p.range, false)?;
    let sw = ed.options.num_or("shiftwidth", 8).max(1) as usize;
    let cmd_char = if p.command == ">" { '>' } else { '<' };
    // ex_docmd.c: each extra '>' / '<' after the command adds one to the shift
    // amount; the following digits are a COUNT OF LINES, not columns.
    let mut amount = 1usize;
    let mut rest = p.args.trim();
    while rest.starts_with(cmd_char) {
        amount += 1;
        rest = rest[cmd_char.len_utf8()..].trim_start();
    }
    let line_count = rest.parse::<usize>().ok().filter(|n| *n > 0);
    let total = ed.line_count();
    let (start, end) = match line_count {
        // ":{range}>N" starts at the last line of the range and covers N lines.
        Some(n) => ((e - 1).min(total.saturating_sub(1)), (e + n - 1).min(total)),
        None => (s, e),
    };
    let indent = sw * amount;
    let expandtab = ed.options.bool_or("expandtab", false);
    let ts = ed.options.num_or("tabstop", 8).max(1) as usize;
    let right = p.command == ">";
    // One shift command is one undo step even across many lines.
    ed.begin_undo_block();
    let mut failure = None;
    for line in start..end {
        let text = match ed.current_buffer().line_str(line) {
            Ok(text) => text.to_string(),
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        let new_text = if right {
            shift_right(&text, indent, expandtab, ts)
        } else {
            shift_left(&text, indent, ts)
        };
        if new_text != text {
            if let Err(e) = ed.set_line_text(line, new_text) {
                failure = Some(e);
                break;
            }
        }
    }
    ed.end_undo_block();
    if let Some(e) = failure {
        return Err(e);
    }
    // Vim's ex_shift() always calls u_save() for the range, so even a no-op
    // shift marks the buffer modified.
    ed.set_modified(true);
    // Vim leaves the cursor on the last shifted line, on its first non-blank
    // (ex_docmd.c ex_shift() -> beginline(BL_SOL|BL_FIX)).
    if end > start {
        let line = (end - 1).min(ed.line_count().saturating_sub(1));
        let text = ed.current_buffer().line_str(line)?.to_string();
        let col = first_nonblank_byte(&text);
        ed.set_cursor(Pos::new(line, col));
    }
    Ok(ExOutcome::Continue)
}

/// Byte offset of the first non-blank character (Vim's beginline(BL_WHITE)).
fn first_nonblank_byte(text: &str) -> usize {
    text.char_indices()
        .find(|(_, c)| !matches!(c, ' ' | '\t'))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Add `amount` columns of indent, using tabs when 'expandtab' is off.
fn shift_right(text: &str, amount: usize, expandtab: bool, tabstop: usize) -> String {
    if expandtab {
        return format!("{}{}", " ".repeat(amount), text);
    }
    // Prefer tabs when the added indent lands on tab stops.
    let mut tabs = amount / tabstop;
    let spaces = amount % tabstop;
    let mut prefix = String::new();
    if text.starts_with(' ') {
        // Convert leading spaces into tabs where possible.
        let mut spaces_total = text.chars().take_while(|c| *c == ' ').count();
        spaces_total += spaces;
        tabs += spaces_total / tabstop;
        let rest = &text[spaces_total % tabstop..];
        for _ in 0..tabs {
            prefix.push('\t');
        }
        prefix.push_str(&" ".repeat(spaces_total % tabstop));
        return format!("{prefix}{rest}");
    }
    for _ in 0..tabs {
        prefix.push('\t');
    }
    prefix.push_str(&" ".repeat(spaces));
    format!("{prefix}{text}")
}

/// Remove up to `amount` columns of leading indent.
fn shift_left(text: &str, amount: usize, tabstop: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut col = 0usize;
    let mut i = 0usize;
    while i < chars.len() && col < amount {
        match chars[i] {
            ' ' => col += 1,
            '\t' => col += tabstop - (col % tabstop),
            _ => break,
        }
        i += 1;
    }
    chars[i..].iter().collect()
}

// ------------------------------------------------------------------ :retab

pub fn cmd_retab(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let (start, end) = resolve_range_public(ed, &p.range, true)?;
    let expandtab = ed.options.bool_or("expandtab", false);
    let old_ts = ed.options.num_or("tabstop", 8).max(1) as usize;
    // ex_retab(): a leading number is the new 'tabstop'; 0 (or no number) means
    // "keep the current 'tabstop'" (indent.c:1681-1694).
    let mut new_ts = old_ts;
    if let Some(arg) = p.args.split_whitespace().next() {
        let digits: String = arg.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let value = digits
                .parse::<usize>()
                .map_err(|_| VimError::invalid_argument(arg))?;
            // "0" means "keep the current 'tabstop'" (indent.c:1693-1694).
            if value > 0 {
                new_ts = value;
            }
        }
    }
    let forceit = p.bang;
    // One ":retab" is one undo step.
    ed.begin_undo_block();
    let mut failure = None;
    for line in start..end {
        let text = match ed.current_buffer().line_str(line) {
            Ok(text) => text.to_string(),
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        let new_text = retab_line(&text, old_ts, new_ts, expandtab, forceit);
        if new_text != text {
            if let Err(e) = ed.set_line_text(line, new_text) {
                failure = Some(e);
                break;
            }
        }
    }
    ed.end_undo_block();
    if let Some(e) = failure {
        return Err(e);
    }
    // The requested tabstop becomes the buffer's 'tabstop' (indent.c:1857).
    if new_ts != old_ts {
        ed.options
            .set("tabstop", rvim_editor::OptValue::Num(new_ts as i64))?;
    }
    Ok(ExOutcome::Continue)
}

/// Retabulate the white-space runs of one line (indent.c:1696-1804).
///
/// Without `!` only runs that actually contain a TAB are rewritten, so
/// space-only indentation is never converted to tabs (indent.c:1719). Virtual
/// columns are measured with the *old* 'tabstop'; the replacement uses the new
/// one. When 'expandtab' is set, tabs become spaces of the same width.
fn retab_line(text: &str, old_ts: usize, new_ts: usize, expandtab: bool, forceit: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    let mut vcol = 0usize;
    while i < chars.len() {
        if chars[i] != ' ' && chars[i] != '\t' {
            out.push(chars[i]);
            vcol += 1;
            i += 1;
            continue;
        }
        let start_col = i;
        let start_vcol = vcol;
        let mut got_tab = false;
        let mut num_spaces = 0usize;
        while i < chars.len() {
            match chars[i] {
                ' ' => {
                    num_spaces += 1;
                    vcol += 1;
                }
                '\t' => {
                    got_tab = true;
                    vcol += old_ts - (vcol % old_ts);
                }
                _ => break,
            }
            i += 1;
        }
        let len = vcol - start_vcol;
        if got_tab || (forceit && num_spaces > 1) {
            let (num_tabs, spaces) = if expandtab {
                (0usize, len)
            } else {
                let mut spaces = len;
                let mut tabs = 0usize;
                let temp = new_ts - (start_vcol % new_ts);
                if spaces >= temp {
                    spaces -= temp;
                    tabs += 1;
                }
                tabs += spaces / new_ts;
                spaces -= (spaces / new_ts) * new_ts;
                (tabs, spaces)
            };
            if expandtab || got_tab || (num_tabs + spaces < len) {
                out.push_str(&"\t".repeat(num_tabs));
                out.push_str(&" ".repeat(spaces));
                continue;
            }
        }
        // Keep the original run untouched.
        out.extend(chars[start_col..i].iter());
    }
    out
}

// -------------------------------------------------------- :# / :& / :~

pub fn cmd_hash_print(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    // ":#" is ":number" with the current line as default range.
    let (start, end) = resolve_range_public(ed, &p.range, false)?;
    for i in start..end {
        let text = ed.current_buffer().line_str(i)?.to_string();
        ed.messages.info(format!("{:>6}\t{}", i + 1, text));
    }
    Ok(ExOutcome::Continue)
}

/// `:&` and `:~`: repeat the last `:substitute`.
///
/// `:&` reuses the last substitute pattern; `:~` reuses the last *used* search
/// pattern (ex_cmds.c:3715-3718: `which_pat = RE_LAST` for `CMD_tilde`,
/// `RE_SUBST` otherwise). Both reuse the last replacement string.
pub fn cmd_repeat_substitute(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let Some(last) = ed.last_sub.clone() else {
        return Err(VimError::new(
            "E33",
            "No previous substitute regular expression".to_string(),
        ));
    };
    let pattern = if p.command == "~" {
        let pat = ed.search_pattern.clone();
        if pat.is_empty() {
            return Err(VimError::new(
                "E35",
                "No previous regular expression".to_string(),
            ));
        }
        pat
    } else {
        last.pattern.clone()
    };
    let args = p.args.trim();
    // ":&&" keeps the previous flags; a plain ":&"/":~" uses the defaults
    // (ex_cmds.c:3852-3882).
    let keep_flags = args.starts_with('&');
    let mut global = if keep_flags { last.global } else { false };
    let mut ignorecase = None;
    let mut no_error = false;
    let flags: &str = if keep_flags {
        args[1..].trim_start()
    } else {
        args
    };
    for c in flags.chars() {
        match c {
            'g' => global = !global,
            'i' => ignorecase = Some(true),
            'I' => ignorecase = Some(false),
            'e' => no_error = true,
            '"' => break,
            _ => {}
        }
    }
    let (start, end) = resolve_range_public(ed, &p.range, false)?;
    ed.search_pattern = pattern.clone();
    ed.last_sub = Some(rvim_editor::LastSubstitute {
        pattern: pattern.clone(),
        replacement: last.replacement.clone(),
        global,
        count: 0,
    });
    let matched = crate::exec::substitute_lines_public(
        ed,
        start,
        end,
        &pattern,
        &last.replacement,
        global,
        ignorecase,
    )?;
    if matched == 0 && !no_error {
        return Err(VimError::pattern_not_found(&pattern));
    }
    Ok(ExOutcome::Continue)
}

// --------------------------------------------------------- :args and friends

pub fn cmd_args(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    if !p.args.trim().is_empty() {
        // ":args file1 file2" replaces the argument list.
        ed.arglist = p.args.split_whitespace().map(PathBuf::from).collect();
        ed.arg_index = 0;
        return Ok(ExOutcome::Continue);
    }
    let mut line = String::new();
    for (i, arg) in ed.arglist.iter().enumerate() {
        let mark = if i == ed.arg_index { "[" } else { "" };
        let close = if i == ed.arg_index { "]" } else { "" };
        if i == 0 {
            line.push_str(&format!("{mark}{}{close}", arg.display()));
        } else {
            line.push_str(&format!(" {mark}{}{close}", arg.display()));
        }
    }
    ed.messages.info(line);
    Ok(ExOutcome::Continue)
}

pub fn cmd_argadd(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    for arg in p.args.split_whitespace() {
        ed.arglist.push(PathBuf::from(arg));
    }
    Ok(ExOutcome::Continue)
}

pub fn cmd_argdelete(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let args = p.args.trim();
    if args.is_empty() {
        ed.arglist.clear();
        ed.arg_index = 0;
        return Ok(ExOutcome::Continue);
    }
    let indices: Vec<usize> = args
        .split_whitespace()
        .filter_map(|a| a.parse::<usize>().ok())
        .collect();
    let mut new_args = Vec::new();
    for (i, arg) in ed.arglist.iter().enumerate() {
        // Argument numbers shown by :args are 1-based.
        if !indices.contains(&(i + 1)) {
            new_args.push(arg.clone());
        }
    }
    ed.arglist = new_args;
    ed.arg_index = ed.arg_index.min(ed.arglist.len().saturating_sub(1));
    Ok(ExOutcome::Continue)
}

pub fn cmd_badd(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let path = p.args.trim();
    if path.is_empty() {
        return Err(VimError::range_required());
    }
    let id = ed.new_buffer();
    // Buffer::open would switch to it; open the file into the new buffer by
    // reusing :edit semantics without switching.
    let name = PathBuf::from(path);
    let buf = rvim_buffer::Buffer::open(id, &name)?;
    ed.replace_buffer(id, buf)?;
    Ok(ExOutcome::Continue)
}

/// `:bfirst`, `:blast` — jump to the first/last buffer.
pub fn cmd_buffer_edge(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let all = ed.all_buffers();
    let id = if p.command == "bfirst" {
        all.first().map(|b| b.id)
    } else {
        all.last().map(|b| b.id)
    };
    match id {
        Some(id) => {
            ed.switch_buffer(id)?;
            Ok(ExOutcome::Continue)
        }
        None => Err(VimError::new("E86", "Buffer not found".to_string())),
    }
}

/// `:bunload` / `:bwipeout` — remove a buffer from the pool.
pub fn cmd_bunload(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    if ed.buffer_count() <= 1 {
        return Err(VimError::new(
            "E937",
            "Cannot unload last buffer".to_string(),
        ));
    }
    let id = p
        .args
        .trim()
        .parse::<i32>()
        .unwrap_or_else(|_| ed.current_bufnr());
    if !ed.all_buffers().iter().any(|b| b.id == id) {
        return Err(VimError::new("E86", format!("Buffer not found: {id}")));
    }
    if id == ed.current_bufnr() {
        let fallback = ed
            .all_buffers()
            .iter()
            .find(|b| b.id != id)
            .map(|b| b.id)
            .unwrap();
        ed.switch_buffer(fallback)?;
    }
    ed.drop_buffer(id);
    Ok(ExOutcome::Continue)
}

/// `:bmodified` — go to the next modified buffer.
pub fn cmd_bmodified(ed: &mut Editor, _p: &ParsedEx) -> VimResult<ExOutcome> {
    let ids: Vec<i32> = ed
        .all_buffers()
        .iter()
        .filter(|b| b.is_changed())
        .map(|b| b.id)
        .collect();
    if ids.is_empty() {
        return Err(VimError::new("E84", "No modified buffer found".to_string()));
    }
    let current = ed.current_bufnr();
    let next = ids
        .iter()
        .find(|id| **id > current)
        .or_else(|| ids.first())
        .copied()
        .unwrap();
    ed.switch_buffer(next)?;
    Ok(ExOutcome::Continue)
}

// --------------------------------------------------- write-all family

pub fn cmd_update(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    if !ed.current_buffer().is_changed() {
        return Ok(ExOutcome::Continue);
    }
    crate::exec::write_current(ed, env, state, p)
}

pub fn cmd_saveas(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let path = p.args.trim();
    if path.is_empty() {
        return Err(VimError::range_required());
    }
    let target = PathBuf::from(path);
    crate::autodispatch::write_file(ed, env, state, Some(&target), p.bang)?;
    Ok(ExOutcome::Continue)
}

pub fn cmd_wqall(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let ids: Vec<i32> = ed
        .all_buffers()
        .iter()
        .filter(|b| b.is_changed())
        .map(|b| b.id)
        .collect();
    for id in ids {
        crate::autodispatch::switch_buffer(ed, env, state, id)?;
        if ed.current_buffer().name.is_some() {
            crate::autodispatch::write_file(ed, env, state, None, p.bang)?;
        }
    }
    Ok(ExOutcome::QuitAll {
        bang: p.bang,
        exit_code: 0,
    })
}

pub fn cmd_stop(_ed: &mut Editor, _p: &ParsedEx) -> VimResult<ExOutcome> {
    // No job control in headless batch mode; vim also suspends only in a TUI.
    Ok(ExOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::run_line;

    fn editor(lines: &[&str]) -> Editor {
        let mut ed = Editor::new();
        let text: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        ed.replace_lines(0, 1, text).unwrap();
        ed
    }

    fn text(ed: &Editor) -> String {
        ed.current_buffer().full_text()
    }

    #[test]
    fn global_move_to_top_reverses_lines() {
        // ex_cmds.c:4907-4910/5085: marks are processed in ascending order and
        // follow moved lines, so :g/x/m0 reverses them.
        let mut ed = editor(&["x1", "x2", "x3", "x4"]);
        run_line(&mut ed, "g/x/m0").unwrap();
        assert_eq!(text(&ed), "x4\nx3\nx2\nx1");
    }

    #[test]
    fn global_copy_to_top_keeps_matching_lines_tracked() {
        let mut ed = editor(&["x1", "x2", "x3", "x4"]);
        run_line(&mut ed, "g/x/t0").unwrap();
        assert_eq!(text(&ed), "x4\nx3\nx2\nx1\nx1\nx2\nx3\nx4");
    }

    #[test]
    fn global_copy_dot_and_delete() {
        // ":g/t/t." copies each matching line below itself.
        let mut ed = editor(&["one", "two", "three", "four"]);
        run_line(&mut ed, "g/t/t.").unwrap();
        assert_eq!(text(&ed), "one\ntwo\ntwo\nthree\nthree\nfour");

        let mut ed = editor(&["x1", "x2", "x3", "x4"]);
        run_line(&mut ed, "g/x2/d").unwrap();
        assert_eq!(text(&ed), "x1\nx3\nx4");

        let mut ed = editor(&["x1", "x2", "x3", "x4"]);
        run_line(&mut ed, "g/x/s/x/Y/").unwrap();
        assert_eq!(text(&ed), "Y1\nY2\nY3\nY4");

        let mut ed = editor(&["x1", "x2", "x3"]);
        run_line(&mut ed, "v/x2/d").unwrap();
        assert_eq!(text(&ed), "x2");
    }

    #[test]
    fn global_without_match_is_a_message_not_an_error() {
        // smsg(), not E486/emsg (ex_cmds.c:5026-5042).
        let mut ed = editor(&["x1", "x2"]);
        run_line(&mut ed, "g/zzz/d").unwrap();
        assert_eq!(ed.error_count, 0);
        assert_eq!(text(&ed), "x1\nx2");
        assert!(
            ed.messages
                .history()
                .iter()
                .any(|m| m.contains("Pattern not found: zzz"))
        );
    }

    #[test]
    fn sort_reverse_numeric_and_hex_flags() {
        // "!" reverses (ex_cmds.c:611), n/b/o/x/f select the number format.
        let mut ed = editor(&["banana", "Apple", "cherry", "apple", "10", "2", "1"]);
        run_line(&mut ed, "sort!").unwrap();
        assert_eq!(text(&ed), "cherry\nbanana\napple\nApple\n2\n10\n1");

        let mut ed = editor(&["10", "9", "100"]);
        run_line(&mut ed, "sort n").unwrap();
        assert_eq!(text(&ed), "9\n10\n100");

        let mut ed = editor(&["banana", "Apple", "cherry", "apple", "10", "2", "1"]);
        run_line(&mut ed, "sort x").unwrap();
        assert_eq!(text(&ed), "1\n2\nApple\napple\ncherry\n10\nbanana");

        // STR2NR_HEX accepts an explicit 0x prefix.
        let mut ed = editor(&["0x10", "2", "0xf", "10"]);
        run_line(&mut ed, "sort x").unwrap();
        assert_eq!(text(&ed), "2\n0xf\n0x10\n10");
    }

    #[test]
    fn align_left_sets_the_requested_indent() {
        // ":left N" indents to column N (ex_cmds.c:156-185).
        let mut ed = editor(&["  alpha", "    beta", "gamma"]);
        run_line(&mut ed, "1,3left 4").unwrap();
        assert_eq!(text(&ed), "    alpha\n    beta\n    gamma");
        run_line(&mut ed, "1,3left").unwrap();
        assert_eq!(text(&ed), "alpha\nbeta\ngamma");
    }

    #[test]
    fn align_right_and_center_use_text_width() {
        let mut ed = editor(&["  alpha", "    beta", "gamma"]);
        run_line(&mut ed, "1,3right 10").unwrap();
        assert_eq!(text(&ed), "     alpha\n      beta\n     gamma");
        let mut ed = editor(&["  alpha", "    beta", "gamma"]);
        run_line(&mut ed, "1,3center 10").unwrap();
        assert_eq!(text(&ed), "  alpha\n   beta\n  gamma");
    }

    #[test]
    fn retab_without_bang_leaves_space_indentation_alone() {
        let mut ed = editor(&["        alpha", "    beta", "\tDelta"]);
        run_line(&mut ed, "retab").unwrap();
        assert_eq!(text(&ed), "        alpha\n    beta\n\tDelta");
        run_line(&mut ed, "retab!").unwrap();
        assert_eq!(text(&ed), "\talpha\n    beta\n\tDelta");
    }

    #[test]
    fn retab_zero_uses_the_current_tabstop() {
        // ":retab 0" must not set 'tabstop' to 0 (indent.c:1693-1694).
        let mut ed = editor(&["\tDelta"]);
        run_line(&mut ed, "set tabstop=4").unwrap();
        run_line(&mut ed, "retab 0").unwrap();
        assert_eq!(ed.options.num_or("tabstop", 0), 4);
        assert_eq!(text(&ed), "\tDelta");
    }
}
