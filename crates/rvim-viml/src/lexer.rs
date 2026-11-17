//! Lexer for legacy VimL expressions (reference: eval.c `eval7()` etc.).

use rvim_core::{VimError, VimResult};

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Number(i64),
    Float(f64),
    Str(String),
    Blob(Vec<u8>),
    /// Identifier, possibly scope-qualified (`g:foo`, `s:Bar`, `a:1`, `foo#bar`).
    Ident(String),
    /// `&option` / `&l:option` / `&g:option`
    Option(String),
    /// `$ENV`
    Env(String),
    /// `@r`
    Register(char),
    /// Operator text, canonicalised: `+ - * / % . == ==# =~ !~? && || ! ? : -> << >> =`
    Op(String),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,
    Arrow,
    Eof,
}

impl Token {
    pub fn is_op(&self, op: &str) -> bool {
        matches!(self, Token::Op(o) if o == op)
    }
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

/// Scope prefixes that make `x:name` a scoped variable rather than a dict key.
pub const SCOPES: &[char] = &['g', 's', 'l', 'b', 'w', 't', 'v', 'a'];

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '#'
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
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

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c == ' ' || c == '\t' || c == '\n' || c == '\r') {
            self.pos += 1;
        }
    }

    pub fn tokenize(self) -> VimResult<Vec<Token>> {
        Ok(self
            .tokenize_spans()?
            .into_iter()
            .map(|(token, _, _)| token)
            .collect())
    }

    /// Tokenize and keep the `(start, end)` character offsets of every token.
    /// Used by `parse_expression_prefix()` to report how much text one
    /// expression consumed (needed by `:execute`, eval.c:6699-6757).
    pub fn tokenize_spans(mut self) -> VimResult<Vec<(Token, usize, usize)>> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else {
                out.push((Token::Eof, self.pos, self.pos));
                break;
            };
            let start = self.pos;
            let token = match c {
                '(' => {
                    self.bump();
                    Token::LParen
                }
                ')' => {
                    self.bump();
                    Token::RParen
                }
                '[' => {
                    self.bump();
                    Token::LBracket
                }
                ']' => {
                    self.bump();
                    Token::RBracket
                }
                '{' => {
                    self.bump();
                    Token::LBrace
                }
                '}' => {
                    self.bump();
                    Token::RBrace
                }
                ',' => {
                    self.bump();
                    Token::Comma
                }
                ':' => {
                    self.bump();
                    Token::Colon
                }
                ';' => {
                    self.bump();
                    Token::Semicolon
                }
                '\'' => Token::Str(self.lex_single_quoted()?),
                '"' => Token::Str(self.lex_double_quoted()?),
                '$' => {
                    self.bump();
                    let name = self.lex_ident_run();
                    Token::Env(name)
                }
                '@' => {
                    self.bump();
                    let reg = self.bump().ok_or_else(|| {
                        VimError::new("E354", "Invalid register name".to_string())
                    })?;
                    Token::Register(reg)
                }
                '&' => {
                    if self.peek_at(1) == Some('&') {
                        // "&&" is a logical operator, not an option reference.
                        self.lex_operator()?
                    } else {
                        self.bump();
                        let mut name = String::new();
                        // &l:opt / &g:opt prefixes.
                        if matches!(self.peek(), Some('l') | Some('g'))
                            && self.peek_at(1) == Some(':')
                        {
                            name.push(self.bump().unwrap());
                            name.push(':');
                            self.bump();
                        }
                        name.push_str(&self.lex_ident_run());
                        Token::Option(name)
                    }
                }
                '0' if self.peek_at(1) == Some('z') => {
                    self.bump();
                    self.bump();
                    let mut bytes = Vec::new();
                    while let Some(h) = self.peek() {
                        if !h.is_ascii_hexdigit() {
                            break;
                        }
                        self.bump();
                        let lo = self.peek();
                        let byte = match lo {
                            Some(l) if l.is_ascii_hexdigit() => {
                                self.bump();
                                u8::from_str_radix(&format!("{h}{l}"), 16).unwrap_or(0)
                            }
                            _ => u8::from_str_radix(&h.to_string(), 16).unwrap_or(0) * 16,
                        };
                        bytes.push(byte);
                    }
                    Token::Blob(bytes)
                }
                c if c.is_ascii_digit() => self.lex_number()?,
                c if is_ident_start(c) => {
                    let mut name = self.lex_ident_run();
                    // "is?" is a comparison operator like "is#"; '?' is not an
                    // identifier character, so it has to be attached here.
                    if (name == "is" || name == "isnot") && self.peek() == Some('?') {
                        name.push('?');
                        self.bump();
                    }
                    // Scope-qualified variable: single scope letter followed by ':'.
                    if self.peek() == Some(':')
                        && name.len() == 1
                        && SCOPES.contains(&name.chars().next().unwrap())
                    {
                        self.bump();
                        let rest = self.lex_ident_run();
                        Token::Ident(format!("{name}:{rest}"))
                    } else if self.peek() == Some(':') && name == "v" {
                        self.bump();
                        let rest = self.lex_ident_run();
                        Token::Ident(format!("v:{rest}"))
                    } else {
                        Token::Ident(name)
                    }
                }
                _ => self.lex_operator()?,
            };
            out.push((token, start, self.pos));
        }
        Ok(out)
    }

    fn lex_ident_run(&mut self) -> String {
        let mut name = String::new();
        if let Some(c) = self.peek().filter(|c| is_ident_start(*c)) {
            name.push(c);
            self.bump();
        }
        while let Some(c) = self.peek().filter(|c| is_ident_char(*c)) {
            name.push(c);
            self.bump();
        }
        name
    }

    fn lex_number(&mut self) -> VimResult<Token> {
        let start = self.pos;
        // 0x / 0b / 0o prefixed integers.
        if self.peek() == Some('0') {
            match self.peek_at(1) {
                Some('x') | Some('X') => {
                    self.pos += 2;
                    let digits = self.take_while(|c| c.is_ascii_hexdigit());
                    return Ok(Token::Number(clamp_radix(&digits, 16)));
                }
                Some('b') | Some('B') => {
                    self.pos += 2;
                    let digits = self.take_while(|c| c == '0' || c == '1');
                    return Ok(Token::Number(clamp_radix(&digits, 2)));
                }
                // 0o17: baseline 8.2 added STR2NR_OOCT (charset.c:1859-1863).
                Some('o') | Some('O') => {
                    self.pos += 2;
                    let digits = self.take_while(|c| ('0'..='7').contains(&c));
                    return Ok(Token::Number(clamp_radix(&digits, 8)));
                }
                _ => {}
            }
        }
        let int_part = self.take_while(|c| c.is_ascii_digit());
        // Float: digits '.' digits (a lone '.' is string concatenation).
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            let frac = self.take_while(|c| c.is_ascii_digit());
            let mut exp = String::new();
            if matches!(self.peek(), Some('e') | Some('E')) {
                let save = self.pos;
                self.bump();
                if matches!(self.peek(), Some('+') | Some('-')) {
                    exp.push(self.bump().unwrap());
                }
                let digits = self.take_while(|c| c.is_ascii_digit());
                if digits.is_empty() {
                    self.pos = save; // not an exponent
                } else {
                    exp.push_str(&digits);
                }
            }
            let text = if exp.is_empty() {
                format!("{int_part}.{frac}")
            } else {
                format!("{int_part}.{frac}e{exp}")
            };
            let value: f64 = text.parse().unwrap_or(0.0);
            return Ok(Token::Float(value));
        }
        let _ = start;
        // Legacy: a leading 0 with only octal digits is octal.
        if int_part.len() > 1
            && int_part.starts_with('0')
            && int_part.chars().all(|c| ('0'..='7').contains(&c))
        {
            return Ok(Token::Number(clamp_radix(&int_part[1..], 8)));
        }
        // Overflowing literals clamp to VARNUM_MAX instead of wrapping/zeroing
        // (typval.c:2048-2051 eval_number -> vim_str2nr clamps, charset.c:1993).
        Ok(Token::Number(clamp_radix(&int_part, 10)))
    }

    fn take_while(&mut self, pred: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek().filter(|c| pred(*c)) {
            out.push(c);
            self.bump();
        }
        out
    }

    fn lex_single_quoted(&mut self) -> VimResult<String> {
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.bump() {
                None => {
                    return Err(VimError::new("E115", "Missing single quote".to_string()));
                }
                Some('\'') => {
                    if self.peek() == Some('\'') {
                        self.bump();
                        out.push('\'');
                    } else {
                        break;
                    }
                }
                Some(c) => out.push(c),
            }
        }
        Ok(out)
    }

    fn lex_double_quoted(&mut self) -> VimResult<String> {
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.bump() {
                None => {
                    return Err(VimError::new("E114", "Missing double quote".to_string()));
                }
                Some('"') => break,
                Some('\\') => {
                    let Some(e) = self.bump() else {
                        return Err(VimError::new("E114", "Missing double quote".to_string()));
                    };
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'e' => out.push('\u{1b}'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        '\\' => out.push('\\'),
                        '"' => out.push('"'),
                        '0'..='7' => {
                            let mut digits = String::new();
                            digits.push(e);
                            for _ in 0..2 {
                                if let Some(d) = self.peek().filter(|c| ('0'..='7').contains(c)) {
                                    digits.push(d);
                                    self.bump();
                                } else {
                                    break;
                                }
                            }
                            if let Ok(v) = u8::from_str_radix(&digits, 8) {
                                out.push(v as char);
                            }
                        }
                        // typval.c:2136-2157: "\x"/"\X" take up to 2 hex digits,
                        // "\u" 4 and "\U" 8. Without a hex digit the backslash
                        // is dropped and the letter itself kept.
                        'x' | 'X' => {
                            let mut digits = String::new();
                            for _ in 0..2 {
                                if let Some(d) = self.peek().filter(|c| c.is_ascii_hexdigit()) {
                                    digits.push(d);
                                    self.bump();
                                }
                            }
                            if digits.is_empty() {
                                out.push(e);
                            } else if let Ok(v) = u8::from_str_radix(&digits, 16) {
                                out.push(v as char);
                            }
                        }
                        'u' | 'U' => {
                            let max = if e == 'u' { 4 } else { 8 };
                            let mut digits = String::new();
                            for _ in 0..max {
                                if let Some(d) = self.peek().filter(|c| c.is_ascii_hexdigit()) {
                                    digits.push(d);
                                    self.bump();
                                }
                            }
                            if digits.is_empty() {
                                out.push(e);
                            } else if let Ok(v) = u32::from_str_radix(&digits, 16) {
                                if let Some(c) = char::from_u32(v) {
                                    out.push(c);
                                }
                            }
                        }
                        '<' => {
                            // \<Esc> / \<Char-65> style key notation inside a
                            // string (typval.c:2171-2187 + keycodes.c
                            // find_special_key()).
                            let mut name = String::new();
                            while let Some(c) = self.peek() {
                                if c == '>' {
                                    self.bump();
                                    break;
                                }
                                name.push(c);
                                self.bump();
                            }
                            match key_notation_text(&name) {
                                Some(text) => out.push_str(&text),
                                // Unknown key name: trans_special() fails and the
                                // text is copied literally, without the '\'.
                                None => {
                                    out.push('<');
                                    out.push_str(&name);
                                    out.push('>');
                                }
                            }
                        }
                        // typval.c:2188-2190: a backslash before an unknown
                        // character is dropped ("\a" is "a", "\z" is "z").
                        other => out.push(other),
                    }
                }
                Some(c) => out.push(c),
            }
        }
        Ok(out)
    }

    fn lex_operator(&mut self) -> VimResult<Token> {
        // Longest-match operator table.
        const OPS: &[&str] = &[
            "==#", "==?", "!=#", "!=?", ">=#", ">=?", "<=#", "<=?", "=~#", "=~?", "!~#", "!~?",
            ">#", ">?", "<#", "<?", "is#", "is?", "isnot", "&&", "||", "->", "<<", ">>", "+=",
            "-=", ".=", "..=", "==", "!=", ">=", "<=", "=~", "!~", "is", "..", "+", "-", "*", "/",
            "%", ".", "!", ">", "<", "=", "~", "?",
        ];
        let rest: String = self.chars[self.pos..].iter().collect();
        for op in OPS {
            if rest.starts_with(op) {
                // Word operators must not be a prefix of an identifier.
                if op.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                    && rest
                        .strip_prefix(op)
                        .and_then(|tail| tail.chars().next())
                        .is_some_and(is_ident_char)
                {
                    continue;
                }
                self.pos += op.chars().count();
                if *op == "->" {
                    return Ok(Token::Arrow);
                }
                return Ok(Token::Op(op.to_string()));
            }
        }
        let bad = self.bump().unwrap();
        Err(VimError::new(
            "E15",
            format!("Invalid expression: unexpected '{bad}'"),
        ))
    }
}

