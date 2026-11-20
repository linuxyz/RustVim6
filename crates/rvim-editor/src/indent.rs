//! C indenting and smart indenting (reference: indent.c, cindent.c).
//!
//! The single home for the indentation rules so the insert engine and the
//! cindent() builtin share one implementation.  cindent() follows Vim's
//! get_c_indent() for the common C/C++ constructs (braces, switch labels,
//! jump labels, preprocessor lines, statement continuations and the usual
//! 'cinoptions' flags); new_line_indent() picks those rules, then
//! 'smartindent', then 'autoindent'.

use crate::Editor;

/// The 'cinoptions' values that change our rules.  Letters without a value
/// are parsed (their number is consumed) and ignored, like Vim's parse_cino()
/// does for flags this implementation does not need.
#[derive(Debug, Clone)]
struct Cino {
    /// '>' - spaces from an opening brace to the block indent.
    level: i64,
    /// 'e' - imaginary edge of an opening brace at the end of a line.
    open_imag: i64,
    /// 'n' - extra indent for a statement without an opening brace.
    no_brace: i64,
    /// 'f' - column of an opening brace at the start of the file.
    first_open: i64,
    /// '{' - extra indent for a leftmost opening brace.
    open_extra: i64,
    /// '}' - extra indent for a closing brace.
    close_extra: i64,
    /// '^' - imaginary edge of an opening brace in column 0.
    open_left_imag: i64,
    /// 'L' - jump label shift; negative puts labels in column 0.
    jump_label: i64,
    /// True when 'L' was given explicitly (otherwise Vim's -1 default).
    jump_label_set: bool,
    /// ':' - indent of a switch label from the switch indent.
    case: i64,
    /// '=' - indent of the code after a switch label.
    case_code: i64,
    /// 'g' - indent of a scope declaration label (public:, ...).
    scopedecl: i64,
    /// True when 'g' was given explicitly.
    scopedecl_set: bool,
    /// 'h' - indent of the code after a scope declaration label.
    scopedecl_code: i64,
    /// 'p' - indent of K&R-style function parameters.
    param: i64,
    /// 't' - indent of a function type specifier line.
    func_type: i64,
    /// 'i' - indent of a C++ constructor initializer list.
    cpp_baseclass: i64,
    /// '+' - extra indent for a continuation line.
    continuation: i64,
    /// '(' - extra indent after an unclosed parenthesis.
    unclosed: i64,
    /// 'u' - extra indent for each nested unclosed parenthesis.
    unclosed2: i64,
    /// 'U' - non-zero selects the unclosed indent for every open parenthesis.
    unclosed_noignore: i64,
    /// 'm' - line up a closing parenthesis with the line of its match.
    matching_paren: i64,
    /// ')' - maximum number of lines to search for an open parenthesis.
    maxparen: i64,
    /// '*' - maximum number of lines to search backwards for context.
    maxcomment: i64,
    /// '#' - indent of a preprocessor line.
    hash_comment: i64,
    /// 'N' - extra indent inside a C++ namespace.
    cpp_namespace: i64,
    /// 'E' - extra indent inside an extern "C" block.
    cpp_extern_c: i64,
}

impl Cino {
    fn new(sw: i64) -> Self {
        Cino {
            level: sw,
            open_imag: 0,
            no_brace: 0,
            first_open: 0,
            open_extra: 0,
            close_extra: 0,
            open_left_imag: 0,
            jump_label: -1,
            jump_label_set: false,
            case: sw,
            case_code: sw,
            scopedecl: sw,
            scopedecl_set: false,
            scopedecl_code: sw,
            param: sw,
            func_type: sw,
            cpp_baseclass: sw,
            continuation: sw,
            unclosed: sw * 2,
            unclosed2: sw,
            unclosed_noignore: 0,
            matching_paren: 0,
            maxparen: 20,
            maxcomment: 70,
            hash_comment: 0,
            cpp_namespace: 0,
            cpp_extern_c: 0,
        }
    }

