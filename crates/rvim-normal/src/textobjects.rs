//! Text objects (M5): pure functions mirroring Vim's textobject.c.
//!
//! [text_object] maps a buffer, a cursor position and one of Vim's text
//! object keys to the region that object covers.  Positions follow Vim's
//! internal convention: start_col is the byte offset of the first character
//! of the region and end_col is the byte offset of the *last byte of the
//! last character* (inclusive), i.e. the same columns getpos("'[") and
//! getpos("']") report after a yank.
//!
//! The port covers the operator-pending case of Vim (count = 1, no Visual
//! area) for current_word, current_block, current_quote, current_par,
//! current_sent and current_tagblock, including their exclusive ends,
//! quote/backslash handling and Word vs WORD classes.
//!
//! # Line breaks
//!
//! A character-wise region can never start or end on a line break here: a
//! start on the NUL position at the end of a line moves to the first character
//! of the next line, and an inclusive end there (Vim would include the line
//! break) stops on the last character of the line.  Line breaks *inside* a
//! multi-line region are included, matching how Vim joins them into a
//! character-wise register.

use rvim_core::charclass;

/// A text-object region.
///
/// start_col is the byte offset of the first character; end_col is the byte
/// offset of the last byte of the last character (inclusive).  When the cursor
/// is on an empty line a region may be empty, in which case both positions
/// name the same (zero-width) spot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRange {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub linewise: bool,
}

/// Which flavour of an object to select: 'i' (inner) or 'a' (around).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Inner,
    Around,
}

/// Vim's NUL character (the position just past the last character of a line).
const NUL: char = '\0';

/// Default 'paragraphs' option (current_par consults it through startPS).
const P_PARA: &str = "IPLPPPQPP TPHPLIPpLpItpplpipbp";
/// Default 'sections' option.
const P_SECTIONS: &str = "SHNHH HUnhsh";

/// How far current_block may search backwards for an opening bracket.
const MAX_BLOCK_BACK_LINES: usize = 100;
/// How far it/at may search for tags.
const MAX_TAG_LINES: usize = 500;

/// Select the text object obj (w W s p ( ) b [ ] { } B < > t " ' and the
/// backtick variant) at cursor ((line, byte_col), zero based).
///
/// Returns None when Vim's object would fail (no block around or after the
/// cursor, no quoted string on the line, no matching tag, ...).
pub fn text_object(
    lines: &[String],
    cursor: (usize, usize),
    obj: char,
    which: Which,
) -> Option<TextRange> {
    let buf = Buf::new(lines);
    let pos = buf.clamp_cursor(cursor);
    let raw = match obj {
        'w' => buf.word_object(pos, false, which)?,
        'W' => buf.word_object(pos, true, which)?,
        's' => buf.sentence_object(pos, which)?,
        'p' => buf.paragraph_object(pos, which)?,
        '(' | ')' | 'b' => buf.block_object(pos, which, '(', ')')?,
        '[' | ']' => buf.block_object(pos, which, '[', ']')?,
        '{' | '}' | 'B' => buf.block_object(pos, which, '{', '}')?,
        '<' | '>' => buf.block_object(pos, which, '<', '>')?,
        '"' | '\'' | '\u{0060}' => buf.quote_object(pos, which, obj)?,
        't' => buf.tag_object(pos, which)?,
        _ => return None,
    };
    buf.to_range(raw)
}

/// A character position: line index plus index into that line's characters.
/// col may equal the line length, Vim's NUL position past the last char.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pos {
    line: usize,
    col: usize,
}