/// Fold a digit string in `radix`, clamping at `VARNUM_MAX` like
/// `vim_str2nr()` does on overflow (charset.c:1993-2009).
fn clamp_radix(digits: &str, radix: u32) -> i64 {
    if digits.is_empty() {
        return 0;
    }
    let mut value: u128 = 0;
    let max = i64::MAX as u128;
    for c in digits.chars() {
        let Some(d) = c.to_digit(radix) else { break };
        value = value
            .saturating_mul(u128::from(radix))
            .saturating_add(u128::from(d));
        if value > max {
            return i64::MAX;
        }
    }
    value as i64
}

/// Decode `<...>` key notation for double-quoted string escapes; `None` when
/// the name is not a known special key (the caller then keeps the text).
/// Reference: keycodes.c `find_special_key()` / `trans_special()`, including
/// the `Char-` form (decimal, 0x hex, 0o octal, 0b binary).
fn key_notation_text(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let single = |c: char| Some(c.to_string());
    match lower.as_str() {
        "esc" => return single('\u{1b}'),
        "cr" | "enter" | "return" => return single('\r'),
        "nl" | "newline" => return single('\n'),
        "tab" => return single('\t'),
        "space" => return single(' '),
        "bs" | "backspace" => return single('\u{8}'),
        "del" => return single('\u{7f}'),
        "bar" => return single('|'),
        "lt" => return single('<'),
        "gt" => return single('>'),
        "nul" => return single('\0'),
        _ => {}
    }
    if let Some(rest) = lower.strip_prefix("char-") {
        let value = if let Some(hex) = rest.strip_prefix("0x") {
            u32::from_str_radix(hex, 16).ok()
        } else if let Some(oct) = rest.strip_prefix("0o") {
            u32::from_str_radix(oct, 8).ok()
        } else if let Some(bin) = rest.strip_prefix("0b") {
            u32::from_str_radix(bin, 2).ok()
        } else {
            rest.parse::<u32>().ok()
        };
        return value.and_then(char::from_u32).map(|c| c.to_string());
    }
    if let Some(rest) = lower.strip_prefix("c-") {
        // CTRL-X maps to the control character.
        let c = rest.chars().next()?.to_ascii_uppercase();
        return char::from_u32((c as u32) & 0x1f).map(|c| c.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Token> {
        Lexer::new(src).tokenize().unwrap()
    }

    #[test]
    fn numbers_strings_and_ops() {
        assert_eq!(
            toks("1 + 2.5"),
            vec![
                Token::Number(1),
                Token::Op("+".into()),
                Token::Float(2.5),
                Token::Eof
            ]
        );
        assert_eq!(toks("'a''b'"), vec![Token::Str("a'b".into()), Token::Eof]);
        assert_eq!(
            toks("\"a\\tb\\<Esc>\""),
            vec![Token::Str("a\tb\u{1b}".into()), Token::Eof]
        );
        assert_eq!(toks("0x1f"), vec![Token::Number(31), Token::Eof]);
        assert_eq!(toks("010"), vec![Token::Number(8), Token::Eof]);
        assert_eq!(
            toks("a ==# b"),
            vec![
                Token::Ident("a".into()),
                Token::Op("==#".into()),
                Token::Ident("b".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn scoped_idents_options_env_and_registers() {
        assert_eq!(
            toks("g:foo"),
            vec![Token::Ident("g:foo".into()), Token::Eof]
        );
        assert_eq!(
            toks("s:Helper()"),
            vec![
                Token::Ident("s:Helper".into()),
                Token::LParen,
                Token::RParen,
                Token::Eof
            ]
        );
        assert_eq!(
            toks("&compatible"),
            vec![Token::Option("compatible".into()), Token::Eof]
        );
        assert_eq!(
            toks("&l:shiftwidth"),
            vec![Token::Option("l:shiftwidth".into()), Token::Eof]
        );
        assert_eq!(toks("$HOME"), vec![Token::Env("HOME".into()), Token::Eof]);
        assert_eq!(toks("@a"), vec![Token::Register('a'), Token::Eof]);
        // A bare word with ':' that is not a scope stays an identifier.
        assert_eq!(
            toks("foo#bar"),
            vec![Token::Ident("foo#bar".into()), Token::Eof]
        );
    }

    #[test]
    fn method_arrow_and_lambda_tokens() {
        assert_eq!(
            toks("x->len()"),
            vec![
                Token::Ident("x".into()),
                Token::Arrow,
                Token::Ident("len".into()),
                Token::LParen,
                Token::RParen,
                Token::Eof
            ]
        );
        assert_eq!(
            toks("{a -> a + 1}"),
            vec![
                Token::LBrace,
                Token::Ident("a".into()),
                Token::Arrow,
                Token::Ident("a".into()),
                Token::Op("+".into()),
                Token::Number(1),
                Token::RBrace,
                Token::Eof
            ]
        );
        assert_eq!(
            toks("0z1f2A"),
            vec![Token::Blob(vec![0x1f, 0x2a]), Token::Eof]
        );
    }

    #[test]
    fn integer_literals_follow_vim_str2nr() {
        // 0o prefix is recognized by baseline 8.2 (STR2NR_OOCT,
        // charset.c:1859-1863).
        assert_eq!(toks("0o17"), vec![Token::Number(15), Token::Eof]);
        assert_eq!(toks("0O17"), vec![Token::Number(15), Token::Eof]);
        // Overflowing literals clamp to VARNUM_MAX (charset.c:1993-2009).
        assert_eq!(
            toks("100000000000000000000"),
            vec![Token::Number(i64::MAX), Token::Eof]
        );
        assert_eq!(
            toks("0xffffffffffffffff"),
            vec![Token::Number(i64::MAX), Token::Eof]
        );
        assert_eq!(
            toks("0b1111111111111111111111111111111111111111111111111111111111111111111"),
            vec![Token::Number(i64::MAX), Token::Eof]
        );
        // "08"/"0129" stay decimal (not octal).
        assert_eq!(toks("08"), vec![Token::Number(8), Token::Eof]);
        assert_eq!(toks("0129"), vec![Token::Number(129), Token::Eof]);
        // ".." is the 8.2 concatenation alias of "." (eval.c:3143-3144).
        assert_eq!(
            toks("'a' .. 'b'"),
            vec![
                Token::Str("a".into()),
                Token::Op("..".into()),
                Token::Str("b".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn double_quoted_escapes_match_typval() {
        // typval.c:2136-2210: a backslash before an unknown character is
        // dropped; "\x"/"\X" need at least one hex digit; "\<Char-N>" is a
        // character by code point.
        assert_eq!(toks("\"\\a\""), vec![Token::Str("a".into()), Token::Eof]);
        assert_eq!(toks("\"\\z\""), vec![Token::Str("z".into()), Token::Eof]);
        assert_eq!(toks("\"\\x\""), vec![Token::Str("x".into()), Token::Eof]);
        assert_eq!(toks("\"\\x41\""), vec![Token::Str("A".into()), Token::Eof]);
        assert_eq!(toks("\"\\X41\""), vec![Token::Str("A".into()), Token::Eof]);
        assert_eq!(
            toks("\"\\x4\""),
            vec![Token::Str("\u{4}".into()), Token::Eof]
        );
        assert_eq!(
            toks("\"\\<Char-65>\""),
            vec![Token::Str("A".into()), Token::Eof]
        );
        assert_eq!(
            toks("\"\\<Char-0x41>\""),
            vec![Token::Str("A".into()), Token::Eof]
        );
        assert_eq!(
            toks("\"\\<Char-0o101>\""),
            vec![Token::Str("A".into()), Token::Eof]
        );
        assert_eq!(
            toks("\"\\<Char-97>\""),
            vec![Token::Str("a".into()), Token::Eof]
        );
        // Unknown key name survives literally, minus the backslash.
        assert_eq!(
            toks("\"\\<foo>\""),
            vec![Token::Str("<foo>".into()), Token::Eof]
        );
    }
}
