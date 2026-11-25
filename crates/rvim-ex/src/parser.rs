//! Parse an Ex command line into range + command + arguments.
//!
//! Reference: ex_docmd.c `do_one_cmd()` / `get_address()` / `find_ex_command()`.
//!
//! Supported in M2:
//! * addresses: `.` `$` `%` `N` `+N` `-N`, comma/semicolon pairs, marks `'a`,
//!   search addresses `/pat/` and `?pat?` (resolved later, see [`Range::Deferred`]);
//! * command word with Vim's abbreviation rules (via [`crate::table::lookup`]),
//!   the `:dl`/`:dp`/`:d` flag quirk and the `:fina` → `:finally` legacy quirk;
//! * `!` bang, trailing `|` handling driven by the command's `EX_TRLBAR` flag,
//!   trailing comments (`"`) unless the command has `EX_NOTRLCOM`;
//! * `++opt=val` and `+cmd` file arguments for commands declaring
//!   `EX_ARGOPT` / `EX_CMDARG`.

use rvim_core::{VimError, VimResult};

use crate::table::{ExFlags, lookup};

/// A resolved or deferred range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Range {
    /// No range supplied.
    None,
    /// Explicit [start, end) zero-based half-open line indexes.
    Lines(usize, usize),
    /// Whole buffer ("%").
    Whole,
    /// Current line (the bare ".").
    Current,
    /// An address expression that needs buffer/cursor state to resolve
    /// (`/pat/`, `?pat?`, `'a`, and combinations such as `.,/pat/+2`).
    Deferred(String),
}

/// One `++opt` file argument (`++ff=dos`, `++enc=utf-8`, `++bin`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOpt {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedEx {
    pub range: Range,
    /// Number of addresses given (`0` for none, 1 for `:3`, 2 for `:3,4`).
    /// Vim's `exarg_T.addr_count`; needed by `:join` (`:2j` vs `:2,2j`) and the
    /// user-command range/count attributes.
    pub addr_count: usize,
    /// Command word exactly as typed; resolved through the table by the caller.
    pub command: String,
    pub bang: bool,
    /// Arguments after the command word (bars/`++opts`/`+cmd` removed).
    pub args: String,
    /// `++opt` file arguments, in order.
    pub fileopts: Vec<FileOpt>,
    /// Leading `+cmd` / `+N` argument (EX_CMDARG commands).
    pub plus: Option<String>,
    /// Remainder after a `|` when the command allows trailing bars.
    pub trailing: Option<String>,
    /// Raw line as parsed (for messages).
    pub raw: String,
}

impl ParsedEx {
    /// Convenience constructor for tests and internal callers.
    pub fn simple(command: &str, args: &str) -> Self {
        Self {
            range: Range::None,
            addr_count: 0,
            command: command.to_string(),
            bang: false,
            args: args.to_string(),
            fileopts: Vec::new(),
            plus: None,
            trailing: None,
            raw: String::new(),
        }
    }
}

/// Strip a trailing double-quote comment. A `"` that begins a line or follows
/// whitespace/bar starts a comment; quotes inside strings are skipped.
fn strip_comment(line: &str) -> &str {
    let chars: Vec<char> = line.chars().collect();
    let mut quote: Option<char> = None;
    for (i, &ch) in chars.iter().enumerate() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            let prev_is_sep =
                i == 0 || chars[i - 1] == ' ' || chars[i - 1] == '\t' || chars[i - 1] == '|';
            if ch == '"' && prev_is_sep {
                return line[..line
                    .char_indices()
                    .nth(i)
                    .map(|(b, _)| b)
                    .unwrap_or(line.len())]
                    .trim_end();
            }
            quote = Some(ch);
        }
    }
    line
}

