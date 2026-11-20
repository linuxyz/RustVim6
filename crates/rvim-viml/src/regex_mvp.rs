//! A compact Vim-compatible regular-expression engine.
//!
//! This is the engine behind '=~' / '!~', ':s', ':g', search addresses,
//! syntax highlighting and 'taglist()'.  It implements a practical subset of
//! Vim's regexp dialect (see vim's regexp.c / regexp_bt.c) with a
//! backtracking matcher:
//!
//! * literals, '.', '*' (any number), '\+', '\?', '\=' (0 or 1),
//!   '\{n,m}' and the non-greedy '\{-n,m}' family;
//! * '^' / '$' (line anchors), '\<', '\>', '\zs', '\ze', '\%^', '\%$';
//! * character classes with ranges, negation, POSIX '[:name:]' groups and the
//!   predefined classes \d \D \s \S \w \W \x \X \o \O \h \H \p \P \a \A
//!   \l \L \u \U \f \F (plus \i / \I / \k / \K approximations);
//! * capturing groups '\(...\)', non-capturing '\%(...)', alternation '\|',
//!   back-references \1..\9, look-ahead '\@=' / '\@!';
//! * multiline atoms: '\_.' (any character or end-of-line), the '\_x' class
//!   forms, '\_[' collections, '\_^' / '\_$', and '\n';
//! * magic levels \v (very magic), \m (magic, the default), \M (nomagic) and
//!   \V (very nomagic).  As in Vim the change applies to the rest of the
//!   pattern; a following '(' is therefore a group in very magic, while in
//!   very nomagic the parentheses are literal;
//! * \c / \C to force case-insensitive / case-sensitive matching.
//!
//! Matching is leftmost-first with full backtracking; zero-width items never
//! make a repetition loop forever, and an internal step budget keeps
//! pathological patterns from hanging.
//!
//! Known deviations from Vim (see also docs/compat_deviations.md):
//! look-behind '\@<=' / '\@<!' and atomic '\@>' are accepted but treated as
//! zero-width no-ops, '\z1'..'\z9' extmatches are no-ops, '~' is a literal,
//! the '\%l' / '\%c' / '\%V' position atoms are not implemented, repeat
//! counts are capped at 1000, and inside a collection '\d' etc. are class
//! shorthands unless digits follow (where Vim reads a character code).

use std::cell::Cell;

/// Maximum number of copies expanded for a repeat count.
const MAX_REPEAT: u32 = 1000;
/// Maximum group nesting depth (guards the recursive-descent parser).
const MAX_NEST: usize = 128;
/// Base and per-character step budgets for one search.
const START_BUDGET: u64 = 100_000;
const BUDGET_PER_CHAR: u64 = 20;
const MAX_BUDGET: u64 = 2_000_000;
/// Hard cap on live backtracking threads so a pathological pattern cannot
/// exhaust memory even within the step budget.
const MAX_THREADS: usize = 500_000;

// ---------------------------------------------------------------- magic levels

/// The magic level requested by \v, \m, \M or \V.  The ordering matters:
/// anything at or above On makes '.', '[' and '*' special, anything at or
/// above Off keeps '^' and '$' magic, and only All makes '(){}|+?=' magic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Magic {
    None = 0,
    Off = 1,
    On = 2,
    All = 3,
}

// --------------------------------------------------------------- compiled form

#[derive(Clone, Debug)]
enum Inst {
    Char(char),
    /// '.' : any character except a newline.
    Any,
    /// '\_.' : any character including a newline.
    AnyNl,
    Class(Class),
    /// '^' in the current mode (string start, or line start in multiline mode).
    StartLine,
    /// '$' in the current mode.
    EndLine,
    /// '\_^' : start of a line regardless of mode.
    StartLineAny,
    /// '\_$' : end of a line regardless of mode.
    EndLineAny,
    StartText,
    EndText,
    WordStart,
    WordEnd,
    /// Store the current position in a capture slot.
    Save(usize),
    /// '\zs' : set the reported start of the match.
    Zs,
    /// '\ze' : set the reported end of the match.
    Ze,
    /// Try the first target first, then the second one.
    Split(usize, usize),
    Jmp(usize),
    /// '\1'..'\9' : match the text of group 0..8 (zero based).
    Backref(usize),
    /// '\@=' / '\@!' with a subprogram ending in Accept.
    Look {
        positive: bool,
        prog: usize,
    },
    /// Remember the current position for an empty-match loop guard.
    Mark(usize),
    /// Jump back if the position advanced since Mark, otherwise fall through.
    Progress {
        slot: usize,
        back: usize,
    },
    Accept,
}

// ---------------------------------------------------------------------- parser

#[derive(Clone, Debug)]
enum Node {
    Char(char),
    Any {
        nl: bool,
    },
    Class(Class),
    Caret,
    Dollar,
    CaretAny,
    DollarAny,
    StartText,
    EndText,
    WordStart,
    WordEnd,
    Group {
        cap: Option<usize>,
        branches: Vec<Vec<Node>>,
    },
    Repeat {
        node: Box<Node>,
        lo: u32,
        hi: u32,
        greedy: bool,
    },
    Backref(usize),
    Look {
        positive: bool,
        node: Box<Node>,
    },
    Zs,
    Ze,
    Noop,
}

#[derive(Clone, Debug)]
struct Class {
    items: Vec<ClassItem>,
    negated: bool,
    /// Set by the '\_' prefix: the class also matches an end-of-line.
    with_nl: bool,
}

impl Class {
    fn single(item: ClassItem) -> Self {
        Self {
            items: vec![item],
            negated: false,
            with_nl: false,
        }
    }

    fn single_nl(item: ClassItem) -> Self {
        Self {
            items: vec![item],
            negated: false,
            with_nl: true,
        }
    }
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
    Hex,
    NotHex,
    Octal,
    NotOctal,
    Head,
    NotHead,
    /// Printable character.
    Printable,
    /// Printable but not a digit (vim's '\P').
    NotPrintable,
    Alpha,
    NotAlpha,
    Lower,
    NotLower,
    Upper,
    NotUpper,
    FileName,
    NotFileName,
    Blank,
    Cntrl,
    Graph,
    Punct,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_file_name_char(c: char) -> bool {
    c.is_alphanumeric() || "._-~".contains(c)
}

/// Vim's '\s' is space and tab (we also accept CR / VT / FF); end-of-line is
/// deliberately excluded so that the plain classes stay line oriented.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\u{b}' | '\u{c}')
}