    /// parse_cino(): letters with an optional number, a '-' sign, and an 's'
    /// suffix that multiplies by 'shiftwidth' (".5s" adds a fraction).
    fn parse(text: &str, sw: i64) -> Self {
        let mut cino = Cino::new(sw);
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let key = bytes[i] as char;
            i += 1;
            let negative = i < bytes.len() && bytes[i] == b'-';
            if negative {
                i += 1;
            }
            let digits_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let had_digits = i > digits_start;
            let mut n = 0i64;
            for &digit in &bytes[digits_start..i] {
                n = n.saturating_mul(10).saturating_add(i64::from(digit - b'0'));
            }
            let mut fraction = 0i64;
            let mut digits_after_dot = 0u32;
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                for &digit in &bytes[start..i] {
                    fraction = fraction
                        .saturating_mul(10)
                        .saturating_add(i64::from(digit - b'0'));
                    digits_after_dot = digits_after_dot.saturating_add(1);
                }
            }
            if i < bytes.len() && bytes[i] == b's' {
                if had_digits {
                    n = n.saturating_mul(sw);
                    if digits_after_dot > 0 {
                        let divider = 10i64.saturating_pow(digits_after_dot);
                        n += (sw.saturating_mul(fraction) + divider / 2) / divider;
                    }
                } else {
                    n = sw;
                }
                i += 1;
            }
            if negative {
                n = -n;
            }
            match key {
                '>' => cino.level = n,
                'e' => cino.open_imag = n,
                'n' => cino.no_brace = n,
                'f' => cino.first_open = n,
                '{' => cino.open_extra = n,
                '}' => cino.close_extra = n,
                '^' => cino.open_left_imag = n,
                'L' => {
                    cino.jump_label = n;
                    cino.jump_label_set = true;
                }
                ':' => cino.case = n,
                '=' => cino.case_code = n,
                'g' => {
                    cino.scopedecl = n;
                    cino.scopedecl_set = true;
                }
                'h' => cino.scopedecl_code = n,
                'p' => cino.param = n,
                't' => cino.func_type = n,
                'i' => cino.cpp_baseclass = n,
                '+' => cino.continuation = n,
                '(' => cino.unclosed = n,
                'u' => cino.unclosed2 = n,
                'U' => cino.unclosed_noignore = n,
                'm' => cino.matching_paren = n,
                ')' => cino.maxparen = n,
                '*' => cino.maxcomment = n,
                '#' => cino.hash_comment = n,
                'N' => cino.cpp_namespace = n,
                'E' => cino.cpp_extern_c = n,
                _ => {}
            }
            if i < bytes.len() && bytes[i] == b',' {
                i += 1;
            }
        }
        cino
    }
}

/// Snapshot of the options both indenters need.
struct IndentOpts {
    sw: i64,
    ts: i64,
    expandtab: bool,
    shiftround: bool,
    autoindent: bool,
    cindent: bool,
    smartindent: bool,
    cinwords: Vec<String>,
    cino: Cino,
}

impl IndentOpts {
    fn from_editor(ed: &Editor) -> Self {
        let ts = ed.wopt_num("tabstop", 8).max(1);
        let mut sw = ed.wopt_num("shiftwidth", 8);
        if sw <= 0 {
            sw = ts;
        }
        let cinwords = ed
            .wopt_str("cinwords", "if,else,while,do,for,switch")
            .split(',')
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_string)
            .collect();
        IndentOpts {
            sw,
            ts,
            expandtab: ed.wopt_bool("expandtab", false),
            shiftround: ed.wopt_bool("shiftround", false),
            autoindent: ed.wopt_bool("autoindent", false),
            cindent: ed.wopt_bool("cindent", false),
            smartindent: ed.wopt_bool("smartindent", false),
            cinwords,
            cino: Cino::parse(&ed.wopt_str("cinoptions", ""), sw),
        }
    }
}