/// Find the first unquoted `|`, honouring single- and double-quoted strings.
///
/// `||` is the VimL logical-or operator, not a command separator: vim evaluates
/// the expression first (consuming `||`) and only then looks for a bar, so
/// `:if a || b`, `:echo a || b` and `:let x = a || b` stay in one piece.
fn find_bar(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, ch) in text.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' && q == '"' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '|' => {
                // Both halves of a `||` operator belong to the expression.
                if text[..i].ends_with('|') || text[i + 1..].starts_with('|') {
                    let _ = bytes;
                    continue;
                }
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    total_lines: usize,
    /// True when an address `0` was parsed (possible in EX_ZEROR commands).
    saw_zero: bool,
}

impl Parser {
    fn new(line: &str, total_lines: usize) -> Self {
        Self {
            chars: line.chars().collect(),
            pos: 0,
            total_lines,
            saw_zero: false,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c == ' ' || c == '\t') {
            self.pos += 1;
        }
    }

    fn rest(&self) -> String {
        self.chars[self.pos..].iter().collect()
    }

    /// Read a `/pat/` or `?pat?` address, including the delimiters.
    fn search_atom(&mut self, delim: char) -> String {
        let mut out = String::new();
        out.push(delim);
        self.pos += 1; // skip opening delimiter
        let mut escaped = false;
        while let Some(c) = self.peek() {
            self.pos += 1;
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == delim {
                break;
            }
        }
        out
    }

