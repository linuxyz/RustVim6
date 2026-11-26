//! :tag, :tselect, :pop and the rest of the tag command family
//! (reference: tag.c).
//!
//! # Model
//!
//! The tag stack and the match list follow Vim's model:
//!
//! * jumping with `:tag`/`:tjump`/CTRL-] pushes one entry that remembers the
//!   position we came from (`from`) and the match number the jump used
//!   (`matchnr`).  Entries above the current index are dropped first, and the
//!   stack is capped at Vim's TAGSTACKSIZE (100) by dropping the oldest entry.
//!   `tag_stack_idx` is the Vim index: it points just *past* the entry that
//!   describes the current position, so `gettagstack()` reports
//!   `curidx = tag_stack_idx + 1` (length + 1 after a jump), exactly like the
//!   reference (`do_tag()` increments `w_tagstackidx` after pushing).
//! * `:pop` walks back: it restores buffer and cursor from the entry the index
//!   moves to and then moves the index down.  `E73` is reported when there is
//!   nothing left to pop.
//! * `:tselect` lists the matches in Vim's "  # pri kind tag ... file" format
//!   and remembers them; `:tnext`/`:tprevious`/`:tfirst`/`:tlast`/`:trewind`
//!   cycle through that remembered list, wrapping at both ends.  The list lives
//!   in a thread-local cache because `ScriptState` belongs to another module;
//!   it is re-derived from the tag stack when the cache does not match the
//!   tag the current stack entry names.
//!
//! # Honest limitations
//!
//! * `:tselect`/`:tjump` never prompt: the list is printed and the first match
//!   is not jumped to.  `:tjump` jumps when exactly one tag matches.
//! * `:ptag` and `:stag` behave like `:tag` (no preview window, no split);
//!   `:ltag` restricts the search to tags pointing into the current buffer and
//!   falls back to a normal `:tag` jump when there is none (RustVim has no
//!   location list for tags yet).  `:stselect`, `:dlist` act like `:tselect`
//!   and `:djump` like `:tjump`.
//! * the tag command field is handled as Vim does for the common ctags shapes:
//!   a leading line number, or a `/pat/`/`?pat?` address.  A field that is not a
//!   number is treated as a search pattern (as the task requires) instead of
//!   being run as an arbitrary Ex command, so `execute`-style tag commands out
//!   of a hand-written tags file are not supported.
//! * `:tags` marks the top of the stack: because the index is one past the
//!   last entry, the ">" is printed on the last entry instead of on a trailing
//!   empty line (cosmetic deviation from `do_tags()`).

use std::cell::RefCell;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::Editor;
use rvim_editor::tags::TagEntry;
use rvim_viml::Env;

use crate::exec::ExOutcome;
use crate::host_win::TagStackEntry;
use crate::parser::{ParsedEx, Range};
use crate::script::ScriptState;

/// Vim's TAGSTACKSIZE: the tag stack keeps at most this many entries.
const TAGSTACKSIZE: usize = 100;

/// Matches remembered for :tnext/:tprevious/:tfirst/:tlast/:trewind.
#[derive(Clone, Debug, Default)]
struct MatchCache {
    name: String,
    matches: Vec<TagEntry>,
    /// Index into `matches` the current position came from; `None` after
    /// `:tselect`, which lists without jumping.
    current: Option<usize>,
}

thread_local! {
    /// Vim keeps the match list in a `static` inside tag.c; RustVim keeps it
    /// next to the commands instead of in `ScriptState` (owned elsewhere).
    /// A session is single threaded, so one cache per thread is enough.
    static MATCH_CACHE: RefCell<MatchCache> = const {
        RefCell::new(MatchCache {
            name: String::new(),
            matches: Vec::new(),
            current: None,
        })
    };
}

fn err(code: &'static str, message: impl Into<String>) -> VimError {
    VimError::new(code, message)
}

fn cache_get() -> Option<MatchCache> {
    MATCH_CACHE.with(|cache| {
        let cache = cache.borrow();
        (!cache.matches.is_empty()).then(|| cache.clone())
    })
}