impl Pos {
    fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// Raw object result before exclusive ends and NUL positions are resolved.
#[derive(Clone, Copy, Debug)]
struct Raw {
    start: Pos,
    end: Pos,
    inclusive: bool,
    linewise: bool,
}

fn is_white(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// True for the punctuation characters findsent() treats as sentence ends.
fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

/// True for characters that may follow a sentence end (quotes, brackets).
fn is_sentence_close(c: char) -> bool {
    matches!(c, ')' | ']' | '"' | '\'')
}

/// Characters findsent() skips when scanning backwards for a sentence start.
fn is_sentence_skip(c: char) -> bool {
    is_sentence_end(c) || is_sentence_close(c)
}

/// Vim's inmacro(): does line text s start with a two-character macro from
/// the option string opt?  A space in opt matches a space or end of line.
fn in_macro(opt: &str, s: &[char]) -> bool {
    let macro_chars: Vec<char> = opt.chars().collect();
    let s0 = s.first().copied().unwrap_or(NUL);
    let s1 = s.get(1).copied().unwrap_or(NUL);
    let mut i = 0;
    while i < macro_chars.len() && macro_chars[i] != NUL {
        let m0 = macro_chars[i];
        let m1 = macro_chars.get(i + 1).copied().unwrap_or(NUL);
        if (m0 == s0 || (m0 == ' ' && (s0 == NUL || s0 == ' ')))
            && (m1 == s1 || ((m1 == NUL || m1 == ' ') && (s0 == NUL || s1 == NUL || s1 == ' ')))
        {
            return true;
        }
        i += 2;
    }
    false
}

/// The line buffer together with per-line character and byte-offset tables.
struct Buf {
    chars: Vec<Vec<char>>,
    /// For each line: the byte offset of every character start, plus the line
    /// length as the final entry.
    byte_of: Vec<Vec<usize>>,
}

impl Buf {
    fn new(lines: &[String]) -> Self {
        if lines.is_empty() {
            return Self {
                chars: vec![Vec::new()],
                byte_of: vec![vec![0]],
            };
        }
        let mut chars = Vec::with_capacity(lines.len());
        let mut byte_of = Vec::with_capacity(lines.len());
        for line in lines {
            let mut offs: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
            offs.push(line.len());
            chars.push(line.chars().collect());
            byte_of.push(offs);
        }
        Self { chars, byte_of }
    }

    fn nlines(&self) -> usize {
        self.chars.len()
    }

    fn len(&self, line: usize) -> usize {
        self.chars[line].len()
    }

    fn is_empty_line(&self, line: usize) -> bool {
        self.chars[line].is_empty()
    }

    /// Character under p, or NUL when p is on the position past the line.
    fn ch(&self, p: Pos) -> char {
        self.chars[p.line].get(p.col).copied().unwrap_or(NUL)
    }

    /// Clamp a (line, byte_col) cursor onto a real character.  A column past
    /// the end of a non-empty line lands on the last character; a byte in the
    /// middle of a multi-byte character snaps back to its first byte.
    fn clamp_cursor(&self, cursor: (usize, usize)) -> Pos {
        let line = cursor.0.min(self.nlines() - 1);
        let offs = &self.byte_of[line];
        let nchars = offs.len() - 1;
        let count = offs[..nchars].partition_point(|&b| b <= cursor.1);
        let mut col = count.saturating_sub(1);
        if nchars > 0 && col >= nchars {
            col = nchars - 1;
        }
        Pos::new(line, col)
    }

    // --- character classes --------------------------------------------------

    /// Vim's cls(): 0 for white space/NUL, 1 for punctuation or (WORD mode)
    /// any non-blank, 2 for keyword characters.
    fn cls(&self, p: Pos, bigword: bool) -> u8 {
        let c = self.ch(p);
        if c == NUL || is_white(c) {
            return 0;
        }
        if bigword {
            return 1;
        }
        if charclass::is_word_char(c) { 2 } else { 1 }
    }

    fn linewhite(&self, line: usize) -> bool {
        self.chars[line].iter().all(|c| is_white(*c))
    }

    /// startPS(lnum, NUL, FALSE) with the default 'paragraphs'/'sections'.
    fn start_ps(&self, line: usize) -> bool {
        let text = &self.chars[line];
        match text.first() {
            None => true,
            Some('\u{000c}') => true,
            Some('.') => in_macro(P_SECTIONS, &text[1..]) || in_macro(P_PARA, &text[1..]),
            Some(_) => false,
        }
    }

    /// inindent(extra): is the cursor before/at the first non-blank?
    fn inindent(&self, p: Pos, extra: usize) -> bool {
        let indent = self.chars[p.line]
            .iter()
            .take_while(|c| is_white(**c))
            .count();
        indent >= p.col + extra
    }

    // --- cursor movement (misc2.c inc/dec/incl/decl) ------------------------

    /// inc(): 0 within the line, 2 on landing on the NUL, 1 when crossing to
    /// the next line and -1 at end of file.
    fn inc(&self, p: &mut Pos) -> i32 {
        if p.col < self.len(p.line) {
            p.col += 1;
            if p.col == self.len(p.line) { 2 } else { 0 }
        } else if p.line + 1 < self.nlines() {
            p.line += 1;
            p.col = 0;
            1
        } else {
            -1
        }
    }

    /// dec(): 0 within the line, 1 when crossing to the previous line and
    /// -1 at the start of the file.
    fn dec(&self, p: &mut Pos) -> i32 {
        if p.col > 0 {
            p.col -= 1;
            0
        } else if p.line > 0 {
            p.line -= 1;
            p.col = self.len(p.line);
            1
        } else {
            -1
        }
    }

    /// incl(): like inc() but skip the NUL at the end of a line.
    fn incl(&self, p: &mut Pos) -> i32 {
        let mut r = self.inc(p);
        if r >= 1 && p.col != 0 {
            r = self.inc(p);
        }
        r
    }

    /// decl(): like dec() but skip the NUL at the end of a line.
    fn decl(&self, p: &mut Pos) -> i32 {
        let mut r = self.dec(p);
        if r == 1 && p.col != 0 {
            r = self.dec(p);
        }
        r
    }

    fn step(&self, p: &mut Pos, forward: bool) -> i32 {
        if forward { self.incl(p) } else { self.decl(p) }
    }

    /// oneleft(): move one character left inside the line.
    fn oneleft(&self, p: &mut Pos) -> bool {
        if p.col == 0 {
            false
        } else {
            p.col -= 1;
            true
        }
    }

    // --- word motions (textobject.c) ----------------------------------------

    /// skip_chars(): skip a run of class cclass; true when EOF was hit.
    fn skip_chars(&self, p: &mut Pos, cclass: u8, forward: bool, bigword: bool) -> bool {
        while self.cls(*p, bigword) == cclass {
            let r = if forward { self.inc(p) } else { self.dec(p) };
            if r == -1 {
                return true;
            }
        }
        false
    }

    /// fwd_word(): move forward one word; false at end of file.
    fn fwd_word(&self, p: &mut Pos, count: usize, bigword: bool, eol: bool) -> bool {
        let mut count = count;
        while count > 0 {
            count -= 1;
            let sclass = self.cls(*p, bigword);
            let last_line = p.line == self.nlines() - 1;
            let i = self.inc(p);
            if i == -1 || (i >= 1 && last_line) {
                return false;
            }
            if i >= 1 && eol && count == 0 {
                return true;
            }
            if sclass != 0 {
                while self.cls(*p, bigword) == sclass {
                    let i = self.inc(p);
                    if i == -1 || (i >= 1 && eol && count == 0) {
                        return true;
                    }
                }
            }
            while self.cls(*p, bigword) == 0 {
                if p.col == 0 && self.is_empty_line(p.line) {
                    break;
                }
                let i = self.inc(p);
                if i == -1 || (i >= 1 && eol && count == 0) {
                    return true;
                }
            }
        }
        true
    }

    /// end_word(): move to the end of a word; false at end of file.
    fn end_word(&self, p: &mut Pos, count: usize, bigword: bool, stop: bool, empty: bool) -> bool {
        let mut count = count;
        let mut stop = stop;
        while count > 0 {
            count -= 1;
            let sclass = self.cls(*p, bigword);
            if self.inc(p) == -1 {
                return false;
            }
            let mut finished = false;
            if self.cls(*p, bigword) == sclass && sclass != 0 {
                if self.skip_chars(p, sclass, true, bigword) {
                    return false;
                }
            } else if !stop || sclass == 0 {
                loop {
                    if self.cls(*p, bigword) != 0 {
                        break;
                    }
                    if empty && p.col == 0 && self.is_empty_line(p.line) {
                        finished = true;
                        break;
                    }
                    if self.inc(p) == -1 {
                        return false;
                    }
                }
                if !finished {
                    let sclass = self.cls(*p, bigword);
                    if self.skip_chars(p, sclass, true, bigword) {
                        return false;
                    }
                }
            }
            if !finished {
                self.dec(p);
            }
            stop = false;
        }
        true
    }

    /// back_in_line(): back to the start of the word/white-space run.
    fn back_in_line(&self, p: &mut Pos, bigword: bool) {
        let sclass = self.cls(*p, bigword);
        loop {
            if p.col == 0 {
                break;
            }
            self.dec(p);
            if self.cls(*p, bigword) != sclass {
                self.inc(p);
                break;
            }
        }
    }

    /// find_first_blank(): back to the first blank before the cursor.
    fn find_first_blank(&self, p: &mut Pos) {
        while self.decl(p) != -1 {
            if !is_white(self.ch(*p)) {
                self.incl(p);
                break;
            }
        }
    }

    // --- current_word -------------------------------------------------------

    /// current_word() for iw/aw/iW/aW.
    fn word_object(&self, cursor: Pos, bigword: bool, which: Which) -> Option<Raw> {
        let include = which == Which::Around;
        let mut p = cursor;
        self.back_in_line(&mut p, bigword);
        let mut start = p;
        let mut include_white = false;
        if (self.cls(p, bigword) == 0) == include {
            if !self.end_word(&mut p, 1, bigword, true, true) {
                return None;
            }
        } else {
            // Like vim, ignore the failure: the cursor is wherever the motion
            // stopped and the object is built from that.
            self.fwd_word(&mut p, 1, bigword, true);
            if p.col == 0 {
                self.decl(&mut p);
            } else {
                self.oneleft(&mut p);
            }
            include_white = include;
        }
        // With count 1 the motion is always inclusive, so only the "no white
        // space at the end" arm of vim's test can fire.
        if include_white && self.cls(p, bigword) != 0 {
            let save = p;
            p = start;
            if self.oneleft(&mut p) {
                self.back_in_line(&mut p, bigword);
                if self.cls(p, bigword) == 0 && p.col > 0 {
                    start = p;
                }
            }
            p = save;
        }
        Some(Raw {
            start,
            end: p,
            inclusive: true,
            linewise: false,
        })
    }

    // --- current_block ------------------------------------------------------

    /// Count the backslashes immediately before p on its line.
    fn backslashes_before(&self, p: Pos) -> usize {
        let chars = &self.chars[p.line];
        let mut n = 0;
        while p.col > n && chars[p.col - n - 1] == '\\' {
            n += 1;
        }
        n
    }

    /// findmatchlimit() bracket search with 'cpo' containing '%': quotes are
    /// not special and a bracket preceded by an odd number of backslashes is
    /// skipped.  incr is counted, ret is the character returned when the
    /// nesting count reaches zero.
    fn scan_brackets(
        &self,
        from: Pos,
        incr: char,
        ret: char,
        back: bool,
        max_back_lines: Option<usize>,
    ) -> Option<Pos> {
        let mut p = from;
        let mut count = 0usize;
        let start_line = from.line;
        loop {
            if back {
                if p.col == 0 {
                    if p.line == 0 {
                        return None;
                    }
                    p.line -= 1;
                    p.col = self.len(p.line);
                    if let Some(max) = max_back_lines {
                        if start_line - p.line > max {
                            return None;
                        }
                    }
                } else {
                    p.col -= 1;
                }
            } else if p.col >= self.len(p.line) {
                if p.line + 1 >= self.nlines() {
                    return None;
                }
                p.line += 1;
                p.col = 0;
            } else {
                p.col += 1;
            }
            let c = self.ch(p);
            if self.backslashes_before(p) % 2 == 1 {
                continue;
            }
            if c == incr {
                count += 1;
            } else if c == ret {
                if count == 0 {
                    return Some(p);
                }
                count -= 1;
            }
        }
    }

    /// findmatch(NULL, initc) with 'matchpairs' = "(:),{:},[:],<:>".
    fn findmatch(&self, from: Pos, initc: char, max_back_lines: Option<usize>) -> Option<Pos> {
        let (open, close) = match initc {
            '(' | ')' => ('(', ')'),
            '[' | ']' => ('[', ']'),
            '{' | '}' => ('{', '}'),
            '<' | '>' => ('<', '>'),
            _ => return None,
        };
        if initc == open {
            self.scan_brackets(from, close, open, true, max_back_lines)
        } else {
            self.scan_brackets(from, open, close, false, None)
        }
    }

    /// current_block() for i(/a( and friends.
    fn block_object(&self, cursor: Pos, which: Which, open: char, close: char) -> Option<Raw> {
        let include = which == Which::Around;
        let mut p = cursor;
        if open == '{' {
            while self.inindent(p, 1) {
                if self.inc(&mut p) != 0 {
                    break;
                }
            }
        }
        if self.ch(p) == open {
            p.col += 1;
        }
        // Backwards to the [count]'th unmatched opening bracket.
        let mut start = match self.findmatch(p, open, Some(MAX_BLOCK_BACK_LINES)) {
            Some(open_pos) => open_pos,
            // No opening bracket before the cursor: vim falls back to the
            // block that starts after it.
            None => self.scan_brackets(p, close, open, false, None)?,
        };
        let mut cur = self.scan_brackets(start, open, close, false, None)?;
        let mut sol = false;
        if !include {
            self.incl(&mut start);
            sol = cur.col == 0;
            self.decl(&mut cur);
            while self.inindent(cur, 1) {
                sol = true;
                if self.decl(&mut cur) != 0 {
                    break;
                }
            }
        }
        let mut inclusive = false;
        if sol {
            self.incl(&mut cur);
        } else if start <= cur {
            inclusive = true;
        } else {
            // End before start: no text between the brackets, operate on
            // nothing at the start position.
            cur = start;
        }
        Some(Raw {
            start,
            end: cur,
            inclusive,
            linewise: false,
        })
    }

    // --- current_quote ------------------------------------------------------

    /// find_next_quote(): next unescaped q at or after col.
    fn find_next_quote(
        chars: &[char],
        mut col: usize,
        q: char,
        escape: Option<char>,
    ) -> Option<usize> {
        loop {
            let c = *chars.get(col)?;
            if escape == Some(c) {
                col += 1;
                if col >= chars.len() {
                    return None;
                }
            } else if c == q {
                return Some(col);
            }
            col += 1;
        }
    }

    /// find_prev_quote(): previous unescaped q before col_start.
    fn find_prev_quote(chars: &[char], mut col_start: usize, q: char) -> usize {
        while col_start > 0 {
            col_start -= 1;
            let mut n = 0;
            while col_start > n && chars[col_start - n - 1] == '\\' {
                n += 1;
            }
            if n % 2 == 1 {
                // Odd number of escapes: this quote is escaped, skip it.
                col_start -= n;
            } else if chars[col_start] == q {
                break;
            }
        }
        col_start
    }

    /// current_quote() for i"/a" and the single-quote/backtick variants.
    fn quote_object(&self, cursor: Pos, which: Which, q: char) -> Option<Raw> {
        let include = which == Which::Around;
        let chars: &[char] = &self.chars[cursor.line];
        let escape = Some('\\');
        let mut col_start = cursor.col;
        let mut col_end;
        if chars.get(col_start).copied() == Some(q) {
            // Cursor on a quote: search from the start of the line to find out
            // whether it opens or closes the string under the cursor.
            let first_col = col_start;
            col_start = 0;
            loop {
                col_start = Self::find_next_quote(chars, col_start, q, None)?;
                if col_start > first_col {
                    return None;
                }
                col_end = Self::find_next_quote(chars, col_start + 1, q, escape)?;
                if col_start <= first_col && first_col <= col_end {
                    break;
                }
                col_start = col_end + 1;
            }
        } else {
            col_start = Self::find_prev_quote(chars, col_start, q);
            if chars.get(col_start).copied() != Some(q) {
                // No quote before the cursor, look after it.
                col_start = Self::find_next_quote(chars, col_start, q, None)?;
            }
            col_end = Self::find_next_quote(chars, col_start + 1, q, escape)?;
        }
        if include {
            let after = chars.get(col_end + 1).copied().unwrap_or(NUL);
            if is_white(after) {
                while is_white(chars.get(col_end + 1).copied().unwrap_or(NUL)) {
                    col_end += 1;
                }
            } else {
                while col_start > 0 && is_white(chars[col_start - 1]) {
                    col_start -= 1;
                }
            }
        } else {
            // Exclude the opening quote.
            col_start += 1;
        }
        let start = Pos::new(cursor.line, col_start);
        let mut end = Pos::new(cursor.line, col_end);
        let mut inclusive = false;
        if include && self.inc(&mut end) == 2 {
            // Closing quote in the last column: include the line break (which
            // resolves back onto the quote).
            inclusive = true;
        }
        Some(Raw {
            start,
            end,
            inclusive,
            linewise: false,
        })
    }

    // --- current_par --------------------------------------------------------

    /// current_par() for ip/ap, always linewise.
    fn paragraph_object(&self, cursor: Pos, which: Which) -> Option<Raw> {
        let include = which == Which::Around;
        let n = self.nlines() as isize;
        let mut start = cursor.line as isize;
        let white_in_front = self.linewhite(cursor.line);
        // Back to the first line of the paragraph (or blank run).
        while start > 0 {
            if white_in_front {
                if !self.linewhite((start - 1) as usize) {
                    break;
                }
            } else if self.linewhite((start - 1) as usize) || self.start_ps(start as usize) {
                break;
            }
            start -= 1;
        }
        // Past the end of any blank lines.  When the start line is not blank
        // this deliberately lands one line early; the loop below moves it on.
        let mut end = start;
        while end < n && self.linewhite(end as usize) {
            end += 1;
        }
        end -= 1;
        let mut i = if !include && white_in_front { 0 } else { 1 };
        while i > 0 {
            i -= 1;
            if end == n - 1 {
                return None;
            }
            let mut do_white = false;
            if !include {
                do_white = self.linewhite((end + 1) as usize);
            }
            if include || !do_white {
                end += 1;
                while end < n - 1
                    && !self.linewhite((end + 1) as usize)
                    && !self.start_ps((end + 1) as usize)
                {
                    end += 1;
                }
            }
            if i == 0 && white_in_front && include {
                break;
            }
            if include || do_white {
                while end < n - 1 && self.linewhite((end + 1) as usize) {
                    end += 1;
                }
            }
        }
        if !white_in_front && !self.linewhite(end as usize) && include {
            while start > 0 && self.linewhite((start - 1) as usize) {
                start -= 1;
            }
        }
        Some(Raw {
            start: Pos::new(start as usize, 0),
            end: Pos::new(end as usize, 0),
            inclusive: true,
            linewise: true,
        })
    }

    // --- current_sent -------------------------------------------------------

    /// findsent(): move to the start of the next (forward) or previous
    /// sentence; None when there is none.
    fn findsent(&self, cursor: Pos, forward: bool, count: usize) -> Option<Pos> {
        let mut pos = cursor;
        let mut count = count;
        while count > 0 {
            count -= 1;
            let prev_pos = pos;
            let mut jumped = false;
            if self.ch(pos) == NUL {
                // On an empty line: skip to a non-empty one.
                loop {
                    if self.step(&mut pos, forward) == -1 {
                        break;
                    }
                    if self.ch(pos) != NUL {
                        break;
                    }
                }
                if forward {
                    jumped = true;
                }
            } else if forward && pos.col == 0 && self.start_ps(pos.line) {
                // At the start of a paragraph or section.
                if pos.line == self.nlines() - 1 {
                    return None;
                }
                pos.line += 1;
                jumped = true;
            } else if !forward {
                self.decl(&mut pos);
            }
            let mut noskip = false;
            if !jumped {
                // Back to the previous non-white non-punctuation character.
                let mut found_dot = false;
                loop {
                    let c = self.ch(pos);
                    if !(is_white(c) || is_sentence_skip(c)) {
                        break;
                    }
                    let mut tpos = pos;
                    if self.decl(&mut tpos) == -1 || (self.is_empty_line(tpos.line) && forward) {
                        break;
                    }
                    if found_dot {
                        break;
                    }
                    if is_sentence_end(c) {
                        found_dot = true;
                    }
                    if is_sentence_close(c) && !is_sentence_skip(self.ch(tpos)) {
                        break;
                    }
                    self.decl(&mut pos);
                }
                let start_line = pos.line;
                loop {
                    // Find the end of the sentence.
                    let c = self.ch(pos);
                    if c == NUL || (pos.col == 0 && self.start_ps(pos.line)) {
                        if !forward && pos.line != start_line && pos.line + 1 < self.nlines() {
                            pos.line += 1;
                        }
                        break;
                    }
                    if is_sentence_end(c) {
                        let mut tpos = pos;
                        let mut c2: i32;
                        loop {
                            if self.inc(&mut tpos) == -1 {
                                c2 = -1;
                                break;
                            }
                            c2 = self.ch(tpos) as i32;
                            if !is_sentence_close(self.ch(tpos)) {
                                break;
                            }
                        }
                        // 'cpo' does not contain 'J' by default, so a single
                        // space or tab after the punctuation ends the sentence.
                        if c2 == -1 || c2 == ' ' as i32 || c2 == '\t' as i32 || c2 == 0 {
                            pos = tpos;
                            if self.ch(pos) == NUL {
                                // Skip the NUL at the end of the line.
                                self.inc(&mut pos);
                            }
                            break;
                        }
                    }
                    if self.step(&mut pos, forward) == -1 {
                        if count > 0 {
                            return None;
                        }
                        noskip = true;
                        break;
                    }
                }
            }
            // Skip white space (always forwards, even for a backward search).
            while !noskip && is_white(self.ch(pos)) {
                if self.incl(&mut pos) == -1 {
                    break;
                }
            }
            if pos == prev_pos {
                // Didn't move: advance one character and try again.
                if self.step(&mut pos, forward) == -1 {
                    if count > 0 {
                        return None;
                    }
                    break;
                }
                count += 1;
            }
        }
        Some(pos)
    }

    /// findsent_forward(): skip count sentences and separators.
    fn findsent_forward(&self, pos: &mut Pos, count: usize, at_start_sent: bool) {
        let mut count = count;
        let mut at_start_sent = at_start_sent;
        while count > 0 {
            count -= 1;
            if let Some(p) = self.findsent(*pos, true, 1) {
                *pos = p;
            }
            if at_start_sent {
                self.find_first_blank(pos);
            }
            if count == 0 || at_start_sent {
                self.decl(pos);
            }
            at_start_sent = !at_start_sent;
        }
    }

    /// current_sent() for is/as.
    ///
    /// Vim keeps two positions here: the local `pos` (the original cursor,
    /// moved forward over white space) and `curwin->w_cursor` (moved by
    /// findsent()).  Comparing the two tells it whether the cursor started in
    /// the white space before the next sentence.
    fn sentence_object(&self, cursor: Pos, which: Which) -> Option<Raw> {
        let include = which == Which::Around;
        let mut start_pos = cursor;
        let mut pos = cursor;
        let mut cur = cursor;
        if let Some(p) = self.findsent(cur, true, 1) {
            cur = p;
        }
        while is_white(self.ch(pos)) {
            if self.incl(&mut pos) == -1 {
                break;
            }
        }
        let start_blank = if pos == cur {
            self.find_first_blank(&mut start_pos);
            true
        } else {
            if let Some(p) = self.findsent(cur, false, 1) {
                cur = p;
            }
            start_pos = cur;
            false
        };
        let ncount = if include {
            2
        } else if start_blank {
            0
        } else {
            1
        };
        if ncount > 0 {
            self.findsent_forward(&mut cur, ncount, true);
        } else {
            self.decl(&mut cur);
        }
        if include {
            if start_blank {
                self.find_first_blank(&mut cur);
                if is_white(self.ch(cur)) {
                    self.decl(&mut cur);
                }
            } else if !is_white(self.ch(cur)) {
                self.find_first_blank(&mut start_pos);
            }
        }
        let inclusive = self.incl(&mut cur) == -1;
        Some(Raw {
            start: start_pos,
            end: cur,
            inclusive,
            linewise: false,
        })
    }

    // --- current_tagblock ---------------------------------------------------

    /// Find the '>' of a tag at or after from (tags may span lines).
    fn find_gt(&self, from: Pos) -> Option<Pos> {
        let mut p = from;
        loop {
            if p.col < self.len(p.line) {
                if self.chars[p.line][p.col] == '>' {
                    return Some(p);
                }
                p.col += 1;
            } else if p.line + 1 < self.nlines() {
                p.line += 1;
                p.col = 0;
                if p.line - from.line > MAX_TAG_LINES {
                    return None;
                }
            } else {
                return None;
            }
        }
    }

    /// All <...>/</...> tags in the buffer, in buffer order.
    fn tags(&self) -> Vec<TagTok> {
        let mut out = Vec::new();
        for line in 0..self.nlines() {
            let chars = &self.chars[line];
            let mut i = 0;
            while i < chars.len() {
                if chars[i] != '<' {
                    i += 1;
                    continue;
                }
                let start = Pos::new(line, i);
                let Some(&next) = chars.get(i + 1) else {
                    i += 1;
                    continue;
                };
                if next == '/' {
                    // End tag: "</name>".
                    if let Some(gt) = self.find_gt(Pos::new(line, i + 2)) {
                        let name = self.chars_between(Pos::new(line, i + 2), gt);
                        out.push(TagTok {
                            start,
                            name,
                            end_tag: true,
                            self_closing: false,
                        });
                        i = if gt.line == line { gt.col + 1 } else { i + 1 };
                        continue;
                    }
                } else if !matches!(next, ' ' | '\t' | '>' | '!' | '/') {
                    // Start tag: "<name ...>" (a tag may also end at EOL).
                    let mut j = i + 1;
                    while j < chars.len() && !matches!(chars[j], ' ' | '\t' | '>') {
                        j += 1;
                    }
                    let name: Vec<char> = chars[i + 1..j].to_vec();
                    let (closed, next_i) = match self.find_gt(Pos::new(line, j)) {
                        Some(gt) => {
                            let before = self.pos_before(gt);
                            let closed = before.is_some_and(|b| self.ch(b) == '/');
                            (closed, if gt.line == line { gt.col + 1 } else { i + 1 })
                        }
                        None => (false, i + 1),
                    };
                    out.push(TagTok {
                        start,
                        name,
                        end_tag: false,
                        self_closing: closed,
                    });
                    i = next_i;
                    continue;
                }
                i += 1;
            }
        }
        out
    }

    fn chars_between(&self, start: Pos, end: Pos) -> Vec<char> {
        let mut out = Vec::new();
        let mut p = start;
        while p < end {
            if p.col < self.len(p.line) {
                out.push(self.chars[p.line][p.col]);
            }
            if self.inc(&mut p) == -1 {
                break;
            }
        }
        out
    }

    fn pos_before(&self, p: Pos) -> Option<Pos> {
        let mut q = p;
        if self.dec(&mut q) == -1 {
            None
        } else {
            Some(q)
        }
    }

    /// True when the cursor is on/inside a <...> tag (end_tag false) or a
    /// </...> tag (end_tag true).  Only the current line is inspected for the
    /// opening '<', like in_html_tag().
    fn in_html_tag(&self, p: Pos, end_tag: bool) -> bool {
        let chars = &self.chars[p.line];
        let mut i = p.col.min(chars.len());
        loop {
            if chars.get(i) == Some(&'<') {
                break;
            }
            if i == 0 {
                break;
            }
            i -= 1;
            if chars.get(i) == Some(&'>') {
                break;
            }
        }
        if chars.get(i) != Some(&'<') {
            return false;
        }
        let tag_start = Pos::new(p.line, i);
        if end_tag {
            return self.ch(Pos::new(p.line, i + 1)) == '/';
        }
        if self.ch(Pos::new(p.line, i + 1)) == '/' {
            return false;
        }
        // The matching '>' must not be preceded by '/'.
        let mut q = tag_start;
        let mut last = NUL;
        loop {
            if self.inc(&mut q) == -1 {
                return false;
            }
            let c = self.ch(q);
            if c == '>' {
                break;
            }
            last = c;
        }
        last != '/'
    }

    /// Search backwards for the first unmatched start tag before from.
    fn find_start_tag(&self, from: Pos, tags: &[TagTok]) -> Option<TagTok> {
        let mut nest = 0usize;
        for tok in tags.iter().rev() {
            if tok.start >= from {
                continue;
            }
            if from.line - tok.start.line > MAX_TAG_LINES {
                return None;
            }
            if tok.self_closing {
                continue;
            }
            if tok.end_tag {
                nest += 1;
            } else if nest == 0 {
                return Some(tok.clone());
            } else {
                nest -= 1;
            }
        }
        None
    }

    /// Search forward for the </name> matching start.
    fn find_end_tag(&self, start: &TagTok, tags: &[TagTok]) -> Option<TagTok> {
        let mut nest = 0usize;
        for tok in tags {
            if tok.start <= start.start {
                continue;
            }
            if tok.start.line - start.start.line > MAX_TAG_LINES {
                return None;
            }
            if tok.self_closing {
                continue;
            }
            if !tok.end_tag {
                if tag_name_matches(&tok.name, &start.name) {
                    nest += 1;
                }
            } else if names_equal_ci(&tok.name, &start.name) {
                if nest == 0 {
                    return Some(tok.clone());
                }
                nest -= 1;
            }
        }
        None
    }

    /// current_tagblock() for it/at.
    fn tag_object(&self, cursor: Pos, which: Which) -> Option<Raw> {
        let include = which == Which::Around;
        let mut old_end = cursor;
        self.decl(&mut old_end);
        let mut p = cursor;
        while self.inindent(p, 1) {
            if self.inc(&mut p) != 0 {
                break;
            }
        }
        if self.in_html_tag(p, false) {
            // Cursor on a start tag: move to its '>'.
            while self.ch(p) != '>' {
                if self.inc(&mut p) == -1 {
                    break;
                }
            }
        } else if self.in_html_tag(p, true) {
            // Cursor on an end tag: move to just before it.
            while self.ch(p) != '<' {
                if self.dec(&mut p) == -1 {
                    break;
                }
            }
            self.dec(&mut p);
            old_end = p;
        }
        let tags = self.tags();
        loop {
            let start_tok = self.find_start_tag(p, &tags)?;
            let mut start_pos = start_tok.start;
            if start_tok.name.is_empty() {
                return None;
            }
            match self.find_end_tag(&start_tok, &tags) {
                Some(end_tok) => {
                    p = end_tok.start;
                    if p < old_end {
                        // The end tag is before where we started: look for an
                        // earlier start tag (an HTML tag without its own end).
                        p = start_pos;
                        continue;
                    }
                }
                None => {
                    p = start_pos;
                    continue;
                }
            }
            let mut is_inclusive = true;
            if include {
                // Up to and including the '>' of the end tag.
                while self.ch(p) != '>' {
                    if self.inc(&mut p) == -1 {
                        break;
                    }
                }
            } else if self.ch(p) == '<' && p.col == 0 {
                // Closing tag on its own line: exclude the line break by
                // making the operation exclusive.
                is_inclusive = false;
            } else {
                self.dec(&mut p);
            }
            let end_pos = p;
            if !include {
                // Exclude the start tag.
                let mut q = start_pos;
                loop {
                    if self.inc(&mut q) == -1 {
                        break;
                    }
                    if self.ch(q) == '>' {
                        self.inc(&mut q);
                        start_pos = q;
                        break;
                    }
                }
            }
            // No text between the tags: vim operates on an empty area at the
            // start position.
            let (end_pos, is_inclusive) = if end_pos < start_pos {
                (start_pos, false)
            } else {
                (end_pos, is_inclusive)
            };
            return Some(Raw {
                start: start_pos,
                end: end_pos,
                inclusive: is_inclusive,
                linewise: false,
            });
        }
    }

    // --- result conversion --------------------------------------------------

    /// Byte offset of the last byte of the last character of line (0 for an
    /// empty line).
    fn last_char_byte(&self, line: usize) -> usize {
        let n = self.len(line);
        if n == 0 { 0 } else { self.byte_of[line][n] - 1 }
    }

    fn to_range(&self, raw: Raw) -> Option<TextRange> {
        if raw.linewise {
            let end_line = raw.end.line;
            return Some(TextRange {
                start_line: raw.start.line,
                start_col: 0,
                end_line,
                end_col: self.last_char_byte(end_line),
                linewise: true,
            });
        }
        let mut start = raw.start;
        let mut end = raw.end;
        if !raw.inclusive && self.dec(&mut end) == -1 {
            end = start;
        }
        // A character-wise region cannot begin on a line break: a start on the
        // NUL past the end of a non-empty line moves to the next line.
        if start.col >= self.len(start.line) && self.len(start.line) > 0 {
            if start.line + 1 < self.nlines() {
                start = Pos::new(start.line + 1, 0);
            } else {
                start.col = self.len(start.line) - 1;
            }
        }
        // Likewise an end on a NUL means "just before the line break": drop
        // the break and end on the previous character.
        let mut guard = self.nlines() + 1;
        while end.col >= self.len(end.line) {
            if self.len(end.line) > 0 {
                end.col = self.len(end.line) - 1;
                break;
            }
            if end.line > start.line {
                end.line -= 1;
                end.col = self.len(end.line);
            } else {
                end = start;
                break;
            }
            guard -= 1;
            if guard == 0 {
                end = start;
                break;
            }
        }
        if end < start {
            end = start;
        }
        let start_col = self.byte_of[start.line][start.col.min(self.len(start.line))];
        let end_col = if end.col < self.len(end.line) {
            self.byte_of[end.line][end.col + 1] - 1
        } else {
            0
        };
        Some(TextRange {
            start_line: start.line,
            start_col,
            end_line: end.line,
            end_col,
            linewise: false,
        })
    }
}

/// A tag found by [Buf::tags].
#[derive(Clone, Debug)]
struct TagTok {
    start: Pos,
    name: Vec<char>,
    end_tag: bool,
    self_closing: bool,
}

fn names_equal_ci(a: &[char], b: &[char]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// Does the tag name found match the \<name\> pattern Vim builds?  The name
/// must match and be followed by a non-keyword character.
fn tag_name_matches(found: &[char], name: &[char]) -> bool {
    if found.len() < name.len() {
        return false;
    }
    if !found[..name.len()]
        .iter()
        .zip(name)
        .all(|(x, y)| x.eq_ignore_ascii_case(y))
    {
        return false;
    }
    found
        .get(name.len())
        .is_none_or(|c| !charclass::is_word_char(*c))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn inner(sl: usize, sc: usize, el: usize, ec: usize) -> Option<TextRange> {
        Some(TextRange {
            start_line: sl,
            start_col: sc,
            end_line: el,
            end_col: ec,
            linewise: false,
        })
    }

    fn linewise(sl: usize, el: usize, ec: usize) -> Option<TextRange> {
        Some(TextRange {
            start_line: sl,
            start_col: 0,
            end_line: el,
            end_col: ec,
            linewise: true,
        })
    }

    /// Text covered by a range, joining lines with newlines (Vim's charwise
    /// register view of the region).
    fn covered(lines: &[String], range: TextRange) -> String {
        if range.start_line == range.end_line {
            let line = &lines[range.start_line];
            if range.start_col >= line.len() || range.end_col < range.start_col {
                return String::new();
            }
            return line[range.start_col..=range.end_col].to_string();
        }
        let mut out = String::from(&lines[range.start_line][range.start_col..]);
        for line in &lines[range.start_line + 1..range.end_line] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        let last = &lines[range.end_line];
        if range.end_col < last.len() {
            out.push_str(&last[..=range.end_col]);
        }
        out
    }

    #[test]
    fn inner_word_selects_the_character_class_run() {
        let text = lines(&["foo bar baz"]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 2)
        );
        assert_eq!(
            text_object(&text, (0, 4), 'w', Which::Inner),
            inner(0, 4, 0, 6)
        );
        assert_eq!(
            text_object(&text, (0, 8), 'w', Which::Inner),
            inner(0, 8, 0, 10)
        );
        // Cursor on white space: "iw" selects the white space run.
        let space = lines(&["foo bar baz"]);
        assert_eq!(
            text_object(&space, (0, 3), 'w', Which::Inner),
            inner(0, 3, 0, 3)
        );
        assert_eq!(
            text_object(&lines(&["a b"]), (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 0)
        );
        // Punctuation is its own run.
        let text = lines(&["foo.bar baz"]);
        assert_eq!(
            text_object(&text, (0, 3), 'w', Which::Inner),
            inner(0, 3, 0, 3)
        );
    }

    #[test]
    fn around_word_includes_white_space() {
        let text = lines(&["foo bar baz"]);
        let r = text_object(&text, (0, 0), 'w', Which::Around).unwrap();
        assert_eq!(covered(&text, r), "foo ");
        let r = text_object(&text, (0, 4), 'w', Which::Around).unwrap();
        assert_eq!(covered(&text, r), "bar ");
        // Last word of the line: no trailing blanks, so leading ones are used.
        let r = text_object(&text, (0, 8), 'w', Which::Around).unwrap();
        assert_eq!(covered(&text, r), " baz");
        // Trailing blanks win when they exist.
        let text = lines(&["foo   bar"]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Around),
            inner(0, 0, 0, 5)
        );
        // Indentation is not pulled in at the start of a line.
        let text = lines(&["  foo"]);
        assert_eq!(
            text_object(&text, (0, 2), 'w', Which::Around),
            inner(0, 2, 0, 4)
        );
        // A line containing only white space.
        let text = lines(&["  "]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 1)
        );
    }

    #[test]
    fn around_word_fails_without_surrounding_white_space() {
        // Vim's fwd_word() hits the end of the file: "aw" fails.
        let text = lines(&["foo   "]);
        assert_eq!(text_object(&text, (0, 3), 'w', Which::Around), None);
        assert_eq!(text_object(&lines(&[""]), (0, 0), 'w', Which::Around), None);
        assert_eq!(
            text_object(&lines(&["  "]), (0, 0), 'w', Which::Around),
            None
        );
    }

    #[test]
    fn word_versus_big_word() {
        let text = lines(&["foo.bar baz"]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 2)
        );
        assert_eq!(
            text_object(&text, (0, 0), 'W', Which::Inner),
            inner(0, 0, 0, 6)
        );
        assert_eq!(
            text_object(&text, (0, 0), 'W', Which::Around),
            inner(0, 0, 0, 7)
        );
        // "iW" inside the WORD still selects the whole WORD.
        assert_eq!(
            text_object(&text, (0, 4), 'W', Which::Inner),
            inner(0, 0, 0, 6)
        );
    }

