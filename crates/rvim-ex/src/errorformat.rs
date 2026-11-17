//! 'errorformat' parsing and matching for the quickfix list.
//!
//! Vim's 'errorformat' option is a comma separated list of scanf-like patterns
//! that describe how compiler output is turned into quickfix entries.  This
//! module implements the mini language from ":help errorformat" without a
//! regular expression engine: every alternative is compiled into a short list
//! of items that is matched against a line with plain backtracking.
//!
//! # Supported conversions
//!
//! %f (file name), %o (module), %l (line), %e (end line), %c (column),
//! %k (end column), %v (virtual column), %t (type), %n (error number),
//! %m (message), %r (rest of the line), %p (pointer line) and %s (search
//! text).
//!
//! The scanf form %*[^:], %*[^ ], %*\d and %*. skips a field, %% is a
//! literal percent sign, and %., %^, %$, %~, %[, %# and %> are accepted with
//! their Vim meaning.  %\X is a Vim character class (\d, \D, \w, \W, \s,
//! \S, \a, \l, \u, \x, \o, \h, \k, \i, \f, \p, \t, \n, \b, each in both
//! cases) and %[...] / %*[...] is a bracket class with ^ negation and a-z
//! ranges.  The %# suffix makes the preceding item match zero or more times;
//! %\+ (and %\?) applies the usual Vim quantifier to it.
//!
//! # Prefixes
//!
//! %- marks a line as ignored (ErrorItem::valid is false), %+ copies the
//! whole line into the message, %G is a general message and %A/%E/%W/%I/%N
//! set the entry type.  The file and directory prefixes %D, %X, %O, %P, %Q
//! and the continuation prefixes %C, %Z are parsed but produce entries with
//! valid == false: matching is stateless here, so there is no directory
//! stack and no multi-line message merging.
//!
//! # Deviations from Vim
//!
//! * Matching is anchored at the start of the line, but trailing text after
//!   the pattern is allowed (%m, %r and %s still consume the rest of the
//!   line).  An explicit %$ item does require the end of the line.
//! * %f is copied verbatim (no "~/" or environment expansion) and %s stores
//!   the raw search text instead of Vim's "^\V...\$" pattern.  A %f name is
//!   an arbitrary run of characters; the item that follows it ends the name
//!   through backtracking, where Vim also consults 'isfname' when %f is
//!   directly followed by another conversion.
//! * Duplicate conversions, an invalid pattern and a pattern without %f are
//!   reported as E372, E373 and E374.  Patterns that carry a type prefix
//!   (for example %C%m or %-G%.%#) may omit %f.
//! * Empty comma separated entries are ignored and matching is always
//!   case-insensitive, like Vim's "regmatch.rm_ic = TRUE".
//! * %.\d is accepted as a shorthand for %*\d (one or more digits); in Vim
//!   %. on its own means "any character".

/// One quickfix entry, modelled on Vim's "qfline_T".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorItem {
    pub filename: Option<String>,
    pub lnum: i64,
    pub end_lnum: i64,
    pub col: i64,
    pub end_col: i64,
    pub text: String,
    pub entry_type: char,
    pub nr: i64,
    pub module: Option<String>,
    pub pattern: Option<String>,
    pub valid: bool,
}

/// A parsed 'errorformat' value.
#[derive(Clone, Debug, Default)]
pub struct ErrorFormat {
    alternatives: Vec<Alternative>,
}

impl ErrorFormat {
    /// Compile an 'errorformat' string into matchable alternatives.
    ///
    /// # Errors
    ///
    /// Returns a message starting with E372 for a duplicated conversion,
    /// E373 for an invalid pattern and E374 for a pattern without %f.
    pub fn parse(spec: &str) -> Result<ErrorFormat, String> {
        let chars: Vec<char> = spec.chars().collect();
        let mut alternatives = Vec::new();
        for part in split_alternatives(&chars) {
            if part.is_empty() {
                continue;
            }
            alternatives.push(parse_alternative(&part)?);
        }
        Ok(ErrorFormat { alternatives })
    }

    /// Match one line against the alternatives, in order.
    ///
    /// The first alternative that matches wins.  A trailing line break is
    /// stripped, like Vim does when reading an error file.
    pub fn match_line(&self, line: &str) -> Option<ErrorItem> {
        let line = line.trim_end_matches(['\n', '\r']);
        let chars: Vec<char> = line.chars().collect();
        for alt in &self.alternatives {
            let mut caps = vec![None; alt.items.len()];
            if match_items(&alt.items, 0, 0, &chars, &mut caps) {
                return Some(alt.build_item(&chars, &caps, line));
            }
        }
        None
    }