    /// Parse one atomic address. -1 means current line, -2 means whole buffer.
    ///
    /// Only static bases (digits, `$`, `%`) reach this path; `.`, `+N`, `-N`,
    /// `/pat/`, `?pat?` and `'a` force a deferred range because they need the
    /// cursor/search state (see [`Parser::range_is_deferred`]).
    fn atom(&mut self) -> VimResult<Option<isize>> {
        self.skip_ws();
        match self.peek() {
            Some('$') => {
                self.pos += 1;
                Ok(Some(self.total_lines as isize - 1))
            }
            Some('%') => {
                self.pos += 1;
                Ok(Some(-2))
            }
            Some(c) if c.is_ascii_digit() => {
                let mut n = 0isize;
                while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
                    n = n * 10 + d as isize;
                    self.pos += 1;
                }
                if n == 0 {
                    // Address 0 ("before the first line", ex_docmd.c get_address)
                    // is only meaningful for EX_ZEROR commands; keep the raw text
                    // so the command layer can decide (put/read accept it).
                    self.saw_zero = true;
                    Ok(Some(-1))
                } else {
                    Ok(Some(n - 1)) // vim 1-based -> internal 0-based
                }
            }
            _ => Ok(None),
        }
    }

    fn parse_range(&mut self) -> VimResult<(Range, usize)> {
        self.skip_ws();
        self.saw_zero = false;
        if self.range_is_deferred() {
            return self.parse_deferred_range();
        }
        let start_pos = self.pos;
        let (range, count) = self.parse_static_range()?;
        // A search/mark/relative component may continue a static address:
        // ":2/pat/" is "next match after line 2", ":1;/pat/" sets the cursor to
        // line 1 first (ex_docmd.c:4597 `while (*cmd == '/' || *cmd == '?')`).
        if matches!(
            self.peek(),
            Some('/') | Some('?') | Some('\'') | Some('.') | Some('+') | Some('-')
        ) {
            self.parse_deferred_range()?;
            let raw: String = self.chars[start_pos..self.pos].iter().collect();
            return Ok((Range::Deferred(raw.trim().to_string()), count));
        }
        Ok((range, count))
    }

    /// True when the leading address expression needs runtime state
    /// (`.`/`+`/`-`/`/pat/`/`?pat?`/`'a` as its first atom).
    fn range_is_deferred(&self) -> bool {
        matches!(
            self.chars.get(self.pos),
            Some('.') | Some('+') | Some('-') | Some('/') | Some('?') | Some('\'')
        )
    }

    /// Consume a whole deferred address expression as raw text.
    fn parse_deferred_range(&mut self) -> VimResult<(Range, usize)> {
        let mut text = String::new();
        let mut commas = 0usize;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('/') => text.push_str(&self.search_atom('/')),
                Some('?') => text.push_str(&self.search_atom('?')),
                Some('\'') => {
                    text.push('\'');
                    self.pos += 1;
                    if let Some(c) = self.peek() {
                        text.push(c);
                        self.pos += 1;
                    }
                }
                Some(c) if c.is_ascii_digit() => {
                    while let Some(d) = self.peek().filter(|c| c.is_ascii_digit()) {
                        text.push(d);
                        self.pos += 1;
                    }
                }
                Some('.') => {
                    text.push('.');
                    self.pos += 1;
                }
                Some('$') => {
                    text.push('$');
                    self.pos += 1;
                }
                Some('%') => {
                    self.pos += 1;
                    return Ok((Range::Whole, 1));
                }
                Some(',') | Some(';') => {
                    // A ';' also resets the cursor to the previous address in
                    // vim (ex_docmd.c:3407-3422); both are address separators.
                    text.push(',');
                    commas += 1;
                    self.pos += 1;
                }
                Some('+') | Some('-') => {
                    // ex_docmd.c:4501-4596: a bare +/- is relative to the
                    // cursor; "+" is "+1", but "+0" keeps its explicit zero.
                    let sign = self.peek().unwrap();
                    text.push(sign);
                    self.pos += 1;
                    self.skip_ws();
                    let mut got = false;
                    while let Some(d) = self.peek().filter(|c| c.is_ascii_digit()) {
                        text.push(d);
                        self.pos += 1;
                        got = true;
                    }
                    if !got {
                        text.push('1');
                    }
                }
                _ => break,
            }
        }
        if text.is_empty() {
            return Ok((Range::None, 0));
        }
        Ok((Range::Deferred(text), commas + 1))
    }

    fn parse_static_range(&mut self) -> VimResult<(Range, usize)> {
        let start_pos = self.pos;
        let Some(first) = self.atom()? else {
            return Ok((Range::None, 0));
        };
        if first == -2 {
            return Ok((Range::Whole, 1));
        }
        let mut start = first;
        let mut end = first;
        let mut offsets_only = true;
        let mut commas = 0usize;
        let mut empty_second = false;
        let mut relative_after_comma = false;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('+') | Some('-') => {
                    let sign = if self.peek() == Some('+') { 1 } else { -1 };
                    self.pos += 1;
                    self.skip_ws();
                    let mut n = 0isize;
                    let mut got = false;
                    while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
                        n = n * 10 + d as isize;
                        self.pos += 1;
                        got = true;
                    }
                    if !got {
                        n = 1;
                    }
                    if offsets_only {
                        // Before any comma, "+N/-N" shifts both endpoints
                        // (ex_docmd.c:4501-4596, first address rules).
                        start += sign * n;
                    } else if !got {
                        // A bare +/- after the comma is relative to the cursor.
                        relative_after_comma = true;
                    }
                    end += sign * n;
                    if !offsets_only {
                        relative_after_comma = true;
                    }
                }
                Some(',') | Some(';') => {
                    self.pos += 1;
                    commas += 1;
                    offsets_only = false;
                    self.skip_ws();
                    match self.peek() {
                        None => {
                            // ":2,d" defaults the second address to the cursor
                            // line (ex_docmd.c:3295-3296).
                            empty_second = true;
                        }
                        Some('/') | Some('?') | Some('\'') | Some('.') => {
                            // A deferred address component follows the comma;
                            // the outer parse_range() consumes it as raw text.
                            break;
                        }
                        Some('+') | Some('-') => {
                            // Offset after the comma: resolved against the
                            // cursor at run time.
                            relative_after_comma = true;
                        }
                        Some(_) => match self.atom()? {
                            None => empty_second = true,
                            Some(-2) => return Ok((Range::Whole, commas + 1)),
                            Some(next) => {
                                if next == -1 && self.saw_zero {
                                    empty_second = true;
                                } else {
                                    end = next;
                                }
                            }
                        },
                    }
                }
                _ => break,
            }
        }
        if self.saw_zero || empty_second || relative_after_comma {
            // Re-parse at run time: address 0 (EX_ZEROR commands), an implicit
            // second address (cursor) and cursor-relative offsets need editor
            // state that the parser does not have.
            let raw: String = self.chars[start_pos..self.pos].iter().collect();
            return Ok((Range::Deferred(raw.trim().to_string()), commas + 1));
        }
        let norm = |v: isize| -> VimResult<usize> {
            if v < 0 {
                return Err(VimError::new("E16", "Invalid range".to_string()));
            }
            Ok(v as usize)
        };
        let s = norm(start)?;
        let e = norm(end)?;
        // Backwards ranges are NOT swapped here: vim reports E493 when running
        // a script or in Ex mode (ex_docmd.c:2146-2162); the Ex layer checks it.
        Ok((Range::Lines(s, e + 1), commas + 1))
    }

    fn parse_command(&mut self) -> String {
        self.skip_ws();
        let start = self.pos;
        // ex_docmd.c find_ex_command(): a command word is either a run of
        // ASCII letters, or a single non-alpha command character. Digits are
        // never part of the word, so ":1co3", ":>2" and ":s/a/b/" all parse
        // with the digits/arguments after the command.
        match self.peek() {
            Some(c) if c.is_ascii_alphabetic() => {
                while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.pos += 1;
                }
                // ':vim9cmd' / ':vim9script' are the only commands whose name
                // contains a digit; keep the digit and the letters after it.
                let word: String = self.chars[start..self.pos].iter().collect();
                if word == "vim" && self.peek() == Some('9') {
                    self.pos += 1;
                    while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                        self.pos += 1;
                    }
                }
            }
            Some('@' | '*' | '!' | '=' | '>' | '<' | '&' | '~' | '#' | '}') => {
                self.pos += 1;
            }
            Some(c) if c == '+' || c == '-' => {
                // ":++nr" / ":%--nr" style commands are two characters.
                let next = self.chars.get(self.pos + 1).copied();
                if next == Some(c) {
                    self.pos += 2;
                }
            }
            _ => {}
        }
        self.chars[start..self.pos].iter().collect()
    }
}

