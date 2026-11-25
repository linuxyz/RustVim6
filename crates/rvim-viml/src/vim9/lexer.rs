//! Lexer for Vim9 script expressions and statements (reference: vim9compile.c
//! and vim9expr.c, which parse directly; we keep a separate token stream).

use rvim_core::{VimError, VimResult};

/// One lexical token. Literal payloads are already decoded.
#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Number(i64),
    Float(f64),
    Str(String),
    /// Raw body of a '$' interpolated string (escapes still present).
    Interp(String),
    Blob(Vec<u8>),
    Bool(bool),
    Null,
    Option(String),
    Env(String),
    Register(char),
    Op(String),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    /// '#' immediately followed by '{' (dict literal).
    HashBrace,
    Comma,
    Colon,
    Semicolon,
    Arrow,
    Eof,
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(name) => format!("identifier '{name}'"),
            Tok::Number(n) => format!("number {n}"),
            Tok::Float(f) => format!("float {f}"),
            Tok::Str(_) => "string".to_string(),
            Tok::Interp(_) => "interpolated string".to_string(),
            Tok::Blob(_) => "blob".to_string(),
            Tok::Bool(b) => format!("bool {b}"),
            Tok::Null => "null".to_string(),
            Tok::Option(name) => format!("option '&{name}'"),
            Tok::Env(_) => "environment variable".to_string(),
            Tok::Register(r) => format!("register '@{r}'"),
            Tok::Op(op) => format!("operator '{op}'"),
            Tok::LParen => "'('".to_string(),
            Tok::RParen => "')'".to_string(),
            Tok::LBracket => "'['".to_string(),
            Tok::RBracket => "']'".to_string(),
            Tok::LBrace => "'{'".to_string(),
            Tok::RBrace => "'}'".to_string(),
            Tok::HashBrace => "'#{'".to_string(),
            Tok::Comma => "','".to_string(),
            Tok::Colon => "':'".to_string(),
            Tok::Semicolon => "';'".to_string(),
            Tok::Arrow => "'=>'".to_string(),
            Tok::Eof => "end of line".to_string(),
        }
    }
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
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
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
    }

    /// Tokenize the whole line. A '#' that does not start '#{' begins a
    /// comment: the rest of the line is dropped.
    pub fn tokenize(mut self) -> VimResult<Vec<(Tok, usize)>> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            let start = self.pos;
            let Some(c) = self.peek() else {
                out.push((Tok::Eof, start));
                return Ok(out);
            };
            let tok = match c {
                '#' => {
                    if self.peek_at(1) == Some('{') {
                        self.pos += 2;
                        Tok::HashBrace
                    } else {
                        out.push((Tok::Eof, start));
                        return Ok(out);
                    }
                }
                '(' => {
                    self.pos += 1;
                    Tok::LParen
                }
                ')' => {
                    self.pos += 1;
                    Tok::RParen
                }
                '[' => {
                    self.pos += 1;
                    Tok::LBracket
                }
                ']' => {
                    self.pos += 1;
                    Tok::RBracket
                }
                '{' => {
                    self.pos += 1;
                    Tok::LBrace
                }
                '}' => {
                    self.pos += 1;
                    Tok::RBrace
                }
                ',' => {
                    self.pos += 1;
                    Tok::Comma
                }
                ':' => {
                    self.pos += 1;
                    Tok::Colon
                }
                ';' => {
                    self.pos += 1;
                    Tok::Semicolon
                }
                '\'' | '"' => Tok::Str(self.scan_string(c)?),
                '$' => {
                    if self.peek_at(1) == Some('"') {
                        self.pos += 2;
                        Tok::Interp(self.scan_interp_body()?)
                    } else if self.peek_at(1) == Some('\'') {
                        self.pos += 2;
                        Tok::Interp(self.scan_interp_single()?)
                    } else {
                        self.pos += 1;
                        let name = self.scan_name();
                        if name.is_empty() {
                            return Err(self.error("E15", "Missing environment variable name"));
                        }
                        Tok::Env(name)
                    }
                }
                '&' if self.peek_at(1) != Some('&') => {
                    self.pos += 1;
                    let name = self.scan_name();
                    if name.is_empty() {
                        return Err(self.error("E15", "Missing option name after &"));
                    }
                    Tok::Option(name)
                }
                '@' => {
                    self.pos += 1;
                    let Some(name) = self.bump() else {
                        return Err(self.error("E15", "Missing register name after @"));
                    };
                    Tok::Register(name)
                }
                c if c.is_ascii_digit() => self.scan_number()?,
                c if is_ident_start(c) => self.scan_ident(),
                _ => {
                    if let Some(op) = self.scan_operator() {
                        if op == "=>" {
                            Tok::Arrow
                        } else {
                            Tok::Op(op.to_string())
                        }
                    } else {
                        return Err(
                            self.error("E15", &format!("Unexpected character {c:?} in expression"))
                        );
                    }
                }
            };
            out.push((tok, start));
        }
    }

    fn error(&self, code: &'static str, message: &str) -> VimError {
        VimError::new(code, format!("{message} (at column {})", self.pos + 1))
    }

    fn scan_name(&mut self) -> String {
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                name.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        name
    }

    fn scan_ident(&mut self) -> Tok {
        let mut name = self.scan_name();
        // Scope prefix: g:, b:, w:, t:, s:, v:, a:, l:.
        if matches!(name.as_str(), "g" | "b" | "w" | "t" | "s" | "v" | "a" | "l")
            && self.peek() == Some(':')
            && self
                .peek_at(1)
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            name.push(':');
            self.pos += 1;
            name.push_str(&self.scan_name());
        }
        // Autoload name: dist#ft#Func.
        loop {
            if self.peek() == Some('#')
                && self
                    .peek_at(1)
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                name.push('#');
                self.pos += 1;
                name.push_str(&self.scan_name());
            } else {
                break;
            }
        }
        match name.as_str() {
            "true" | "v:true" => Tok::Bool(true),
            "false" | "v:false" => Tok::Bool(false),
            "null" | "v:null" => Tok::Null,
            _ => Tok::Ident(name),
        }
    }

    fn scan_number(&mut self) -> VimResult<Tok> {
        let start = self.pos;
        if self.peek() == Some('0') && matches!(self.peek_at(1), Some('z') | Some('Z')) {
            self.pos += 2;
            let mut bytes = Vec::new();
            while let (Some(a), Some(b)) = (self.peek(), self.peek_at(1)) {
                let Some(high) = a.to_digit(16) else { break };
                let Some(low) = b.to_digit(16) else { break };
                bytes.push((high * 16 + low) as u8);
                self.pos += 2;
            }
            return Ok(Tok::Blob(bytes));
        }
        for (prefix, radix) in [
            ("0x", 16u32),
            ("0X", 16),
            ("0b", 2),
            ("0B", 2),
            ("0o", 8),
            ("0O", 8),
        ] {
            if self.chars[self.pos..].starts_with(&prefix.chars().collect::<Vec<_>>()) {
                self.pos += 2;
                let digits_start = self.pos;
                while self.peek().is_some_and(|c| c.is_digit(radix)) {
                    self.pos += 1;
                }
                if self.pos == digits_start {
                    return Err(self.error("E15", "Missing digits in number"));
                }
                let text: String = self.chars[digits_start..self.pos].iter().collect();
                let value = i64::from_str_radix(&text, radix)
                    .map_err(|_| self.error("E15", "Number too large"))?;
                return Ok(Tok::Number(value));
            }
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            let mut look = 1;
            if matches!(self.peek_at(1), Some('+') | Some('-')) {
                look = 2;
            }
            if self.peek_at(look).is_some_and(|c| c.is_ascii_digit()) {
                is_float = true;
                self.pos += look;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            let value = text
                .parse::<f64>()
                .map_err(|_| self.error("E15", "Invalid float"))?;
            Ok(Tok::Float(value))
        } else {
            let value = text
                .parse::<i64>()
                .map_err(|_| self.error("E15", "Number too large"))?;
            Ok(Tok::Number(value))
        }
    }

    fn scan_string(&mut self, quote: char) -> VimResult<String> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(self.error("E115", "Missing closing quote"));
            };
            if c == quote {
                if quote == '\'' && self.peek() == Some('\'') {
                    out.push('\'');
                    self.pos += 1;
                    continue;
                }
                return Ok(out);
            }
            if quote == '"' && c == '\\' {
                out.push_str(&self.scan_escape()?);
            } else {
                out.push(c);
            }
        }
    }

    /// Raw body of a single-quoted interpolated string; '' is a literal quote.
    fn scan_interp_single(&mut self) -> VimResult<String> {
        let mut out = String::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(self.error("E115", "Missing closing quote"));
            };
            if c == '\'' {
                if self.peek() == Some('\'') {
                    out.push('\'');
                    self.pos += 1;
                    continue;
                }
                return Ok(out);
            }
            out.push(c);
        }
    }

    /// Raw body of a double-quoted interpolated string; escapes are kept for
    /// the part splitter because '{expr}' segments must be parsed separately.
    fn scan_interp_body(&mut self) -> VimResult<String> {
        let mut out = String::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(self.error("E115", "Missing closing quote"));
            };
            if c == '"' {
                return Ok(out);
            }
            if c == '\\' {
                out.push('\\');
                if let Some(next) = self.bump() {
                    out.push(next);
                }
            } else {
                out.push(c);
            }
        }
    }

    /// Decode one backslash escape (the backslash is already consumed).
    pub fn scan_escape(&mut self) -> VimResult<String> {
        let Some(c) = self.bump() else {
            return Err(self.error("E15", "Trailing backslash in string"));
        };
        let out = match c {
            'n' => "\n".to_string(),
            't' => "\t".to_string(),
            'r' => "\r".to_string(),
            'e' => "\u{1b}".to_string(),
            'b' => "\u{8}".to_string(),
            'f' => "\u{c}".to_string(),
            '\\' => "\\".to_string(),
            '"' => "\"".to_string(),
            '\'' => "'".to_string(),
            'x' | 'X' => {
                let mut value = 0u32;
                let mut digits = 0;
                while digits < 2 {
                    let Some(d) = self.peek().and_then(|c| c.to_digit(16)) else {
                        break;
                    };
                    value = value * 16 + d;
                    self.pos += 1;
                    digits += 1;
                }
                if digits == 0 {
                    return Err(self.error("E15", "Invalid \\x escape"));
                }
                char::from_u32(value).unwrap_or('\u{fffd}').to_string()
            }
            'u' | 'U' => {
                let max = if c == 'u' { 4 } else { 8 };
                let mut value = 0u32;
                let mut digits = 0;
                while digits < max {
                    let Some(d) = self.peek().and_then(|c| c.to_digit(16)) else {
                        break;
                    };
                    value = value * 16 + d;
                    self.pos += 1;
                    digits += 1;
                }
                if digits == 0 {
                    return Err(self.error("E15", "Invalid unicode escape"));
                }
                char::from_u32(value).unwrap_or('\u{fffd}').to_string()
            }
            '<' => self.scan_key_escape()?,
            other => other.to_string(),
        };
        Ok(out)
    }

    fn scan_key_escape(&mut self) -> VimResult<String> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == '>' {
                break;
            }
            self.pos += 1;
        }
        let Some(name) = self
            .chars
            .get(start..self.pos)
            .map(|c| c.iter().collect::<String>())
        else {
            return Err(self.error("E15", "Missing > in key escape"));
        };
        if self.peek() == Some('>') {
            self.pos += 1;
        } else {
            return Err(self.error("E15", "Missing > in key escape"));
        }
        let lower = name.to_ascii_lowercase();
        let out = match lower.as_str() {
            "esc" => "\u{1b}".to_string(),
            "cr" | "enter" | "return" => "\r".to_string(),
            "nl" => "\n".to_string(),
            "tab" => "\t".to_string(),
            "bs" | "backspace" => "\u{8}".to_string(),
            "space" => " ".to_string(),
            "nul" | "nil" => "\0".to_string(),
            _ => {
                if let Some(rest) = lower.strip_prefix("c-") {
                    let ch = rest.chars().next().unwrap_or('@');
                    let code = (ch as u8) & 0x1f;
                    char::from_u32(u32::from(code))
                        .unwrap_or('\u{fffd}')
                        .to_string()
                } else {
                    format!("<{name}>")
                }
            }
        };
        Ok(out)
    }

    fn scan_operator(&mut self) -> Option<&'static str> {
        const OPS: &[&str] = &[
            "..=", "...", "->", "==#", "==?", "!=#", "!=?", ">=#", ">=?", "<=#", "<=?", "=~#",
            "=~?", "!~#", "!~?", "==", "!=", ">=", "<=", "=~", "!~", "&&", "||", "<<", ">>", "++",
            "--", "+=", "-=", "*=", "/=", "%=", "..", "=>", "??", "?", ":", "=", "+", "-", "*",
            "/", "%", "!", "<", ">", ".",
        ];
        for op in OPS {
            let chars: Vec<char> = op.chars().collect();
            if self.chars[self.pos..].starts_with(&chars) {
                self.pos += chars.len();
                return Some(op);
            }
        }
        None
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