    #[test]
    fn words_across_lines_and_empty_lines() {
        let text = lines(&["foo", "", "bar"]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 2)
        );
        assert_eq!(
            text_object(&text, (2, 0), 'w', Which::Inner),
            inner(2, 0, 2, 2)
        );
        // Empty line: "iw" selects nothing there.
        assert_eq!(
            text_object(&lines(&[""]), (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 0)
        );
        // "aw" on a leading empty line takes the next word across the break.
        let text = lines(&["", "foo"]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 0)
        );
        let r = text_object(&text, (0, 0), 'w', Which::Around).unwrap();
        assert_eq!(r, inner(0, 0, 1, 2).unwrap());
        assert_eq!(covered(&text, r), "\nfoo");
        // A word ending its line without trailing blanks: no leading blanks to
        // take, so the word alone is selected.
        let text = lines(&["one", "two"]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Around),
            inner(0, 0, 0, 2)
        );
    }

    #[test]
    fn words_with_multibyte_text_and_tabs() {
        let text = lines(&["héllo wörld"]);
        // h(1) é(2) l l o: the last 'o' is byte 5.
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 5)
        );
        let r = text_object(&text, (0, 0), 'w', Which::Inner).unwrap();
        assert_eq!(covered(&text, r), "héllo");
        // Leading white space is included when nothing follows the word.
        assert_eq!(
            text_object(&text, (0, 1), 'w', Which::Around),
            inner(0, 0, 0, 6)
        );
        let text = lines(&["日本語 テスト"]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 8)
        );
        let r = text_object(&text, (0, 0), 'w', Which::Around).unwrap();
        assert_eq!(r, inner(0, 0, 0, 9).unwrap());
        assert_eq!(covered(&text, r), "日本語 ");
        // A tab is white space like a space.
        let text = lines(&["foo\tbar"]);
        assert_eq!(
            text_object(&text, (0, 0), 'w', Which::Around),
            inner(0, 0, 0, 3)
        );
    }

    #[test]
    fn inner_paren_blocks_skip_nesting() {
        let text = lines(&["a(b(c)d)e"]);
        assert_eq!(
            text_object(&text, (0, 4), '(', Which::Inner),
            inner(0, 4, 0, 4)
        );
        assert_eq!(
            text_object(&text, (0, 2), '(', Which::Inner),
            inner(0, 2, 0, 6)
        );
        let r = text_object(&text, (0, 2), '(', Which::Inner).unwrap();
        assert_eq!(covered(&text, r), "b(c)d");
        // Alias keys b, ) and the cursor sitting on a bracket.
        assert_eq!(
            text_object(&text, (0, 4), 'b', Which::Inner),
            inner(0, 4, 0, 4)
        );
        assert_eq!(
            text_object(&text, (0, 4), ')', Which::Inner),
            inner(0, 4, 0, 4)
        );
        let text = lines(&["foo(bar)baz"]);
        assert_eq!(
            text_object(&text, (0, 5), '(', Which::Inner),
            inner(0, 4, 0, 6)
        );
        assert_eq!(
            text_object(&text, (0, 3), '(', Which::Inner),
            inner(0, 4, 0, 6)
        );
        assert_eq!(
            text_object(&text, (0, 7), '(', Which::Inner),
            inner(0, 4, 0, 6)
        );
        // Cursor outside (before) the block: the next block is used.
        assert_eq!(
            text_object(&text, (0, 0), '(', Which::Inner),
            inner(0, 4, 0, 6)
        );
    }

    #[test]
    fn around_paren_blocks_and_multiline_ones() {
        let text = lines(&["foo(bar)baz"]);
        assert_eq!(
            text_object(&text, (0, 5), '(', Which::Around),
            inner(0, 3, 0, 7)
        );
        // White space around the block is not part of it.
        let text = lines(&["foo (bar) baz"]);
        assert_eq!(
            text_object(&text, (0, 5), '(', Which::Around),
            inner(0, 4, 0, 8)
        );
        // Multi-line block: the line break inside the block is covered.
        let text = lines(&["f(a,", "  b)"]);
        assert_eq!(
            text_object(&text, (1, 2), '(', Which::Inner),
            inner(0, 2, 1, 2)
        );
        let r = text_object(&text, (1, 2), '(', Which::Inner).unwrap();
        assert_eq!(covered(&text, r), "a,\n  b");
        // Inner block whose content starts and ends on line breaks collapses
        // onto the first/last character of the content.
        let text = lines(&["f(", "    arg", ")"]);
        assert_eq!(
            text_object(&text, (1, 4), '(', Which::Inner),
            inner(1, 0, 1, 6)
        );
        let r = text_object(&text, (1, 4), '(', Which::Around).unwrap();
        assert_eq!(r, inner(0, 1, 2, 0).unwrap());
        assert_eq!(covered(&text, r), "(\n    arg\n)");
    }

    #[test]
    fn around_brace_blocks_on_separate_lines() {
        let text = lines(&["if (a) {", "    b;", "}"]);
        // "i{" excludes the braces and their lines.
        assert_eq!(
            text_object(&text, (1, 4), '{', Which::Inner),
            inner(1, 0, 1, 5)
        );
        let r = text_object(&text, (1, 4), '{', Which::Inner).unwrap();
        assert_eq!(covered(&text, r), "    b;");
        assert_eq!(
            text_object(&text, (1, 4), 'B', Which::Inner),
            inner(1, 0, 1, 5)
        );
        // "a{" spans the braces and the intermediate lines.
        assert_eq!(
            text_object(&text, (1, 4), '{', Which::Around),
            inner(0, 7, 2, 0)
        );
        assert_eq!(
            text_object(&text, (1, 4), 'B', Which::Around),
            inner(0, 7, 2, 0)
        );
        let text = lines(&["{", "  x", "}"]);
        assert_eq!(
            text_object(&text, (1, 0), '{', Which::Inner),
            inner(1, 0, 1, 2)
        );
    }

    #[test]
    fn bracket_brace_and_angle_variants() {
        let text = lines(&["x[aa]y"]);
        assert_eq!(
            text_object(&text, (0, 3), '[', Which::Inner),
            inner(0, 2, 0, 3)
        );
        assert_eq!(
            text_object(&text, (0, 3), '[', Which::Around),
            inner(0, 1, 0, 4)
        );
        assert_eq!(
            text_object(&text, (0, 3), ']', Which::Inner),
            inner(0, 2, 0, 3)
        );
        let text = lines(&["x{aa}y"]);
        assert_eq!(
            text_object(&text, (0, 3), '{', Which::Inner),
            inner(0, 2, 0, 3)
        );
        assert_eq!(
            text_object(&text, (0, 3), 'B', Which::Inner),
            inner(0, 2, 0, 3)
        );
        assert_eq!(
            text_object(&text, (0, 3), 'B', Which::Around),
            inner(0, 1, 0, 4)
        );
        let text = lines(&["x<aa>y"]);
        assert_eq!(
            text_object(&text, (0, 3), '<', Which::Inner),
            inner(0, 2, 0, 3)
        );
        assert_eq!(
            text_object(&text, (0, 3), '<', Which::Around),
            inner(0, 1, 0, 4)
        );
        // Nested brackets: the innermost unmatched pair is used.
        let text = lines(&["[[x]]"]);
        assert_eq!(
            text_object(&text, (0, 2), '[', Which::Inner),
            inner(0, 2, 0, 2)
        );
    }

    #[test]
    fn empty_and_unmatched_blocks() {
        // No text between the brackets: zero-width at the start position.
        let text = lines(&["x()y"]);
        assert_eq!(
            text_object(&text, (0, 1), '(', Which::Inner),
            inner(0, 2, 0, 2)
        );
        let r = text_object(&text, (0, 1), '(', Which::Around).unwrap();
        assert_eq!(r, inner(0, 1, 0, 2).unwrap());
        assert_eq!(covered(&text, r), "()");
        // Cursor past the end of an unmatched opening bracket.
        assert_eq!(
            text_object(&lines(&["f(ab"]), (0, 2), '(', Which::Inner),
            None
        );
        assert_eq!(
            text_object(&lines(&["f(ab"]), (0, 2), ')', Which::Inner),
            None
        );
        let text = lines(&["if (a) {", "    b;", "}"]);
        assert_eq!(
            text_object(&text, (0, 4), '(', Which::Inner),
            inner(0, 4, 0, 4)
        );
    }

    #[test]
    fn quotes_double_single_and_backtick() {
        let text = lines(&["x \"abc\" y"]);
        assert_eq!(
            text_object(&text, (0, 4), '"', Which::Inner),
            inner(0, 3, 0, 5)
        );
        assert_eq!(
            text_object(&text, (0, 4), '"', Which::Around),
            inner(0, 2, 0, 7)
        );
        let r = text_object(&text, (0, 4), '"', Which::Around).unwrap();
        assert_eq!(covered(&text, r), "\"abc\" ");
        // Cursor on either quote selects the same string.
        assert_eq!(
            text_object(&text, (0, 2), '"', Which::Inner),
            inner(0, 3, 0, 5)
        );
        assert_eq!(
            text_object(&text, (0, 6), '"', Which::Inner),
            inner(0, 3, 0, 5)
        );
        let text = lines(&["x 'abc' y"]);
        assert_eq!(
            text_object(&text, (0, 4), '\'', Which::Inner),
            inner(0, 3, 0, 5)
        );
        assert_eq!(
            text_object(&text, (0, 4), '\'', Which::Around),
            inner(0, 2, 0, 7)
        );
        let text = lines(&["x \u{0060}abc\u{0060} y"]);
        assert_eq!(
            text_object(&text, (0, 4), '\u{0060}', Which::Inner),
            inner(0, 3, 0, 5)
        );
        // No quote on the line.
        assert_eq!(
            text_object(&lines(&["foo"]), (0, 1), '"', Which::Inner),
            None
        );
    }

    #[test]
    fn quotes_honour_escapes_and_white_space() {
        // An escaped quote does not end the string.
        let text = lines(&["a \"b\\\"c\" d"]);
        assert_eq!(
            text_object(&text, (0, 4), '"', Which::Inner),
            inner(0, 3, 0, 6)
        );
        let r = text_object(&text, (0, 4), '"', Which::Inner).unwrap();
        assert_eq!(covered(&text, r), "b\\\"c");
        assert_eq!(
            text_object(&text, (0, 4), '"', Which::Around),
            inner(0, 2, 0, 8)
        );
        // Trailing blanks after the closing quote belong to "a".
        let text = lines(&["x \"a\"  "]);
        assert_eq!(
            text_object(&text, (0, 4), '"', Which::Around),
            inner(0, 2, 0, 6)
        );
        // With no trailing blanks the leading ones are included...
        let text = lines(&["  \"a\" x"]);
        assert_eq!(
            text_object(&text, (0, 3), '"', Which::Around),
            inner(0, 2, 0, 5)
        );
        // ...but only when there is no trailing white space at all.
        let text = lines(&["x \"a\"y"]);
        assert_eq!(
            text_object(&text, (0, 3), '"', Which::Around),
            inner(0, 1, 0, 4)
        );
        // An empty string is a zero-width inner region.
        let text = lines(&["x \"\" y"]);
        assert_eq!(
            text_object(&text, (0, 3), '"', Which::Inner),
            inner(0, 3, 0, 3)
        );
        // Quotes at the very start/end of a line.
        let text = lines(&["\"abc\""]);
        assert_eq!(
            text_object(&text, (0, 0), '"', Which::Inner),
            inner(0, 1, 0, 3)
        );
        assert_eq!(
            text_object(&text, (0, 0), '"', Which::Around),
            inner(0, 0, 0, 4)
        );
    }

    #[test]
    fn paragraphs_inner_and_around() {
        let text = lines(&["aa", "bb", "", "cc", "dd"]);
        assert_eq!(
            text_object(&text, (0, 0), 'p', Which::Inner),
            linewise(0, 1, 1)
        );
        assert_eq!(
            text_object(&text, (4, 0), 'p', Which::Inner),
            linewise(3, 4, 1)
        );
        // "ap" takes the blank lines that follow the paragraph.
        assert_eq!(
            text_object(&text, (0, 0), 'p', Which::Around),
            linewise(0, 2, 0)
        );
        // Starting on a blank line: "ip" is the blank run, "ap" extends into
        // the next paragraph.
        assert_eq!(
            text_object(&text, (2, 0), 'p', Which::Inner),
            linewise(2, 2, 0)
        );
        assert_eq!(
            text_object(&text, (2, 0), 'p', Which::Around),
            linewise(2, 4, 1)
        );
        // No blank lines at all: the whole buffer is one paragraph.
        let text = lines(&["aa", "bb"]);
        assert_eq!(
            text_object(&text, (0, 0), 'p', Which::Inner),
            linewise(0, 1, 1)
        );
        assert_eq!(
            text_object(&text, (0, 0), 'p', Which::Around),
            linewise(0, 1, 1)
        );
    }

    #[test]
    fn paragraphs_with_leading_and_white_only_lines() {
        let text = lines(&["", "aa", "", "bb", ""]);
        assert_eq!(
            text_object(&text, (1, 0), 'p', Which::Inner),
            linewise(1, 1, 1)
        );
        assert_eq!(
            text_object(&text, (1, 0), 'p', Which::Around),
            linewise(1, 2, 0)
        );
        // A white-space only line is a paragraph boundary too.
        let text = lines(&["aa", "  ", "bb"]);
        assert_eq!(
            text_object(&text, (0, 0), 'p', Which::Inner),
            linewise(0, 0, 1)
        );
        assert_eq!(
            text_object(&text, (0, 0), 'p', Which::Around),
            linewise(0, 1, 1)
        );
        // The only line is blank: "ap" fails (vim's current_par returns FAIL).
        assert_eq!(
            text_object(&lines(&[""]), (0, 0), 'p', Which::Inner),
            linewise(0, 0, 0)
        );
        assert_eq!(text_object(&lines(&[""]), (0, 0), 'p', Which::Around), None);
        // Runs of blank lines are kept together.
        let text = lines(&["", "", "aa", "", "", "bb"]);
        assert_eq!(
            text_object(&text, (4, 0), 'p', Which::Inner),
            linewise(3, 4, 0)
        );
    }

    #[test]
    fn inner_sentences() {
        let text = lines(&["One. Two. Three."]);
        assert_eq!(
            text_object(&text, (0, 0), 's', Which::Inner),
            inner(0, 0, 0, 3)
        );
        assert_eq!(
            text_object(&text, (0, 5), 's', Which::Inner),
            inner(0, 5, 0, 8)
        );
        assert_eq!(
            text_object(&text, (0, 10), 's', Which::Inner),
            inner(0, 10, 0, 15)
        );
        // Cursor on a sentence end belongs to that sentence.
        assert_eq!(
            text_object(&text, (0, 3), 's', Which::Inner),
            inner(0, 0, 0, 3)
        );
        assert_eq!(
            text_object(&text, (0, 8), 's', Which::Inner),
            inner(0, 5, 0, 8)
        );
        // Cursor in the blanks before a sentence selects just the blanks.
        assert_eq!(
            text_object(&text, (0, 4), 's', Which::Inner),
            inner(0, 4, 0, 4)
        );
        // Sentences do not stop at the end of a line; the last sentence of a
        // line ends at the last character before the line break.
        let text = lines(&["First sentence.", "Second sentence."]);
        assert_eq!(
            text_object(&text, (0, 0), 's', Which::Inner),
            inner(0, 0, 0, 14)
        );
        assert_eq!(
            text_object(&text, (1, 0), 's', Which::Inner),
            inner(1, 0, 1, 15)
        );
        // Without punctuation a sentence runs to the end of the buffer.
        let text = lines(&["Hello", "World"]);
        assert_eq!(
            text_object(&text, (0, 0), 's', Which::Inner),
            inner(0, 0, 1, 4)
        );
        let r = text_object(&text, (0, 0), 's', Which::Inner).unwrap();
        assert_eq!(covered(&text, r), "Hello\nWorld");
        // "!" ends a sentence as well.
        let text = lines(&["Hi there!  Bye."]);
        assert_eq!(
            text_object(&text, (0, 0), 's', Which::Inner),
            inner(0, 0, 0, 8)
        );
        assert_eq!(
            text_object(&text, (0, 9), 's', Which::Inner),
            inner(0, 9, 0, 10)
        );
    }

    #[test]
    fn around_sentences_take_the_separator() {
        let text = lines(&["One. Two. Three."]);
        assert_eq!(
            text_object(&text, (0, 0), 's', Which::Around),
            inner(0, 0, 0, 4)
        );
        assert_eq!(
            text_object(&text, (0, 5), 's', Which::Around),
            inner(0, 5, 0, 9)
        );
        // From the white space before a sentence, the sentence is taken whole.
        assert_eq!(
            text_object(&text, (0, 4), 's', Which::Around),
            inner(0, 4, 0, 8)
        );
        let r = text_object(&text, (0, 4), 's', Which::Around).unwrap();
        assert_eq!(covered(&text, r), " Two.");
        let text = lines(&["a.", " b."]);
        assert_eq!(
            text_object(&text, (0, 0), 's', Which::Around),
            inner(0, 0, 1, 0)
        );
        let text = lines(&["First sentence.", "Second sentence."]);
        assert_eq!(
            text_object(&text, (1, 0), 's', Which::Around),
            inner(1, 0, 1, 15)
        );
    }

    #[test]
    fn inner_tag_blocks() {
        let text = lines(&["<a>foo</a>"]);
        assert_eq!(
            text_object(&text, (0, 1), 't', Which::Inner),
            inner(0, 3, 0, 5)
        );
        assert_eq!(
            text_object(&text, (0, 6), 't', Which::Inner),
            inner(0, 3, 0, 5)
        );
        // Cursor on the end tag.
        assert_eq!(
            text_object(&text, (0, 7), 't', Which::Inner),
            inner(0, 3, 0, 5)
        );
        // Nested tags: the innermost block wins.
        let text = lines(&["<div><p>hi</p></div>"]);
        assert_eq!(
            text_object(&text, (0, 9), 't', Which::Inner),
            inner(0, 8, 0, 9)
        );
        // On the outer tag the inner region still contains the nested tags.
        assert_eq!(
            text_object(&text, (0, 3), 't', Which::Inner),
            inner(0, 5, 0, 13)
        );
        // Tags may span lines (bounded to 500 lines).
        let text = lines(&["<div>", "  <p>hi</p>", "</div>"]);
        assert_eq!(
            text_object(&text, (1, 7), 't', Which::Inner),
            inner(1, 5, 1, 6)
        );
        // Multi-line content: the line breaks around it are dropped.
        let text = lines(&["<a>", "  text", "</a>"]);
        assert_eq!(
            text_object(&text, (1, 2), 't', Which::Inner),
            inner(1, 0, 1, 5)
        );
        let r = text_object(&text, (1, 2), 't', Which::Inner).unwrap();
        assert_eq!(covered(&text, r), "  text");
        // Empty block, self-closing tag and an unmatched tag.
        assert_eq!(
            text_object(&lines(&["<a></a>"]), (0, 0), 't', Which::Inner),
            inner(0, 3, 0, 3)
        );
        assert_eq!(
            text_object(&lines(&["<br/>"]), (0, 1), 't', Which::Inner),
            None
        );
        assert_eq!(
            text_object(&lines(&["<a>foo"]), (0, 3), 't', Which::Inner),
            None
        );
        // Tag names are matched case-insensitively; attributes are skipped.
        assert_eq!(
            text_object(&lines(&["<A>x</A>"]), (0, 3), 't', Which::Inner),
            inner(0, 3, 0, 3)
        );
        let text = lines(&["<a href=\"x\">t</a>"]);
        assert_eq!(
            text_object(&text, (0, 2), 't', Which::Inner),
            inner(0, 12, 0, 12)
        );
        // A tag without a matching end tag is skipped, the enclosing one used.
        let text = lines(&["<div><br>hi</div>"]);
        assert_eq!(
            text_object(&text, (0, 9), 't', Which::Inner),
            inner(0, 5, 0, 10)
        );
        let r = text_object(&text, (0, 9), 't', Which::Inner).unwrap();
        assert_eq!(covered(&text, r), "<br>hi");
    }

    #[test]
    fn around_tag_blocks() {
        let text = lines(&["<a>foo</a>"]);
        assert_eq!(
            text_object(&text, (0, 1), 't', Which::Around),
            inner(0, 0, 0, 9)
        );
        let text = lines(&["<div><p>hi</p></div>"]);
        assert_eq!(
            text_object(&text, (0, 9), 't', Which::Around),
            inner(0, 5, 0, 13)
        );
        let r = text_object(&text, (0, 9), 't', Which::Around).unwrap();
        assert_eq!(covered(&text, r), "<p>hi</p>");
        let text = lines(&["<a>", "  text", "</a>"]);
        assert_eq!(
            text_object(&text, (1, 2), 't', Which::Around),
            inner(0, 0, 2, 3)
        );
        let r = text_object(&text, (1, 2), 't', Which::Around).unwrap();
        assert_eq!(covered(&text, r), "<a>\n  text\n</a>");
        assert_eq!(
            text_object(&lines(&["<a></a>"]), (0, 0), 't', Which::Around),
            inner(0, 0, 0, 6)
        );
        let text = lines(&["<a><b>x</b></a>"]);
        assert_eq!(
            text_object(&text, (0, 3), 't', Which::Around),
            inner(0, 3, 0, 10)
        );
    }

    #[test]
    fn multibyte_blocks_and_edges() {
        let text = lines(&["日本語(テスト)です"]);
        assert_eq!(
            text_object(&text, (0, 4), 'w', Which::Inner),
            inner(0, 0, 0, 8)
        );
        let range = text_object(&text, (0, 4), '(', Which::Inner).unwrap();
        assert_eq!(Some(range), inner(0, 10, 0, 18));
        assert_eq!(covered(&text, range), "テスト");
        // Every reported column is a character boundary.
        let line = &text[0];
        assert!(line.is_char_boundary(range.start_col) && line.is_char_boundary(range.end_col + 1));
        // A block at the start of an indented line.
        let text = lines(&["  (x)"]);
        assert_eq!(
            text_object(&text, (0, 3), '(', Which::Inner),
            inner(0, 3, 0, 3)
        );
        assert_eq!(
            text_object(&text, (0, 3), '(', Which::Around),
            inner(0, 2, 0, 4)
        );
    }

    #[test]
    fn empty_buffer_and_out_of_range_cursors() {
        let objs = [
            'w', 'W', 's', 'p', '(', ')', 'b', '[', ']', '{', '}', 'B', '<', '>', 't', '"', '\'',
            '\u{0060}',
        ];
        let buffers = [
            Vec::new(),
            vec![String::new()],
            lines(&["foo", "bar"]),
            lines(&["日本語"]),
        ];
        for obj in objs {
            for which in [Which::Inner, Which::Around] {
                for text in &buffers {
                    for cursor in [(0, 0), (usize::MAX, usize::MAX), (1, 99), (99, 0)] {
                        if let Some(r) = text_object(text, cursor, obj, which) {
                            let n = text.len().max(1);
                            assert!(r.start_line < n && r.end_line < n, "{obj} {r:?}");
                            assert!(r.start_line <= r.end_line, "{obj} {r:?}");
                        }
                    }
                }
            }
        }
        // An empty buffer behaves like a single empty line.
        assert_eq!(
            text_object(&[], (0, 0), 'w', Which::Inner),
            inner(0, 0, 0, 0)
        );
        assert_eq!(
            text_object(&[], (0, 0), 'p', Which::Inner),
            linewise(0, 0, 0)
        );
        // A column inside a multi-byte character snaps back to its start.
        let text = lines(&["日本語"]);
        assert_eq!(
            text_object(&text, (0, 1), 'w', Which::Inner),
            inner(0, 0, 0, 8)
        );
        // A column past the line end lands on the last character.
        let text = lines(&["abc"]);
        assert_eq!(
            text_object(&text, (0, 99), 'w', Which::Inner),
            inner(0, 0, 0, 2)
        );
    }
}