/// Apply Vim's `:dl` / `:dp` quirk: a `:delete`-prefixed word ending in `l` or
/// `p` is `:delete` plus the LIST/PRINT flag. Returns the shortened word and
/// whether a flag was consumed.
fn delete_flag_quirk(word: &str) -> (&str, Option<char>) {
    if word.len() >= 2 && word.starts_with('d') {
        let last = word.as_bytes()[word.len() - 1] as char;
        if last == 'l' || last == 'p' {
            let stem = &word[..word.len() - 1];
            if "delete".starts_with(stem) {
                return (stem, Some(last));
            }
        }
    }
    (word, None)
}

/// Vim's `one_letter_cmd()` special case: short `:s...` spellings that are not
/// prefixes of any other command still mean `:substitute` (`:sc`, `:sg`, `:si`,
/// `:sI`, `:sr`). Without this the table lookup would resolve e.g. `:sc` to
/// `:scriptnames`, which is what Vim explicitly avoids.
fn normalize_command_word(word: &str) -> String {
    match word {
        "sc" | "sg" | "si" | "sI" | "sr" => "substitute".to_string(),
        other => other.to_string(),
    }
}

/// Parse a full (comment-stripped) Ex line.
pub fn parse_line(line: &str, total_lines: usize) -> VimResult<ParsedEx> {
    // A UTF-8 BOM at the start of a sourced script must not become part of the
    // command word (Windows editors routinely write one). Trailing comments are
    // stripped later, once the command's EX_NOTRLCOM flag is known.
    let cleaned = line.trim_start_matches('\u{feff}').to_string();
    let mut p = Parser::new(&cleaned, total_lines);
    p.skip_ws();
    // A leading ':' is allowed ("::set" style).
    while p.peek() == Some(':') {
        p.pos += 1;
    }
    let (range, addr_count) = p.parse_range()?;
    let mut command = p.parse_command();
    // Vim's one_letter_cmd(): ":ka" means ":k a" — only the first letter is the
    // command and the rest is the mark-name argument (ex_docmd.c:3513-3535,
    // 3799-3802). Only a two-letter word is split so ":keepalt" & friends still
    // resolve through the command table.
    if command.len() == 2 && command.starts_with('k') && command.as_bytes()[1].is_ascii_alphabetic()
    {
        p.pos -= 1;
        command = "k".to_string();
    }
    // ":fina" means ":finally" in legacy script (ex_docmd.c:3911).
    let mut bang = false;
    // The bang must follow the command word immediately (`:w!`), not after
    // whitespace: `:echomsg !0` has "!0" as its argument.
    if p.peek() == Some('!') {
        bang = true;
        p.pos += 1;
        if command.is_empty() {
            command = "!".to_string();
        }
        p.skip_ws();
    }

    // The :dl/:dp/:d quirk (ex_docmd.c:3825-3842).
    let (shortened, flag_char) = {
        let (stem, flag) = delete_flag_quirk(&command);
        (stem.to_string(), flag)
    };
    command = normalize_command_word(&shortened);
    if command == "fina" {
        command = "finally".to_string();
    }

    let def = lookup(&command);
    let flags = def.map(|d| d.flags()).unwrap_or_default();

    // File arguments: ++opt and +cmd (only for commands that accept them).
    let mut fileopts = Vec::new();
    let mut plus = None;
    if flags.contains(ExFlags::ARGOPT) || flags.contains(ExFlags::CMDARG) {
        p.skip_ws();
        loop {
            let rest = p.rest();
            if let Some(after) = rest.strip_prefix("++") {
                let token_end = after
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(after.len());
                let token = &after[..token_end];
                let (name, value) = match token.split_once('=') {
                    Some((n, v)) => (n.to_string(), Some(v.to_string())),
                    None => (token.to_string(), None),
                };
                fileopts.push(FileOpt { name, value });
                p.pos += 2 + token.chars().count();
                p.skip_ws();
                continue;
            }
            if let Some(after) = rest.strip_prefix('+') {
                let token_end = after
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(after.len());
                plus = Some(after[..token_end].to_string());
                p.pos += 1 + token_end;
                p.skip_ws();
            }
            break;
        }
    }

    // Arguments: comments are removed only for commands without EX_NOTRLCOM
    // (`:if` keeps `"..."` string literals), then an unquoted bar starts the
    // next command when the command allows a trailing bar.
    //
    // Commands that take an expression are also terminated by an unquoted bar
    // even though they lack EX_TRLBAR: vim evaluates the expression and then
    // finds the bar (`:echo 'a' | echo 'b'`). EX_EXPR_ARG marks most of them;
    // `:let`/`:eval`/`:call` use skip_expr(), which stops at a bar as well.
    let rest = p.rest();
    let known = lookup(&command).is_some();
    let rest = if known && !flags.contains(ExFlags::NOTRLCOM) {
        strip_comment(&rest).to_string()
    } else {
        // Unknown command words may be user-defined commands, whose comment/bar
        // handling depends on the -bar attribute and is applied at run time
        // (usercmd.c:816-817).
        rest
    };
    let splits_bars = flags.contains(ExFlags::TRLBAR)
        || flags.contains(ExFlags::EXPR_ARG)
        || matches!(command.as_str(), "let" | "eval" | "call");
    let (args, trailing) = if splits_bars {
        match find_bar(&rest) {
            Some(idx) => (
                rest[..idx].trim().to_string(),
                Some(rest[idx + 1..].trim().to_string()),
            ),
            None => (rest.trim().to_string(), None),
        }
    } else {
        (rest.trim().to_string(), None)
    };

    // Keep the delete flag visible to the caller through the raw text: handlers
    // read it back with `delete_flag()`.
    let mut args_final = args;
    if let Some(flag) = flag_char {
        args_final = format!("{flag}{args_final}");
    }

    Ok(ParsedEx {
        range,
        addr_count,
        command,
        bang,
        args: args_final,
        fileopts,
        plus,
        trailing,
        raw: cleaned,
    })
}