fn cache_put(name: &str, matches: &[TagEntry], current: Option<usize>) {
    MATCH_CACHE.with(|cache| {
        *cache.borrow_mut() = MatchCache {
            name: name.to_string(),
            matches: matches.to_vec(),
            current,
        };
    });
}

/// The count prefix of a tag command (`:2tag foo` jumps to match 2): Vim's
/// `eap->addr_count > 0 ? eap->line2 : 1` (ex_docmd.c:9013).
fn command_count(ed: &Editor, p: &ParsedEx) -> usize {
    if p.addr_count == 0 {
        return 1;
    }
    match &p.range {
        Range::Lines(_, end) => (*end).max(1),
        Range::Current => ed.cursor().line + 1,
        Range::Whole => ed.line_count().max(1),
        Range::None | Range::Deferred(_) => 1,
    }
}

/// The tag name: the argument, or the keyword under the cursor when it is
/// empty (:tag without an argument, CTRL-]).  E349 when there is none.
fn tag_name(ed: &Editor, argument: &str) -> VimResult<String> {
    let argument = argument.trim();
    if !argument.is_empty() {
        return Ok(argument.to_string());
    }
    let pos = ed.cursor();
    let text = ed.current_buffer().line_str(pos.line).unwrap_or("");
    let word = rvim_editor::tags::keyword_at(text, pos.col);
    if word.is_empty() {
        return Err(err("E349", "No identifier under cursor"));
    }
    Ok(word)
}

/// Every match for `name`.  `:tag /pattern` searches the tags files with the
/// regexp engine (tag.c:662); everything else is an exact match.
fn find_matches(ed: &Editor, name: &str) -> Vec<TagEntry> {
    match name.strip_prefix('/') {
        Some(pattern) => {
            let ignore_case = ed.options.bool_or("ignorecase", false);
            rvim_editor::tags::find_tags_where(ed, |candidate| {
                rvim_viml::regex_mvp::is_match(pattern, candidate, ignore_case)
            })
        }
        None => rvim_editor::tags::find_tags(ed, name),
    }
}

fn matches_for(ed: &Editor, name: &str) -> VimResult<Vec<TagEntry>> {
    let matches = find_matches(ed, name);
    if matches.is_empty() {
        return Err(err("E426", format!("tag not found: {name}")));
    }
    Ok(matches)
}

// ---------------------------------------------------------------------------
// Jumping
// ---------------------------------------------------------------------------

/// Jump to the `index`th match in `matches`.
///
/// The target file is opened through [`crate::autodispatch::edit_file`] so the
/// BufRead/FileType chain fires, the tag command field is executed to position
/// the cursor, and the previous position is pushed on the tag stack (unless
/// `push` is false, which is how `:tnext` moves inside an existing entry).
fn jump_to_match(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    name: &str,
    matches: &[TagEntry],
    index: usize,
    push: bool,
) -> VimResult<()> {
    // A missing target file is skipped in favour of the next match, like
    // do_tag()'s NOTAGFILE retry loop (tag.c:879-901).
    let mut chosen = None;
    let mut first_missing = String::new();
    for (offset, entry) in matches.iter().enumerate().skip(index) {
        let path = entry.path(&ed.cwd);
        if path.is_file() {
            chosen = Some((offset, path));
            break;
        }
        if first_missing.is_empty() {
            first_missing = path.display().to_string();
        }
    }
    let Some((index, path)) = chosen else {
        return Err(err(
            "E429",
            format!("File \"{first_missing}\" does not exist"),
        ));
    };

    let previous_bufnr = ed.current_bufnr();
    let previous = ed.cursor();
    let command = matches[index].cmd.clone();

    crate::autodispatch::edit_file(ed, env, state, &path)?;
    apply_tag_command(ed, &command)?;

    let entry = TagStackEntry {
        tagname: name.to_string(),
        matchnr: index as i64 + 1,
        bufnr: previous_bufnr as i64,
        from: [
            previous_bufnr as i64,
            previous.line as i64 + 1,
            previous.col as i64,
            0,
        ],
        user_data: None,
    };
    let mut updated = false;
    if !push {
        // :tnext edits the entry that led here; it does not grow the stack.
        let slot = state
            .win
            .tag_stack_idx
            .checked_sub(1)
            .and_then(|index| state.win.tag_stack.get_mut(index));
        if let Some(existing) = slot {
            existing.matchnr = entry.matchnr;
            updated = true;
        }
    }
    if !updated {
        push_tag_stack(state, entry);
    }
    cache_put(name, matches, Some(index));
    Ok(())
}

