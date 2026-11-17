//! Resolve *deferred* Ex addresses against editor state.
//!
//! The parser only captures the raw address text (`/pat/`, `?pat?`, `'a`,
//! `.,/end/+1`, ...) because line numbers and search results depend on the
//! buffer and cursor. This module turns that text into a line range, following
//! `get_address()` semantics from ex_docmd.c.
//!
//! M2 pattern matching is literal (there is no regex engine until M6), so
//! search addresses compare with `str::contains` honouring `'ignorecase'`.

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;

/// Split `text` at the top-level comma, respecting escapes inside search
/// patterns. Returns the two halves, or `None` for a single address.
fn split_comma(text: &str) -> Option<(&str, &str)> {
    let mut delim: Option<char> = None;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if let Some(d) = delim {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == d {
                delim = None;
            }
            continue;
        }
        match c {
            '/' | '?' => delim = Some(c),
            // ';' is a range separator too (ex_docmd.c:3407-3426), see
            // resolve_deferred_pair for the cursor-placement difference.
            ',' | ';' => return Some((&text[..i], &text[i + 1..])),
            _ => {}
        }
    }
    None
}

/// Un-escape the pattern delimiter/backslash escapes that Vim allows inside
/// search addresses. Note that regex metacharacters keep their backslash so the
/// pattern remains a regexp (`:\\+` still means "one or more").
fn unescape_pattern(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len());
    let mut chars = pat.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek().copied() {
                Some('/') | Some('?') | Some('\\') => out.push(chars.next().unwrap()),
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A parsed single address: base plus offsets.
struct Addr {
    base: Base,
    /// Signed offsets applied in order after the base resolves.
    offsets: Vec<isize>,
    /// Search atoms that continue the address (`:2/pat/` searches after line 2,
    /// ex_docmd.c:4597 `while (*cmd == '/' || *cmd == '?')`).
    searches: Vec<(String, bool)>,
}

enum Base {
    /// `.` or empty: the default line supplied by the caller.
    Default,
    /// `$`: last line.
    Last,
    /// `N`: 1-based line number.
    Line(usize),
    /// `'x`: mark.
    Mark(char),
    /// `/pat/` (forward) or `?pat?` (backward).
    Search { pattern: String, forward: bool },
}

fn parse_addr(text: &str) -> VimResult<Addr> {
    let s = text.trim();
    let mut chars = s.char_indices().peekable();
    let base = match chars.peek().map(|(_, c)| *c) {
        None | Some('.') => {
            if s.starts_with('.') {
                chars.next();
            }
            Base::Default
        }
        // ex_docmd.c:4501-4515: an address starting with "+"/"-" is relative to
        // the current line ("-1" is ".-1").
        Some('+') | Some('-') => Base::Default,
        Some('$') => {
            chars.next();
            Base::Last
        }
        Some('\'') => {
            chars.next();
            let name = chars
                .next()
                .map(|(_, c)| c)
                .ok_or_else(|| VimError::mark_not_set('\''))?;
            Base::Mark(name)
        }
        Some('/') | Some('?') => {
            let delim = s.chars().next().unwrap();
            let rest = &s[1..];
            // The closing delimiter may be omitted (":/pat" is a valid
            // address, ex_docmd.c:4391-4441).
            let end = rest
                .char_indices()
                .find(|(i, c)| *c == delim && (*i == 0 || rest.as_bytes()[i - 1] != b'\\'))
                .map(|(i, _)| i)
                .unwrap_or(rest.len());
            let has_close = end < rest.len();
            // Skip over the pattern and its closing delimiter.
            let skip = 1 + end + if has_close { delim.len_utf8() } else { 0 };
            for _ in 0..s[..skip].chars().count() {
                chars.next();
            }
            Base::Search {
                pattern: unescape_pattern(&rest[..end]),
                forward: delim == '/',
            }
        }
        Some(c) if c.is_ascii_digit() => {
            let mut n = 0usize;
            while let Some((_, d)) = chars.peek().filter(|(_, c)| c.is_ascii_digit()) {
                n = n * 10 + d.to_digit(10).unwrap() as usize;
                chars.next();
            }
            Base::Line(n)
        }
        Some(other) => {
            return Err(VimError::new(
                "E492",
                format!("Not an editor command: {other}"),
            ));
        }
    };

    // Trailing +/-N offsets.
    let mut offsets = Vec::new();
    loop {
        let rest: String = chars.clone().map(|(_, c)| c).collect();
        let rest = rest.trim_start();
        let consumed_ws = chars.clone().count() - rest.chars().count();
        for _ in 0..consumed_ws {
            chars.next();
        }
        let Some(c) = chars.peek().map(|(_, c)| *c) else {
            break;
        };
        if c != '+' && c != '-' {
            break;
        }
        let sign = if c == '+' { 1 } else { -1 };
        chars.next();
        let mut digits = String::new();
        while let Some((_, d)) = chars.peek().filter(|(_, c)| c.is_ascii_digit()) {
            digits.push(*d);
            chars.next();
        }
        let n: isize = if digits.is_empty() {
            1
        } else {
            digits
                .parse()
                .map_err(|_| VimError::new("E16", format!("Invalid range: {text}")))?
        };
        offsets.push(sign * n);
    }
    // A search may continue the address (":2/pat/" = next match after line 2).
    let mut searches = Vec::new();
    while let Some((_, delim)) = chars.peek().copied() {
        if delim != '/' && delim != '?' {
            break;
        }
        chars.next();
        let mut pat = String::new();
        let mut escaped = false;
        for (_, ch) in chars.by_ref() {
            if escaped {
                pat.push(ch);
                escaped = false;
            } else if ch == '\\' {
                pat.push(ch);
                escaped = true;
            } else if ch == delim {
                break;
            } else {
                pat.push(ch);
            }
        }
        searches.push((unescape_pattern(&pat), delim == '/'));
    }
    let trailing: String = chars.map(|(_, c)| c).collect();
    if !trailing.trim().is_empty() {
        return Err(VimError::new(
            "E488",
            format!("Trailing characters: {}", trailing.trim()),
        ));
    }
    Ok(Addr {
        base,
        offsets,
        searches,
    })
}

/// Find `pattern` starting at `from` in the given direction, wrapping around.
/// Returns the matching line index.
///
/// An empty pattern searches for the last used search pattern
/// (search_regcomp with RE_SEARCH, search.c:140-158).
fn search(
    ed: &Editor,
    pattern: &str,
    forward: bool,
    from: usize,
    wrapscan: bool,
) -> VimResult<usize> {
    let pattern = if pattern.is_empty() {
        ed.search_pattern.as_str()
    } else {
        pattern
    };
    if pattern.is_empty() {
        return Err(VimError::new(
            "E35",
            "No previous regular expression".to_string(),
        ));
    }
    let ignorecase = ed.options.bool_or("ignorecase", false);
    let total = ed.line_count();
    let matches = |line: usize| -> bool {
        let text = ed.current_buffer().line_str(line).unwrap_or("");
        rvim_viml::regex_mvp::is_match(pattern, text, ignorecase)
    };
    let mut line = from;
    for _ in 0..total {
        if forward {
            line += 1;
            if line >= total {
                if !wrapscan {
                    break;
                }
                line = 0;
            }
        } else {
            line = line.checked_sub(1).unwrap_or(total - 1);
        }
        if matches(line) {
            return Ok(line);
        }
    }
    Err(VimError::pattern_not_found(pattern))
}

/// The last `/pat/` or `?pat?` atom in a deferred address expression (the one
/// vim would save as the last search pattern, ex_docmd.c:4406-4441).
pub fn last_search_atom(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut last = None;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' || c == '?' {
            let delim = c;
            i += 1;
            let mut pat = String::new();
            let mut escaped = false;
            while i < chars.len() {
                let ch = chars[i];
                i += 1;
                if escaped {
                    pat.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    pat.push(ch);
                    escaped = true;
                } else if ch == delim {
                    break;
                } else {
                    pat.push(ch);
                }
            }
            if !pat.is_empty() {
                last = Some(unescape_pattern(&pat));
            }
        } else {
            i += 1;
        }
    }
    last
}