/// One source line with comments, strings and preprocessor lines blanked out.
#[derive(Clone)]
struct LineInfo {
    code: Vec<char>,
    preproc: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DelimKind {
    Brace,
    Paren,
    Bracket,
}

/// An unmatched delimiter found while scanning towards the target line.
#[derive(Clone)]
struct Delim {
    kind: DelimKind,
    line0: usize,
    /// Character index of the opener in its line (used by 'cino=(0').
    col: usize,
    base: i64,
    /// The opener is the last non-blank on its line (cin_ends_in()).
    open_at_eol: bool,
    /// The brace belongs to a switch statement.
    is_switch: bool,
    /// Indent of the most recent case/default label in this switch block.
    last_case: Option<i64>,
    /// Indent of the most recent scope declaration label in this block.
    last_scopedecl: Option<i64>,
}

/// The previous line that matters for indenting the next one.
#[derive(Clone)]
struct PrevLine {
    line0: usize,
    indent: i64,
    is_cinword: bool,
    is_switch: bool,
    is_case: bool,
    is_scopedecl: bool,
    terminated: bool,
    is_func_decl: bool,
}

/// Delimiter stack and last relevant line, built by scanning forward.
#[derive(Default)]
struct Scan {
    stack: Vec<Delim>,
    prev: Option<PrevLine>,
}

impl Scan {
    fn nearest_brace(&self) -> Option<&Delim> {
        self.stack.iter().rev().find(|d| d.kind == DelimKind::Brace)
    }

    fn nearest_switch_brace(&self) -> Option<&Delim> {
        self.stack
            .iter()
            .rev()
            .find(|d| d.kind == DelimKind::Brace && d.is_switch)
    }

    /// The innermost open parenthesis/bracket, when it is the innermost
    /// construct (a brace inside it wins).
    fn innermost_paren(&self) -> Option<&Delim> {
        self.stack
            .last()
            .filter(|d| matches!(d.kind, DelimKind::Paren | DelimKind::Bracket))
    }

    fn paren_nesting(&self) -> i64 {
        let count = self
            .stack
            .iter()
            .filter(|d| matches!(d.kind, DelimKind::Paren | DelimKind::Bracket))
            .count();
        i64::try_from(count.saturating_sub(1)).unwrap_or(0)
    }

    fn close(&mut self, kind: DelimKind) {
        if let Some(pos) = self.stack.iter().rposition(|d| d.kind == kind) {
            self.stack.truncate(pos);
        }
    }

    /// Track the delimiters on line line0 (called after its indent was
    /// computed).
    fn observe(&mut self, line0: usize, ctx: &Ctx, indent: i64) {
        let info = &ctx.infos[line0];
        if info.preproc {
            return;
        }
        let original: Vec<char> = ctx.lines[line0].chars().collect();
        let chars = &info.code;
        for (idx, &ch) in chars.iter().enumerate() {
            match ch {
                '(' | '[' => {
                    let kind = if ch == '(' {
                        DelimKind::Paren
                    } else {
                        DelimKind::Bracket
                    };
                    let tail: String = chars[idx + 1..].iter().collect();
                    self.stack.push(Delim {
                        kind,
                        line0,
                        col: idx,
                        base: indent,
                        open_at_eol: tail.trim().is_empty(),
                        is_switch: false,
                        last_case: None,
                        last_scopedecl: None,
                    });
                }
                '{' => {
                    let col = display_col(&original, idx, ctx.opts.ts);
                    let at_start = original[..idx].iter().all(|c| c.is_whitespace());
                    let mut base = if at_start {
                        if col == 0 {
                            ctx.opts.cino.open_left_imag
                        } else {
                            col
                        }
                    } else {
                        indent + ctx.opts.cino.open_imag
                    };
                    let seg_start = chars[..idx]
                        .iter()
                        .rposition(|c| matches!(c, ';' | '{' | '}'))
                        .map(|pos| pos + 1)
                        .unwrap_or(0);
                    let segment: String = chars[seg_start..idx].iter().collect();
                    let segment = segment.trim();
                    let is_switch = if segment.is_empty() {
                        self.prev
                            .as_ref()
                            .is_some_and(|prev| prev.is_switch && prev.line0 + 1 == line0)
                    } else {
                        starts_with_word(segment, "switch")
                    };
                    if !at_start {
                        if is_namespace_decl(segment) {
                            base += ctx.opts.cino.cpp_namespace;
                        } else if is_extern_c_decl(segment) {
                            base += ctx.opts.cino.cpp_extern_c;
                        }
                    }
                    self.stack.push(Delim {
                        kind: DelimKind::Brace,
                        line0,
                        col: idx,
                        base,
                        open_at_eol: false,
                        is_switch,
                        last_case: None,
                        last_scopedecl: None,
                    });
                }
                ')' => self.close(DelimKind::Paren),
                ']' => self.close(DelimKind::Bracket),
                '}' => self.close(DelimKind::Brace),
                _ => {}
            }
        }
    }