/// Push one tag stack entry (tag.c:387-408): drop the entries above the current
/// index, append, then cap the stack at TAGSTACKSIZE by dropping the oldest.
pub(crate) fn push_tag_stack(state: &mut ScriptState, entry: TagStackEntry) {
    let keep = state.win.tag_stack_idx.min(state.win.tag_stack.len());
    state.win.tag_stack.truncate(keep);
    state.win.tag_stack.push(entry);
    if state.win.tag_stack.len() > TAGSTACKSIZE {
        state.win.tag_stack.remove(0);
    }
    state.win.tag_stack_idx = state.win.tag_stack.len();
}

/// Move the cursor to the first non-blank of `line`, clamped to the buffer.
fn move_to_line(ed: &mut Editor, line: usize) {
    let line = line.min(ed.line_count().saturating_sub(1));
    let col = ed.current_buffer().first_nonblank_byte(line).unwrap_or(0);
    ed.set_cursor(Pos::new(line, col));
}

/// Execute a tags file command field.
///
/// A leading decimal number is a line number; `/pat/` and `?pat?` search forward
/// and backward; anything else is used as a forward search pattern.
fn apply_tag_command(ed: &mut Editor, command: &str) -> VimResult<()> {
    let command = command.trim();
    if command.is_empty() {
        move_to_line(ed, 0);
        return Ok(());
    }
    let digits: String = command.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        if let Ok(number) = digits.parse::<usize>() {
            move_to_line(ed, number.saturating_sub(1));
            return Ok(());
        }
    }
    let (delimiter, pattern) = match command.chars().next() {
        Some(delimiter @ ('/' | '?')) => (delimiter, tag_search_pattern(command, delimiter)),
        // An address without delimiters ("foo") is a pattern too.
        _ => ('/', command.to_string()),
    };
    if search_tag_pattern(ed, &pattern, delimiter == '?') {
        Ok(())
    } else {
        Err(err("E434", "Can't find tag pattern"))
    }
}