fn class_from_letter(c: char) -> Option<ClassItem> {
    Some(match c {
        'd' => ClassItem::Digit,
        'D' => ClassItem::NotDigit,
        'w' | 'i' | 'k' => ClassItem::Word,
        'W' | 'I' | 'K' => ClassItem::NotWord,
        's' => ClassItem::Space,
        'S' => ClassItem::NotSpace,
        'x' => ClassItem::Hex,
        'X' => ClassItem::NotHex,
        'o' => ClassItem::Octal,
        'O' => ClassItem::NotOctal,
        'h' => ClassItem::Head,
        'H' => ClassItem::NotHead,
        'p' => ClassItem::Printable,
        'P' => ClassItem::NotPrintable,
        'a' => ClassItem::Alpha,
        'A' => ClassItem::NotAlpha,
        'l' => ClassItem::Lower,
        'L' => ClassItem::NotLower,
        'u' => ClassItem::Upper,
        'U' => ClassItem::NotUpper,
        'f' => ClassItem::FileName,
        'F' => ClassItem::NotFileName,
        _ => return None,
    })
}

fn class_item_matches(item: &ClassItem, c: char) -> bool {
    match item {
        ClassItem::Char(x) => *x == c,
        ClassItem::Range(a, b) => *a <= c && c <= *b,
        ClassItem::Digit => c.is_ascii_digit(),
        ClassItem::NotDigit => !c.is_ascii_digit(),
        ClassItem::Word => is_word_char(c),
        ClassItem::NotWord => !is_word_char(c),
        ClassItem::Space => is_space(c),
        ClassItem::NotSpace => !is_space(c) && c != '\n',
        ClassItem::Hex => c.is_ascii_hexdigit(),
        ClassItem::NotHex => !c.is_ascii_hexdigit(),
        ClassItem::Octal => ('0'..='7').contains(&c),
        ClassItem::NotOctal => !('0'..='7').contains(&c),
        ClassItem::Head => c.is_alphabetic() || c == '_',
        ClassItem::NotHead => !(c.is_alphabetic() || c == '_'),
        ClassItem::Printable => !c.is_control(),
        ClassItem::NotPrintable => !c.is_control() && !c.is_ascii_digit(),
        ClassItem::Alpha => c.is_alphabetic(),
        ClassItem::NotAlpha => !c.is_alphabetic(),
        ClassItem::Lower => c.is_lowercase(),
        ClassItem::NotLower => !c.is_lowercase(),
        ClassItem::Upper => c.is_uppercase(),
        ClassItem::NotUpper => !c.is_uppercase(),
        ClassItem::FileName => is_file_name_char(c),
        ClassItem::NotFileName => !is_file_name_char(c),
        ClassItem::Blank => c == ' ' || c == '\t',
        ClassItem::Cntrl => c.is_control(),
        ClassItem::Graph => !c.is_control() && c != ' ',
        ClassItem::Punct => {
            !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control() && c != '\n'
        }
    }
}

fn class_items_match(items: &[ClassItem], c: char) -> bool {
    items.iter().any(|item| class_item_matches(item, c))
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    magic: Magic,
    saw_icase: bool,
    saw_noicase: bool,
    ngroups: usize,
    depth: usize,
}