    /// Number of comma separated alternatives.
    pub fn len(&self) -> usize {
        self.alternatives.len()
    }

    /// True when the format has no alternative at all.
    pub fn is_empty(&self) -> bool {
        self.alternatives.is_empty()
    }
}

/// Vim's default 'errorformat' (DFLT_EFM from src/option.h, Unix build).
///
/// The Windows build of Vim uses a slightly longer variant; the Unix one is
/// the portable default and is what this function returns.
pub fn default_errorformat() -> &'static str {
    DEFAULT_ERRORFORMAT
}

const DEFAULT_ERRORFORMAT: &str = r#"%*[^"]"%f"%*\D%l: %m,"%f"%*\D%l: %m,%-G%f:%l: (Each undeclared identifier is reported only once,%-G%f:%l: for each function it appears in.),%-GIn file included from %f:%l:%c:,%-GIn file included from %f:%l:%c\,,%-GIn file included from %f:%l:%c,%-GIn file included from %f:%l,%-G%*[ ]from %f:%l:%c,%-G%*[ ]from %f:%l:,%-G%*[ ]from %f:%l\,,%-G%*[ ]from %f:%l,%f:%l:%c:%m,%f(%l):%m,%f:%l:%m,"%f"\, line %l%*\D%c%*[^ ] %m,%D%*\a[%*\d]: Entering directory %*[`']%f',%X%*\a[%*\d]: Leaving directory %*[`']%f',%D%*\a: Entering directory %*[`']%f',%X%*\a: Leaving directory %*[`']%f',%DMaking %*\a in %f,%f|%l| %m"#;

/// One comma separated pattern of the 'errorformat' option.
#[derive(Clone, Debug)]
struct Alternative {
    prefix: char,
    flags: char,
    items: Vec<Item>,
}

impl Alternative {
    fn build_item(&self, chars: &[char], caps: &[Option<(usize, usize)>], line: &str) -> ErrorItem {
        let mut item = ErrorItem::default();
        let mut has_message = false;
        if matches!(self.prefix, 'E' | 'W' | 'I' | 'N') {
            item.entry_type = self.prefix;
        }
        for (idx, node) in self.items.iter().enumerate() {
            let field = match &node.kind {
                ItemKind::Field(field) => *field,
                _ => continue,
            };
            let Some((start, end)) = caps[idx] else {
                continue;
            };
            let text: String = chars[start..end].iter().collect();
            match field {
                Field::File => item.filename = Some(text),
                Field::Module => item.module = Some(text),
                Field::Line => item.lnum = parse_number(&text),
                Field::EndLine => item.end_lnum = parse_number(&text),
                Field::Column | Field::VisColumn => item.col = parse_number(&text),
                Field::EndColumn => item.end_col = parse_number(&text),
                Field::Number => item.nr = parse_number(&text),
                Field::Type => {
                    if let Some(c) = text.chars().next() {
                        item.entry_type = c;
                    }
                }
                Field::Message | Field::Rest => {
                    item.text = text;
                    has_message = true;
                }
                Field::Search => item.pattern = Some(text),
                Field::Pointer => item.col = pointer_column(&text),
            }
        }
        item.valid =
            self.flags != '-' && !matches!(self.prefix, 'D' | 'X' | 'O' | 'P' | 'Q' | 'C' | 'Z');
        if self.flags == '+' || (!item.valid && !has_message) {
            item.text = line.to_string();
        }
        item
    }
}

fn parse_number(text: &str) -> i64 {
    text.parse::<i64>().unwrap_or(i64::MAX)
}

/// Column of a "%p" pointer line: one based, with tabs expanded to multiples
/// of eight.
fn pointer_column(text: &str) -> i64 {
    let mut col: i64 = 0;
    for c in text.chars() {
        col += 1;
        if c == '\t' {
            col += 7;
            col -= col % 8;
        }
    }
    col + 1
}