    /// Remember the last line that carries code (comments, blank lines and
    /// preprocessor lines are ignored, like the backwards scans in cindent.c).
    fn remember(&mut self, line0: usize, ctx: &Ctx, indent: i64) {
        let info = &ctx.infos[line0];
        if info.preproc {
            return;
        }
        let text = code_text(ctx, line0);
        if text.is_empty() {
            return;
        }
        if is_jump_label(&text)
            && !is_case_label(&text)
            && !is_scopedecl(&text)
            && after_label(&text).is_none()
        {
            return;
        }
        let first = word_at_start(&text);
        self.prev = Some(PrevLine {
            line0,
            indent,
            is_cinword: ctx.opts.cinwords.iter().any(|word| word == first),
            is_switch: first == "switch",
            is_case: is_case_label(&text),
            is_scopedecl: is_scopedecl(&text),
            terminated: is_terminated(&text),
            is_func_decl: looks_like_func_decl(&text),
        });
    }
}

/// Buffer lines with their blanked code, plus the options in effect.
struct Ctx<'a> {
    lines: &'a [String],
    infos: &'a [LineInfo],
    opts: &'a IndentOpts,
}

fn code_text(ctx: &Ctx, line0: usize) -> String {
    match ctx.infos.get(line0) {
        Some(info) => info.code.iter().collect::<String>().trim().to_string(),
        None => String::new(),
    }
}

fn buffer_lines(ed: &Editor) -> Vec<String> {
    let buf = ed.current_buffer();
    let count = buf.line_count();
    let mut lines = Vec::with_capacity(count);
    for line0 in 0..count {
        lines.push(buf.line_str(line0).map(str::to_string).unwrap_or_default());
    }
    lines
}

/// Blank out comments and literals so delimiters and words inside them do not
/// confuse the structural scan.  Preprocessor directives are blanked whole.
fn scan_lines(lines: &[String]) -> Vec<LineInfo> {
    let mut infos = Vec::with_capacity(lines.len());
    let mut in_comment = false;
    let mut in_preproc = false;
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        let first = chars.iter().position(|c| !c.is_whitespace());
        let starts_hash = first.is_some_and(|idx| chars[idx] == '#');
        let preproc = starts_hash || in_preproc;
        let code = if preproc {
            vec![' '; chars.len()]
        } else {
            blank_code(&chars, &mut in_comment)
        };
        let continues = chars.last().is_some_and(|c| *c == '\\');
        in_preproc = preproc && continues;
        infos.push(LineInfo { code, preproc });
    }
    infos
}