/// Strip the delimiters of a `/pat/` address, keeping regexp escapes intact.
fn tag_search_pattern(command: &str, delimiter: char) -> String {
    let mut pattern = String::new();
    let mut escaped = false;
    for ch in command.chars().skip(1) {
        if escaped {
            // ctags escapes the address delimiter itself; the other escapes
            // belong to the regexp (`\/` is a literal '/', `\<` a word start).
            if ch != delimiter {
                pattern.push('\\');
            }
            pattern.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == delimiter {
            break;
        } else {
            pattern.push(ch);
        }
    }
    if escaped {
        pattern.push('\\');
    }
    pattern
}

/// Search the whole buffer for `pattern`, case sensitively first and then
/// ignoring case, like jumpto_tag()'s two attempts (tag.c:3907-3921).
/// Backward addresses (`?pat?`) search from the last line up.
fn search_tag_pattern(ed: &mut Editor, pattern: &str, backward: bool) -> bool {
    let line_count = ed.line_count();
    if line_count == 0 {
        return false;
    }
    let lines: Vec<String> = (0..line_count)
        .filter_map(|line| ed.current_buffer().line_str(line).ok().map(str::to_string))
        .collect();
    for ignore_case in [false, true] {
        let indexes: Vec<usize> = if backward {
            (0..lines.len()).rev().collect()
        } else {
            (0..lines.len()).collect()
        };
        for line in indexes {
            if rvim_viml::regex_mvp::find(pattern, &lines[line], ignore_case).is_some() {
                move_to_line(ed, line);
                return true;
            }
        }
    }
    false
}

/// Jump to a tag: `:tag`, `:ptag`, `:stag` and CTRL-].
///
/// An empty `name` uses the keyword under the cursor.  `count` selects the
/// `count`th match (`:2tag foo`); out-of-range counts clamp to the last match.
pub fn jump_to_tag(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    name: &str,
    count: usize,
) -> VimResult<()> {
    if ed.current_buffer().is_help() {
        let topic = if name.trim().is_empty() {
            crate::handlers_help::topic_at_cursor(ed)?
        } else {
            name.to_string()
        };
        crate::handlers_help::cmd_help(ed, env, state, &topic)?;
        return Ok(());
    }
    let name = tag_name(ed, name)?;
    let matches = matches_for(ed, &name)?;
    let index = count.saturating_sub(1).min(matches.len() - 1);
    jump_to_match(ed, env, state, &name, &matches, index, true)
}

/// `:ltag`: prefer tags that point into the current buffer and fall back to a
/// normal tag jump when there is none.
fn jump_to_tag_in_buffer(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    name: &str,
    count: usize,
) -> VimResult<()> {
    let name = tag_name(ed, name)?;
    let all = matches_for(ed, &name)?;
    let current = ed.current_buffer().name.clone();
    let local: Vec<TagEntry> = all
        .iter()
        .filter(|entry| Some(entry.path(&ed.cwd)) == current)
        .cloned()
        .collect();
    let (matches, index) = if local.is_empty() {
        let index = count.saturating_sub(1).min(all.len() - 1);
        (all, index)
    } else {
        let index = count.saturating_sub(1).min(local.len() - 1);
        (local, index)
    };
    jump_to_match(ed, env, state, &name, &matches, index, true)
}

/// `:pop [count]`: restore the position saved by an earlier tag jump.
pub fn tag_pop(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    count: usize,
) -> VimResult<()> {
    let count = count.max(1);
    if state.win.tag_stack.is_empty() || state.win.tag_stack_idx == 0 {
        return Err(err("E73", "Tag stack empty"));
    }
    // Vim subtracts the count from the index and jumps all the way to the
    // bottom when it would run past the first entry (tag.c:433-445).
    let target = state.win.tag_stack_idx.saturating_sub(count);
    let entry = state.win.tag_stack[target].clone();
    let bufnr = entry.from[0] as i32;
    let line = (entry.from[1].max(1) - 1) as usize;
    let col = entry.from[2].max(0) as usize;
    if bufnr != ed.current_bufnr() && ed.buffer(bufnr).is_ok() {
        crate::autodispatch::switch_buffer(ed, env, state, bufnr)?;
    }
    let pos = ed.current_buffer().clamp_pos(Pos::new(line, col));
    ed.set_cursor(pos);
    state.win.tag_stack_idx = target;
    Ok(())
}

// ---------------------------------------------------------------------------
// :tselect / :tnext
// ---------------------------------------------------------------------------

/// `:tselect [name]`: list the matches and remember them for :tnext.
pub fn tag_select(ed: &mut Editor, name: &str) -> VimResult<()> {
    let name = tag_name(ed, name)?;
    let matches = matches_for(ed, &name)?;
    cache_put(&name, &matches, None);
    print_tag_list(ed, &matches);
    Ok(())
}

/// `:tjump [name]`: jump when there is exactly one match, list otherwise.
pub fn tag_select_or_jump(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    name: &str,
    count: usize,
) -> VimResult<()> {
    let name = tag_name(ed, name)?;
    let matches = matches_for(ed, &name)?;
    if matches.len() == 1 {
        return jump_to_match(ed, env, state, &name, &matches, 0, true);
    }
    cache_put(&name, &matches, None);
    print_tag_list(ed, &matches);
    let _ = count;
    Ok(())
}

/// Which direction :tnext and friends move in the remembered match list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cycle {
    Next,
    Previous,
    First,
    Last,
}