/// True when the `:delete` LIST/PRINT flag (`:dl`, `:dp`) was used. The flag is
/// stored as a leading marker on `args` by [`parse_line`].
pub fn take_delete_flag(p: &mut ParsedEx) -> Option<char> {
    let mut chars = p.args.chars();
    let first = chars.next()?;
    if first == 'l' || first == 'p' {
        p.args = chars.as_str().trim_start().to_string();
        Some(first)
    } else {
        None
    }
}

/// Split `text` at the first unquoted bar, returning `(before, after)`.
///
/// Used for user-defined commands with `-bar`, whose EX_TRLBAR flag is not in
/// the builtin table (usercmd.c:816-817).
pub fn split_first_bar(text: &str) -> (String, Option<String>) {
    match find_bar(text) {
        Some(idx) => (
            text[..idx].trim().to_string(),
            Some(text[idx + 1..].trim().to_string()),
        ),
        None => (text.trim().to_string(), None),
    }
}

/// Apply a user command's `-bar` attribute to its argument text.
///
/// Reference: `separate_nextcmd()` (ex_docmd.c:2279) — with EX_TRLBAR an
/// unquoted `|` starts the next command and a `"` at the start of a word starts
/// a comment. Without `-bar` the whole line stays in the argument, including
/// bars and quotes.
pub fn split_user_command_tail(text: &str, allow_bar: bool) -> (String, Option<String>) {
    if !allow_bar {
        return (text.trim().to_string(), None);
    }
    let mut prev = '\0';
    for (i, c) in text.char_indices() {
        if c == '|' {
            return (
                text[..i].trim().to_string(),
                Some(text[i + 1..].trim().to_string()),
            );
        }
        if c == '"' && (i == 0 || prev == ' ' || prev == '\t' || prev == '|') {
            return (text[..i].trim().to_string(), None);
        }
        prev = c;
    }
    (text.trim().to_string(), None)
}