fn blank_code(chars: &[char], in_comment: &mut bool) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if *in_comment {
            if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                out.push(' ');
                out.push(' ');
                i += 2;
                *in_comment = false;
            } else {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        let ch = chars[i];
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() {
                out.push(' ');
                i += 1;
            }
            break;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            out.push(' ');
            out.push(' ');
            i += 2;
            *in_comment = true;
            continue;
        }
        if ch == '"' || ch == '\'' {
            let quote = ch;
            out.push(' ');
            i += 1;
            while i < chars.len() {
                let inner = chars[i];
                if inner == '\\' && i + 1 < chars.len() {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                    continue;
                }
                out.push(' ');
                i += 1;
                if inner == quote {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// Screen column of the character at idx (tabs advance to tab stops).
fn display_col(chars: &[char], idx: usize, ts: i64) -> i64 {
    let ts = ts.max(1);
    let mut col = 0i64;
    for &ch in chars.iter().take(idx) {
        if ch == '\t' {
            col += ts - col % ts;
        } else {
            col += 1;
        }
    }
    col
}

fn indent_width(line: &str, ts: i64) -> i64 {
    let ts = ts.max(1);
    let mut col = 0i64;
    for ch in line.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col += ts - col % ts,
            _ => break,
        }
    }
    col
}

fn indent_of(line: &str) -> &str {
    let end = line
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(line.len());
    &line[..end]
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn word_at_start(text: &str) -> &str {
    let text = text.trim_start();
    let end = text.find(|c: char| !is_word_char(c)).unwrap_or(text.len());
    &text[..end]
}

fn starts_with_word(text: &str, word: &str) -> bool {
    let text = text.trim_start();
    if !text.starts_with(word) {
        return false;
    }
    !text
        .get(word.len()..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(is_word_char)
}

fn is_default_label(text: &str) -> bool {
    if !starts_with_word(text, "default") {
        return false;
    }
    let rest = text
        .trim_start()
        .get("default".len()..)
        .unwrap_or("")
        .trim_start();
    rest.starts_with(':') && !rest.starts_with("::")
}

fn is_case_label(text: &str) -> bool {
    if is_default_label(text) {
        return true;
    }
    let text = text.trim_start();
    if !starts_with_word(text, "case") {
        return false;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut i = 4;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let ch = chars[i];
        if let Some(open) = quote {
            if ch == '\\' && i + 1 < chars.len() {
                i += 2;
                continue;
            }
            if ch == open {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == ':' {
            if i + 1 < chars.len() && chars[i + 1] == ':' {
                i += 2;
                continue;
            }
            return true;
        }
        i += 1;
    }
    false
}

fn is_jump_label(text: &str) -> bool {
    let text = text.trim_start();
    let Some(first) = text.chars().next() else {
        return false;
    };
    if !is_word_char(first) {
        return false;
    }
    let mut end = first.len_utf8();
    for (idx, ch) in text.char_indices().skip(1) {
        if is_word_char(ch) {
            end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    let rest = text[end..].trim_start();
    rest.starts_with(':') && !rest.starts_with("::")
}

fn is_scopedecl(text: &str) -> bool {
    let text = text.trim_start();
    ["public", "private", "protected"].iter().any(|word| {
        if !starts_with_word(text, word) {
            return false;
        }
        let rest = text.get(word.len()..).unwrap_or("").trim_start();
        rest.starts_with(':') && !rest.starts_with("::")
    })
}

/// Text after a label: (None when there is nothing but white space).
fn after_label(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                i += 2;
                continue;
            }
            let rest = text[i + 1..].trim();
            return if rest.is_empty() { None } else { Some(rest) };
        }
        i += 1;
    }
    None
}

fn is_terminated(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if text.starts_with('{') {
        return true;
    }
    if text.starts_with('}') {
        let after = text.trim_start_matches('}').trim_start();
        if !starts_with_word(after, "else") {
            return true;
        }
    }
    matches!(text.chars().last(), Some(';') | Some('{') | Some('}'))
}

fn looks_like_func_decl(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || !text.ends_with(')') || !text.contains('(') {
        return false;
    }
    let first = word_at_start(text);
    !matches!(
        first,
        "if" | "for" | "while" | "switch" | "return" | "else" | "do" | "sizeof"
    )
}

fn is_namespace_decl(segment: &str) -> bool {
    let segment = segment.trim_start();
    let segment = segment
        .strip_prefix("inline ")
        .map(str::trim_start)
        .unwrap_or(segment);
    starts_with_word(segment, "namespace")
}

fn is_extern_c_decl(segment: &str) -> bool {
    let segment = segment.trim_start();
    starts_with_word(segment, "extern")
        && (segment.contains("\"C\"") || segment.contains("\"C++\""))
}

/// Render cols columns of indent the way Vim's set_indent() does: spaces with
/// 'expandtab', otherwise tabs up to the last full tab stop plus spaces.
pub fn indent_string(cols: i64, expandtab: bool, tabstop: i64) -> String {
    let cols = usize::try_from(cols.max(0)).unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    if expandtab {
        return " ".repeat(cols);
    }
    let ts = usize::try_from(tabstop.max(1)).unwrap_or(1);
    let mut out = "\t".repeat(cols / ts);
    out.push_str(&" ".repeat(cols % ts));
    out
}

/// cindent(): the indent for {lnum}/{col}.  {lnum} is 1-based, {col} is
/// 1-based (0 means "use the first non-blank").  Returns -1 when {lnum} is
/// invalid.
pub fn cindent(ed: &Editor, lnum: i64, col: i64) -> i64 {
    let count = ed.line_count();
    if lnum <= 0 || lnum > i64::try_from(count).unwrap_or(i64::MAX) {
        return -1;
    }
    let opts = IndentOpts::from_editor(ed);
    let lines = buffer_lines(ed);
    let infos = scan_lines(&lines);
    let ctx = Ctx {
        lines: &lines,
        infos: &infos,
        opts: &opts,
    };
    let target0 = usize::try_from(lnum - 1).unwrap_or(0);
    let (computed, scan) = run_scan(&ctx, target0);
    let mut code = infos[target0].code.clone();
    if col > 0 {
        let idx = usize::try_from(col - 1).unwrap_or(usize::MAX);
        if idx < code.len() && code[idx] == ')' {
            code.truncate(idx);
        }
    }
    let text = code.iter().collect::<String>().trim().to_string();
    let hash = lines[target0].trim_start().starts_with('#');
    indent_from_state(&scan, &text, hash, &computed, &ctx, target0)
}

/// The leading white space a new line should inherit when 'cindent',
/// 'smartindent' or 'autoindent' is on.  prev_line is 0-based and names the
/// line above the new (still empty) line.
pub fn new_line_indent(ed: &Editor, prev_line: usize) -> Option<String> {
    let opts = IndentOpts::from_editor(ed);
    if !opts.cindent && !opts.smartindent && !opts.autoindent {
        return None;
    }
    let lines = buffer_lines(ed);
    if lines.is_empty() {
        return None;
    }
    let prev = prev_line.min(lines.len() - 1);
    let infos = scan_lines(&lines);
    let ctx = Ctx {
        lines: &lines,
        infos: &infos,
        opts: &opts,
    };
    if opts.cindent {
        let target = prev + 1;
        let (computed, scan) = run_scan(&ctx, target.min(lines.len()));
        let cols = indent_from_state(&scan, "", false, &computed, &ctx, target);
        Some(indent_string(cols, opts.expandtab, opts.ts))
    } else if opts.smartindent {
        let cols = smartindent_cols(&ctx, prev);
        Some(indent_string(cols, opts.expandtab, opts.ts))
    } else {
        Some(indent_of(&lines[prev]).to_string())
    }
}

/// Forward pass: compute the indent of every line before the target and build
/// the delimiter stack / previous-line state the target sees.
fn run_scan(ctx: &Ctx, through: usize) -> (Vec<i64>, Scan) {
    let mut computed = Vec::with_capacity(through);
    let mut scan = Scan::default();
    for line0 in 0..through {
        let text = code_text(ctx, line0);
        let indent = indent_from_state(
            &scan,
            &text,
            ctx.infos[line0].preproc,
            &computed,
            ctx,
            line0,
        );
        computed.push(indent);
        if is_case_label(&text) {
            if let Some(brace) = scan
                .stack
                .iter_mut()
                .rev()
                .find(|d| d.kind == DelimKind::Brace)
            {
                if brace.is_switch {
                    brace.last_case = Some(indent);
                }
            }
        }
        if is_scopedecl(&text) {
            if let Some(brace) = scan
                .stack
                .iter_mut()
                .rev()
                .find(|d| d.kind == DelimKind::Brace)
            {
                brace.last_scopedecl = Some(indent);
            }
        }
        scan.observe(line0, ctx, indent);
        scan.remember(line0, ctx, indent);
    }
    (computed, scan)
}

fn reverse_depth_step(depth: i64, code: &str) -> i64 {
    let mut depth = depth;
    for ch in code.chars() {
        match ch {
            '}' => depth += 1,
            '{' => depth -= 1,
            _ => {}
        }
    }
    depth.max(0)
}

fn search_limit(ctx: &Ctx) -> usize {
    usize::try_from(ctx.opts.cino.maxcomment.max(1)).unwrap_or(1)
}

/// Find the if an else belongs to (backwards, skipping nested blocks).
fn find_matching_if(ctx: &Ctx, target0: usize, floor: usize, computed: &[i64]) -> Option<i64> {
    let limit = search_limit(ctx);
    let mut depth = 0i64;
    let mut line0 = target0;
    let mut steps = 0usize;
    while line0 > floor && steps < limit {
        line0 -= 1;
        steps += 1;
        let code = code_text(ctx, line0);
        if code.is_empty() {
            continue;
        }
        depth = reverse_depth_step(depth, &code);
        if depth > 0 {
            continue;
        }
        if is_case_label(&code) {
            return None;
        }
        if starts_with_word(&code, "if") {
            return computed.get(line0).copied();
        }
    }
    None
}

/// Find the do a while belongs to (backwards, skipping its body).
fn find_matching_do(ctx: &Ctx, target0: usize, floor: usize, computed: &[i64]) -> Option<i64> {
    let limit = search_limit(ctx);
    let mut depth = 0i64;
    let mut line0 = target0;
    let mut steps = 0usize;
    while line0 > floor && steps < limit {
        line0 -= 1;
        steps += 1;
        let code = code_text(ctx, line0);
        if code.is_empty() {
            continue;
        }
        depth = reverse_depth_step(depth, &code);
        if depth > 0 {
            continue;
        }
        if is_case_label(&code) {
            return None;
        }
        if starts_with_word(&code, "do") {
            return computed.get(line0).copied();
        }
        if starts_with_word(&code, "while")
            || starts_with_word(&code, "for")
            || starts_with_word(&code, "switch")
        {
            return None;
        }
    }
    None
}

/// Column of the first non-blank after an open parenthesis (or one past the
/// parenthesis when it ends the line), for 'cino=(0'.
fn column_after_paren(ctx: &Ctx, paren: &Delim) -> i64 {
    let chars: Vec<char> = match ctx.lines.get(paren.line0) {
        Some(line) => line.chars().collect(),
        None => return 0,
    };
    let mut idx = paren.col + 1;
    while idx < chars.len() && (chars[idx] == ' ' || chars[idx] == '\t') {
        idx += 1;
    }
    if idx < chars.len() {
        display_col(&chars, idx, ctx.opts.ts)
    } else {
        display_col(&chars, paren.col, ctx.opts.ts) + 1
    }
}

/// The actual C-indent decision for text given the context built so far.
fn indent_from_state(
    scan: &Scan,
    text: &str,
    hash: bool,
    computed: &[i64],
    ctx: &Ctx,
    target0: usize,
) -> i64 {
    let cino = &ctx.opts.cino;
    if hash {
        return cino.hash_comment.max(0);
    }
    let text = text.trim_start();

    if is_scopedecl(text) {
        if cino.scopedecl_set {
            if let Some(brace) = scan.nearest_brace() {
                return (brace.base + cino.scopedecl).max(0);
            }
        }
        // Access specifiers live in column 0 unless 'g' says otherwise.
        return 0;
    }
    if !is_case_label(text) && is_jump_label(text) {
        if cino.jump_label_set && cino.jump_label >= 0 {
            let base = scan
                .nearest_brace()
                .map(|brace| brace.base + cino.level)
                .unwrap_or(0);
            return (base - cino.jump_label).max(0);
        }
        return 0;
    }
    if text.starts_with('}') {
        return scan
            .nearest_brace()
            .map(|brace| (brace.base + cino.close_extra).max(0))
            .unwrap_or_else(|| cino.close_extra.max(0));
    }
    if is_case_label(text) {
        if let Some(brace) = scan.nearest_switch_brace() {
            return (brace.base + cino.case).max(0);
        }
        return 0;
    }

    let floor = scan
        .nearest_brace()
        .map(|brace| brace.line0 + 1)
        .unwrap_or(0);

    // Continuation inside an unclosed '(' or '['.
    if let Some(paren) = scan.innermost_paren() {
        let distance = target0.saturating_sub(paren.line0);
        if distance <= usize::try_from(cino.maxparen.max(1)).unwrap_or(1) {
            let base = computed.get(paren.line0).copied().unwrap_or(0);
            if (text.starts_with(')') || text.starts_with(']')) && cino.matching_paren != 0 {
                return base.max(0);
            }
            // With 'cino=(0' Vim lines the continuation up with the first
            // character after the open parenthesis (get_c_indent()).
            if cino.unclosed == 0 && !text.starts_with(')') && !text.starts_with(']') {
                return column_after_paren(ctx, paren).max(0);
            }
            let mut extra = if paren.open_at_eol || cino.unclosed_noignore > 0 {
                cino.unclosed
            } else {
                cino.continuation
            };
            extra += cino.unclosed2.saturating_mul(scan.paren_nesting());
            return (base + extra).max(0);
        }
    }

    if starts_with_word(text, "else") {
        if let Some(indent) = find_matching_if(ctx, target0, floor, computed) {
            return indent.max(0);
        }
    }
    if starts_with_word(text, "while") {
        if let Some(indent) = find_matching_do(ctx, target0, floor, computed) {
            return indent.max(0);
        }
    }

    if let Some(brace) = scan.nearest_brace() {
        let body = brace.base + cino.level;
        if let Some(prev) = &scan.prev {
            if prev.line0 >= brace.line0 {
                if prev.is_case && brace.is_switch {
                    let case_indent = brace.last_case.unwrap_or(body);
                    if text.starts_with('{') {
                        return case_indent.max(0);
                    }
                    return (case_indent + cino.case_code).max(0);
                }
                if prev.is_scopedecl {
                    return (prev.indent + cino.scopedecl_code).max(0);
                }
                if prev.is_cinword && !prev.terminated {
                    if text.starts_with('{') {
                        return (prev.indent + cino.open_extra).max(0);
                    }
                    return (prev.indent + cino.level + cino.no_brace).max(0);
                }
                if !prev.terminated {
                    if prev.is_func_decl && text.starts_with('{') {
                        return prev.indent.max(0);
                    }
                    if text.starts_with(':') {
                        return (prev.indent + cino.cpp_baseclass).max(0);
                    }
                    if text.starts_with('{') {
                        return (prev.indent + cino.open_extra).max(0);
                    }
                    return (prev.indent + cino.continuation).max(0);
                }
            }
        }
        if brace.is_switch {
            if let Some(case_indent) = brace.last_case {
                return (case_indent + cino.case_code).max(0);
            }
        }
        if let Some(scoped) = brace.last_scopedecl {
            return (scoped + cino.scopedecl_code).max(0);
        }
        let mut amount = body;
        if text.starts_with('{') {
            amount += cino.open_extra;
        }
        return amount.max(0);
    }

    // No braces at all: top level.
    if text.starts_with('{') {
        return cino.first_open.max(0);
    }
    if !text.is_empty()
        && !text.ends_with(';')
        && !text.ends_with(':')
        && !text.ends_with(',')
        && !text.contains('{')
        && !text.contains('}')
        && !looks_like_func_decl(text)
        && ctx.lines.get(target0 + 1).is_some_and(|_| {
            let next = code_text(ctx, target0 + 1);
            looks_like_func_decl(&next)
        })
    {
        return cino.func_type.max(0);
    }
    if let Some(prev) = &scan.prev {
        if prev.is_func_decl && !prev.terminated {
            if text.starts_with(':') {
                return (prev.indent + cino.cpp_baseclass).max(0);
            }
            return cino.param.max(0);
        }
        if prev.is_scopedecl {
            return (prev.indent + cino.scopedecl_code).max(0);
        }
        if prev.is_cinword && !prev.terminated {
            return (prev.indent + cino.level + cino.no_brace).max(0);
        }
        if !prev.terminated {
            return (prev.indent + cino.continuation).max(0);
        }
        // At the top level a terminated statement starts from column 0 again;
        // the previous line may itself have been a continuation line.
        return 0;
    }
    0
}

/// 'smartindent' (indent.c may_do_si(), change.c open_line()).
fn smartindent_cols(ctx: &Ctx, prev: usize) -> i64 {
    let opts = ctx.opts;
    let mut base_line = prev;
    if ctx.lines[base_line].trim_start().starts_with('#') {
        while base_line > 0 {
            let text = ctx.lines[base_line].trim_start();
            if text.starts_with('#') || text.is_empty() {
                base_line -= 1;
            } else {
                break;
            }
        }
        let text = ctx.lines[base_line].trim_start();
        if text.starts_with('#') || text.is_empty() {
            return 0;
        }
    }
    let mut base = indent_width(&ctx.lines[base_line], opts.ts);
    let target = prev + 1;
    if let Some(text) = ctx.lines.get(target) {
        let text = text.trim_start();
        if text.starts_with('#') {
            return 0;
        }
        if text.starts_with('}') {
            let mut cols = base - opts.sw;
            if opts.shiftround {
                cols -= cols.rem_euclid(opts.sw);
            }
            return cols.max(0);
        }
    }
    let code = code_text(ctx, prev);
    let code = code.trim_end();
    let first = word_at_start(code);
    let is_cinword = opts.cinwords.iter().any(|word| word == first);
    let terminated = code.ends_with(';') || code.ends_with('}');
    if code.ends_with('{') || (is_cinword && !terminated) {
        if opts.shiftround {
            base -= base.rem_euclid(opts.sw);
        }
        base += opts.sw;
    }
    base.max(0)
}