/// Split the option value on unescaped commas, keeping commas that appear
/// inside a %[...] character class.  Spaces after a comma are skipped, like
/// Vim's skip_to_option_part().
fn split_alternatives(chars: &[char]) -> Vec<Vec<char>> {
    let mut parts: Vec<Vec<char>> = Vec::new();
    let mut current: Vec<char> = Vec::new();
    let mut in_class = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_class {
            current.push(c);
            if c == '\\' && i + 1 < chars.len() {
                i += 1;
                current.push(chars[i]);
            } else if c == ']' {
                in_class = false;
            }
            i += 1;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            current.push(c);
            current.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == ',' {
            parts.push(std::mem::take(&mut current));
            i += 1;
            while i < chars.len() && chars[i] == ' ' {
                i += 1;
            }
            continue;
        }
        if c == '%' {
            let star_class = i + 2 < chars.len() && chars[i + 1] == '*' && chars[i + 2] == '[';
            if (i + 1 < chars.len() && chars[i + 1] == '[') || star_class {
                in_class = true;
            }
        }
        current.push(c);
        i += 1;
    }
    parts.push(current);
    parts
}

fn parse_alternative(part: &[char]) -> Result<Alternative, String> {
    let spec: String = part.iter().collect();
    let mut flags = '\0';
    let mut prefix = '\0';
    let mut start = 0;
    if part.first() == Some(&'%') {
        let mut j = 1;
        let mut candidate = '\0';
        if j < part.len() && (part[j] == '+' || part[j] == '-') {
            candidate = part[j];
            j += 1;
        }
        if j < part.len() && is_prefix_letter(part[j]) {
            flags = candidate;
            prefix = part[j];
            start = j + 1;
        } else if candidate != '\0' {
            return Err(format!(
                "E373: Invalid %{candidate} prefix in format string: {spec}"
            ));
        }
    }

    let mut items: Vec<Item> = Vec::new();
    let mut seen: u16 = 0;
    let mut has_file = false;
    let mut i = start;
    while i < part.len() {
        let c = part[i];
        if c == '%' {
            if i + 1 >= part.len() {
                return Err(format!("E373: Trailing % in format string: {spec}"));
            }
            let conv = part[i + 1];
            match conv {
                '%' => {
                    push_literal(&mut items, '%');
                    i += 2;
                }
                '.' => {
                    if i + 3 < part.len() && part[i + 2] == '\\' && is_class_letter(part[i + 3]) {
                        push_class(
                            &mut items,
                            CharClass::Shorthand(part[i + 3]),
                            Rep::OneOrMore,
                        );
                        i += 4;
                    } else {
                        push_class(&mut items, CharClass::Any, Rep::One);
                        i += 2;
                    }
                }
                '^' => i += 2,
                '$' => {
                    items.push(Item::new(ItemKind::AnchorEnd));
                    i += 2;
                }
                '~' => {
                    push_literal(&mut items, '~');
                    i += 2;
                }
                '[' => {
                    let (class, next) = parse_bracket(part, i + 2, &spec)?;
                    push_class(&mut items, class, Rep::One);
                    i = next;
                }
                '#' => {
                    apply_rep(&mut items, Rep::ZeroOrMore, &spec)?;
                    i += 2;
                }
                '>' => i += 2,
                '*' => {
                    if i + 2 >= part.len() {
                        return Err(format!("E373: Unsupported %* in format string: {spec}"));
                    }
                    let kind = part[i + 2];
                    if kind == '[' {
                        let (class, next) = parse_bracket(part, i + 3, &spec)?;
                        push_class(&mut items, class, Rep::OneOrMore);
                        i = next;
                    } else if kind == '\\' {
                        if i + 3 >= part.len() || !is_class_letter(part[i + 3]) {
                            return Err(format!(
                                "E373: Unsupported %* conversion in format string: {spec}"
                            ));
                        }
                        push_class(
                            &mut items,
                            CharClass::Shorthand(part[i + 3]),
                            Rep::OneOrMore,
                        );
                        i += 4;
                    } else if kind == '.' {
                        push_class(&mut items, CharClass::Any, Rep::OneOrMore);
                        i += 3;
                    } else {
                        return Err(format!(
                            "E373: Unsupported %* conversion in format string: {spec}"
                        ));
                    }
                }
                '\\' => {
                    if i + 2 >= part.len() {
                        return Err(format!("E373: Trailing %\\ in format string: {spec}"));
                    }
                    let letter = part[i + 2];
                    if letter == '+' {
                        apply_rep(&mut items, Rep::OneOrMore, &spec)?;
                        i += 3;
                    } else if letter == '?' {
                        apply_rep(&mut items, Rep::ZeroOrOne, &spec)?;
                        i += 3;
                    } else if is_class_letter(letter) {
                        push_class(&mut items, CharClass::Shorthand(letter), Rep::One);
                        i += 3;
                    } else {
                        push_literal(&mut items, letter);
                        i += 3;
                    }
                }
                '+' | '-' => {
                    return Err(format!("E373: Unexpected %{conv} in format string: {spec}"));
                }
                _ => {
                    let Some(field) = field_from_char(conv) else {
                        return Err(format!("E373: Unexpected %{conv} in format string: {spec}"));
                    };
                    let bit = 1u16 << field as u16;
                    if seen & bit != 0 {
                        return Err(format!("E372: Too many %{conv} in format string: {spec}"));
                    }
                    seen |= bit;
                    if field == Field::File {
                        has_file = true;
                    }
                    let lazy = field == Field::File
                        && i + 2 < part.len()
                        && part[i + 2] != '%'
                        && part[i + 2] != '\\';
                    items.push(Item {
                        kind: ItemKind::Field(field),
                        rep: Rep::One,
                        lazy,
                    });
                    i += 2;
                }
            }
        } else if c == '\\' {
            if i + 1 >= part.len() {
                return Err(format!("E373: Trailing \\ in format string: {spec}"));
            }
            push_literal(&mut items, part[i + 1]);
            i += 2;
        } else {
            push_literal(&mut items, c);
            i += 1;
        }
    }

    if !has_file && prefix == '\0' {
        return Err(format!("E374: Missing %f in format string: {spec}"));
    }

    Ok(Alternative {
        prefix,
        flags,
        items,
    })
}