impl Parser {
    fn new(src: &str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            magic: Magic::On,
            saw_icase: false,
            saw_noicase: false,
            ngroups: 0,
            depth: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    // --------------------------------------------------- magic-level helpers

    fn at_alt(&self) -> bool {
        if self.magic == Magic::All {
            self.peek() == Some('|')
        } else {
            self.peek() == Some('\\') && self.peek_at(1) == Some('|')
        }
    }

    fn eat_alt(&mut self) {
        self.pos += if self.magic == Magic::All { 1 } else { 2 };
    }

    fn at_close(&self) -> bool {
        if self.magic == Magic::All {
            self.peek() == Some(')')
        } else {
            self.peek() == Some('\\') && self.peek_at(1) == Some(')')
        }
    }

    fn eat_close(&mut self) {
        if self.at_close() {
            self.pos += if self.magic == Magic::All { 1 } else { 2 };
        }
    }

    /// Handle \c, \C and the magic switches anywhere in the pattern.  These
    /// never produce a node and do not count as the first atom of a branch.
    fn eat_flags(&mut self) -> bool {
        if self.peek() != Some('\\') {
            return false;
        }
        let Some(flag) = self.peek_at(1) else {
            return false;
        };
        match flag {
            'c' => self.saw_icase = true,
            'C' => self.saw_noicase = true,
            'v' => self.magic = Magic::All,
            'm' => self.magic = Magic::On,
            'M' => self.magic = Magic::Off,
            'V' => self.magic = Magic::None,
            'Z' => {}
            _ => return false,
        }
        self.pos += 2;
        true
    }

    /// True when the '$' at self.pos is at the end of its branch (Vim only
    /// treats it as an anchor there).  Case / magic flags in between count as
    /// transparent.
    fn dollar_is_end(&self) -> bool {
        if self.magic == Magic::All {
            return true;
        }
        if self.magic < Magic::Off {
            return false;
        }
        let mut i = self.pos + 1;
        while let (Some('\\'), Some('c' | 'C' | 'm' | 'M' | 'v' | 'V' | 'Z')) =
            (self.chars.get(i).copied(), self.chars.get(i + 1).copied())
        {
            i += 2;
        }
        match self.chars.get(i).copied() {
            None => true,
            Some('\\') => matches!(
                self.chars.get(i + 1).copied(),
                Some('|') | Some(')') | Some('n') | Some('&')
            ),
            _ => false,
        }
    }

    // ------------------------------------------------------------- structure

    fn parse_alt(&mut self) -> Vec<Vec<Node>> {
        let mut branches = vec![self.parse_concat()];
        while self.at_alt() {
            self.eat_alt();
            branches.push(self.parse_concat());
        }
        branches
    }

    fn parse_concat(&mut self) -> Vec<Node> {
        let mut nodes: Vec<Node> = Vec::new();
        loop {
            if self.eat_flags() {
                continue;
            }
            if self.pos >= self.chars.len() || self.at_alt() || self.at_close() {
                break;
            }
            let at_start = nodes.is_empty() || matches!(nodes.last(), Some(Node::Char('\n')));
            let Some((node, quantifiable)) = self.parse_atom(at_start) else {
                break;
            };
            let node = if quantifiable {
                self.parse_quantifiers(node)
            } else {
                node
            };
            nodes.push(node);
        }
        nodes
    }

    fn parse_atom(&mut self, at_start: bool) -> Option<(Node, bool)> {
        let c = self.peek()?;
        match c {
            '\\' => {
                self.pos += 1;
                self.parse_backslash_atom()
            }
            '.' => {
                self.pos += 1;
                if self.magic >= Magic::On {
                    Some((Node::Any { nl: false }, true))
                } else {
                    Some((Node::Char('.'), true))
                }
            }
            '[' => {
                if self.magic >= Magic::On {
                    Some((self.class_node(false), true))
                } else {
                    self.pos += 1;
                    Some((Node::Char('['), true))
                }
            }
            '^' => {
                let special = if self.magic == Magic::All {
                    true
                } else {
                    self.magic >= Magic::Off && at_start
                };
                self.pos += 1;
                if special {
                    Some((Node::Caret, false))
                } else {
                    Some((Node::Char('^'), true))
                }
            }
            '$' => {
                let special = if self.magic == Magic::All {
                    true
                } else {
                    self.magic >= Magic::Off && self.dollar_is_end()
                };
                self.pos += 1;
                if special {
                    Some((Node::Dollar, false))
                } else {
                    Some((Node::Char('$'), true))
                }
            }
            '(' if self.magic == Magic::All => {
                self.pos += 1;
                Some((self.parse_group_node(true), true))
            }
            '%' if self.magic == Magic::All => {
                self.pos += 1;
                Some((self.parse_percent_node(), true))
            }
            '<' if self.magic == Magic::All => {
                self.pos += 1;
                Some((Node::WordStart, false))
            }
            '>' if self.magic == Magic::All => {
                self.pos += 1;
                Some((Node::WordEnd, false))
            }
            _ => {
                self.pos += 1;
                Some((Node::Char(c), true))
            }
        }
    }

    fn parse_group_node(&mut self, capturing: bool) -> Node {
        if self.depth >= MAX_NEST {
            return Node::Noop;
        }
        self.depth += 1;
        let cap = if capturing {
            let idx = self.ngroups;
            self.ngroups += 1;
            Some(idx)
        } else {
            None
        };
        let branches = self.parse_alt();
        self.eat_close();
        self.depth -= 1;
        Node::Group { cap, branches }
    }

    // ------------------------------------------------------------ quantifiers

    fn lookahead_at(&self) -> Option<bool> {
        if self.magic == Magic::All {
            if self.peek() == Some('@') {
                return match self.peek_at(1) {
                    Some('=') => Some(true),
                    Some('!') => Some(false),
                    _ => None,
                };
            }
            return None;
        }
        if self.peek() == Some('\\') && self.peek_at(1) == Some('@') {
            return match self.peek_at(2) {
                Some('=') => Some(true),
                Some('!') => Some(false),
                _ => None,
            };
        }
        None
    }

    /// Consume an unsupported look-behind / atomic suffix so that it does not
    /// get mistaken for literals.  These are treated as zero-width no-ops.
    fn skip_unsupported_assertion(&mut self) -> bool {
        let offset = if self.magic == Magic::All {
            if self.peek() != Some('@') {
                return false;
            }
            0
        } else {
            if self.peek() != Some('\\') {
                return false;
            }
            1
        };
        if self.peek_at(offset) != Some('@') {
            return false;
        }
        match self.peek_at(offset + 1) {
            Some('>') => {
                self.pos += offset + 2;
                true
            }
            Some('<') => {
                if matches!(self.peek_at(offset + 2), Some('=') | Some('!')) {
                    self.pos += offset + 3;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn parse_quantifiers(&mut self, mut node: Node) -> Node {
        loop {
            if self.skip_unsupported_assertion() {
                continue;
            }
            if let Some(positive) = self.lookahead_at() {
                self.pos += if self.magic == Magic::All { 2 } else { 3 };
                node = Node::Look {
                    positive,
                    node: Box::new(node),
                };
                continue;
            }
            let magic = self.magic;
            let quant = match (self.peek(), self.peek_at(1)) {
                (Some('*'), _) if magic >= Magic::On => Some(('*', 1)),
                (Some('\\'), Some('*')) if magic < Magic::On => Some(('*', 2)),
                (Some('+'), _) if magic == Magic::All => Some(('+', 1)),
                (Some('\\'), Some('+')) if magic != Magic::All => Some(('+', 2)),
                (Some('?'), _) if magic == Magic::All => Some(('?', 1)),
                (Some('\\'), Some('?')) if magic != Magic::All => Some(('?', 2)),
                (Some('='), _) if magic == Magic::All => Some(('=', 1)),
                (Some('\\'), Some('=')) if magic != Magic::All => Some(('=', 2)),
                (Some('{'), _) if magic == Magic::All => Some(('{', 1)),
                (Some('\\'), Some('{')) if magic != Magic::All => Some(('{', 2)),
                _ => None,
            };
            let Some((kind, cost)) = quant else {
                break;
            };
            match kind {
                '*' => {
                    self.pos += cost;
                    node = repeat(node, 0, u32::MAX, true);
                }
                '+' => {
                    self.pos += cost;
                    node = repeat(node, 1, u32::MAX, true);
                }
                '?' | '=' => {
                    self.pos += cost;
                    node = repeat(node, 0, 1, true);
                }
                '{' => {
                    let save = self.pos;
                    self.pos += cost;
                    match self.parse_braces() {
                        Some((lo, hi, greedy)) => node = repeat(node, lo, hi, greedy),
                        None => {
                            self.pos = save;
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        node
    }

    fn parse_braces(&mut self) -> Option<(u32, u32, bool)> {
        let greedy = if self.peek() == Some('-') {
            self.bump();
            false
        } else {
            true
        };
        let mut min = String::new();
        while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
            min.push(c);
            self.bump();
        }
        let mut max = String::new();
        let mut has_comma = false;
        if self.peek() == Some(',') {
            has_comma = true;
            self.bump();
            while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
                max.push(c);
                self.bump();
            }
        }
        // Vim accepts both \{n,m} and \{n,m\} (read_limits allows the
        // optional backslash before the closing brace).
        let close_len = if self.peek() == Some('}') {
            1
        } else if self.peek() == Some('\\') && self.peek_at(1) == Some('}') {
            2
        } else {
            return None;
        };
        self.pos += close_len;
        let lo: u32 = min.parse().unwrap_or(0);
        let hi = if !has_comma {
            // A bare \{-} repeats zero or more times, \{-n} exactly n.
            if min.is_empty() { u32::MAX } else { lo }
        } else if max.is_empty() {
            u32::MAX
        } else {
            max.parse().unwrap_or(lo)
        };
        // A reversed range is swapped, like read_limits() does.
        if greedy && lo > hi {
            Some((hi, lo, greedy))
        } else {
            Some((lo, hi, greedy))
        }
    }

    // ----------------------------------------------------------------- atoms

    fn parse_backslash_atom(&mut self) -> Option<(Node, bool)> {
        let e = self.bump()?;
        let magic = self.magic;
        let (node, quantifiable) = match e {
            'n' => (Node::Char('\n'), true),
            't' => (Node::Char('\t'), true),
            'r' => (Node::Char('\r'), true),
            'e' => (Node::Char('\u{1b}'), true),
            'b' => (Node::Char('\u{8}'), true),
            'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'x' | 'X' | 'o' | 'O' | 'h' | 'H' | 'p' | 'P'
            | 'a' | 'A' | 'l' | 'L' | 'u' | 'U' | 'f' | 'F' | 'i' | 'I' | 'k' | 'K' => {
                let item = class_from_letter(e).unwrap_or(ClassItem::Word);
                (Node::Class(Class::single(item)), true)
            }
            '(' => {
                if magic == Magic::All {
                    (Node::Char('('), true)
                } else {
                    (self.parse_group_node(true), true)
                }
            }
            '%' => {
                if magic == Magic::All {
                    (Node::Char('%'), true)
                } else {
                    (self.parse_percent_node(), true)
                }
            }
            '<' => {
                if magic == Magic::All {
                    (Node::Char('<'), true)
                } else {
                    (Node::WordStart, false)
                }
            }
            '>' => {
                if magic == Magic::All {
                    (Node::Char('>'), true)
                } else {
                    (Node::WordEnd, false)
                }
            }
            '_' => return self.parse_underscore_atom(),
            'z' => self.parse_z_form(),
            '1'..='9' => (Node::Backref(e.to_digit(10).unwrap_or(0) as usize), true),
            '.' => {
                if magic >= Magic::On {
                    (Node::Char('.'), true)
                } else {
                    (Node::Any { nl: false }, true)
                }
            }
            '[' => {
                if magic >= Magic::On {
                    (Node::Char('['), true)
                } else {
                    // Step back onto the '[' so the collection parser sees it.
                    self.pos -= 1;
                    (self.class_node(false), true)
                }
            }
            '^' => {
                if magic == Magic::None {
                    (Node::CaretAny, false)
                } else {
                    (Node::Char('^'), true)
                }
            }
            '$' => {
                if magic == Magic::None {
                    (Node::DollarAny, false)
                } else {
                    (Node::Char('$'), true)
                }
            }
            'c' | 'C' | 'v' | 'm' | 'M' | 'V' | 'Z' => (Node::Noop, false),
            other => (Node::Char(other), true),
        };
        Some((node, quantifiable))
    }

    fn parse_z_form(&mut self) -> (Node, bool) {
        match self.peek() {
            Some('s') => {
                self.bump();
                (Node::Zs, false)
            }
            Some('e') => {
                self.bump();
                (Node::Ze, false)
            }
            Some('(') => {
                self.bump();
                (self.parse_group_node(false), true)
            }
            Some(c) if c.is_ascii_digit() => {
                self.bump();
                (Node::Noop, false)
            }
            _ => (Node::Char('z'), true),
        }
    }

    fn parse_underscore_atom(&mut self) -> Option<(Node, bool)> {
        let c = self.peek()?;
        match c {
            '.' => {
                self.bump();
                Some((Node::Any { nl: true }, true))
            }
            '^' => {
                self.bump();
                Some((Node::CaretAny, false))
            }
            '$' => {
                self.bump();
                Some((Node::DollarAny, false))
            }
            '[' => Some((self.class_node(true), true)),
            '\\' if self.peek_at(1) == Some('[') => {
                self.pos += 1;
                Some((self.class_node(true), true))
            }
            _ => {
                let e = self.bump()?;
                match class_from_letter(e) {
                    Some(item) => Some((Node::Class(Class::single_nl(item)), true)),
                    None => Some((Node::Char(e), true)),
                }
            }
        }
    }

    fn parse_percent_node(&mut self) -> Node {
        let Some(kind) = self.bump() else {
            return Node::Char('%');
        };
        match kind {
            '(' => self.parse_group_node(false),
            'd' => Node::Char(self.take_radix(10, 10)),
            'o' => Node::Char(self.take_radix(8, 3)),
            'x' => Node::Char(self.take_radix(16, 2)),
            'u' => Node::Char(self.take_radix(16, 4)),
            'U' => Node::Char(self.take_radix(16, 8)),
            '^' => Node::StartText,
            '$' => Node::EndText,
            '#' => {
                if self.peek() == Some('=') {
                    self.bump();
                    if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.bump();
                    }
                }
                Node::Noop
            }
            'V' | 'v' | 'c' | 'l' | '\'' | 'C' => Node::Noop,
            '[' => Node::Noop,
            other => Node::Char(other),
        }
    }

    /// Read an integer in the given radix, at most max_digits digits, and turn
    /// it into a character.  A zero code point is vim's way of spelling a
    /// newline.
    fn take_radix(&mut self, radix: u32, max_digits: usize) -> char {
        let mut value: u32 = 0;
        let mut digits = 0usize;
        while digits < max_digits {
            let Some(d) = self.peek().and_then(|c| c.to_digit(radix)) else {
                break;
            };
            value = value.saturating_mul(radix).saturating_add(d);
            self.bump();
            digits += 1;
        }
        if digits == 0 {
            return '\u{fffd}';
        }
        let value = if value == 0 { 0x0a } else { value };
        char::from_u32(value).unwrap_or('\u{fffd}')
    }

    // ----------------------------------------------------------- collections

    /// Is there a closing ']' for the collection starting at start?  Vim
    /// treats an unterminated '[' as a literal character.
    fn class_terminated_at(&self, start: usize) -> bool {
        let mut i = start + 1;
        if self.chars.get(i).copied() == Some('^') {
            i += 1;
        }
        if self.chars.get(i).copied() == Some(']') {
            i += 1;
            if i >= self.chars.len() {
                // "[]" works like "[]]": a collection holding ']'.
                return true;
            }
        }
        while let Some(&c) = self.chars.get(i) {
            match c {
                '\\' => i += 2,
                ']' => return true,
                _ => i += 1,
            }
        }
        false
    }

    fn class_node(&mut self, with_nl: bool) -> Node {
        if !self.class_terminated_at(self.pos) {
            self.pos += 1;
            return Node::Char('[');
        }
        Node::Class(self.parse_class(with_nl))
    }

    fn try_posix(&mut self) -> Option<Vec<ClassItem>> {
        let start = self.pos;
        let mut i = start + 2;
        while self.chars.get(i).is_some_and(|c| c.is_ascii_lowercase()) {
            i += 1;
        }
        if self.chars.get(i).copied() != Some(':') || self.chars.get(i + 1).copied() != Some(']') {
            return None;
        }
        let name: String = self.chars[start + 2..i].iter().collect();
        let items = match name.as_str() {
            "alnum" => vec![ClassItem::Alpha, ClassItem::Digit],
            "alpha" => vec![ClassItem::Alpha],
            "blank" => vec![ClassItem::Blank],
            "cntrl" => vec![ClassItem::Cntrl],
            "digit" => vec![ClassItem::Digit],
            "graph" => vec![ClassItem::Graph],
            "lower" => vec![ClassItem::Lower],
            "print" => vec![ClassItem::Printable],
            "punct" => vec![ClassItem::Punct],
            "space" => vec![ClassItem::Space],
            "upper" => vec![ClassItem::Upper],
            "xdigit" => vec![ClassItem::Hex],
            "return" => vec![ClassItem::Char('\r')],
            "tab" => vec![ClassItem::Char('\t')],
            "escape" => vec![ClassItem::Char('\u{1b}')],
            "backspace" => vec![ClassItem::Char('\u{8}')],
            "ident" | "keyword" => vec![ClassItem::Word],
            "fname" => vec![ClassItem::FileName],
            _ => return None,
        };
        self.pos = i + 2;
        Some(items)
    }

    fn parse_class(&mut self, with_nl: bool) -> Class {
        self.bump();
        let mut negated = false;
        if self.peek() == Some('^') {
            negated = true;
            self.bump();
        }
        let mut items: Vec<ClassItem> = Vec::new();
        let mut last_char: Option<(usize, char)> = None;
        let mut first = true;
        while let Some(c) = self.peek() {
            if c == ']' && !first {
                self.bump();
                break;
            }
            if c == '[' && self.peek_at(1) == Some(':') {
                if let Some(mut posix) = self.try_posix() {
                    items.append(&mut posix);
                    last_char = None;
                    first = false;
                    continue;
                }
            }
            if c == '-' {
                if let Some((idx, lo)) = last_char {
                    if self.peek_at(1).is_some_and(|n| n != ']') {
                        self.bump();
                        if let Some(hi) = self.parse_class_char() {
                            if lo <= hi {
                                items[idx] = ClassItem::Range(lo, hi);
                            } else {
                                items[idx] = ClassItem::Char(lo);
                                items.push(ClassItem::Char('-'));
                                items.push(ClassItem::Char(hi));
                            }
                            last_char = None;
                            first = false;
                            continue;
                        }
                    }
                }
                self.bump();
                items.push(ClassItem::Char('-'));
                last_char = None;
                first = false;
                continue;
            }
            let idx = items.len();
            let (item, single) = if c == '\\' {
                self.bump();
                self.parse_class_escape()
            } else {
                self.bump();
                (ClassItem::Char(c), Some(c))
            };
            items.push(item);
            last_char = single.map(|ch| (idx, ch));
            first = false;
        }
        Class {
            items,
            negated,
            with_nl,
        }
    }

    fn parse_class_char(&mut self) -> Option<char> {
        let c = self.bump()?;
        if c != '\\' {
            return Some(c);
        }
        let e = self.bump()?;
        Some(match e {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'e' => '\u{1b}',
            'b' => '\u{8}',
            'd' => self.take_radix(10, 10),
            'o' => self.take_radix(8, 3),
            'x' => self.take_radix(16, 2),
            'u' => self.take_radix(16, 4),
            'U' => self.take_radix(16, 8),
            other => other,
        })
    }

    fn parse_class_escape(&mut self) -> (ClassItem, Option<char>) {
        let Some(e) = self.bump() else {
            return (ClassItem::Char('\\'), None);
        };
        match e {
            'n' => (ClassItem::Char('\n'), Some('\n')),
            't' => (ClassItem::Char('\t'), Some('\t')),
            'r' => (ClassItem::Char('\r'), Some('\r')),
            'e' => (ClassItem::Char('\u{1b}'), Some('\u{1b}')),
            'b' => (ClassItem::Char('\u{8}'), Some('\u{8}')),
            'd' | 'o' | 'x' | 'u' | 'U' => {
                let (radix, max) = match e {
                    'd' => (10, 10),
                    'o' => (8, 3),
                    'x' => (16, 2),
                    'u' => (16, 4),
                    _ => (16, 8),
                };
                if self.peek().is_some_and(|c| c.is_digit(radix)) {
                    let ch = self.take_radix(radix, max);
                    (ClassItem::Char(ch), Some(ch))
                } else {
                    match class_from_letter(e) {
                        Some(item) => (item, None),
                        None => (ClassItem::Char(e), Some(e)),
                    }
                }
            }
            ']' | '^' | '-' | '\\' => (ClassItem::Char(e), Some(e)),
            _ => match class_from_letter(e) {
                Some(item) => (item, None),
                None => (ClassItem::Char(e), Some(e)),
            },
        }
    }
}

fn repeat(node: Node, lo: u32, hi: u32, greedy: bool) -> Node {
    Node::Repeat {
        node: Box::new(node),
        lo,
        hi,
        greedy,
    }
}
// -------------------------------------------------------------------- compiler

struct Ctx {
    progs: Vec<Vec<Inst>>,
    aux_len: usize,
}

fn compile_alt(branches: &[Vec<Node>], prog: &mut Vec<Inst>, ctx: &mut Ctx) {
    if branches.len() <= 1 {
        if let Some(branch) = branches.first() {
            compile_seq(branch, prog, ctx);
        }
        return;
    }
    let mut jumps: Vec<usize> = Vec::new();
    let count = branches.len();
    for (i, branch) in branches.iter().enumerate() {
        if i + 1 < count {
            let split_at = prog.len();
            prog.push(Inst::Split(0, 0));
            let body_at = prog.len();
            compile_seq(branch, prog, ctx);
            let jump_at = prog.len();
            prog.push(Inst::Jmp(0));
            let next_at = prog.len();
            prog[split_at] = Inst::Split(body_at, next_at);
            jumps.push(jump_at);
        } else {
            compile_seq(branch, prog, ctx);
        }
    }
    let end = prog.len();
    for jump in jumps {
        prog[jump] = Inst::Jmp(end);
    }
}

fn compile_seq(nodes: &[Node], prog: &mut Vec<Inst>, ctx: &mut Ctx) {
    for node in nodes {
        compile_node(node, prog, ctx);
    }
}

fn compile_node(node: &Node, prog: &mut Vec<Inst>, ctx: &mut Ctx) {
    match node {
        Node::Char(c) => prog.push(Inst::Char(*c)),
        Node::Any { nl } => prog.push(if *nl { Inst::AnyNl } else { Inst::Any }),
        Node::Class(class) => prog.push(Inst::Class(class.clone())),
        Node::Caret => prog.push(Inst::StartLine),
        Node::Dollar => prog.push(Inst::EndLine),
        Node::CaretAny => prog.push(Inst::StartLineAny),
        Node::DollarAny => prog.push(Inst::EndLineAny),
        Node::StartText => prog.push(Inst::StartText),
        Node::EndText => prog.push(Inst::EndText),
        Node::WordStart => prog.push(Inst::WordStart),
        Node::WordEnd => prog.push(Inst::WordEnd),
        Node::Zs => prog.push(Inst::Zs),
        Node::Ze => prog.push(Inst::Ze),
        Node::Noop => {}
        Node::Backref(number) => prog.push(Inst::Backref(number.saturating_sub(1))),
        Node::Group { cap, branches } => {
            if let Some(idx) = cap {
                prog.push(Inst::Save(idx * 2));
            }
            compile_alt(branches, prog, ctx);
            if let Some(idx) = cap {
                prog.push(Inst::Save(idx * 2 + 1));
            }
        }
        Node::Look { positive, node } => {
            let mut sub = Vec::new();
            compile_node(node, &mut sub, ctx);
            sub.push(Inst::Accept);
            let idx = ctx.progs.len();
            ctx.progs.push(sub);
            prog.push(Inst::Look {
                positive: *positive,
                prog: idx,
            });
        }
        Node::Repeat {
            node,
            lo,
            hi,
            greedy,
        } => compile_repeat(node, *lo, *hi, *greedy, prog, ctx),
    }
}

fn compile_repeat(
    node: &Node,
    lo: u32,
    hi: u32,
    greedy: bool,
    prog: &mut Vec<Inst>,
    ctx: &mut Ctx,
) {
    let lo = lo.min(MAX_REPEAT);
    for _ in 0..lo {
        compile_node(node, prog, ctx);
    }
    if hi == u32::MAX {
        compile_star(node, greedy, prog, ctx);
    } else {
        let hi = hi.min(lo.saturating_add(MAX_REPEAT));
        for _ in lo..hi {
            let split_at = prog.len();
            prog.push(Inst::Split(0, 0));
            let body_at = prog.len();
            compile_node(node, prog, ctx);
            let after = prog.len();
            prog[split_at] = if greedy {
                Inst::Split(body_at, after)
            } else {
                Inst::Split(after, body_at)
            };
        }
    }
}

/// Compile an unbounded repetition.  A body that can match the empty string
/// needs a progress guard so the backtracker cannot loop forever.
fn compile_star(node: &Node, greedy: bool, prog: &mut Vec<Inst>, ctx: &mut Ctx) {
    if !nullable(node) {
        let split_at = prog.len();
        prog.push(Inst::Split(0, 0));
        let body_at = prog.len();
        compile_node(node, prog, ctx);
        prog.push(Inst::Jmp(split_at));
        let end = prog.len();
        prog[split_at] = if greedy {
            Inst::Split(body_at, end)
        } else {
            Inst::Split(end, body_at)
        };
    } else {
        let slot = ctx.aux_len;
        ctx.aux_len += 1;
        let mark_at = prog.len();
        prog.push(Inst::Mark(slot));
        let split_at = prog.len();
        prog.push(Inst::Split(0, 0));
        let body_at = prog.len();
        compile_node(node, prog, ctx);
        prog.push(Inst::Progress {
            slot,
            back: mark_at,
        });
        let end = prog.len();
        prog[split_at] = if greedy {
            Inst::Split(body_at, end)
        } else {
            Inst::Split(end, body_at)
        };
    }
}

fn nullable(node: &Node) -> bool {
    match node {
        Node::Char(_) | Node::Any { .. } | Node::Class(_) => false,
        Node::Backref(_) | Node::Look { .. } | Node::Noop => true,
        Node::Caret
        | Node::Dollar
        | Node::CaretAny
        | Node::DollarAny
        | Node::StartText
        | Node::EndText
        | Node::WordStart
        | Node::WordEnd
        | Node::Zs
        | Node::Ze => true,
        Node::Group { branches, .. } => branches.iter().any(|b| b.iter().all(nullable)),
        Node::Repeat { node, lo, .. } => *lo == 0 || nullable(node),
    }
}

// --------------------------------------------------------------------- matcher

struct Regex {
    prog: Vec<Inst>,
    progs: Vec<Vec<Inst>>,
    ngroups: usize,
    aux_len: usize,
    ci: bool,
}

#[derive(Clone)]
struct State {
    pos: usize,
    caps: Vec<Option<usize>>,
    zs: usize,
    ze: Option<usize>,
    aux: Vec<usize>,
}

struct Matcher<'a> {
    text: &'a [char],
    progs: &'a [Vec<Inst>],
    ci: bool,
    multiline: bool,
    budget: Cell<u64>,
}

impl Matcher<'_> {
    fn char_eq(&self, a: char, b: char) -> bool {
        if a == b {
            return true;
        }
        if !self.ci {
            return false;
        }
        a.to_lowercase().eq(b.to_lowercase())
    }

    fn class_hit(&self, class: &Class, c: char) -> bool {
        let mut positive = class_items_match(&class.items, c);
        if self.ci && !positive {
            for other in c.to_lowercase().chain(c.to_uppercase()) {
                if other != c && class_items_match(&class.items, other) {
                    positive = true;
                    break;
                }
            }
        }
        if c == '\n' {
            if class.negated {
                return !positive;
            }
            return positive || class.with_nl;
        }
        positive != class.negated
    }

    fn exec(&self, prog: &[Inst], mut st: State) -> Option<State> {
        let mut stack: Vec<(usize, State)> = Vec::new();
        let mut pc = 0usize;
        loop {
            let budget = self.budget.get();
            if budget == 0 {
                return None;
            }
            self.budget.set(budget - 1);
            let mut next = pc + 1;
            let mut failed = false;
            match &prog[pc] {
                Inst::Char(c) => {
                    if st.pos < self.text.len() && self.char_eq(self.text[st.pos], *c) {
                        st.pos += 1;
                    } else {
                        failed = true;
                    }
                }
                Inst::Any => {
                    if st.pos < self.text.len() && self.text[st.pos] != '\n' {
                        st.pos += 1;
                    } else {
                        failed = true;
                    }
                }
                Inst::AnyNl => {
                    if st.pos < self.text.len() {
                        st.pos += 1;
                    } else {
                        failed = true;
                    }
                }
                Inst::Class(class) => {
                    if st.pos < self.text.len() && self.class_hit(class, self.text[st.pos]) {
                        st.pos += 1;
                    } else {
                        failed = true;
                    }
                }
                Inst::StartLine => {
                    let ok = if self.multiline {
                        st.pos == 0 || self.text[st.pos - 1] == '\n'
                    } else {
                        st.pos == 0
                    };
                    failed = !ok;
                }
                Inst::EndLine => {
                    let ok = if self.multiline {
                        st.pos == self.text.len() || self.text[st.pos] == '\n'
                    } else {
                        st.pos == self.text.len()
                    };
                    failed = !ok;
                }
                Inst::StartLineAny => {
                    failed = !(st.pos == 0 || self.text[st.pos - 1] == '\n');
                }
                Inst::EndLineAny => {
                    failed = !(st.pos == self.text.len() || self.text[st.pos] == '\n');
                }
                Inst::StartText => failed = st.pos != 0,
                Inst::EndText => failed = st.pos != self.text.len(),
                Inst::WordStart => {
                    let before_ok = st.pos == 0 || !is_word_char(self.text[st.pos - 1]);
                    let after_ok = st.pos < self.text.len() && is_word_char(self.text[st.pos]);
                    failed = !(before_ok && after_ok);
                }
                Inst::WordEnd => {
                    let before_ok = st.pos > 0 && is_word_char(self.text[st.pos - 1]);
                    let after_ok = st.pos >= self.text.len() || !is_word_char(self.text[st.pos]);
                    failed = !(before_ok && after_ok);
                }
                Inst::Save(slot) => {
                    st.caps[*slot] = Some(st.pos);
                }
                Inst::Zs => st.zs = st.pos,
                Inst::Ze => st.ze = Some(st.pos),
                Inst::Split(a, b) => {
                    // Prune alternatives once the backtracking stack is
                    // enormous; the step budget normally stops us first.
                    if stack.len() < MAX_THREADS {
                        stack.push((*b, st.clone()));
                    }
                    next = *a;
                }
                Inst::Jmp(target) => next = *target,
                Inst::Backref(idx) => {
                    let open = st.caps.get(idx * 2).copied().flatten();
                    let close = st.caps.get(idx * 2 + 1).copied().flatten();
                    // Vim matches a back-reference to a group that did not
                    // participate as the empty string (checked against the
                    // backtracking and the NFA engines of Vim 9.9).
                    if let (Some(from), Some(to)) = (open, close) {
                        let len = to - from;
                        if st.pos + len <= self.text.len() {
                            let target = &self.text[st.pos..st.pos + len];
                            let source = &self.text[from..to];
                            let same = target
                                .iter()
                                .zip(source.iter())
                                .all(|(a, b)| self.char_eq(*a, *b));
                            if same {
                                st.pos += len;
                            } else {
                                failed = true;
                            }
                        } else {
                            failed = true;
                        }
                    }
                }
                Inst::Look {
                    positive,
                    prog: sub,
                } => {
                    let sub_state = State {
                        pos: st.pos,
                        caps: st.caps.clone(),
                        zs: st.zs,
                        ze: st.ze,
                        aux: st.aux.clone(),
                    };
                    let hit = self.exec(&self.progs[*sub], sub_state);
                    if *positive {
                        match hit {
                            Some(sub_state) => st.caps = sub_state.caps,
                            None => failed = true,
                        }
                    } else if hit.is_some() {
                        failed = true;
                    }
                }
                Inst::Mark(slot) => st.aux[*slot] = st.pos,
                Inst::Progress { slot, back } => {
                    if st.aux[*slot] != st.pos {
                        next = *back;
                    }
                }
                Inst::Accept => return Some(st),
            }
            if failed {
                match stack.pop() {
                    Some((p, s)) => {
                        pc = p;
                        st = s;
                    }
                    None => return None,
                }
            } else {
                pc = next;
            }
        }
    }
}

fn compile_pattern(pattern: &str, ignore_case: bool) -> Regex {
    let mut parser = Parser::new(pattern);
    let branches = parser.parse_alt();
    let mut ctx = Ctx {
        progs: Vec::new(),
        aux_len: 0,
    };
    let mut prog = Vec::new();
    compile_alt(&branches, &mut prog, &mut ctx);
    prog.push(Inst::Accept);
    Regex {
        prog,
        progs: ctx.progs,
        ngroups: parser.ngroups,
        aux_len: ctx.aux_len,
        // Vim's \c wins over \C regardless of their order in the pattern.
        ci: if parser.saw_icase {
            true
        } else if parser.saw_noicase {
            false
        } else {
            ignore_case
        },
    }
}

fn search(re: &Regex, text: &str, from_byte: usize, multiline: bool) -> Option<Match> {
    let chars: Vec<char> = text.chars().collect();
    let mut offsets: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    let mut byte = 0usize;
    for ch in &chars {
        offsets.push(byte);
        byte += ch.len_utf8();
    }
    offsets.push(byte);
    let from_char = offsets
        .partition_point(|&off| off < from_byte)
        .min(chars.len());
    let budget = (START_BUDGET + BUDGET_PER_CHAR * chars.len() as u64).min(MAX_BUDGET);
    let matcher = Matcher {
        text: &chars,
        progs: &re.progs,
        ci: re.ci,
        multiline,
        budget: Cell::new(budget),
    };
    for start in from_char..=chars.len() {
        if matcher.budget.get() == 0 {
            return None;
        }
        let state = State {
            pos: start,
            caps: vec![None; re.ngroups * 2],
            zs: start,
            ze: None,
            aux: if re.aux_len == 0 {
                Vec::new()
            } else {
                vec![0; re.aux_len]
            },
        };
        if let Some(end_state) = matcher.exec(&re.prog, state) {
            let start_char = end_state.zs.min(chars.len());
            let end_char = end_state
                .ze
                .unwrap_or(end_state.pos)
                .max(start_char)
                .min(chars.len());
            let mut groups = Vec::with_capacity(re.ngroups);
            for pair in end_state.caps.chunks_exact(2) {
                let open = pair.first().copied().flatten();
                let close = pair.get(1).copied().flatten();
                groups.push(match (open, close) {
                    (Some(a), Some(b)) => Some((offsets[a], offsets[b])),
                    _ => None,
                });
            }
            return Some(Match {
                start: offsets[start_char],
                end: offsets[end_char],
                groups,
            });
        }
    }
    None
}

/// The text and byte ranges (whole match first, then groups) of the match
/// currently being replaced by `:s\=`.
type LastMatch = Option<(String, Vec<Option<(usize, usize)>>)>;

thread_local! {
    /// The match `submatch()` sees while a `:s\=` replacement is evaluated.
    static LAST_MATCH: std::cell::RefCell<LastMatch> = const { std::cell::RefCell::new(None) };
}

/// Remember the current match for `submatch()` (called by the `:`s\=` layer).
pub fn set_last_match(text: &str, groups: &[Option<(usize, usize)>]) {
    LAST_MATCH.with(|slot| {
        *slot.borrow_mut() = Some((text.to_string(), groups.to_vec()));
    });
}

/// Forget the remembered match.
pub fn clear_last_match() {
    LAST_MATCH.with(|slot| *slot.borrow_mut() = None);
}

/// `submatch({nr})`: the text of capture group {nr} of the last match.
pub fn submatch(nr: usize) -> String {
    LAST_MATCH.with(|slot| {
        let guard = slot.borrow();
        let Some((text, groups)) = guard.as_ref() else {
            return String::new();
        };
        // groups[0] is the whole match, groups[n] is capture group n.
        groups
            .get(nr)
            .and_then(|group| *group)
            .and_then(|(start, end)| text.get(start..end))
            .unwrap_or("")
            .to_string()
    })
}

/// A single regular-expression match.
///
/// start / end are byte offsets into the searched text; end is exclusive.
/// groups[i] is capture group i + 1 (byte offsets), or None when that group
/// did not participate in the match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    pub groups: Vec<Option<(usize, usize)>>,
}

/// Find the first match and its capture groups.
pub fn find_captures(pattern: &str, text: &str, ignore_case: bool) -> Option<Match> {
    let re = compile_pattern(pattern, ignore_case);
    search(&re, text, 0, false)
}

/// Find the first match in multiline mode: '^' and '$' bind to line starts and
/// line ends, and the '\_' atoms may cross line breaks.
pub fn find_multiline(pattern: &str, text: &str, ignore_case: bool) -> Option<Match> {
    let re = compile_pattern(pattern, ignore_case);
    search(&re, text, 0, true)
}

/// Find the first match at or after byte offset from.
pub fn find_from(pattern: &str, text: &str, from: usize, ignore_case: bool) -> Option<Match> {
    let re = compile_pattern(pattern, ignore_case);
    search(&re, text, from.min(text.len()), false)
}

/// Find the first match; returns (start, end) in byte offsets.
pub fn find(pattern: &str, text: &str, ignore_case: bool) -> Option<(usize, usize)> {
    find_captures(pattern, text, ignore_case).map(|m| (m.start, m.end))
}

/// Boolean match for '=~' / '!~'.
pub fn is_match(pattern: &str, text: &str, ignore_case: bool) -> bool {
    find_captures(pattern, text, ignore_case).is_some()
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

    #[test]
    fn captures_and_markers_are_available_internally() {
        let hit = find_captures("\\(a\\)b", "xab", false).unwrap();
        assert_eq!((hit.start, hit.end), (1, 3));
        assert_eq!(hit.groups, vec![Some((1, 2))]);
    }
}