/// Resolve the match list a cycle command works on: the remembered list when it
/// still names the tag of the current tag stack entry, otherwise a fresh search
/// for that entry's tag name.
fn cycle_context(
    ed: &Editor,
    state: &ScriptState,
) -> VimResult<(String, Vec<TagEntry>, Option<usize>)> {
    let stack_entry = state
        .win
        .tag_stack_idx
        .checked_sub(1)
        .and_then(|index| state.win.tag_stack.get(index))
        .cloned();
    if let Some(mut cache) = cache_get() {
        let agrees = stack_entry
            .as_ref()
            .is_none_or(|entry| entry.tagname == cache.name);
        if agrees {
            if let (Some(entry), None) = (&stack_entry, cache.current) {
                cache.current = Some((entry.matchnr.max(1) - 1) as usize);
            }
            return Ok((cache.name, cache.matches, cache.current));
        }
    }
    let Some(entry) = stack_entry else {
        return Err(err("E73", "Tag stack empty"));
    };
    let matches = matches_for(ed, &entry.tagname)?;
    let current = ((entry.matchnr.max(1) - 1) as usize).min(matches.len() - 1);
    Ok((entry.tagname, matches, Some(current)))
}

/// `:tnext`/`:tprevious`/`:tfirst`/`:tlast`/`:trewind`.
fn tag_cycle(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    cycle: Cycle,
) -> VimResult<()> {
    let (name, matches, current) = cycle_context(ed, state)?;
    let length = matches.len();
    let index = match cycle {
        Cycle::Next => current.map_or(0, |index| (index + 1) % length),
        Cycle::Previous => current.map_or(length - 1, |index| (index + length - 1) % length),
        Cycle::First => 0,
        Cycle::Last => length - 1,
    };
    // A cycle moves inside the entry the jump created; only a list remembered
    // by :tselect (no stack entry for this tag) needs a new entry.
    let reuse_entry = state.win.tag_stack_idx > 0;
    jump_to_match(ed, env, state, &name, &matches, index, !reuse_entry)?;
    if length > 1 {
        ed.messages.info(format!("tag {} of {}", index + 1, length));
    }
    Ok(())
}

/// Line for the ":tselect" header and rows, aligned like print_tag_list():
/// the tag name column is `max(len(first tag) + 2, 18)` wide.
fn tag_list_width(matches: &[TagEntry]) -> usize {
    let longest = matches
        .first()
        .map_or(0, |entry| entry.name.chars().count());
    (longest + 2).max(18)
}

fn print_tag_list(ed: &mut Editor, matches: &[TagEntry]) {
    let width = tag_list_width(matches);
    ed.messages.info(format!(
        "  # pri kind tag{:width$}file",
        "",
        width = width - 3
    ));
    for (index, entry) in matches.iter().enumerate() {
        let path = entry.path(&ed.cwd);
        let same_file = ed.current_buffer().name.as_deref() == Some(path.as_path());
        // mt_names[] from tag.c: "FSC", "F C", "F  ", "FS " (S = static).
        let priority = match (entry.static_tag, same_file) {
            (true, true) => "FSC",
            (true, false) => "FS ",
            (false, true) => "F C",
            (false, false) => "F  ",
        };
        let kind = entry.kind.as_deref().unwrap_or("");
        ed.messages.info(format!(
            "{:>3} {} {:<5}{:<width$}{}",
            index + 1,
            priority,
            kind,
            entry.name,
            path.display(),
        ));
    }
}

/// `:tags`: print the tag stack, "> " marking the current entry.
fn print_tag_stack(ed: &mut Editor, state: &ScriptState) {
    ed.messages
        .info("  # TO tag         FROM line  in file/text".to_string());
    let Some(last) = state.win.tag_stack.len().checked_sub(1) else {
        return;
    };
    // tag_stack_idx is one past the entry that describes the current position,
    // so clamp it to the top entry to keep the marker on a real tag.
    let current = state.win.tag_stack_idx.saturating_sub(1).min(last);
    for (index, entry) in state.win.tag_stack.iter().enumerate() {
        let marker = if index == current { '>' } else { ' ' };
        ed.messages.info(format!(
            "{}{:2} {:2} {:<15} {:5}  {}",
            marker,
            index + 1,
            entry.matchnr,
            entry.tagname,
            entry.from[1],
            stack_location(ed, entry)
        ));
    }
}

