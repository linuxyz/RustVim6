//! Vim9 script parser: expression grammar plus the statement/block grammar.
//!
//! Vim9 source is line oriented but expressions may continue over several
//! lines. We first join logical lines (handling comments, backslash and
//! operator continuations, heredocs), then parse blocks.

use std::rc::Rc;

use rvim_core::{VimError, VimResult};

use super::ast::*;
use super::lexer::{Tok, tokenize};
use super::types::Vim9Type;

/// One logical source line after continuation/heredoc processing.
#[derive(Clone, Debug)]
pub struct SourceLine {
    pub text: String,
    pub number: usize,
}

/// Commands that are Ex commands even though they may contain '=' characters.
const EX_WORDS: &[&str] = &[
    "autocmd",
    "augroup",
    "bdelete",
    "buffer",
    "buffers",
    "call",
    "cd",
    "chdir",
    "close",
    "command",
    "comclear",
    "copen",
    "cclose",
    "cnext",
    "cprevious",
    "cfirst",
    "clast",
    "cc",
    "cexpr",
    "caddexpr",
    "cgetexpr",
    "cfile",
    "cgetfile",
    "clist",
    "delete",
    "delcommand",
    "diffsplit",
    "diffthis",
    "diffoff",
    "diffupdate",
    "diffget",
    "diffput",
    "display",
    "echo",
    "echoerr",
    "echohl",
    "echomsg",
    "echon",
    "edit",
    "enew",
    "eval",
    "execute",
    "exit",
    "exe",
    "file",
    "filetype",
    "finish",
    "fold",
    "foldopen",
    "foldclose",
    "global",
    "help",
    "hide",
    "highlight",
    "hi",
    "join",
    "left",
    "let",
    "lockvar",
    "ls",
    "lunmap",
    "messages",
    "mkdir",
    "mksession",
    "mkview",
    "loadview",
    "move",
    "new",
    "nohlsearch",
    "noremap",
    "normal",
    "only",
    "packadd",
    "packloadall",
    "pop",
    "print",
    "put",
    "pwd",
    "quit",
    "qall",
    "read",
    "redir",
    "redraw",
    "registers",
    "resize",
    "retab",
    "right",
    "runtime",
    "saveas",
    "set",
    "setfiletype",
    "setf",
    "setglobal",
    "setlocal",
    "silent",
    "silent!",
    "sleep",
    "sort",
    "source",
    "so",
    "split",
    "stop",
    "substitute",
    "s",
    "syntax",
    "syn",
    "tabclose",
    "tabedit",
    "tabnew",
    "tabnext",
    "tabprevious",
    "tabs",
    "tag",
    "tselect",
    "tjump",
    "tnext",
    "tprevious",
    "unlet",
    "unlockvar",
    "update",
    "version",
    "vertical",
    "vglobal",
    "vimgrep",
    "vsplit",
    "wall",
    "wincmd",
    "windo",
    "write",
    "wq",
    "xit",
    "yank",
    "defcompile",
];

/// Keywords that end a block; recognised at the start of a logical line.
fn line_keyword(text: &str) -> String {
    let mut rest = text.trim_start();
    if let Some(after) = rest.strip_prefix(':') {
        rest = after.trim_start();
    }
    let mut word = String::new();
    for c in rest.chars() {
        if c.is_ascii_alphabetic() || c == '!' || c == '_' {
            word.push(c.to_ascii_lowercase());
        } else {
            break;
        }
    }
    // 'enddef: value' is a dictionary entry, not a block terminator.
    if rest[word.len()..].trim_start().starts_with(':') {
        return String::new();
    }
    word
}

/// Terminator word of a '=<< ... MARKER' line, if any.
pub fn heredoc_marker(text: &str) -> Option<String> {
    let idx = find_outside_quotes(text, "=<<")?;
    text[idx + 3..]
        .split_whitespace()
        .last()
        .map(|marker| marker.to_string())
}

/// Build logical lines: strip comments, join continuations, expand heredocs.
pub fn prepare_lines(raw: &[String], first_line: usize) -> VimResult<Vec<SourceLine>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < raw.len() {
        let original = raw[i].clone();
        let number = first_line + i;
        let mut text = strip_comment(&original);
        if text.trim().is_empty() {
            i += 1;
            continue;
        }
        // Heredoc: 'var lines =<< trim END' (outside quoted strings).
        if let Some(idx) = find_outside_quotes(&text, "=<<") {
            let prefix = text[..idx].trim_end().to_string();
            let spec = text[idx + 3..].trim().to_string();
            let mut words: Vec<&str> = spec.split_whitespace().collect();
            if let Some(marker) = words.last().copied() {
                let options: Vec<&str> = words.drain(..words.len().saturating_sub(1)).collect();
                let trim = options.iter().any(|o| o.eq_ignore_ascii_case("trim"));
                let eval = options.iter().any(|o| o.eq_ignore_ascii_case("eval"));
                let mut content = Vec::new();
                i += 1;
                while i < raw.len() {
                    let line = raw[i].clone();
                    if line.trim() == marker {
                        break;
                    }
                    content.push(line.trim_end_matches('\r').to_string());
                    i += 1;
                }
                if i >= raw.len() {
                    return Err(VimError::new(
                        "E990",
                        format!("Missing heredoc terminator {marker} (line {number})"),
                    ));
                }
                if trim {
                    let indent = content
                        .iter()
                        .filter(|l| !l.trim().is_empty())
                        .map(|l| l.len() - l.trim_start().len())
                        .min()
                        .unwrap_or(0);
                    for line in &mut content {
                        if line.len() >= indent {
                            *line = line[indent..].to_string();
                        }
                    }
                }
                let items: Vec<String> = content
                    .iter()
                    .map(|line| {
                        if eval {
                            format!("\"{}\"", escape_double(line))
                        } else {
                            format!("'{}'", line.replace('\'', "''"))
                        }
                    })
                    .collect();
                let sep = if prefix.contains('=') { "" } else { " = " };
                text = format!("{prefix}{sep}[{}]", items.join(", "));
            }
        }
        // Join continuation lines.
        loop {
            let mut j = i + 1;
            while j < raw.len() {
                let candidate = strip_comment(&raw[j]);
                if candidate.trim().is_empty() {
                    j += 1;
                } else {
                    break;
                }
            }
            if j >= raw.len() {
                break;
            }
            let next = strip_comment(&raw[j]);
            let next_trim = next.trim_start();
            let explicit = next_trim.starts_with('\\');
            // A :catch pattern ends in '/', which is also the division
            // operator; never continue a catch line into its body.
            let continues = if line_keyword(&text) == "catch" {
                false
            } else {
                // A line starting with ':' is normally an Ex command, but it is
                // the else-branch of a ternary when the expression already has a
                // '?' and no matching ':'.
                let ternary_else = next_trim.starts_with(':') && text.contains('?');
                ends_with_operator(&text) || starts_with_operator(next_trim) || ternary_else
            };
            if !explicit && !continues {
                break;
            }
            if explicit {
                text.push(' ');
                text.push_str(next_trim.trim_start_matches('\\').trim_start());
            } else {
                text.push(' ');
                text.push_str(next_trim);
            }
            i = j;
        }
        out.push(SourceLine {
            text: text.trim_end().to_string(),
            number,
        });
        i += 1;
    }
    Ok(out)
}

