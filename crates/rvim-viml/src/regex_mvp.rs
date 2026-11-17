//! A compact regular-expression matcher for the VimL operators `=~` / `!~`.
//!
//! This is a placeholder for the full `rvim-regex` engine (M6): it implements
//! the "magic" subset used by runtime scripts — literals, `.`, `*`, `\+`,
//! `\?`, `\=`, `\{n,m}`, `^`, `$`, character classes (ranges, negation and
//! `\d \w \s \D \W \S`), non-capturing groups `\(...\)` and alternation `\|`.
//! There are no back-references or assertions yet; documented in
//! docs/compat_deviations.md.

#[derive(Clone, Debug)]
enum Node {
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool),
    Start,
    End,
    Group(Vec<Vec<Node>>),
    Repeat(Box<Node>, u32, u32),
}

#[derive(Clone, Debug)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Digit,
    NotDigit,
    Word,
    NotWord,
    Space,
    NotSpace,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn class_match(items: &[ClassItem], negated: bool, c: char) -> bool {
    let mut hit = false;
    for item in items {
        let m = match item {
            ClassItem::Char(x) => *x == c,
            ClassItem::Range(a, b) => *a <= c && c <= *b,
            ClassItem::Digit => c.is_ascii_digit(),
            ClassItem::NotDigit => !c.is_ascii_digit(),
            ClassItem::Word => is_word_char(c),
            ClassItem::NotWord => !is_word_char(c),
            ClassItem::Space => c.is_whitespace(),
            ClassItem::NotSpace => !c.is_whitespace(),
        };
        if m {
            hit = true;
        }
    }
    hit != negated
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    _src: &'a str,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            _src: src,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    /// Parse an alternation (`\|` at this level).
    fn parse_alt(&mut self) -> Vec<Vec<Node>> {
        let mut branches = vec![self.parse_concat()];
        while self.peek() == Some('\\') && self.peek_at(1) == Some('|') {
            self.pos += 2;
            branches.push(self.parse_concat());
        }
        branches
    }

    fn parse_concat(&mut self) -> Vec<Node> {
        let mut nodes = Vec::new();
        while let Some(c) = self.peek() {
            if c == '\\' && (self.peek_at(1) == Some('|') || self.peek_at(1) == Some(')')) {
                break;
            }
            match self.parse_atom() {
                Some(node) => {
                    let node = self.parse_quantifiers(node);
                    nodes.push(node);
                }
                None => break,
            }
        }
        nodes
    }

    fn parse_quantifiers(&mut self, mut node: Node) -> Node {
        loop {
            match (self.peek(), self.peek_at(1)) {
                (Some('*'), _) => {
                    self.bump();
                    node = Node::Repeat(Box::new(node), 0, u32::MAX);
                }
                (Some('\\'), Some('+')) => {
                    self.pos += 2;
                    node = Node::Repeat(Box::new(node), 1, u32::MAX);
                }
                (Some('\\'), Some('?')) => {
                    self.pos += 2;
                    node = Node::Repeat(Box::new(node), 0, 1);
                }
                (Some('\\'), Some('=')) => {
                    self.pos += 2;
                    node = Node::Repeat(Box::new(node), 0, 1);
                }
                (Some('\\'), Some('{')) => {
                    // \{n,m} / \{n} / \{,m} / \{n,}
                    let save = self.pos;
                    self.pos += 2;
                    let mut min = String::new();
                    let mut max = String::new();
                    while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
                        min.push(c);
                        self.bump();
                    }
                    let mut has_comma = false;
                    if self.peek() == Some(',') {
                        has_comma = true;
                        self.bump();
                        while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
                            max.push(c);
                            self.bump();
                        }
                    }
                    if self.peek() == Some('\\') && self.peek_at(1) == Some('}') {
                        self.pos += 2;
                        let lo: u32 = min.parse().unwrap_or(0);
                        let hi: u32 = if !has_comma {
                            lo
                        } else if max.is_empty() {
                            u32::MAX
                        } else {
                            max.parse().unwrap_or(lo)
                        };
                        node = Node::Repeat(Box::new(node), lo, hi);
                    } else {
                        self.pos = save;
                        break;
                    }
                }
                _ => break,
            }
        }
        node
    }

    fn parse_atom(&mut self) -> Option<Node> {
        let c = self.bump()?;
        // `\c` / `\C` are case flags handled by the caller; skip them here.
        if c == '\\' && matches!(self.peek(), Some('c') | Some('C')) {
            // Skip the case flag and continue with the next atom.
            self.bump();
            return self.parse_atom();
        }
        match c {
            '.' => Some(Node::Any),
            '^' => Some(Node::Start),
            '$' => Some(Node::End),
            '[' => Some(self.parse_class()),
            '\\' => {
                let e = self.bump()?;
                Some(match e {
                    '(' => {
                        let branches = self.parse_alt();
                        if self.peek() == Some('\\') && self.peek_at(1) == Some(')') {
                            self.pos += 2;
                        }
                        Node::Group(branches)
                    }
                    'd' => Node::Class(vec![ClassItem::Digit], false),
                    'D' => Node::Class(vec![ClassItem::Digit], true),
                    'w' => Node::Class(vec![ClassItem::Word], false),
                    'W' => Node::Class(vec![ClassItem::Word], true),
                    's' => Node::Class(vec![ClassItem::Space], false),
                    'S' => Node::Class(vec![ClassItem::Space], true),
                    'n' => Node::Char('\n'),
                    't' => Node::Char('\t'),
                    'r' => Node::Char('\r'),
                    'e' => Node::Char('\u{1b}'),
                    'a' => Node::Char('\u{7}'),
                    'f' => Node::Char('\u{c}'),
                    'x' => Node::Char(self.take_hex(2)),
                    'u' => Node::Char(self.take_hex(4)),
                    'U' => Node::Char(self.take_hex(8)),
                    other => Node::Char(other),
                })
            }
            other => Some(Node::Char(other)),
        }
    }

    fn take_hex(&mut self, max: usize) -> char {
        let mut digits = String::new();
        for _ in 0..max {
            if let Some(c) = self.peek().filter(|c| c.is_ascii_hexdigit()) {
                digits.push(c);
                self.bump();
            }
        }
        u32::from_str_radix(&digits, 16)
            .ok()
            .and_then(char::from_u32)
            .unwrap_or('?')
    }

    fn parse_class(&mut self) -> Node {
        let mut negated = false;
        if matches!(self.peek(), Some('^')) {
            negated = true;
            self.bump();
        }
        let mut items = Vec::new();
        // A leading ']' is a literal member.
        if self.peek() == Some(']') {
            items.push(ClassItem::Char(']'));
            self.bump();
        }
        while let Some(c) = self.peek() {
            if c == ']' {
                self.bump();
                break;
            }
            self.bump();
            let item = match c {
                '\\' => {
                    let e = self.bump().unwrap_or('\\');
                    match e {
                        'd' => ClassItem::Digit,
                        'D' => ClassItem::NotDigit,
                        'w' => ClassItem::Word,
                        'W' => ClassItem::NotWord,
                        's' => ClassItem::Space,
                        'S' => ClassItem::NotSpace,
                        'n' => ClassItem::Char('\n'),
                        't' => ClassItem::Char('\t'),
                        other => ClassItem::Char(other),
                    }
                }
                _ => {
                    if self.peek() == Some('-') && self.peek_at(1).is_some_and(|n| n != ']') {
                        self.bump();
                        let hi = self.bump().unwrap_or(c);
                        ClassItem::Range(c, hi)
                    } else {
                        ClassItem::Char(c)
                    }
                }
            };
            items.push(item);
        }
        Node::Class(items, negated)
    }
}