fn resolve_one(
    ed: &Editor,
    addr: &Addr,
    default_line: usize,
    search_from: usize,
) -> VimResult<usize> {
    let mut line = match &addr.base {
        Base::Default => default_line.min(ed.line_count().saturating_sub(1)),
        Base::Last => ed.line_count().saturating_sub(1),
        Base::Line(n) => {
            if *n == 0 {
                0
            } else if *n > ed.line_count() {
                return Err(VimError::new(
                    "E16",
                    format!("Invalid range: line {n} out of range"),
                ));
            } else {
                n - 1
            }
        }
        Base::Mark(name) => {
            ed.marks
                .resolve(ed.current_bufnr(), *name)
                .ok_or_else(|| VimError::mark_not_set(*name))?
                .line
        }
        Base::Search { pattern, forward } => search(
            ed,
            pattern,
            *forward,
            search_from,
            ed.options.bool_or("wrapscan", true),
        )?,
    };
    let max = ed.line_count().saturating_sub(1);
    for off in &addr.offsets {
        let next = line as isize + off;
        if next < 0 || next as usize > max {
            return Err(VimError::new("E16", "Invalid range".to_string()));
        }
        line = next as usize;
    }
    // A search following the base address starts after the resolved line
    // (ex_docmd.c:4597; "/pat" after an address never matches that line).
    for (pat, forward) in &addr.searches {
        line = search(
            ed,
            pat,
            *forward,
            line,
            ed.options.bool_or("wrapscan", true),
        )?;
    }
    Ok(line)
}

