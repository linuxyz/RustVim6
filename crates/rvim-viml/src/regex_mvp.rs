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
    /// \< : start of a word (vimregex §4.1).
    WordStart,
    /// \> : end of a word.
    WordEnd,
    Group(Vec<Vec<Node>>),
    Repeat {
        node: Box<Node>,
        lo: u32,
        hi: u32,
        /// Greedy quantifiers (\{n,m}) prefer the longest match, the
        /// non-greedy \{-n,m} form the shortest (vimregex §4.3).
        greedy: bool,
    },
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
    // The rest of vim's predefined classes (vimregex §4.2).
    Hex,
    Octal,
    /// Head of a word: a letter or `_`.
    Head,
    /// Printable character.
    Printable,
    /// Printable but not a digit (vim's \P).
    NotPrintable,
    Alpha,
    Lower,
    Upper,
    /// File-name character: alphanumeric plus `._-~`.
    FileName,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_file_name_char(c: char) -> bool {
    c.is_alphanumeric() || "._-~".contains(c)
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
            ClassItem::Hex => c.is_ascii_hexdigit(),
            ClassItem::Octal => ('0'..='7').contains(&c),
            ClassItem::Head => c.is_alphabetic() || c == '_',
            ClassItem::Printable => !c.is_control(),
            ClassItem::NotPrintable => !c.is_control() && !c.is_ascii_digit(),
            ClassItem::Alpha => c.is_alphabetic(),
            ClassItem::Lower => c.is_lowercase(),
            ClassItem::Upper => c.is_uppercase(),
            ClassItem::FileName => is_file_name_char(c),
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
                    node = Node::Repeat {
                        node: Box::new(node),
                        lo: 0,
                        hi: u32::MAX,
                        greedy: true,
                    };
                }
                (Some('\\'), Some('+')) => {
                    self.pos += 2;
                    node = Node::Repeat {
                        node: Box::new(node),
                        lo: 1,
                        hi: u32::MAX,
                        greedy: true,
                    };
                }
                (Some('\\'), Some('?')) => {
                    self.pos += 2;
                    node = Node::Repeat {
                        node: Box::new(node),
                        lo: 0,
                        hi: 1,
                        greedy: true,
                    };
                }
                (Some('\\'), Some('=')) => {
                    self.pos += 2;
                    node = Node::Repeat {
                        node: Box::new(node),
                        lo: 0,
                        hi: 1,
                        greedy: true,
                    };
                }
                (Some('\\'), Some('{')) => {
                    // \{n,m} / \{n} / \{,m} / \{n,} (greedy) and the
                    // non-greedy \{-n,m} family (vimregex §4.3).
                    let save = self.pos;
                    self.pos += 2;
                    let greedy = self.peek() != Some('-');
                    if !greedy {
                        self.bump();
                    }
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
                            // A bare \{-} repeats zero or more times; \{-n} is
                            // exactly n (vimregex §4.3).
                            if min.is_empty() { u32::MAX } else { lo }
                        } else if max.is_empty() {
                            u32::MAX
                        } else {
                            max.parse().unwrap_or(lo)
                        };
                        node = Node::Repeat {
                            node: Box::new(node),
                            lo,
                            hi,
                            greedy,
                        };
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
        // \c / \C are case flags handled by the caller; skip them here.
        if c == '\\' && matches!(self.peek(), Some('c') | Some('C')) {
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
                    // \%(...\) : non-capturing group (vimregex §4.6).
                    '%' => {
                        let kind = self.bump()?;
                        match kind {
                            '(' => {
                                let branches = self.parse_alt();
                                if self.peek() == Some('\\') && self.peek_at(1) == Some(')') {
                                    self.pos += 2;
                                }
                                Node::Group(branches)
                            }
                            // \%d123 / \%o17 / \%x1f / \%u00e9 / \%U0001F600
                            'd' => Node::Char(self.take_radix(10)),
                            'o' => Node::Char(self.take_radix(8)),
                            'x' => Node::Char(self.take_radix(16)),
                            'u' => Node::Char(self.take_radix(16)),
                            'U' => Node::Char(self.take_radix(16)),
                            _ => Node::Char(kind),
                        }
                    }
                    // Word boundaries (vimregex §4.1).
                    '<' => Node::WordStart,
                    '>' => Node::WordEnd,
                    'd' => Node::Class(vec![ClassItem::Digit], false),
                    'D' => Node::Class(vec![ClassItem::Digit], true),
                    'w' => Node::Class(vec![ClassItem::Word], false),
                    'W' => Node::Class(vec![ClassItem::Word], true),
                    's' => Node::Class(vec![ClassItem::Space], false),
                    'S' => Node::Class(vec![ClassItem::Space], true),
                    // The rest of vim's classes (vimregex §4.2).
                    'x' => Node::Class(vec![ClassItem::Hex], false),
                    'X' => Node::Class(vec![ClassItem::Hex], true),
                    'o' => Node::Class(vec![ClassItem::Octal], false),
                    'O' => Node::Class(vec![ClassItem::Octal], true),
                    'h' => Node::Class(vec![ClassItem::Head], false),
                    'H' => Node::Class(vec![ClassItem::Head], true),
                    'p' => Node::Class(vec![ClassItem::Printable], false),
                    'P' => Node::Class(vec![ClassItem::NotPrintable], false),
                    'a' => Node::Class(vec![ClassItem::Alpha], false),
                    'A' => Node::Class(vec![ClassItem::Alpha], true),
                    'l' => Node::Class(vec![ClassItem::Lower], false),
                    'L' => Node::Class(vec![ClassItem::Lower], true),
                    'u' => Node::Class(vec![ClassItem::Upper], false),
                    'U' => Node::Class(vec![ClassItem::Upper], true),
                    'f' => Node::Class(vec![ClassItem::FileName], false),
                    'F' => Node::Class(vec![ClassItem::FileName], true),
                    'n' => Node::Char('\n'),
                    't' => Node::Char('\t'),
                    'r' => Node::Char('\r'),
                    'e' => Node::Char('\u{1b}'),
                    other => Node::Char(other),
                })
            }
            other => Some(Node::Char(other)),
        }
    }

    /// Read an integer in `radix` for the `\%d`/`\%o`/`\%x`/`\%u` forms.
    fn take_radix(&mut self, radix: u32) -> char {
        let mut digits = String::new();
        while let Some(c) = self.peek().filter(|c| c.is_digit(radix)) {
            digits.push(c);
            self.bump();
        }
        u32::from_str_radix(&digits, radix)
            .ok()
            .and_then(char::from_u32)
            .unwrap_or('\u{fffd}')
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
        // \< : the previous char is not a word char and the next one is.
        Node::WordStart => {
            let before_ok = pos == 0 || !text.get(pos - 1).is_some_and(|c| is_word_char(*c));
            let after_ok = text.get(pos).is_some_and(|c| is_word_char(*c));
            if before_ok && after_ok {
                match_nodes(rest, text, pos, ci)
            } else {
                None
            }
        }
        // \> : the previous char is a word char and the next one is not.
        Node::WordEnd => {
            let before_ok = pos > 0 && text.get(pos - 1).is_some_and(|c| is_word_char(*c));
            let after_ok = !text.get(pos).is_some_and(|c| is_word_char(*c));
            if before_ok && after_ok {
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
            // With \c a class matches either case (vim's ignorecase makes
            // \u find a lowercase letter too); test the positive set first so
            // a negated class keeps its meaning.
            let mut positive = class_match(items, false, *t);
            if ci && !positive {
                let other = if t.is_lowercase() {
                    t.to_uppercase().next()
                } else {
                    t.to_lowercase().next()
                };
                if let Some(other) = other {
                    positive = class_match(items, false, other);
                }
            }
            let hit = positive != *negated;
            if hit {
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
        Node::Repeat {
            node: inner,
            lo,
            hi,
            greedy,
        } => {
            // Collect the position after each allowed repetition, then try the
            // ends from the longest (greedy) or shortest (\{-}) match.
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
            // ends[i] is the position after i repetitions.
            let count = ends.len();
            if *greedy {
                for reps in (0..count).rev() {
                    if (reps as u32) < *lo {
                        break;
                    }
                    if let Some(found) = match_nodes(rest, text, ends[reps], ci) {
                        return Some(found);
                    }
                }
            } else {
                for (reps, candidate) in ends.iter().enumerate() {
                    if (reps as u32) < *lo {
                        continue;
                    }
                    if let Some(found) = match_nodes(rest, text, *candidate, ci) {
                        return Some(found);
                    }
                }
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