fn is_prefix_letter(c: char) -> bool {
    matches!(
        c,
        'D' | 'X' | 'A' | 'E' | 'W' | 'I' | 'N' | 'C' | 'Z' | 'G' | 'O' | 'P' | 'Q'
    )
}

fn is_class_letter(c: char) -> bool {
    matches!(
        c,
        'd' | 'D'
            | 'w'
            | 'W'
            | 's'
            | 'S'
            | 'a'
            | 'A'
            | 'l'
            | 'L'
            | 'u'
            | 'U'
            | 'x'
            | 'X'
            | 'o'
            | 'O'
            | 'h'
            | 'H'
            | 'k'
            | 'K'
            | 'i'
            | 'I'
            | 'f'
            | 'F'
            | 'p'
            | 'P'
            | 't'
            | 'T'
            | 'n'
            | 'N'
            | 'b'
            | 'B'
    )
}

fn field_from_char(c: char) -> Option<Field> {
    Some(match c {
        'f' => Field::File,
        'o' => Field::Module,
        'l' => Field::Line,
        'e' => Field::EndLine,
        'c' => Field::Column,
        'k' => Field::EndColumn,
        't' => Field::Type,
        'm' => Field::Message,
        'r' => Field::Rest,
        'p' => Field::Pointer,
        'v' => Field::VisColumn,
        's' => Field::Search,
        'n' => Field::Number,
        _ => return None,
    })
}

fn push_literal(items: &mut Vec<Item>, c: char) {
    items.push(Item::new(ItemKind::Literal(c)));
}

fn push_class(items: &mut Vec<Item>, class: CharClass, rep: Rep) {
    items.push(Item {
        kind: ItemKind::Class(class),
        rep,
        lazy: false,
    });
}

fn apply_rep(items: &mut [Item], rep: Rep, spec: &str) -> Result<(), String> {
    let Some(last) = items.last_mut() else {
        return Err(format!(
            "E373: Quantifier without an item in format string: {spec}"
        ));
    };
    if matches!(last.kind, ItemKind::Field(_)) || last.rep != Rep::One {
        return Err(format!(
            "E373: Misplaced quantifier in format string: {spec}"
        ));
    }
    last.rep = rep;
    Ok(())
}

/// Parse a bracket class; "start" points just after the opening "[".
fn parse_bracket(part: &[char], start: usize, spec: &str) -> Result<(CharClass, usize), String> {
    let mut i = start;
    let mut negated = false;
    if i < part.len() && part[i] == '^' {
        negated = true;
        i += 1;
    }
    let mut raw: Vec<(char, bool)> = Vec::new();
    while i < part.len() && part[i] != ']' {
        let escaped = part[i] == '\\' && i + 1 < part.len();
        if escaped {
            i += 1;
        }
        raw.push((part[i], escaped));
        i += 1;
    }
    if i >= part.len() {
        return Err(format!("E373: Missing ] in format string: {spec}"));
    }
    if raw.is_empty() {
        return Err(format!("E373: Empty [] in format string: {spec}"));
    }
    let mut atoms = Vec::new();
    let mut k = 0;
    while k < raw.len() {
        if k + 2 < raw.len() && raw[k + 1].0 == '-' && !raw[k + 1].1 {
            atoms.push(ClassAtom::Range(raw[k].0, raw[k + 2].0));
            k += 3;
        } else {
            atoms.push(ClassAtom::Char(raw[k].0));
            k += 1;
        }
    }
    Ok((CharClass::Bracket { negated, atoms }, i + 1))
}