/// Resolve a raw deferred address expression into `[start, end)`, normalising a
/// backwards range (the Ex layer reports E493 before calling this where vim
/// does; see [`resolve_deferred_pair`]).
pub fn resolve_deferred(ed: &Editor, text: &str) -> VimResult<(usize, usize)> {
    let (first, last) = resolve_deferred_pair(ed, text)?;
    Ok((first.min(last), first.max(last) + 1))
}

/// Resolve a deferred address expression to its two endpoint lines, without
/// swapping a backwards range.
///
/// This mirrors vim's `parse_cmd_address()`/`get_address()` order of operations:
/// the default for a missing second address is the cursor line
/// (ex_docmd.c:3295-3296, default_address() returns the cursor line).
pub fn resolve_deferred_pair(ed: &Editor, text: &str) -> VimResult<(usize, usize)> {
    let cursor = ed.cursor().line;
    match split_comma(text) {
        None => {
            let addr = parse_addr(text)?;
            let line = resolve_one(ed, &addr, cursor, cursor)?;
            Ok((line, line))
        }
        Some((a, b)) => {
            let a_addr = parse_addr(a)?;
            // An empty first address defaults to the cursor line as well.
            let first = resolve_one(ed, &a_addr, cursor, cursor)?;
            let b_addr = parse_addr(b)?;
            let last = resolve_one(ed, &b_addr, cursor, first)?;
            Ok((first, last))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_core::Pos;

    fn editor(lines: &[&str]) -> Editor {
        let mut ed = Editor::new();
        let text: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        ed.replace_lines(0, 1, text).unwrap();
        ed
    }

    #[test]
    fn line_mark_and_last_addresses() {
        let mut ed = editor(&["one", "two", "three"]);
        assert_eq!(resolve_deferred(&ed, "2").unwrap(), (1, 2));
        assert_eq!(resolve_deferred(&ed, "$").unwrap(), (2, 3));
        ed.marks.set_local(ed.current_bufnr(), 'a', Pos::new(1, 0));
        assert_eq!(resolve_deferred(&ed, "'a").unwrap(), (1, 2));
        assert_eq!(resolve_deferred(&ed, "$-1").unwrap(), (1, 2));
    }

    #[test]
    fn search_addresses_forward_backward_and_pairs() {
        let mut ed = editor(&["alpha", "beta", "alpha2", "gamma"]);
        ed.set_cursor(Pos::new(0, 0));
        // /beta/ searches after the cursor line.
        assert_eq!(resolve_deferred(&ed, "/beta/").unwrap(), (1, 2));
        // ?alpha? searches backwards from the cursor, wrapping to line 2.
        ed.set_cursor(Pos::new(2, 0));
        assert_eq!(resolve_deferred(&ed, "?alpha?").unwrap(), (0, 1));
        // Range pair: from line 1 to the next /gamma/.
        assert_eq!(resolve_deferred(&ed, "1,/gamma/").unwrap(), (0, 4));
    }

    #[test]
    fn missing_pattern_and_mark_report_vim_errors() {
        let ed = editor(&["only"]);
        assert_eq!(resolve_deferred(&ed, "/nope/").unwrap_err().code, "E486");
        assert_eq!(resolve_deferred(&ed, "'z").unwrap_err().code, "E20");
    }

    #[test]
    fn out_of_range_line_is_rejected() {
        let ed = editor(&["only"]);
        assert_eq!(resolve_deferred(&ed, "9").unwrap_err().code, "E16");
    }

    #[test]
    fn escaped_commas_stay_inside_the_pattern() {
        let ed = editor(&["a,b", "plain"]);
        assert_eq!(resolve_deferred(&ed, "/a\\,b/").unwrap(), (0, 1));
    }

    #[test]
    fn relative_addresses_start_from_the_cursor() {
        let mut ed = editor(&["one", "two", "three", "four"]);
        ed.set_cursor(Pos::new(2, 0));
        // "-1" is ".-1" (ex_docmd.c:4501-4515).
        assert_eq!(resolve_deferred(&ed, "-1").unwrap(), (1, 2));
        assert_eq!(resolve_deferred(&ed, "+1").unwrap(), (3, 4));
        assert_eq!(resolve_deferred(&ed, ".-1").unwrap(), (1, 2));
    }

    #[test]
    fn empty_second_address_defaults_to_the_cursor() {
        let mut ed = editor(&["one", "two", "three", "four"]);
        ed.set_cursor(Pos::new(2, 0));
        // ":2,d" deletes lines 2..cursor (ex_docmd.c:3295-3296).
        assert_eq!(resolve_deferred(&ed, "2,").unwrap(), (1, 3));
        // A backwards pair is reported unswapped by resolve_deferred_pair.
        ed.set_cursor(Pos::new(0, 0));
        assert_eq!(resolve_deferred_pair(&ed, "3,1").unwrap(), (2, 0));
        assert_eq!(resolve_deferred(&ed, "3,1").unwrap(), (0, 3));
    }
}