/// Compile a pattern (returns a list of alternation branches).
fn compile(pattern: &str) -> Vec<Vec<Node>> {
    let mut p = Parser::new(pattern);
    p.parse_alt()
}

fn match_nodes(nodes: &[Node], text: &[char], pos: usize, ci: bool) -> Option<usize> {
    if nodes.is_empty() {
        return Some(pos);
    }
    let (first, rest) = (&nodes[0], &nodes[1..]);
    match first {
        Node::Start => {
            if pos == 0 {
                match_nodes(rest, text, pos, ci)
            } else {
                None
            }
        }
        Node::End => {
            if pos == text.len() {
                match_nodes(rest, text, pos, ci)
            } else {
                None
            }
        }
        Node::Char(c) => {
            let t = text.get(pos)?;
            let hit = if ci {
                t.to_lowercase().eq(c.to_lowercase())
            } else {
                *t == *c
            };
            if hit {
                match_nodes(rest, text, pos + 1, ci)
            } else {
                None
            }
        }
        Node::Any => {
            let t = text.get(pos)?;
            if *t == '\n' {
                None
            } else {
                match_nodes(rest, text, pos + 1, ci)
            }
        }
        Node::Class(items, negated) => {
            let t = text.get(pos)?;
            if class_match(items, *negated, *t) {
                match_nodes(rest, text, pos + 1, ci)
            } else {
                None
            }
        }
        Node::Group(branches) => {
            for branch in branches {
                if let Some(end) = match_nodes(branch, text, pos, ci) {
                    if let Some(found) = match_nodes(rest, text, end, ci) {
                        return Some(found);
                    }
                }
            }
            None
        }
        Node::Repeat(inner, lo, hi) => {
            // Greedy with backtracking.
            let mut ends = vec![pos];
            let mut cur = pos;
            while (ends.len() as u32 - 1) < *hi {
                match match_nodes(std::slice::from_ref(inner.as_ref()), text, cur, ci) {
                    Some(next) if next > cur => {
                        cur = next;
                        ends.push(cur);
                    }
                    _ => break,
                }
            }
            while !ends.is_empty() && (ends.len() as u32 - 1) >= *lo {
                let candidate = *ends.last().unwrap();
                if let Some(found) = match_nodes(rest, text, candidate, ci) {
                    return Some(found);
                }
                ends.pop();
            }
            None
        }
    }
}