pub fn tokenize(src: &str) -> VimResult<Vec<(Tok, usize)>> {
    Lexer::new(src).tokenize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(src)
            .unwrap()
            .into_iter()
            .map(|(tok, _)| tok)
            .collect()
    }

    #[test]
    fn literals_and_names() {
        assert_eq!(toks("42"), vec![Tok::Number(42), Tok::Eof]);
        assert_eq!(toks("true"), vec![Tok::Bool(true), Tok::Eof]);
        assert_eq!(toks("v:null"), vec![Tok::Null, Tok::Eof]);
        assert_eq!(
            toks("g:count"),
            vec![Tok::Ident("g:count".into()), Tok::Eof]
        );
        assert_eq!(
            toks("dist#ft#FTasm"),
            vec![Tok::Ident("dist#ft#FTasm".into()), Tok::Eof]
        );
        assert_eq!(toks("0zDEAD"), vec![Tok::Blob(vec![0xde, 0xad]), Tok::Eof]);
    }

    #[test]
    fn operators_are_longest_match() {
        assert_eq!(
            toks("a ..= b"),
            vec![
                Tok::Ident("a".into()),
                Tok::Op("..=".into()),
                Tok::Ident("b".into()),
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("x->f()"),
            vec![
                Tok::Ident("x".into()),
                Tok::Op("->".into()),
                Tok::Ident("f".into()),
                Tok::LParen,
                Tok::RParen,
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("(x) => x"),
            vec![
                Tok::LParen,
                Tok::Ident("x".into()),
                Tok::RParen,
                Tok::Arrow,
                Tok::Ident("x".into()),
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("a ==# b"),
            vec![
                Tok::Ident("a".into()),
                Tok::Op("==#".into()),
                Tok::Ident("b".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn comments_stop_tokenizing() {
        assert_eq!(
            toks("1 + 2  # comment 'x'"),
            vec![
                Tok::Number(1),
                Tok::Op("+".into()),
                Tok::Number(2),
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("#{a: 1}"),
            vec![
                Tok::HashBrace,
                Tok::Ident("a".into()),
                Tok::Colon,
                Tok::Number(1),
                Tok::RBrace,
                Tok::Eof
            ]
        );
    }

    #[test]
    fn strings_decode_escapes() {
        assert_eq!(toks(r#"'a''b'"#), vec![Tok::Str("a'b".into()), Tok::Eof]);
        assert_eq!(toks(r#""a\nb""#), vec![Tok::Str("a\nb".into()), Tok::Eof]);
        assert_eq!(
            toks("\"\\<Esc>\""),
            vec![Tok::Str("\u{1b}".into()), Tok::Eof]
        );
    }

    #[test]
    fn options_and_interpolation() {
        assert_eq!(toks("&shell"), vec![Tok::Option("shell".into()), Tok::Eof]);
        assert_eq!(
            toks("$\"a {b}\""),
            vec![Tok::Interp("a {b}".into()), Tok::Eof]
        );
        assert_eq!(
            toks("$'a {b}'"),
            vec![Tok::Interp("a {b}".into()), Tok::Eof]
        );
    }
}