/// Split a command line on unquoted bars, honouring single/double quoted strings.
pub fn split_bars(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut remaining = line;
    loop {
        match find_bar(remaining) {
            Some(idx) => {
                out.push(remaining[..idx].trim().to_string());
                remaining = &remaining[idx + 1..];
            }
            None => {
                let last = remaining.trim().to_string();
                if !last.is_empty() {
                    out.push(last);
                }
                break;
            }
        }
    }
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        let total = 10;
        assert_eq!(parse_line("p", total).unwrap().range, Range::None);
        assert_eq!(parse_line("%p", total).unwrap().range, Range::Whole);
        assert_eq!(parse_line("1p", total).unwrap().range, Range::Lines(0, 1));
        assert_eq!(parse_line("2,5p", total).unwrap().range, Range::Lines(1, 5));
        // "." forces a deferred range (cursor-dependent), like vim's get_address.
        assert_eq!(
            parse_line(".,$p", total).unwrap().range,
            Range::Deferred(".,$".to_string())
        );
        // An offset after the comma is relative to the cursor, not to line1.
        assert_eq!(
            parse_line("1,+2p", total).unwrap().range,
            Range::Deferred("1,+2".to_string())
        );
        // Backwards ranges are represented without swapping; the Ex layer
        // reports E493 for them in scripts / Ex mode (ex_docmd.c:2146-2162).
        let r = parse_line("3,1d", total).unwrap();
        assert_eq!(r.range, Range::Lines(2, 1));
        assert_eq!(r.addr_count, 2);
    }

    #[test]
    fn address_counts_and_zero_addresses() {
        assert_eq!(parse_line("p", 5).unwrap().addr_count, 0);
        assert_eq!(parse_line("2p", 5).unwrap().addr_count, 1);
        assert_eq!(parse_line("%p", 5).unwrap().addr_count, 1);
        assert_eq!(parse_line("2,4p", 5).unwrap().addr_count, 2);
        // Address 0 is kept deferred so :0put / :0read can accept it (EX_ZEROR,
        // ex_docmd.c:2354-2384); other commands clamp it to line 1.
        assert_eq!(
            parse_line("0pu", 5).unwrap().range,
            Range::Deferred("0".to_string())
        );
        assert_eq!(
            parse_line("0d", 5).unwrap().range,
            Range::Deferred("0".into())
        );
        // Leading +/- addresses are relative to the cursor (ex_docmd.c:4501-4515).
        assert_eq!(
            parse_line("-1d", 5).unwrap().range,
            Range::Deferred("-1".to_string())
        );
        assert_eq!(
            parse_line("+1d", 5).unwrap().range,
            Range::Deferred("+1".to_string())
        );
        // ":2," leaves the second address implicit (cursor line).
        assert_eq!(
            parse_line("2,d", 5).unwrap().range,
            Range::Deferred("2,".to_string())
        );
    }

    #[test]
    fn deferred_addresses() {
        assert_eq!(
            parse_line("/foo/p", 10).unwrap().range,
            Range::Deferred("/foo/".to_string())
        );
        assert_eq!(
            parse_line("?foo?p", 10).unwrap().range,
            Range::Deferred("?foo?".to_string())
        );
        assert_eq!(
            parse_line("'ap", 10).unwrap().range,
            Range::Deferred("'a".to_string())
        );
        assert_eq!(
            parse_line(".,/end/+1p", 10).unwrap().range,
            Range::Deferred(".,/end/+1".to_string())
        );
        assert_eq!(
            parse_line("/start/,/stop/d", 10).unwrap().range,
            Range::Deferred("/start/,/stop/".to_string())
        );
        assert_eq!(
            parse_line("'a,'bd", 10).unwrap().range,
            Range::Deferred("'a,'b".to_string())
        );
    }

    #[test]
    fn command_bang_and_args() {
        let r = parse_line("w! out.txt", 1).unwrap();
        assert_eq!(r.command, "w");
        assert!(r.bang);
        assert_eq!(r.args, "out.txt");
        let r2 = parse_line("set nu", 1).unwrap();
        assert_eq!(r2.command, "set");
        assert_eq!(r2.args, "nu");
    }

    #[test]
    fn bars_and_comments() {
        assert_eq!(split_bars("set nu|set et"), vec!["set nu", "set et"]);
        assert_eq!(split_bars("echo \"a|b\"|p"), vec!["echo \"a|b\"", "p"]);
        let r = parse_line("set nu \" enable number", 1).unwrap();
        assert_eq!(r.args, "nu");
    }

    #[test]
    fn bar_is_handled_by_trlbar_flag() {
        // :set has EX_TRLBAR, so the rest of the line is a new command.
        let r = parse_line("set nu|set et", 1).unwrap();
        assert_eq!(r.args, "nu");
        assert_eq!(r.trailing.as_deref(), Some("set et"));
        // :! has no EX_TRLBAR: the bar belongs to the shell command.
        let r = parse_line("!echo a|b", 1).unwrap();
        assert_eq!(r.command, "!");
        assert_eq!(r.args, "echo a|b");
        assert_eq!(r.trailing, None);
    }

    #[test]
    fn shell_bang() {
        let r = parse_line("!dir", 1).unwrap();
        assert_eq!(r.command, "!");
        assert_eq!(r.args, "dir");
    }

    #[test]
    fn numeric_argument_follows_command_word() {
        let r = parse_line("1co3", 3).unwrap();
        assert_eq!(r.range, Range::Lines(0, 1));
        assert_eq!(r.command, "co");
        assert_eq!(r.args, "3");
        let r = parse_line("2m3", 3).unwrap();
        assert_eq!(r.command, "m");
        assert_eq!(r.args, "3");
        let r = parse_line("2w out.txt", 3).unwrap();
        assert_eq!(r.range, Range::Lines(1, 2));
        assert_eq!(r.command, "w");
        assert_eq!(r.args, "out.txt");
    }

    #[test]
    fn delete_flag_quirk() {
        // ":dl" / ":dp" / ":del" mean :delete with the list/print flag.
        let mut r = parse_line("dl", 3).unwrap();
        assert_eq!(r.command, "d");
        assert_eq!(take_delete_flag(&mut r), Some('l'));
        let mut r = parse_line("dp", 3).unwrap();
        assert_eq!(r.command, "d");
        assert_eq!(take_delete_flag(&mut r), Some('p'));
        let mut r = parse_line("del", 3).unwrap();
        // vim strips one character: "del" -> command word "de" + LIST flag.
        assert_eq!(r.command, "de");
        assert_eq!(take_delete_flag(&mut r), Some('l'));
        // A plain :d has no flag.
        let mut r = parse_line("d", 3).unwrap();
        assert_eq!(r.command, "d");
        assert_eq!(take_delete_flag(&mut r), None);
    }

    #[test]
    fn fileopts_and_plus_cmd() {
        // :e accepts ++opt and +cmd (EX_ARGOPT|EX_CMDARG).
        let r = parse_line("e ++ff=dos +5 file.txt", 1).unwrap();
        assert_eq!(r.command, "e");
        assert_eq!(r.fileopts.len(), 1);
        assert_eq!(r.fileopts[0].name, "ff");
        assert_eq!(r.fileopts[0].value.as_deref(), Some("dos"));
        assert_eq!(r.plus.as_deref(), Some("5"));
        assert_eq!(r.args, "file.txt");
        // ++bin without a value.
        let r = parse_line("w ++bin out.bin", 1).unwrap();
        assert_eq!(r.fileopts[0].name, "bin");
        assert_eq!(r.fileopts[0].value, None);
        assert_eq!(r.args, "out.bin");
    }

    #[test]
    fn finally_quirk() {
        let r = parse_line("fina", 1).unwrap();
        assert_eq!(r.command, "finally");
    }

    #[test]
    fn leading_colons_are_skipped() {
        assert_eq!(parse_line("::set nu", 1).unwrap().command, "set");
    }
    #[test]
    fn user_command_bar_attribute_controls_trailing_split() {
        // -bar => the first unquoted bar starts the next command; without it the
        // bar (and double quotes) stay part of the argument (usercmd.c:816-817).
        let (args, trailing) = split_user_command_tail(" | echo 'x'", true);
        assert_eq!(args, "");
        assert_eq!(trailing.as_deref(), Some("echo 'x'"));
        let (args, trailing) = split_user_command_tail("hello | world", false);
        assert_eq!(args, "hello | world");
        assert_eq!(trailing, None);
        // A quote after whitespace starts a comment for -bar commands.
        let (args, trailing) = split_user_command_tail("foo \" comment", true);
        assert_eq!(args, "foo");
        assert_eq!(trailing, None);
    }

    #[test]
    fn search_continuing_a_static_address_is_deferred() {
        assert_eq!(
            parse_line("2/cat/", 10).unwrap().range,
            Range::Deferred("2/cat/".to_string())
        );
        assert_eq!(
            parse_line("1;/gamma/d", 10).unwrap().range,
            Range::Deferred("1;/gamma/".to_string())
        );
    }
}