fn escape_double(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Find 'needle' outside single/double quoted strings.
pub fn find_outside_quotes(text: &str, needle: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let needle_chars: Vec<char> = needle.chars().collect();
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == '\\' && q == '"' {
                    i += 1;
                } else if c == q {
                    if q == '\'' && chars.get(i + 1) == Some(&'\'') {
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => {
                if c == '\'' || c == '"' {
                    quote = Some(c);
                } else if chars[i..].starts_with(&needle_chars) {
                    return Some(text.char_indices().nth(i).map(|(o, _)| o).unwrap_or(0));
                }
            }
        }
        i += 1;
    }
    None
}

/// Remove a trailing '#' comment, respecting quoted strings and '#{'.
pub fn strip_comment(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == '\\' && q == '"' {
                    i += 1;
                } else if c == q {
                    if q == '\'' && chars.get(i + 1) == Some(&'\'') {
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => {
                if c == '\'' || c == '"' {
                    quote = Some(c);
                } else if c == '#'
                    && chars.get(i + 1) != Some(&'{')
                    && (i == 0 || chars[i - 1].is_whitespace())
                {
                    // A '#' starts a comment only when it is separated from the
                    // code. This keeps autoload names (dist#ft#Foo) and the
                    // legacy =~# / ==# operator suffixes intact.
                    let prefix: String = chars[..i].iter().collect();
                    return prefix.trim_end().to_string();
                }
            }
        }
        i += 1;
    }
    text.trim_end().to_string()
}

fn ends_with_operator(text: &str) -> bool {
    let t = text.trim_end();
    const ENDS: &[&str] = &[
        ",", "(", "[", "{", "..", "&&", "||", "->", "+", "-", "*", "/", "%", "=", "?", ":", "..=",
        "==", "!=", "=~", "!~", ">=", "<=", "=>",
    ];
    ENDS.iter().any(|op| t.ends_with(op))
}

fn starts_with_operator(text: &str) -> bool {
    // ':++x'/':--x' are statements, not continuations of the previous line.
    if text.starts_with("++") || text.starts_with("--") {
        return false;
    }
    const STARTS: &[&str] = &[
        "->", "..", "&&", "||", "==", "!=", "=~", "!~", ">=", "<=", "?", ")", "]", "}", ",", "+",
        "-", "*", "/", "%", ".",
    ];
    STARTS.iter().any(|op| text.starts_with(op))
}

/// Parse a whole script (the 'vim9script' line included).
pub fn parse_script(raw: &[String], path: &str, sid: usize, prefix: &str) -> VimResult<Vim9Script> {
    let lines = prepare_lines(raw, 1)?;
    let mut noclear = false;
    let mut start = 0usize;
    // Find the vim9script line (it must be the first code line).
    for (idx, line) in lines.iter().enumerate() {
        let lower = line.text.trim().to_ascii_lowercase();
        if lower == "vim9script" || lower.starts_with("vim9script ") {
            noclear = lower.contains("noclear");
            start = idx + 1;
            break;
        }
    }
    let mut parser = StmtParser {
        lines: &lines[start..],
        pos: 0,
        sid,
        prefix,
    };
    let (top, term) = parser.parse_block(&[])?;
    if let Some(term) = term {
        return Err(VimError::new(
            "E580",
            format!("Unexpected :{term} at script level"),
        ));
    }
    let mut funcs = Vec::new();
    let mut exports = Vec::new();
    let mut classes = Vec::new();
    let mut enums = Vec::new();
    let mut type_aliases = Vec::new();
    collect_defs(
        &top,
        &mut funcs,
        &mut exports,
        &mut classes,
        &mut enums,
        &mut type_aliases,
    );
    Ok(Vim9Script {
        path: path.to_string(),
        sid,
        prefix: prefix.to_string(),
        funcs,
        classes,
        enums,
        type_aliases,
        top,
        exports,
        noclear,
        line_count: raw.len(),
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_defs(
    stmts: &[Stmt],
    funcs: &mut Vec<Rc<Vim9Func>>,
    exports: &mut Vec<String>,
    classes: &mut Vec<Rc<VimClass>>,
    enums: &mut Vec<Rc<VimEnum>>,
    type_aliases: &mut Vec<Rc<VimTypeAlias>>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Def(func) => {
                funcs.push(func.clone());
                if func.is_export {
                    exports.push(func.name.clone());
                }
            }
            Stmt::ClassDef(class) => {
                classes.push(class.clone());
                if class.is_export {
                    exports.push(class.name.clone());
                }
            }
            Stmt::EnumDef(item) => {
                enums.push(item.clone());
                if item.is_export {
                    exports.push(item.name.clone());
                }
            }
            Stmt::TypeAlias(item) => {
                type_aliases.push(item.clone());
                if item.is_export {
                    exports.push(item.name.clone());
                }
            }
            Stmt::Var {
                name, export: true, ..
            } => exports.push(name.clone()),
            Stmt::Import {
                kind: ImportKind::Export(inner),
                ..
            } => {
                if let Stmt::Def(func) = inner.as_ref() {
                    funcs.push(func.clone());
                    exports.push(func.name.clone());
                }
            }
            Stmt::Block(inner) => {
                collect_defs(inner, funcs, exports, classes, enums, type_aliases);
            }
            _ => {}
        }
    }
}

/// Parse one :def header plus body lines (used by the legacy script parser).
pub fn parse_function(
    header: &str,
    body: &[String],
    sid: usize,
    prefix: &str,
    line: usize,
) -> VimResult<Rc<Vim9Func>> {
    let joined = prepare_lines(body, line + 1)?;
    let mut parser = StmtParser {
        lines: &joined,
        pos: 0,
        sid,
        prefix,
    };
    let (stmts, term) = parser.parse_block(&[])?;
    if let Some(term) = term {
        return Err(VimError::new(
            "E580",
            format!("Unexpected :{term} in function body"),
        ));
    }
    let (name, params, varargs, ret, is_export) = parse_def_header(header)?;
    // A :def inside a legacy script defines a global function (like
    // :function); a :def inside a vim9script is script-local unless exported.
    let canonical = if prefix.is_empty() {
        name.clone()
    } else {
        canonical_name(&name, sid, prefix, is_export)
    };
    Ok(Rc::new(Vim9Func {
        name,
        canonical,
        params,
        varargs,
        ret,
        body: stmts,
        sid,
        is_export,
        line,
    }))
}

/// Parse 'Name(a: number, b: string = "x"): bool'.
pub fn parse_def_header(line: &str) -> VimResult<(String, Vec<Param>, bool, Vim9Type, bool)> {
    let mut text = line.trim();
    let mut is_export = false;
    if let Some(rest) = text.strip_prefix("export ") {
        is_export = true;
        text = rest.trim_start();
    }
    let rest = text
        .strip_prefix("def")
        .ok_or_else(|| VimError::new("E1093", "Missing :def keyword"))?
        .trim_start();
    let open = rest
        .find('(')
        .ok_or_else(|| VimError::new("E1094", format!("Missing ( in :def: {line}")))?;
    let name = rest[..open].trim().to_string();
    if name.is_empty() {
        return Err(VimError::new("E1093", "Missing function name"));
    }
    let mut depth = 0i32;
    let mut close = None;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or_else(|| VimError::new("E1094", "Missing ) in :def"))?;
    let params_text = &rest[open + 1..close];
    let after = rest[close + 1..].trim();
    let ret = if let Some(type_text) = after.strip_prefix(':') {
        let type_text = type_text.trim();
        // Attributes may follow the type; only the type token matters.
        let type_text = type_text.split_whitespace().next().unwrap_or(type_text);
        if type_text.is_empty() {
            // A missing return type is an error at compile time in Vim
            // (E1056); accept it as 'any' so the surrounding script still loads.
            Vim9Type::Any
        } else {
            Vim9Type::parse(type_text)
                .map_err(|e| VimError::new("E1097", format!("Unknown type: {e}")))?
        }
    } else {
        Vim9Type::Void
    };
    let (params, varargs) = parse_params(params_text)?;
    Ok((name, params, varargs, ret, is_export))
}

fn parse_params(text: &str) -> VimResult<(Vec<Param>, bool)> {
    let mut params = Vec::new();
    let mut varargs = false;
    for part in split_top_level(text, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if varargs {
            return Err(VimError::new("E125", "Argument follows varargs"));
        }
        if part == "..." {
            varargs = true;
            continue;
        }
        let (part, named_varargs) = match part.strip_prefix("...") {
            Some(rest) => (rest.trim(), true),
            None => (part, false),
        };
        let (spec, default) = match find_default(part) {
            Some((left, right)) => (left, Some(parse_expression(default_text(right).as_str())?)),
            None => (part, None),
        };
        let (name, ty) = match find_type_colon(spec) {
            Some(idx) => {
                let ty_text = spec[idx + 1..].trim();
                let ty = if ty_text.is_empty() {
                    Vim9Type::Any
                } else {
                    Vim9Type::parse(ty_text)
                        .map_err(|e| VimError::new("E1097", format!("Unknown type: {e}")))?
                };
                (spec[..idx].trim().to_string(), ty)
            }
            None => (spec.trim().to_string(), Vim9Type::Any),
        };
        if name.is_empty() {
            return Err(VimError::new("E125", "Invalid function argument"));
        }
        if named_varargs && !matches!(ty, Vim9Type::List(_)) {
            return Err(VimError::new(
                "E1180",
                format!("Variable arguments type must be a list: {}", ty.name()),
            ));
        }
        if named_varargs && default.is_some() {
            return Err(VimError::new(
                "E125",
                "Variable arguments cannot have a default",
            ));
        }
        varargs = named_varargs;
        params.push(Param {
            name,
            ty,
            default,
            varargs: named_varargs,
        });
    }
    Ok((params, varargs))
}

fn default_text(text: &str) -> String {
    text.trim().to_string()
}

fn find_default(text: &str) -> Option<(&str, &str)> {
    let idx = find_top_level(text, '=')?;
    let prev = text[..idx].chars().next_back();
    let next = text[idx + 1..].chars().next();
    if matches!(prev, Some('=') | Some('!') | Some('<') | Some('>'))
        || matches!(next, Some('=') | Some('~') | Some('>'))
    {
        return None;
    }
    Some((text[..idx].trim_end(), &text[idx + 1..]))
}

fn find_type_colon(text: &str) -> Option<usize> {
    let idx = find_top_level(text, ':')?;
    Some(idx)
}

/// Like [find_type_colon], but a scope prefix ('g:name') is not a type
/// separator; 'var g:value: number' splits after the second colon.
fn find_type_separator(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let start = if chars.len() > 2
        && chars[1] == ':'
        && matches!(chars[0], 'g' | 'b' | 'w' | 't' | 's' | 'v' | 'a' | 'l')
        && (chars[2].is_alphanumeric() || chars[2] == '_')
    {
        2
    } else {
        0
    };
    let byte_start = text.char_indices().nth(start).map(|(o, _)| o).unwrap_or(0);
    find_top_level(&text[byte_start..], ':').map(|idx| idx + byte_start)
}

/// Split on a separator at nesting depth zero, respecting quotes.
pub fn split_top_level(text: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == '\\' && q == '"' {
                    i += 1;
                } else if c == q {
                    if q == '\'' && chars.get(i + 1) == Some(&'\'') {
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' | '<' => depth += 1,
                ')' | ']' | '}' | '>' => depth -= 1,
                _ if c == sep && depth == 0 => {
                    out.push(&text[start..i]);
                    start = i + c.len_utf8();
                }
                _ => {}
            },
        }
        i += 1;
    }
    out.push(&text[start..]);
    out
}

/// Find a separator character at top level (outside quotes/brackets).
pub fn find_top_level(text: &str, sep: char) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == '\\' && q == '"' {
                    i += 1;
                } else if c == q {
                    if q == '\'' && chars.get(i + 1) == Some(&'\'') {
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' | '<' => depth += 1,
                ')' | ']' | '}' | '>' => depth -= 1,
                _ if c == sep && depth == 0 => return Some(i),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn canonical_name(name: &str, sid: usize, prefix: &str, export: bool) -> String {
    if name.starts_with("g:") {
        return name.to_string();
    }
    let bare = name.strip_prefix("s:").unwrap_or(name);
    if export && !prefix.is_empty() {
        format!("{prefix}#{bare}")
    } else {
        format!("<SNR>{sid}_{bare}")
    }
}

// ---------------------------------------------------------------------------
// Expression parser
// ---------------------------------------------------------------------------

pub struct ExprParser {
    tokens: Vec<(Tok, usize)>,
    pos: usize,
    len: usize,
}

impl ExprParser {
    pub fn new(src: &str) -> VimResult<Self> {
        let tokens = tokenize(src)?;
        Ok(Self {
            tokens,
            pos: 0,
            len: src.chars().count(),
        })
    }

    fn peek(&self) -> &Tok {
        &self.tokens[self.pos.min(self.tokens.len() - 1)].0
    }

    fn bump(&mut self) -> Tok {
        let tok = self.peek().clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.peek() == tok {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, tok: &Tok) -> VimResult<()> {
        if self.eat(tok) {
            Ok(())
        } else {
            Err(self.error(&format!(
                "expected {}, found {}",
                tok.describe(),
                self.peek().describe()
            )))
        }
    }

    fn error(&self, message: &str) -> VimError {
        VimError::new("E15", format!("Invalid expression: {message}"))
    }

    /// Character offset just past the tokens consumed so far.
    fn consumed(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|(_, start)| *start)
            .unwrap_or(self.len)
    }

    pub fn parse_all(&mut self) -> VimResult<Expr> {
        let expr = self.parse_expr(0)?;
        if !matches!(self.peek(), Tok::Eof) {
            return Err(self.error(&format!("trailing {}", self.peek().describe())));
        }
        Ok(expr)
    }

    fn parse_expr(&mut self, min_prec: u8) -> VimResult<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            match self.peek().clone() {
                Tok::LParen => {
                    let args = self.parse_arg_list()?;
                    lhs = Expr::Call(Box::new(lhs), args);
                    continue;
                }
                Tok::LBracket => {
                    lhs = self.parse_index(lhs)?;
                    continue;
                }
                Tok::Op(op) if op == "." => {
                    self.bump();
                    match self.bump() {
                        Tok::Ident(name) => {
                            lhs = Expr::Attr(Box::new(lhs), name);
                            continue;
                        }
                        other => {
                            return Err(self.error(&format!(
                                "expected member name after '.', found {}",
                                other.describe()
                            )));
                        }
                    }
                }
                Tok::Op(op) if op == "->" => {
                    self.bump();
                    let Tok::Ident(name) = self.bump() else {
                        return Err(self.error("expected function name after '->'"));
                    };
                    let args = if self.peek() == &Tok::LParen {
                        self.parse_arg_list()?
                    } else {
                        Vec::new()
                    };
                    lhs = Expr::Method(Box::new(lhs), name, args);
                    continue;
                }
                _ => {}
            }
            let (prec, right_assoc, op) = match self.peek().clone() {
                Tok::Op(op) => match op.as_str() {
                    "?" => (10u8, true, op),
                    "??" => (15, false, op),
                    "||" => (20, false, op),
                    "&&" => (30, false, op),
                    "==" | "!=" | ">" | ">=" | "<" | "<=" | "=~" | "!~" | "==#" | "==?" | "!=#"
                    | "!=?" | ">=#" | ">=?" | "<=#" | "<=?" | "=~#" | "=~?" | "!~#" | "!~?" => {
                        (40, false, op)
                    }
                    "+" | "-" | ".." => (50, false, op),
                    "*" | "/" | "%" => (60, false, op),
                    _ => break,
                },
                Tok::Ident(name) if name == "is" || name == "isnot" => (40, false, name),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.bump();
            if op == "?" {
                let then_branch = self.parse_expr(0)?;
                self.expect(&Tok::Colon)?;
                let else_branch = self.parse_expr(10)?;
                lhs = Expr::Ternary(Box::new(lhs), Box::new(then_branch), Box::new(else_branch));
                continue;
            }
            let next_min = if right_assoc { prec } else { prec + 1 };
            let rhs = self.parse_expr(next_min)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> VimResult<Expr> {
        match self.peek().clone() {
            Tok::Op(op) if op == "!" || op == "-" || op == "+" => {
                self.bump();
                let inner = self.parse_expr(70)?;
                let op: &'static str = match op.as_str() {
                    "!" => "!",
                    "-" => "-",
                    _ => "+",
                };
                Ok(Expr::Unary(op, Box::new(inner)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> VimResult<Expr> {
        match self.bump() {
            Tok::Number(n) => Ok(Expr::Number(n)),
            Tok::Float(f) => Ok(Expr::Float(f)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Blob(b) => Ok(Expr::Blob(b)),
            Tok::Bool(b) => Ok(Expr::Bool(b)),
            Tok::Null => Ok(Expr::Null),
            Tok::Option(name) => Ok(Expr::Option(name)),
            Tok::Env(name) => Ok(Expr::Env(name)),
            Tok::Register(r) => Ok(Expr::Register(r)),
            Tok::Interp(raw) => self.parse_interpolated(&raw),
            Tok::LParen => self.parse_paren(),
            Tok::LBracket => self.parse_list(),
            Tok::LBrace | Tok::HashBrace => self.parse_dict(),
            Tok::Ident(name) => {
                // Bare scope dictionary: get(g:, ...), b:, w:, ...
                if matches!(name.as_str(), "g" | "b" | "w" | "t" | "s" | "v" | "a" | "l")
                    && self.peek() == &Tok::Colon
                    && self
                        .tokens
                        .get(self.pos - 1)
                        .zip(self.tokens.get(self.pos))
                        .is_some_and(|((_, start), (_, colon))| {
                            *colon == *start + name.chars().count()
                        })
                {
                    self.bump();
                    return Ok(Expr::Var(format!("{name}:")));
                }
                Ok(Expr::Var(name))
            }
            other => Err(self.error(&format!("unexpected {}", other.describe()))),
        }
    }

    fn parse_paren(&mut self) -> VimResult<Expr> {
        // Could be a parenthesised expression or a lambda parameter list.
        let save = self.pos;
        if let Ok(lambda) = self.parse_paren_lambda() {
            return Ok(lambda);
        }
        self.pos = save;
        let inner = self.parse_expr(0)?;
        self.expect(&Tok::RParen)?;
        Ok(inner)
    }

    fn parse_paren_lambda(&mut self) -> VimResult<Expr> {
        let mut params = Vec::new();
        if self.eat(&Tok::RParen) {
            // '() => expr'
        } else {
            loop {
                if self.eat(&Tok::Op("...".into())) {
                    params.push(Param {
                        name: "_".to_string(),
                        ty: Vim9Type::List(Box::new(Vim9Type::Any)),
                        default: None,
                        varargs: true,
                    });
                } else {
                    let Tok::Ident(name) = self.bump() else {
                        return Err(self.error("expected lambda parameter"));
                    };
                    let ty = if self.eat(&Tok::Colon) {
                        let text = self.consume_type_text();
                        Vim9Type::parse(&text)
                            .map_err(|e| self.error(&format!("unknown type: {e}")))?
                    } else {
                        Vim9Type::Any
                    };
                    params.push(Param {
                        name,
                        ty,
                        default: None,
                        varargs: false,
                    });
                }
                if self.eat(&Tok::Comma) {
                    continue;
                }
                self.expect(&Tok::RParen)?;
                break;
            }
        }
        let ret = if self.eat(&Tok::Colon) {
            let text = self.consume_type_text();
            Vim9Type::parse(&text).map_err(|e| self.error(&format!("unknown type: {e}")))?
        } else {
            Vim9Type::Any
        };
        self.expect(&Tok::Arrow)?;
        let body = self.parse_expr(0)?;
        Ok(Expr::Lambda(Rc::new(LambdaDef {
            params,
            ret,
            body: Vec::new(),
            expr_body: Some(body),
            captured: Vec::new(),
        })))
    }

    /// Read a type annotation from the token stream as source text.
    fn consume_type_text(&mut self) -> String {
        let mut text = String::new();
        let Tok::Ident(name) = self.bump() else {
            return text;
        };
        text.push_str(&name);
        if self.peek() == &Tok::Op("<".to_string()) {
            let mut depth = 0i32;
            loop {
                match self.bump() {
                    Tok::Op(op) if op == "<" => {
                        depth += 1;
                        text.push('<');
                    }
                    Tok::Op(op) if op == ">" => {
                        depth -= 1;
                        text.push('>');
                        if depth == 0 {
                            break;
                        }
                    }
                    Tok::Comma => text.push(','),
                    Tok::Ident(part) => text.push_str(&part),
                    Tok::LBracket => text.push('['),
                    Tok::RBracket => text.push(']'),
                    Tok::Eof => break,
                    _ => {}
                }
            }
        } else if self.peek() == &Tok::LParen {
            let mut parens = 0i32;
            loop {
                match self.bump() {
                    Tok::LParen => {
                        parens += 1;
                        text.push('(');
                    }
                    Tok::RParen => {
                        parens -= 1;
                        text.push(')');
                        if parens == 0 {
                            break;
                        }
                    }
                    Tok::Ident(part) => text.push_str(&part),
                    Tok::Comma => text.push(','),
                    Tok::Colon => text.push(':'),
                    Tok::Eof => break,
                    _ => {}
                }
            }
        }
        text
    }

    fn parse_list(&mut self) -> VimResult<Expr> {
        let mut items = Vec::new();
        if self.eat(&Tok::RBracket) {
            return Ok(Expr::List(items));
        }
        loop {
            items.push(self.parse_expr(0)?);
            if self.eat(&Tok::Comma) {
                if self.eat(&Tok::RBracket) {
                    break;
                }
                continue;
            }
            self.expect(&Tok::RBracket)?;
            break;
        }
        Ok(Expr::List(items))
    }

    fn parse_dict(&mut self) -> VimResult<Expr> {
        // Check for a legacy-style lambda: {x -> expr}
        let save = self.pos;
        if let Ok(lambda) = self.try_brace_lambda() {
            return Ok(lambda);
        }
        self.pos = save;
        let mut entries = Vec::new();
        if self.eat(&Tok::RBrace) {
            return Ok(Expr::Dict(entries));
        }
        loop {
            let key = match self.bump() {
                Tok::Ident(name) => DictKey::Literal(name),
                Tok::Str(s) => DictKey::Literal(s),
                Tok::Number(n) => DictKey::Literal(n.to_string()),
                Tok::LBracket => {
                    let expr = self.parse_expr(0)?;
                    self.expect(&Tok::RBracket)?;
                    DictKey::Expr(expr)
                }
                other => {
                    return Err(self.error(&format!(
                        "expected dictionary key, found {}",
                        other.describe()
                    )));
                }
            };
            self.expect(&Tok::Colon)?;
            let value = self.parse_expr(0)?;
            entries.push((key, value));
            if self.eat(&Tok::Comma) {
                if self.eat(&Tok::RBrace) {
                    break;
                }
                continue;
            }
            self.expect(&Tok::RBrace)?;
            break;
        }
        Ok(Expr::Dict(entries))
    }

    fn try_brace_lambda(&mut self) -> VimResult<Expr> {
        let mut params = Vec::new();
        loop {
            match self.bump() {
                Tok::Ident(name) => {
                    let ty = if self.eat(&Tok::Colon) {
                        let text = self.consume_type_text();
                        Vim9Type::parse(&text)
                            .map_err(|e| self.error(&format!("unknown type: {e}")))?
                    } else {
                        Vim9Type::Any
                    };
                    params.push(Param {
                        name,
                        ty,
                        default: None,
                        varargs: false,
                    });
                    if self.eat(&Tok::Comma) {
                        continue;
                    }
                    if self.eat(&Tok::Op("->".into())) {
                        break;
                    }
                    return Err(self.error("expected '->' in lambda"));
                }
                Tok::Op(op) if op == "->" => break,
                other => {
                    return Err(self.error(&format!(
                        "expected lambda parameter, found {}",
                        other.describe()
                    )));
                }
            }
        }
        let body = self.parse_expr(0)?;
        self.expect(&Tok::RBrace)?;
        Ok(Expr::Lambda(Rc::new(LambdaDef {
            params,
            ret: Vim9Type::Any,
            body: Vec::new(),
            expr_body: Some(body),
            captured: Vec::new(),
        })))
    }

    fn parse_arg_list(&mut self) -> VimResult<Vec<Expr>> {
        self.expect(&Tok::LParen)?;
        let mut args = Vec::new();
        if self.eat(&Tok::RParen) {
            return Ok(args);
        }
        loop {
            args.push(self.parse_expr(0)?);
            if self.eat(&Tok::Comma) {
                if self.eat(&Tok::RParen) {
                    break;
                }
                continue;
            }
            self.expect(&Tok::RParen)?;
            break;
        }
        Ok(args)
    }

    fn parse_index(&mut self, target: Expr) -> VimResult<Expr> {
        self.expect(&Tok::LBracket)?;
        if self.eat(&Tok::Colon) {
            let end = if self.peek() == &Tok::RBracket {
                None
            } else {
                Some(Box::new(self.parse_expr(0)?))
            };
            self.expect(&Tok::RBracket)?;
            return Ok(Expr::Slice(Box::new(target), None, end));
        }
        let first = self.parse_expr(0)?;
        if self.eat(&Tok::Colon) {
            let end = if self.peek() == &Tok::RBracket {
                None
            } else {
                Some(Box::new(self.parse_expr(0)?))
            };
            self.expect(&Tok::RBracket)?;
            return Ok(Expr::Slice(Box::new(target), Some(Box::new(first)), end));
        }
        self.expect(&Tok::RBracket)?;
        Ok(Expr::Index(Box::new(target), Box::new(first)))
    }

    fn parse_interpolated(&mut self, raw: &str) -> VimResult<Expr> {
        let chars: Vec<char> = raw.chars().collect();
        let mut parts: Vec<Expr> = Vec::new();
        let mut literal = String::new();
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            if c == '\\' {
                if let Some(next) = chars.get(i + 1).copied() {
                    literal.push_str(&decode_simple_escape(next));
                    i += 2;
                    continue;
                }
            }
            if c == '{' {
                if chars.get(i + 1) == Some(&'{') {
                    literal.push('{');
                    i += 2;
                    continue;
                }
                // Find the matching close brace, respecting nested braces/strings.
                let start = i + 1;
                let mut depth = 1i32;
                let mut quote: Option<char> = None;
                let mut j = start;
                while j < chars.len() {
                    let ch = chars[j];
                    match quote {
                        Some(q) => {
                            if ch == '\\' && q == '"' {
                                j += 1;
                            } else if ch == q {
                                quote = None;
                            }
                        }
                        None => match ch {
                            '\'' | '"' => quote = Some(ch),
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        },
                    }
                    j += 1;
                }
                if depth != 0 {
                    return Err(self.error("missing '}' in interpolated string"));
                }
                if !literal.is_empty() {
                    parts.push(Expr::Str(std::mem::take(&mut literal)));
                }
                let expr_text: String = chars[start..j].iter().collect();
                parts.push(parse_expression(&expr_text)?);
                i = j + 1;
                continue;
            }
            literal.push(c);
            i += 1;
        }
        if !literal.is_empty() {
            parts.push(Expr::Str(literal));
        }
        let mut iter = parts.into_iter();
        let Some(mut expr) = iter.next() else {
            return Ok(Expr::Str(String::new()));
        };
        for part in iter {
            expr = Expr::Binary("..".to_string(), Box::new(expr), Box::new(part));
        }
        Ok(expr)
    }
}

fn decode_simple_escape(c: char) -> String {
    match c {
        'n' => "\n".to_string(),
        't' => "\t".to_string(),
        'r' => "\r".to_string(),
        '\\' => "\\".to_string(),
        '"' => "\"".to_string(),
        other => other.to_string(),
    }
}

/// Parse a complete Vim9 expression.
pub fn parse_expression(src: &str) -> VimResult<Expr> {
    ExprParser::new(src)?.parse_all()
}

/// Parse an :import line (the text includes the command word).
pub fn parse_import_line(text: &str, number: usize) -> VimResult<Stmt> {
    let line = SourceLine {
        text: text.to_string(),
        number,
    };
    parse_import(&line)
}

/// Parse one expression and return how many characters it consumed.
pub fn parse_expression_prefix(src: &str) -> VimResult<(Expr, usize)> {
    let mut parser = ExprParser::new(src)?;
    let expr = parser.parse_expr(0)?;
    Ok((expr, parser.consumed()))
}

// ---------------------------------------------------------------------------
// Statement parser
// ---------------------------------------------------------------------------

pub struct StmtParser<'a> {
    pub lines: &'a [SourceLine],
    pub pos: usize,
    pub sid: usize,
    pub prefix: &'a str,
}

impl<'a> StmtParser<'a> {
    pub fn parse_block(&mut self, stops: &[&str]) -> VimResult<(Vec<Stmt>, Option<String>)> {
        let mut out = Vec::new();
        while self.pos < self.lines.len() {
            let line = self.lines[self.pos].clone();
            let keyword = line_keyword(&line.text);
            if stops.contains(&keyword.as_str()) {
                return Ok((out, Some(keyword)));
            }
            if keyword.is_empty() {
                let stmt = self.parse_simple(&line)?;
                out.push(stmt);
                self.pos += 1;
                continue;
            }
            match keyword.as_str() {
                "if" => out.push(self.parse_if().map_err(|e| add_line(e, line.number))?),
                "while" => out.push(self.parse_while().map_err(|e| add_line(e, line.number))?),
                "for" => out.push(self.parse_for().map_err(|e| add_line(e, line.number))?),
                "try" => out.push(self.parse_try().map_err(|e| add_line(e, line.number))?),
                "var" | "const" | "final" => {
                    self.pos += 1;
                    out.push(parse_var_line(&line, false).map_err(|e| add_line(e, line.number))?);
                }
                "let" => {
                    self.pos += 1;
                    out.push(parse_var_line(&line, false).map_err(|e| add_line(e, line.number))?);
                }
                "export" => out.push(
                    self.parse_export(&line)
                        .map_err(|e| add_line(e, line.number))?,
                ),
                "class" => out.push(
                    self.parse_class(&line, false, false, false)
                        .map_err(|e| add_line(e, line.number))?,
                ),
                "interface" => out.push(
                    self.parse_class(&line, true, false, false)
                        .map_err(|e| add_line(e, line.number))?,
                ),
                "abstract" => {
                    let rest = text_after_keyword(&line.text, "abstract");
                    if rest.trim_start().starts_with("class") {
                        out.push(
                            self.parse_class(&line, false, true, false)
                                .map_err(|e| add_line(e, line.number))?,
                        );
                    } else if rest.trim_start().starts_with("def") {
                        // Abstract method in an abstract class: parsed by the
                        // class body parser, not at block level.
                        self.pos += 1;
                        out.push(Stmt::Nop);
                    } else {
                        let stmt = self
                            .parse_simple(&line)
                            .map_err(|e| add_line(e, line.number))?;
                        out.push(stmt);
                        self.pos += 1;
                    }
                }
                "enum" => out.push(
                    self.parse_enum(&line, false)
                        .map_err(|e| add_line(e, line.number))?,
                ),
                "type" => {
                    let stmt = self.parse_type_alias(&line, false)?;
                    self.pos += 1;
                    out.push(stmt);
                }
                "def" => {
                    let rest = text_after_keyword(&line.text, "def");
                    if rest.contains('(') {
                        out.push(
                            self.parse_def(&line, false)
                                .map_err(|e| add_line(e, line.number))?,
                        );
                    } else {
                        // ':def' with no signature lists definitions.
                        let stmt = self
                            .parse_simple(&line)
                            .map_err(|e| add_line(e, line.number))?;
                        out.push(stmt);
                        self.pos += 1;
                    }
                }
                "func" | "function" => out.push(self.parse_legacy_func(&line, false)?),
                "import" => {
                    self.pos += 1;
                    out.push(parse_import(&line)?);
                }
                "defcompile" | "vim9script" | "scriptversion" => {
                    self.pos += 1;
                    out.push(Stmt::Nop);
                }
                "return" => {
                    self.pos += 1;
                    let rest = text_after_keyword(&line.text, "return");
                    let value = if rest.trim().is_empty() {
                        None
                    } else {
                        Some(parse_expression(rest.trim())?)
                    };
                    out.push(Stmt::Return {
                        value,
                        line: line.number,
                    });
                }
                "throw" => {
                    self.pos += 1;
                    let rest = text_after_keyword(&line.text, "throw");
                    if rest.trim().is_empty() {
                        return Err(VimError::new("E471", "Argument required for :throw"));
                    }
                    out.push(Stmt::Throw {
                        value: parse_expression(rest.trim())?,
                        line: line.number,
                    });
                }
                "break" => {
                    self.pos += 1;
                    out.push(Stmt::Break { line: line.number });
                }
                "continue" => {
                    self.pos += 1;
                    out.push(Stmt::Continue { line: line.number });
                }
                "else" | "elseif" | "endif" | "endwhile" | "endfor" | "endtry" | "catch"
                | "finally" | "enddef" | "endclass" | "endinterface" | "endenum" => {
                    return Err(VimError::new(
                        "E580",
                        format!(":{keyword} without matching opener at line {}", line.number),
                    ));
                }
                _ => {
                    let stmt = self
                        .parse_simple(&line)
                        .map_err(|e| add_line(e, line.number))?;
                    out.push(stmt);
                    self.pos += 1;
                }
            }
        }
        Ok((out, None))
    }

    fn parse_if(&mut self) -> VimResult<Stmt> {
        let first = self.lines[self.pos].clone();
        let mut cond_text = text_after_keyword(&first.text, "if").trim().to_string();
        self.pos += 1;
        let mut branches = Vec::new();
        loop {
            if cond_text.is_empty() {
                return Err(VimError::new("E15", "Missing :if condition"));
            }
            let cond = parse_expression(&cond_text)?;
            let (body, term) = self.parse_block(&["elseif", "else", "endif"])?;
            branches.push((cond, body));
            match term.as_deref() {
                Some("elseif") => {
                    let line = self.lines[self.pos].clone();
                    cond_text = text_after_keyword(&line.text, "elseif").trim().to_string();
                    self.pos += 1;
                }
                Some("else") => {
                    self.pos += 1;
                    let (else_body, term2) = self.parse_block(&["endif"])?;
                    if term2.as_deref() != Some("endif") {
                        return Err(VimError::new("E171", "Missing :endif"));
                    }
                    self.pos += 1;
                    return Ok(Stmt::If {
                        branches,
                        else_body: Some(else_body),
                        line: first.number,
                    });
                }
                Some("endif") => {
                    self.pos += 1;
                    return Ok(Stmt::If {
                        branches,
                        else_body: None,
                        line: first.number,
                    });
                }
                _ => return Err(VimError::new("E171", "Missing :endif")),
            }
        }
    }

    fn parse_while(&mut self) -> VimResult<Stmt> {
        let line = self.lines[self.pos].clone();
        let cond_text = text_after_keyword(&line.text, "while").trim().to_string();
        self.pos += 1;
        let cond = parse_expression(&cond_text)?;
        let (body, term) = self.parse_block(&["endwhile"])?;
        if term.as_deref() != Some("endwhile") {
            return Err(VimError::new("E170", "Missing :endwhile"));
        }
        self.pos += 1;
        Ok(Stmt::While {
            cond,
            body,
            line: line.number,
        })
    }

    fn parse_for(&mut self) -> VimResult<Stmt> {
        let line = self.lines[self.pos].clone();
        let rest = text_after_keyword(&line.text, "for").trim().to_string();
        let (left, right) = rest
            .split_once(" in ")
            .ok_or_else(|| VimError::new("E690", "Missing :in"))?;
        let vars = parse_for_vars(left.trim())?;
        let iter = parse_expression(right.trim())?;
        self.pos += 1;
        let (body, term) = self.parse_block(&["endfor"])?;
        if term.as_deref() != Some("endfor") {
            return Err(VimError::new("E170", "Missing :endfor"));
        }
        self.pos += 1;
        Ok(Stmt::For {
            vars,
            iter,
            body,
            line: line.number,
        })
    }

    fn parse_try(&mut self) -> VimResult<Stmt> {
        let line = self.lines[self.pos].clone();
        self.pos += 1;
        let (body, mut term) = self.parse_block(&["catch", "finally", "endtry"])?;
        let mut catches = Vec::new();
        let mut finally = Vec::new();
        loop {
            match term.as_deref() {
                Some("catch") => {
                    let catch_line = self.lines[self.pos].clone();
                    let spec = text_after_keyword(&catch_line.text, "catch");
                    let pattern = parse_catch_pattern(&spec);
                    self.pos += 1;
                    let (catch_body, next) = self.parse_block(&["catch", "finally", "endtry"])?;
                    catches.push(CatchClause {
                        pattern,
                        body: catch_body,
                        line: catch_line.number,
                    });
                    term = next;
                }
                Some("finally") => {
                    self.pos += 1;
                    let (finally_body, next) = self.parse_block(&["endtry"])?;
                    finally = finally_body;
                    term = next;
                }
                Some("endtry") => {
                    self.pos += 1;
                    return Ok(Stmt::Try {
                        body,
                        catches,
                        finally,
                        line: line.number,
                    });
                }
                _ => return Err(VimError::new("E600", "Missing :endtry")),
            }
        }
    }

    fn parse_export(&mut self, line: &SourceLine) -> VimResult<Stmt> {
        let exported = text_after_keyword(&line.text, "export");
        let rest = exported.trim_start();
        if rest.starts_with("def") {
            return self.parse_def(line, true);
        }
        if rest.starts_with("class") {
            return self.parse_class(line, false, false, true);
        }
        if rest.starts_with("interface") {
            return self.parse_class(line, true, false, true);
        }
        if rest.starts_with("abstract class") {
            return self.parse_class(line, false, true, true);
        }
        if rest.starts_with("enum") {
            return self.parse_enum(line, true);
        }
        if rest.starts_with("type") {
            let stmt = self.parse_type_alias(line, true)?;
            self.pos += 1;
            return Ok(stmt);
        }
        if rest.starts_with("func") || rest.starts_with("function") {
            return self.parse_legacy_func(line, true);
        }
        if rest.starts_with("var ") || rest.starts_with("const ") || rest.starts_with("final ") {
            self.pos += 1;
            let inner = SourceLine {
                text: rest.to_string(),
                number: line.number,
            };
            return parse_var_line(&inner, true);
        }
        Err(VimError::new(
            "E1042",
            format!(":export must be followed by def/var/const/final, not: {rest}"),
        ))
    }

    fn parse_def(&mut self, line: &SourceLine, export: bool) -> VimResult<Stmt> {
        let header = line.text.trim();
        let (name, params, varargs, ret, header_export) = parse_def_header(header)?;
        let is_export = export || header_export;
        self.pos += 1;
        let start = self.pos;
        let mut depth = 1usize;
        while self.pos < self.lines.len() {
            let keyword = line_keyword(&self.lines[self.pos].text);
            if keyword == "enddef" {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else if keyword == "def"
                && self.lines[self.pos]
                    .text
                    .split_once("def")
                    .is_some_and(|(_, rest)| rest.contains('('))
            {
                // Nested definitions are legal in Vim9 (closures created by
                // a function that defines a function when it runs).
                depth += 1;
            }
            let marker = heredoc_marker(&self.lines[self.pos].text);
            self.pos += 1;
            if let Some(marker) = marker {
                // Skip the heredoc body: it may contain the word 'enddef'.
                while self.pos < self.lines.len() {
                    let line = &self.lines[self.pos];
                    let done = line.text.trim() == marker;
                    self.pos += 1;
                    if done {
                        break;
                    }
                }
            }
        }
        if self.pos >= self.lines.len() {
            return Err(VimError::new(
                "E126",
                format!("Missing :enddef for function {name}"),
            ));
        }
        let body: &'a [SourceLine] = &self.lines[start..self.pos];
        self.pos += 1; // consume :enddef
        let mut sub = StmtParser {
            lines: body,
            pos: 0,
            sid: self.sid,
            prefix: self.prefix,
        };
        let (stmts, term) = sub.parse_block(&[])?;
        if let Some(term) = term {
            return Err(VimError::new(
                "E580",
                format!("Unexpected :{term} in function {name}"),
            ));
        }
        let canonical = canonical_name(&name, self.sid, self.prefix, is_export);
        Ok(Stmt::Def(Rc::new(Vim9Func {
            name,
            canonical,
            params,
            varargs,
            ret,
            body: stmts,
            sid: self.sid,
            is_export,
            line: line.number,
        })))
    }

    fn parse_legacy_func(&mut self, line: &SourceLine, export: bool) -> VimResult<Stmt> {
        let full = line.text.trim();
        let without_export = full.strip_prefix("export ").unwrap_or(full).trim_start();
        let header = if let Some(rest) = without_export.strip_prefix("function ") {
            rest.trim().to_string()
        } else if let Some(rest) = without_export.strip_prefix("func ") {
            rest.trim().to_string()
        } else {
            return Err(VimError::new("E129", "Invalid :func header"));
        };
        self.pos += 1;
        let mut body = Vec::new();
        while self.pos < self.lines.len() {
            if line_keyword(&self.lines[self.pos].text) == "endfunc" {
                break;
            }
            let text = self.lines[self.pos].text.clone();
            let marker = heredoc_marker(&text);
            body.push(text);
            self.pos += 1;
            if let Some(marker) = marker {
                while self.pos < self.lines.len() {
                    let line = self.lines[self.pos].text.clone();
                    let done = line.trim() == marker;
                    body.push(line);
                    self.pos += 1;
                    if done {
                        break;
                    }
                }
            }
        }
        if self.pos >= self.lines.len() {
            return Err(VimError::new("E126", "Missing :endfunc"));
        }
        self.pos += 1;
        Ok(Stmt::LegacyFunc {
            header,
            body,
            export,
            line: line.number,
        })
    }

    fn parse_class(
        &mut self,
        line: &SourceLine,
        is_interface: bool,
        is_abstract: bool,
        export: bool,
    ) -> VimResult<Stmt> {
        let text = line.text.trim().trim_start_matches(':').trim();
        let mut rest = text;
        if let Some(after) = rest.strip_prefix("export ") {
            rest = after.trim_start();
        }
        let mut abstract_flag = is_abstract;
        if let Some(after) = rest.strip_prefix("abstract ") {
            abstract_flag = true;
            rest = after.trim_start();
        }
        let keyword = if is_interface { "interface" } else { "class" };
        let rest = rest
            .strip_prefix(keyword)
            .ok_or_else(|| VimError::new("E1316", "Missing class keyword"))?
            .trim_start();
        let name = rest
            .split(|c: char| c.is_whitespace() || c == '(' || c == '{' || c == ':')
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return Err(VimError::new("E1314", "Missing class name"));
        }
        let mut parent = None;
        if let Some(pos) = rest.find(" extends ") {
            let after = &rest[pos + 9..];
            parent = after
                .split_whitespace()
                .next()
                .map(|word| word.trim_end_matches(',').to_string());
        }
        self.pos += 1;
        let start = self.pos;
        let end_keyword = if is_interface {
            "endinterface"
        } else {
            "endclass"
        };
        while self.pos < self.lines.len() {
            if line_keyword(&self.lines[self.pos].text) == end_keyword {
                break;
            }
            self.pos += 1;
        }
        if self.pos >= self.lines.len() {
            return Err(VimError::new("E1063", format!("Missing :{end_keyword}")));
        }
        let body: &'a [SourceLine] = &self.lines[start..self.pos];
        self.pos += 1; // consume endclass/endinterface
        let mut members = Vec::new();
        let mut method_list = Vec::new();
        let mut static_methods = Vec::new();
        let mut constructor = None;
        let mut i = 0usize;
        while i < body.len() {
            let mut text = body[i].text.trim().to_string();
            if text.is_empty() {
                i += 1;
                continue;
            }
            let mut public = false;
            let mut static_member = false;
            let mut is_abstract_member = false;
            loop {
                if let Some(after) = text.strip_prefix("public ") {
                    public = true;
                    text = after.trim_start().to_string();
                } else if let Some(after) = text.strip_prefix("static ") {
                    static_member = true;
                    text = after.trim_start().to_string();
                } else if let Some(after) = text.strip_prefix("final ") {
                    text = after.trim_start().to_string();
                } else if let Some(after) = text.strip_prefix("abstract ") {
                    is_abstract_member = true;
                    text = after.trim_start().to_string();
                } else {
                    break;
                }
            }
            if let Some(spec) = text.strip_prefix("var ") {
                let spec = spec.trim();
                let (lhs, default_text) = match find_assignment(spec) {
                    Some((lhs, _, rhs)) => (lhs.trim(), Some(rhs.trim())),
                    None => (spec, None),
                };
                let (member_name, ty) = match find_type_separator(lhs) {
                    Some(idx) => {
                        let ty = Vim9Type::parse(lhs[idx + 1..].trim())
                            .map_err(|e| VimError::new("E1097", format!("Unknown type: {e}")))?;
                        (lhs[..idx].trim().to_string(), ty)
                    }
                    None => (lhs.to_string(), Vim9Type::Any),
                };
                let default = match default_text {
                    Some(expr) if !expr.is_empty() => Some(parse_expression(expr)?),
                    _ => None,
                };
                members.push(VimClassMember {
                    name: member_name,
                    ty,
                    public,
                    static_member,
                    default,
                    line: body[i].number,
                });
                i += 1;
                continue;
            }
            if let Some(header) = text.strip_prefix("def ") {
                let header = format!("def {}", header.trim());
                if is_interface || is_abstract_member {
                    // Interface/abstract declarations have no body.
                    let (method_name, _params, _varargs, _ret, _export) =
                        parse_def_header(&header)?;
                    let func = Rc::new(Vim9Func {
                        name: method_name.clone(),
                        canonical: format!("<SNR>{}_{}_{}", self.sid, name, method_name),
                        params: _params,
                        varargs: _varargs,
                        ret: _ret,
                        body: Vec::new(),
                        sid: self.sid,
                        is_export: false,
                        line: body[i].number,
                    });
                    if method_name == "new" {
                        constructor = Some(func);
                    } else {
                        method_list.push(func);
                    }
                    i += 1;
                    continue;
                }
                let mut j = i + 1;
                while j < body.len() && line_keyword(&body[j].text) != "enddef" {
                    j += 1;
                }
                if j >= body.len() {
                    return Err(VimError::new("E126", "Missing :enddef in class method"));
                }
                let raw_body: Vec<String> = body[i + 1..j]
                    .iter()
                    .map(|line| line.text.clone())
                    .collect();
                let parsed = parse_function(&header, &raw_body, self.sid, "", body[i].number)?;
                let mut func = (*parsed).clone();
                func.canonical = format!("<SNR>{}_{}_{}", self.sid, name, func.name);
                func.line = body[i].number;
                if func.name == "new" {
                    constructor = Some(Rc::new(func));
                } else {
                    if static_member {
                        static_methods.push(func.name.clone());
                    }
                    method_list.push(Rc::new(func));
                }
                i = j + 1;
                continue;
            }
            i += 1;
        }
        let canonical = canonical_name(&name, self.sid, self.prefix, export);
        Ok(Stmt::ClassDef(Rc::new(VimClass {
            name,
            canonical,
            is_export: export,
            is_abstract: abstract_flag,
            is_interface,
            sid: self.sid,
            parent,
            members,
            methods: method_list,
            static_methods,
            constructor,
            line: line.number,
        })))
    }

    fn parse_enum(&mut self, line: &SourceLine, export: bool) -> VimResult<Stmt> {
        let rest = text_after_keyword(&line.text, "enum");
        let name = rest.split_whitespace().next().unwrap_or("").to_string();
        if name.is_empty() {
            return Err(VimError::new("E1414", "Missing enum name"));
        }
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.lines.len() && line_keyword(&self.lines[self.pos].text) != "endenum" {
            self.pos += 1;
        }
        if self.pos >= self.lines.len() {
            return Err(VimError::new("E1415", "Missing :endenum"));
        }
        let body: &'a [SourceLine] = &self.lines[start..self.pos];
        self.pos += 1;
        let mut values = Vec::new();
        let mut next_value = 0i64;
        for body_line in body {
            let text = body_line.text.trim().trim_start_matches(':').trim();
            if text.is_empty() {
                continue;
            }
            for part in split_top_level(text, ',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let (member, explicit) = match find_assignment(part) {
                    Some((lhs, _, rhs)) => (lhs.trim(), Some(rhs.trim())),
                    None => (part, None),
                };
                if let Some(expr_text) = explicit {
                    if let Ok(Expr::Number(n)) = parse_expression(expr_text) {
                        next_value = n;
                    }
                }
                values.push((member.to_string(), next_value));
                next_value += 1;
            }
        }
        Ok(Stmt::EnumDef(Rc::new(VimEnum {
            canonical: canonical_name(&name, self.sid, self.prefix, export),
            name,
            is_export: export,
            values,
            line: line.number,
        })))
    }

    fn parse_type_alias(&mut self, line: &SourceLine, export: bool) -> VimResult<Stmt> {
        let rest = text_after_keyword(&line.text, "type").trim().to_string();
        let (name, ty_text) = rest
            .split_once('=')
            .ok_or_else(|| VimError::new("E1392", "Missing '=' in :type"))?;
        let name = name.trim().to_string();
        let ty = Vim9Type::parse(ty_text.trim())
            .map_err(|e| VimError::new("E1097", format!("Unknown type: {e}")))?;
        Ok(Stmt::TypeAlias(Rc::new(VimTypeAlias {
            canonical: canonical_name(&name, self.sid, self.prefix, export),
            name,
            is_export: export,
            ty,
            line: line.number,
        })))
    }

    fn parse_simple(&mut self, line: &SourceLine) -> VimResult<Stmt> {
        let text = strip_leading_colon(&line.text);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(Stmt::Nop);
        }
        // Prefix ++/--.
        if let Some(rest) = trimmed.strip_prefix("++") {
            let target = parse_expression(rest.trim())?;
            ensure_lvalue(&target, line.number)?;
            return Ok(Stmt::IncDec {
                target,
                delta: 1,
                line: line.number,
            });
        }
        if let Some(rest) = trimmed.strip_prefix("--") {
            let target = parse_expression(rest.trim())?;
            ensure_lvalue(&target, line.number)?;
            return Ok(Stmt::IncDec {
                target,
                delta: -1,
                line: line.number,
            });
        }
        // Postfix x++ / x--.
        if let Some(target_text) = trimmed.strip_suffix("++") {
            if let Ok(target) = parse_expression(target_text.trim()) {
                if is_lvalue(&target) {
                    return Ok(Stmt::IncDec {
                        target,
                        delta: 1,
                        line: line.number,
                    });
                }
            }
        }
        if let Some(target_text) = trimmed.strip_suffix("--") {
            if let Ok(target) = parse_expression(target_text.trim()) {
                if is_lvalue(&target) {
                    return Ok(Stmt::IncDec {
                        target,
                        delta: -1,
                        line: line.number,
                    });
                }
            }
        }
        // :call Func(...)
        if let Some(rest) = trimmed.strip_prefix("call ") {
            let expr = parse_expression(rest.trim())?;
            if matches!(expr, Expr::Call(_, _) | Expr::Method(_, _, _)) {
                return Ok(Stmt::ExprStmt {
                    expr,
                    line: line.number,
                });
            }
            return Ok(Stmt::Ex {
                line: trimmed.to_string(),
                number: line.number,
            });
        }
        let first_word = first_word(trimmed);
        // 'chdir(cwd)' is a function call, not ':chdir (cwd)'. A command word
        // followed directly by '(' is always an expression in Vim9.
        let call_syntax = trimmed.starts_with(&format!("{first_word}("));
        let is_ex = EX_WORDS.contains(&first_word.as_str()) && !call_syntax;
        // Assignment.
        if !is_ex {
            if let Some((lhs, op, rhs)) = find_assignment(trimmed) {
                if let Ok(target) = parse_expression(lhs) {
                    if is_lvalue(&target) {
                        let value = parse_expression(rhs)?;
                        return Ok(Stmt::Assign {
                            target,
                            op,
                            value,
                            line: line.number,
                        });
                    }
                }
            }
        }
        // Bare expression statement (method chains, function calls).
        if !is_ex {
            if let Ok(expr) = parse_expression(trimmed) {
                if matches!(
                    expr,
                    Expr::Call(_, _) | Expr::Method(_, _, _) | Expr::Unary(_, _)
                ) {
                    return Ok(Stmt::ExprStmt {
                        expr,
                        line: line.number,
                    });
                }
            }
        }
        Ok(Stmt::Ex {
            line: trimmed.to_string(),
            number: line.number,
        })
    }
}

fn add_line(error: VimError, line: usize) -> VimError {
    if error.message.contains("(line ") {
        return error;
    }
    VimError {
        code: error.code,
        message: format!("{} (line {})", error.message, line),
        abort: error.abort,
    }
}

fn text_after_keyword(text: &str, keyword: &str) -> String {
    let t = text.trim_start();
    let t = t.strip_prefix(':').unwrap_or(t).trim_start();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with(keyword) {
        t[keyword.len()..].to_string()
    } else {
        String::new()
    }
}

fn strip_leading_colon(text: &str) -> &str {
    let t = text.trim_start();
    t.strip_prefix(':').unwrap_or(t)
}

fn first_word(text: &str) -> String {
    let mut word = String::new();
    for c in text.trim_start().chars() {
        if c.is_alphanumeric() || c == '!' || c == '_' || c == '#' {
            word.push(c.to_ascii_lowercase());
        } else {
            break;
        }
    }
    word
}

fn parse_var_line(line: &SourceLine, export: bool) -> VimResult<Stmt> {
    let text = strip_leading_colon(&line.text);
    let trimmed = text.trim_start();
    let keyword = first_word(trimmed);
    if !matches!(keyword.as_str(), "var" | "const" | "final" | "let") {
        return Err(VimError::new(
            "E1042",
            format!("Expected var/const/final, found {keyword}"),
        ));
    }
    let rest = trimmed[keyword.len()..].trim_start();
    if rest.is_empty() {
        return Err(VimError::new("E471", "Argument required"));
    }
    let (lhs, value_text) = match find_assignment(rest) {
        Some((lhs, _, rhs)) => (lhs.trim(), Some(rhs.trim())),
        None => (rest.trim(), None),
    };
    if lhs.starts_with('[') && lhs.ends_with(']') {
        let inner = &lhs[1..lhs.len() - 1];
        let mut names = Vec::new();
        for part in split_top_level(inner, ',') {
            names.push(parse_one_for_var(part.trim())?);
        }
        let value = match value_text {
            Some(v) if !v.is_empty() => parse_expression(v)?,
            _ => {
                return Err(VimError::new(
                    "E1017",
                    "Destructuring declaration needs a value",
                ));
            }
        };
        return Ok(Stmt::Destructure {
            names,
            value,
            constant: matches!(keyword.as_str(), "const" | "final"),
            line: line.number,
        });
    }
    let (name, ty) = match find_type_separator(lhs) {
        Some(idx) => {
            let ty_text = lhs[idx + 1..].trim();
            let ty = Vim9Type::parse(ty_text)
                .map_err(|e| VimError::new("E1097", format!("Unknown type: {e}")))?;
            (lhs[..idx].trim().to_string(), Some(ty))
        }
        None => (lhs.trim().to_string(), None),
    };
    if name.is_empty() || !name.chars().all(is_name_char) {
        return Err(VimError::new(
            "E461",
            format!("Illegal variable name: {name}"),
        ));
    }
    let value = match value_text {
        Some(v) if !v.is_empty() => Some(parse_expression(v)?),
        _ => None,
    };
    Ok(Stmt::Var {
        name,
        ty,
        value,
        constant: matches!(keyword.as_str(), "const" | "final"),
        export,
        line: line.number,
    })
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '#' || c == ':'
}

fn parse_for_vars(text: &str) -> VimResult<Vec<(String, Option<Vim9Type>)>> {
    let mut out = Vec::new();
    let inner = text.trim();
    if let Some(rest) = inner.strip_prefix('[') {
        let rest = rest
            .strip_suffix(']')
            .ok_or_else(|| VimError::new("E475", "Missing ] in :for"))?;
        for part in split_top_level(rest, ',') {
            out.push(parse_one_for_var(part.trim())?);
        }
    } else {
        out.push(parse_one_for_var(inner)?);
    }
    Ok(out)
}

fn parse_one_for_var(text: &str) -> VimResult<(String, Option<Vim9Type>)> {
    match find_type_colon(text) {
        Some(idx) => {
            let name = text[..idx].trim().to_string();
            let ty = Vim9Type::parse(text[idx + 1..].trim())
                .map_err(|e| VimError::new("E1097", format!("Unknown type: {e}")))?;
            Ok((name, Some(ty)))
        }
        None => Ok((text.trim().to_string(), None)),
    }
}

fn parse_catch_pattern(spec: &str) -> Option<String> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('/') {
        let end = rest.rfind('/').unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }
    Some(s.to_string())
}

fn parse_import(line: &SourceLine) -> VimResult<Stmt> {
    let rest = text_after_keyword(&line.text, "import").trim().to_string();
    let mut rest = rest.as_str();
    let mut autoload = false;
    if let Some(after) = rest.strip_prefix("autoload ") {
        autoload = true;
        rest = after.trim_start();
    }
    if let Some(after) = rest.strip_prefix('{') {
        let close = after
            .find('}')
            .ok_or_else(|| VimError::new("E1049", "Missing } in :import"))?;
        let names: Vec<String> = after[..close]
            .split(',')
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let tail = after[close + 1..].trim_start();
        let tail = tail
            .strip_prefix("from ")
            .ok_or_else(|| VimError::new("E1049", "Missing 'from' in :import"))?
            .trim_start();
        let (path, _) = parse_import_path(tail)?;
        return Ok(Stmt::Import {
            kind: ImportKind::From {
                names,
                path,
                autoload,
            },
            line: line.number,
        });
    }
    let (path, after_path) = parse_import_path(rest)?;
    let alias = after_path
        .trim()
        .strip_prefix("as ")
        .map(|a| a.trim().to_string());
    let kind = if autoload {
        ImportKind::Autoload {
            path,
            as_name: alias,
        }
    } else {
        ImportKind::Script {
            path,
            as_name: alias,
        }
    };
    Ok(Stmt::Import {
        kind,
        line: line.number,
    })
}

fn parse_import_path(text: &str) -> VimResult<(String, &str)> {
    let text = text.trim_start();
    let quote = text
        .chars()
        .next()
        .filter(|c| *c == '\'' || *c == '"')
        .ok_or_else(|| VimError::new("E1049", "Missing file name in :import"))?;
    let mut end = None;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 1usize;
    while i < chars.len() {
        if chars[i] == quote {
            end = Some(i);
            break;
        }
        i += 1;
    }
    let end = end.ok_or_else(|| VimError::new("E1049", "Missing closing quote in :import"))?;
    let raw: String = chars[1..end].iter().collect();
    let rest: String = chars[end + 1..].iter().collect();
    // Re-borrow the tail from the original string: byte offset of end+1.
    let byte_off = text
        .char_indices()
        .nth(end + 1)
        .map(|(o, _)| o)
        .unwrap_or(text.len());
    let _ = rest;
    Ok((raw, &text[byte_off..]))
}

fn find_assignment(text: &str) -> Option<(&str, String, &str)> {
    let chars: Vec<char> = text.chars().collect();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == '\\' && q == '"' {
                    i += 1;
                } else if c == q {
                    if q == '\'' && chars.get(i + 1) == Some(&'\'') {
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                '=' if depth == 0 => {
                    let prev = if i > 0 {
                        chars.get(i - 1).copied()
                    } else {
                        None
                    };
                    let next = chars.get(i + 1).copied();
                    if matches!(
                        prev,
                        Some('=') | Some('!') | Some('<') | Some('>') | Some('~')
                    ) || matches!(next, Some('=') | Some('~') | Some('>'))
                    {
                        i += 1;
                        continue;
                    }
                    // Compound operator: include the preceding character.
                    let (op_start, op) = match prev {
                        Some('+') => (i - 1, "+=".to_string()),
                        Some('-') => (i - 1, "-=".to_string()),
                        Some('*') => (i - 1, "*=".to_string()),
                        Some('/') => (i - 1, "/=".to_string()),
                        Some('%') => (i - 1, "%=".to_string()),
                        Some('.') if i >= 2 && chars[i - 2] == '.' => (i - 2, "..=".to_string()),
                        _ => (i, "=".to_string()),
                    };
                    let byte_start = text
                        .char_indices()
                        .nth(op_start)
                        .map(|(o, _)| o)
                        .unwrap_or(0);
                    let byte_eq = text.char_indices().nth(i).map(|(o, _)| o).unwrap_or(0);
                    let byte_after = text
                        .char_indices()
                        .nth(i + 1)
                        .map(|(o, _)| o)
                        .unwrap_or(text.len());
                    let lhs = &text[..byte_start];
                    let rhs = &text[byte_after..];
                    let _ = byte_eq;
                    return Some((lhs, op, rhs));
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn is_lvalue(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Var(_) | Expr::Index(_, _) | Expr::Attr(_, _) | Expr::Slice(_, _, _)
    )
}

fn ensure_lvalue(expr: &Expr, line: usize) -> VimResult<()> {
    if is_lvalue(expr) {
        Ok(())
    } else {
        Err(VimError::new(
            "E474",
            format!("Invalid argument for ++/-- at line {line}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(src: &str) -> Expr {
        parse_expression(src).unwrap()
    }

    #[test]
    fn parses_named_varargs_as_a_typed_parameter() {
        let (_, params, variadic, _, _) =
            parse_def_header("def Collect(first: number = 7, ...rest: list<number>): number")
                .unwrap();
        assert!(variadic);
        assert_eq!(params[1].name, "rest");
        assert!(params[1].varargs);
        assert_eq!(params[1].ty, Vim9Type::List(Box::new(Vim9Type::Number)));
        assert!(params[0].default.is_some());
    }

    #[test]
    fn rejects_non_list_varargs_types() {
        let error = parse_def_header("def Collect(...rest: number)").unwrap_err();
        assert_eq!(error.code, "E1180");
    }

    #[test]
    fn rejects_parameters_after_varargs_or_a_varargs_default() {
        for header in [
            "def F(...rest: list<number>, n: number)",
            "def F(..., n: number)",
            "def F(...rest: list<number> = [])",
        ] {
            assert!(parse_def_header(header).is_err(), "{header}");
        }
    }

    #[test]
    fn parses_precedence_and_ternary() {
        assert!(matches!(expr("1 + 2 * 3"), Expr::Binary(op, _, _) if op == "+"));
        assert!(matches!(expr("a ? b : c"), Expr::Ternary(_, _, _)));
        assert!(matches!(expr("a && b || c"), Expr::Binary(op, _, _) if op == "||"));
    }

    #[test]
    fn parses_calls_members_and_methods() {
        assert!(matches!(expr("getline(1)"), Expr::Call(_, args) if args.len() == 1));
        assert!(matches!(expr("tag['kind']"), Expr::Index(_, _)));
        assert!(matches!(expr("line[1 : 2]"), Expr::Slice(_, _, _)));
        assert!(matches!(expr("val->has_key('x')"), Expr::Method(_, name, _) if name == "has_key"));
        assert!(matches!(expr("a.b.c"), Expr::Attr(_, _)));
    }

    #[test]
    fn parses_dicts_and_lambdas() {
        assert!(matches!(expr("{a: 1, b: 2}"), Expr::Dict(entries) if entries.len() == 2));
        assert!(matches!(expr("#{a: 1}"), Expr::Dict(entries) if entries.len() == 1));
        assert!(matches!(expr("{x -> x + 1}"), Expr::Lambda(_)));
        assert!(matches!(
            expr("(x: number): number => x + 1"),
            Expr::Lambda(_)
        ));
        assert!(matches!(
            expr("(_, v: dict<any>): bool => v.x == 1"),
            Expr::Lambda(_)
        ));
    }

    #[test]
    fn parses_interpolation() {
        let e = expr("$\"a {name} b\"");
        assert!(matches!(e, Expr::Binary(op, _, _) if op == ".."));
    }

    #[test]
    fn prepares_continuations_and_comments() {
        let raw = vec![
            "var x = 1 ..".to_string(),
            "  'a'".to_string(),
            "# comment".to_string(),
            "var y = [1,".to_string(),
            "  2]".to_string(),
        ];
        let lines = prepare_lines(&raw, 1).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "var x = 1 .. 'a'");
        assert_eq!(lines[1].text, "var y = [1, 2]");
    }

    #[test]
    fn prepares_heredoc() {
        let raw = vec![
            "var lines =<< trim END".to_string(),
            "    one".to_string(),
            "    two".to_string(),
            "END".to_string(),
        ];
        let lines = prepare_lines(&raw, 1).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "var lines = ['one', 'two']");
    }

    #[test]
    fn prepares_heredoc_text_containing_another_heredoc() {
        let raw = vec![
            "def F()".to_string(),
            "  var lines =<< trim END".to_string(),
            "      var Func = (nr: number): int => {".to_string(),
            "          var ll =<< ENDIT".to_string(),
            "             nothing".to_string(),
            "  END".to_string(),
            "  echo len(lines)".to_string(),
            "enddef".to_string(),
        ];
        let lines = prepare_lines(&raw, 1).unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines[1].text.starts_with("  var lines = ["));
        let mut parser = StmtParser {
            lines: &lines,
            pos: 0,
            sid: 1,
            prefix: "",
        };
        let result = parser.parse_block(&[]);
        assert!(result.is_ok());
    }

    #[test]
    fn parses_statements_and_blocks() {
        let raw = vec![
            "var x = 1".to_string(),
            "if x > 0".to_string(),
            "  x += 1".to_string(),
            "else".to_string(),
            "  --x".to_string(),
            "endif".to_string(),
            "def Foo(a: number): number".to_string(),
            "  return a + 1".to_string(),
            "enddef".to_string(),
        ];
        let lines = prepare_lines(&raw, 1).unwrap();
        let mut parser = StmtParser {
            lines: &lines,
            pos: 0,
            sid: 0,
            prefix: "",
        };
        let (stmts, term) = parser.parse_block(&[]).unwrap();
        assert!(term.is_none());
        assert_eq!(stmts.len(), 3);
        assert!(matches!(stmts[0], Stmt::Var { .. }));
        assert!(matches!(stmts[1], Stmt::If { .. }));
        assert!(matches!(stmts[2], Stmt::Def(_)));
    }
}