/// Vim's "in file/text" column (fm_getname()): the line text for an entry in
/// the current buffer, the buffer name for an entry in another one.
fn stack_location(ed: &Editor, entry: &TagStackEntry) -> String {
    let bufnr = entry.from[0] as i32;
    if bufnr == ed.current_bufnr() {
        let line = (entry.from[1].max(1) - 1) as usize;
        if let Ok(text) = ed.current_buffer().line_str(line) {
            return text.chars().take(30).collect();
        }
    }
    ed.buffer(bufnr)
        .map(|buffer| buffer.display_name())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch one tag command (exec.rs routes the whole family here).
pub fn cmd_tag(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    let count = command_count(ed, p);
    match name {
        "tag" | "ptag" | "stag" => jump_to_tag(ed, env, state, &p.args, count)?,
        "ltag" => jump_to_tag_in_buffer(ed, env, state, &p.args, count)?,
        "tselect" | "stselect" | "dlist" => tag_select(ed, &p.args)?,
        "tjump" | "djump" => tag_select_or_jump(ed, env, state, &p.args, count)?,
        "tnext" => tag_cycle(ed, env, state, Cycle::Next)?,
        "tprevious" => tag_cycle(ed, env, state, Cycle::Previous)?,
        "tfirst" | "trewind" => tag_cycle(ed, env, state, Cycle::First)?,
        "tlast" => tag_cycle(ed, env, state, Cycle::Last)?,
        "pop" => tag_pop(ed, env, state, count)?,
        "tags" => print_tag_stack(ed, state),
        other => {
            return Err(err("E492", format!("Not an editor command: {other}")));
        }
    }
    Ok(ExOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_and_pattern_helpers() {
        assert_eq!(tag_search_pattern("/^int foo(/;", '/'), "^int foo(");
        assert_eq!(tag_search_pattern("/a\\/b/", '/'), "a/b");
        assert_eq!(tag_search_pattern("?^end?$", '?'), "^end");
        assert_eq!(tag_search_pattern("/unterminated", '/'), "unterminated");
    }

    #[test]
    fn tag_stack_is_capped_at_vim_size() {
        let mut state = ScriptState::default();
        for index in 0..(TAGSTACKSIZE + 5) {
            push_tag_stack(
                &mut state,
                TagStackEntry {
                    tagname: format!("tag{index}"),
                    matchnr: 1,
                    bufnr: 1,
                    from: [1, 1, 0, 0],
                    user_data: None,
                },
            );
        }
        assert_eq!(state.win.tag_stack.len(), TAGSTACKSIZE);
        // The oldest entries were dropped, the newest is still on top.
        assert_eq!(state.win.tag_stack[0].tagname, "tag5");
        assert_eq!(
            state.win.tag_stack[TAGSTACKSIZE - 1].tagname,
            format!("tag{}", TAGSTACKSIZE + 4)
        );
        assert_eq!(state.win.tag_stack_idx, TAGSTACKSIZE);
    }

    #[test]
    fn pushing_truncates_entries_above_the_current_index() {
        let mut state = ScriptState::default();
        for name in ["a", "b", "c"] {
            push_tag_stack(
                &mut state,
                TagStackEntry {
                    tagname: name.to_string(),
                    ..TagStackEntry::default()
                },
            );
        }
        state.win.tag_stack_idx = 1;
        push_tag_stack(
            &mut state,
            TagStackEntry {
                tagname: "d".to_string(),
                ..TagStackEntry::default()
            },
        );
        let names: Vec<&str> = state
            .win
            .tag_stack
            .iter()
            .map(|entry| entry.tagname.as_str())
            .collect();
        assert_eq!(names, ["a", "d"]);
        assert_eq!(state.win.tag_stack_idx, 2);
    }
}