/// Find the first match; returns `(start, end)` in byte offsets.
pub fn find(pattern: &str, text: &str, ignore_case: bool) -> Option<(usize, usize)> {
    let ci = ignore_case || pattern.contains("\\c");
    let pattern = pattern.replace("\\c", "").replace("\\C", "");
    if pattern.is_empty() {
        return Some((0, 0));
    }
    let branches = compile(&pattern);
    let chars: Vec<char> = text.chars().collect();
    let mut starts = 0usize;
    loop {
        for branch in &branches {
            if let Some(end) = match_nodes(branch, &chars, starts, ci) {
                let start_byte: usize = chars[..starts].iter().map(|c| c.len_utf8()).sum();
                let end_byte: usize = chars[..end].iter().map(|c| c.len_utf8()).sum();
                return Some((start_byte, end_byte));
            }
        }
        if starts >= chars.len() {
            return None;
        }
        starts += 1;
    }
}

/// Boolean match for `=~` / `!~`.
pub fn is_match(pattern: &str, text: &str, ignore_case: bool) -> bool {
    find(pattern, text, ignore_case).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_anchors() {
        assert!(is_match("evim", "evim", false));
        assert!(!is_match("evim", "vim", false));
        assert!(is_match("^git", "gitcommit", false));
        assert!(!is_match("^git", "nogitcommit", false));
        assert!(is_match("commit$", "gitcommit", false));
        assert!(!is_match("commit$", "gitcommitx", false));
        assert!(is_match("xterm", "xterm-256color", false));
    }

    #[test]
    fn dot_star_and_classes() {
        assert!(is_match("a.c", "abc", false));
        assert!(is_match("ab*c", "ac", false));
        assert!(is_match("ab*c", "abbbc", false));
        assert!(is_match("[0-9]\\+", "file7", false));
        assert!(!is_match("[0-9]\\+", "file", false));
        assert!(is_match("\\d\\+", "v2", false));
        assert!(is_match("\\S\\+", "abc", false));
        assert!(is_match("[^a]", "b", false));
        assert!(!is_match("[^a]", "a", false));
    }

    #[test]
    fn groups_and_alternation() {
        assert!(is_match("\\%(a\\|b\\)c", "bc", false));
        assert!(is_match("\\(foo\\|bar\\)", "xxbarxx", false));
        assert!(!is_match("\\(foo\\|bar\\)", "xxbazxx", false));
        assert!(is_match("a\\{2,3\\}", "aaa", false));
        assert!(!is_match("a\\{2,3\\}", "a", false));
        assert!(is_match("a\\+", "aaa", false));
        assert!(is_match("ab\\=c", "ac", false));
    }

    #[test]
    fn case_awareness() {
        assert!(!is_match("Evim", "evim", false));
        assert!(is_match("Evim", "evim", true));
        assert!(is_match("\\cEVIM", "evim", false));
        // Character classes with case folding.
        assert!(!is_match("B", "a", false));
    }

    #[test]
    fn find_returns_byte_offsets() {
        assert_eq!(find("b", "abc", false), Some((1, 2)));
        assert_eq!(find("z", "abc", false), None);
        assert_eq!(find("^", "abc", false), Some((0, 0)));
        assert_eq!(find("", "abc", false), Some((0, 0)));
    }
}