fn match_items(
    items: &[Item],
    index: usize,
    pos: usize,
    chars: &[char],
    caps: &mut [Option<(usize, usize)>],
) -> bool {
    if index == items.len() {
        return true;
    }
    let item = &items[index];
    for end in item.candidate_ends(chars, pos) {
        if matches!(item.kind, ItemKind::Field(_)) {
            caps[index] = Some((pos, end));
        }
        if match_items(items, index + 1, end, chars, caps) {
            return true;
        }
        caps[index] = None;
    }
    false
}

#[derive(Clone, Debug)]
struct Item {
    kind: ItemKind,
    rep: Rep,
    /// Prefer the shortest match for variable length fields (Vim's ".\{-1,}").
    lazy: bool,
}

impl Item {
    fn new(kind: ItemKind) -> Item {
        Item {
            kind,
            rep: Rep::One,
            lazy: false,
        }
    }

    fn candidate_ends(&self, chars: &[char], pos: usize) -> Vec<usize> {
        match &self.kind {
            ItemKind::Literal(c) => {
                let literal = *c;
                self.run_ends(chars, pos, |ch| eq_ignore_case(ch, literal))
            }
            ItemKind::Class(class) => self.run_ends(chars, pos, |ch| class.matches(ch)),
            ItemKind::Field(field) => field_ends(*field, chars, pos, self.lazy),
            ItemKind::AnchorEnd => {
                if pos == chars.len() {
                    vec![pos]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn run_ends(&self, chars: &[char], pos: usize, pred: impl Fn(char) -> bool) -> Vec<usize> {
        let mut run = pos;
        while run < chars.len() && pred(chars[run]) {
            run += 1;
        }
        match self.rep {
            Rep::One => {
                if run > pos {
                    vec![pos + 1]
                } else {
                    Vec::new()
                }
            }
            Rep::ZeroOrOne => {
                let mut ends = Vec::with_capacity(2);
                if run > pos {
                    ends.push(pos + 1);
                }
                ends.push(pos);
                ends
            }
            Rep::ZeroOrMore => (pos..=run).rev().collect(),
            Rep::OneOrMore => {
                if run > pos {
                    (pos + 1..=run).rev().collect()
                } else {
                    Vec::new()
                }
            }
        }
    }
}

fn field_ends(field: Field, chars: &[char], pos: usize, lazy: bool) -> Vec<usize> {
    match field {
        Field::Type => {
            if pos < chars.len() {
                vec![pos + 1]
            } else {
                Vec::new()
            }
        }
        Field::Line
        | Field::EndLine
        | Field::Column
        | Field::EndColumn
        | Field::VisColumn
        | Field::Number => {
            let mut end = pos;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            if end == pos {
                Vec::new()
            } else {
                (pos + 1..=end).rev().collect()
            }
        }
        Field::Pointer => {
            let mut end = pos;
            while end < chars.len() && matches!(chars[end], '-' | ' ' | '\t' | '.') {
                end += 1;
            }
            (pos..=end).rev().collect()
        }
        Field::Message | Field::Search => {
            if pos < chars.len() {
                (pos + 1..=chars.len()).rev().collect()
            } else {
                Vec::new()
            }
        }
        Field::Rest => (pos..=chars.len()).rev().collect(),
        Field::File | Field::Module => {
            if pos >= chars.len() {
                Vec::new()
            } else if lazy {
                (pos + 1..=chars.len()).collect()
            } else {
                (pos + 1..=chars.len()).rev().collect()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    File,
    Module,
    Line,
    EndLine,
    Column,
    EndColumn,
    Type,
    Message,
    Rest,
    Pointer,
    VisColumn,
    Search,
    Number,
}

#[derive(Clone, Debug)]
enum ItemKind {
    Literal(char),
    Class(CharClass),
    Field(Field),
    AnchorEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rep {
    One,
    ZeroOrOne,
    ZeroOrMore,
    OneOrMore,
}

#[derive(Clone, Debug)]
enum CharClass {
    Any,
    Bracket {
        negated: bool,
        atoms: Vec<ClassAtom>,
    },
    Shorthand(char),
}

impl CharClass {
    fn matches(&self, c: char) -> bool {
        match self {
            CharClass::Any => true,
            CharClass::Bracket { negated, atoms } => {
                let hit = atoms.iter().any(|atom| match atom {
                    ClassAtom::Char(ch) => eq_ignore_case(c, *ch),
                    ClassAtom::Range(lo, hi) => in_range_ignore_case(c, *lo, *hi),
                });
                hit != *negated
            }
            CharClass::Shorthand(letter) => shorthand_matches(*letter, c),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ClassAtom {
    Char(char),
    Range(char, char),
}

fn eq_ignore_case(a: char, b: char) -> bool {
    a == b || a.eq_ignore_ascii_case(&b)
}

fn in_range_ignore_case(c: char, lo: char, hi: char) -> bool {
    if (lo..=hi).contains(&c) {
        return true;
    }
    (lo..=hi).contains(&c.to_ascii_lowercase()) || (lo..=hi).contains(&c.to_ascii_uppercase())
}

fn shorthand_matches(letter: char, c: char) -> bool {
    match letter {
        'd' => c.is_ascii_digit(),
        'D' => !c.is_ascii_digit(),
        'w' => is_word_char(c),
        'W' => !is_word_char(c),
        's' => c == ' ' || c == '\t',
        'S' => c != ' ' && c != '\t',
        'a' => c.is_alphabetic(),
        'A' => !c.is_alphabetic(),
        'l' => c.is_lowercase(),
        'L' => !c.is_lowercase(),
        'u' => c.is_uppercase(),
        'U' => !c.is_uppercase(),
        'x' => c.is_ascii_hexdigit(),
        'X' => !c.is_ascii_hexdigit(),
        'o' => matches!(c, '0'..='7'),
        'O' => !matches!(c, '0'..='7'),
        'h' | 'k' | 'i' => is_word_char(c),
        'H' | 'K' | 'I' => !is_word_char(c),
        'f' => !c.is_whitespace() && !c.is_control(),
        'F' => c.is_whitespace() || c.is_control(),
        'p' => !c.is_control(),
        'P' => c.is_control(),
        't' => c == '\t',
        'T' => c != '\t',
        'n' => c == '\n',
        'N' => c != '\n',
        'b' => c == '\u{8}',
        'B' => c != '\u{8}',
        _ => false,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn efm(spec: &str) -> ErrorFormat {
        ErrorFormat::parse(spec).unwrap_or_else(|err| panic!("{spec:?} should parse: {err}"))
    }

    #[test]
    fn default_errorformat_is_vim_default() {
        // v8.2.5172 src/option.h, the "Unix, probably" DFLT_EFM runtime value.
        let expected = r#"%*[^"]"%f"%*\D%l: %m,"%f"%*\D%l: %m,%-G%f:%l: (Each undeclared identifier is reported only once,%-G%f:%l: for each function it appears in.),%-GIn file included from %f:%l:%c:,%-GIn file included from %f:%l:%c\,,%-GIn file included from %f:%l:%c,%-GIn file included from %f:%l,%-G%*[ ]from %f:%l:%c,%-G%*[ ]from %f:%l:,%-G%*[ ]from %f:%l\,,%-G%*[ ]from %f:%l,%f:%l:%c:%m,%f(%l):%m,%f:%l:%m,"%f"\, line %l%*\D%c%*[^ ] %m,%D%*\a[%*\d]: Entering directory %*[`']%f',%X%*\a[%*\d]: Leaving directory %*[`']%f',%D%*\a: Entering directory %*[`']%f',%X%*\a: Leaving directory %*[`']%f',%DMaking %*\a in %f,%f|%l| %m"#;
        assert_eq!(default_errorformat(), expected);
    }

    #[test]
    fn default_format_compiles_to_all_alternatives() {
        let format = efm(default_errorformat());
        assert_eq!(format.len(), 22);
        assert!(!format.is_empty());
    }

    #[test]
    fn default_format_parses_line_with_column() {
        let format = efm(default_errorformat());
        let item = format
            .match_line("a.c:12:3: error: bad thing")
            .expect("gcc style line should match");
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.lnum, 12);
        assert_eq!(item.col, 3);
        // The ':' before the message is part of the format, so the space that
        // follows it belongs to %m.
        assert_eq!(item.text, " error: bad thing");
        assert_eq!(item.entry_type, '\0');
        assert!(item.valid);
    }

    #[test]
    fn default_format_parses_line_without_column() {
        let format = efm(default_errorformat());
        let item = format
            .match_line("a.c:12: error: no column")
            .expect("gcc style line without column should match");
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.lnum, 12);
        assert_eq!(item.col, 0);
        assert_eq!(item.text, " error: no column");
        assert!(item.valid);
    }

    #[test]
    fn gcc_type_word_and_number() {
        let format = efm("%f:%l:%c: %t%*[^:]: %m");
        let item = format
            .match_line("a.c:4:9: warning: unused variable")
            .expect("typed gcc line should match");
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.lnum, 4);
        assert_eq!(item.col, 9);
        assert_eq!(item.entry_type, 'w');
        assert_eq!(item.text, "unused variable");
    }

    #[test]
    fn type_and_error_number_are_extracted() {
        let format = efm("%f(%l) %t%n: %m");
        let item = format
            .match_line("a.c(12) E42: boom")
            .expect("typed line should match");
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.lnum, 12);
        assert_eq!(item.entry_type, 'E');
        assert_eq!(item.nr, 42);
        assert_eq!(item.text, "boom");
    }

    #[test]
    fn grep_style_match() {
        let format = efm("%f:%l:%m");
        let item = format
            .match_line("file.txt:5:match text")
            .expect("grep style line should match");
        assert_eq!(item.filename.as_deref(), Some("file.txt"));
        assert_eq!(item.lnum, 5);
        assert_eq!(item.col, 0);
        assert_eq!(item.text, "match text");
    }

    #[test]
    fn search_text_sets_pattern() {
        let format = efm("%f:%l:%s");
        let item = format
            .match_line("a.c:5:needle here")
            .expect("line should match");
        assert_eq!(item.pattern.as_deref(), Some("needle here"));
        assert_eq!(item.text, "");
    }

    #[test]
    fn module_is_extracted() {
        let format = efm("%o:%f:%l:%m");
        let item = format
            .match_line("mymod:dir/a.c:3: oops")
            .expect("line with a module should match");
        assert_eq!(item.module.as_deref(), Some("mymod"));
        assert_eq!(item.filename.as_deref(), Some("dir/a.c"));
        assert_eq!(item.lnum, 3);
        assert_eq!(item.text, " oops");
    }

    #[test]
    fn rest_of_line_is_captured() {
        let format = efm("%f:%l:%r");
        let item = format
            .match_line("a.c:9:tail text")
            .expect("line should match");
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.lnum, 9);
        assert_eq!(item.text, "tail text");
    }

    #[test]
    fn percent_escapes_are_literal() {
        let format = efm("%f:%l: 100%% %m");
        let item = format
            .match_line("a.c:7: 100% done")
            .expect("literal percent line should match");
        assert_eq!(item.text, "done");
    }

    #[test]
    fn ignored_line_reports_invalid_item() {
        let format = efm("%-G%f:%l: ignore me");
        let item = format
            .match_line("a.c:3: ignore me")
            .expect("ignored lines are still reported as an item");
        assert!(!item.valid);
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.lnum, 3);
        assert_eq!(item.text, "a.c:3: ignore me");
    }

    #[test]
    fn general_ignore_pattern_matches_everything() {
        let format = efm("%-G%.%#");
        assert_eq!(format.len(), 1);
        let item = format
            .match_line("anything at all")
            .expect("general ignore should match");
        assert!(!item.valid);
    }

    #[test]
    fn general_message_uses_whole_line() {
        let format = efm("%+G%.%#");
        let item = format
            .match_line("compiler version 1.2")
            .expect("general message should match");
        assert!(item.valid);
        assert_eq!(item.text, "compiler version 1.2");
    }

    #[test]
    fn scanf_skip_of_digits() {
        let format = efm(r"%f:%l: E%*\d: %m");
        let item = format
            .match_line("a.c:7: E1234: bad")
            .expect("line with a skipped number should match");
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.lnum, 7);
        assert_eq!(item.text, "bad");
    }

    #[test]
    fn digit_class_alias() {
        let format = efm(r"%f:%l: %.\d: %m");
        let item = format
            .match_line("a.c:3: 12345: msg")
            .expect("line with a digit run should match");
        assert_eq!(item.text, "msg");
    }

    #[test]
    fn bracket_class_with_repetition() {
        let format = efm(r"%f:%l: %[a-z]%\+: %m");
        let item = format
            .match_line("a.c:2: abc: msg")
            .expect("line with a class should match");
        assert_eq!(item.text, "msg");
    }

    #[test]
    fn duplicate_file_conversion_is_e372() {
        let err = ErrorFormat::parse("%f:%f:%l").expect_err("duplicate %f must fail");
        assert!(err.starts_with("E372"), "unexpected message: {err}");
    }

    #[test]
    fn missing_file_conversion_is_e374() {
        let err = ErrorFormat::parse("%l: %m").expect_err("format without %f must fail");
        assert!(err.starts_with("E374"), "unexpected message: {err}");
    }

    #[test]
    fn invalid_format_is_e373() {
        let err = ErrorFormat::parse("%f:%q").expect_err("unknown conversion must fail");
        assert!(err.starts_with("E373"), "unexpected message: {err}");
        let err = ErrorFormat::parse("%f:%[abc").expect_err("unterminated class must fail");
        assert!(err.starts_with("E373"), "unexpected message: {err}");
    }

    #[test]
    fn windows_path_with_drive_letter() {
        let format = efm("%f:%l:%c: %m");
        let item = format
            .match_line(r"C:\dir\a.c:10:4: error: x")
            .expect("windows path should match");
        assert_eq!(item.filename.as_deref(), Some(r"C:\dir\a.c"));
        assert_eq!(item.lnum, 10);
        assert_eq!(item.col, 4);
        assert_eq!(item.text, "error: x");
    }

    #[test]
    fn alternatives_are_tried_in_order() {
        // The first alternative wins even though the second would also match.
        let format = efm("%f:%l:%m,%f:%l: %m");
        let item = format.match_line("a.c:3: boom").expect("line should match");
        assert_eq!(item.text, " boom");

        // When the first alternative cannot match, the second one is used.
        let format = efm("%f:%l:%c: %m,%f:%l: %m");
        let item = format.match_line("a.c:3: boom").expect("line should match");
        assert_eq!(item.col, 0);
        assert_eq!(item.text, "boom");
    }

    #[test]
    fn unmatched_line_returns_none() {
        let format = efm("%f:%l:%m");
        assert!(format.match_line("completely unrelated").is_none());
        assert!(format.match_line("").is_none());
        assert!(format.match_line("a.c:notanumber:x").is_none());
    }

    #[test]
    fn pointer_line_sets_column() {
        let format = efm("%f:%l: %m,%-G%p^");
        let item = format
            .match_line("    ^")
            .expect("pointer line should match");
        assert_eq!(item.col, 5);
        assert!(!item.valid);
    }

    #[test]
    fn comma_inside_class_does_not_split_alternatives() {
        let format = efm("%f:%l: %*[^,] %m");
        assert_eq!(format.len(), 1);
        let item = format
            .match_line("a.c:1: abc def")
            .expect("line should match");
        assert_eq!(item.text, "def");
    }

    #[test]
    fn literal_matching_ignores_case() {
        let format = efm("Error in %f:%l: %m");
        let item = format
            .match_line("error in a.c:3: boom")
            .expect("case insensitive line should match");
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.text, "boom");
    }

    #[test]
    fn empty_format_has_no_alternatives() {
        let format = ErrorFormat::parse("").expect("empty format is an empty format");
        assert!(format.is_empty());
        assert_eq!(format.len(), 0);
        assert!(format.match_line("anything").is_none());
        assert!(ErrorFormat::default().is_empty());
    }

    #[test]
    fn virtual_column_is_reported_as_column() {
        let format = efm("%f:%l:%v:%m");
        let item = format
            .match_line("a.c:1:8:XX")
            .expect("virtual column line should match");
        assert_eq!(item.filename.as_deref(), Some("a.c"));
        assert_eq!(item.lnum, 1);
        assert_eq!(item.col, 8);
        assert_eq!(item.text, "XX");
    }

    #[test]
    fn default_format_ignores_included_from_lines() {
        let format = efm(default_errorformat());
        let item = format
            .match_line("In file included from foo.c:3:")
            .expect("gcc include line should match an ignored alternative");
        assert!(!item.valid);
        assert_eq!(item.filename.as_deref(), Some("foo.c"));
        assert_eq!(item.lnum, 3);
    }

    #[test]
    fn trailing_line_break_is_stripped() {
        let format = efm("%f:%l:%m");
        let item = format
            .match_line("a.c:3: boom\n")
            .expect("line should match");
        assert_eq!(item.text, " boom");
    }

    #[test]
    fn end_anchor_requires_end_of_line() {
        let format = efm("%f:%l%$");
        assert!(format.match_line("a.c:12").is_some());
        assert!(format.match_line("a.c:12 trailing").is_none());
    }
}
