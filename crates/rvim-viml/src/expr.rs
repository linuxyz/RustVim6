//! Expression AST and Pratt parser for legacy VimL.

use rvim_core::{VimError, VimResult};

use crate::lexer::{Lexer, Token};

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(i64),
    Float(f64),
    Str(String),
    Blob(Vec<u8>),
    Bool(bool),
    Null,
    /// Variable, possibly scope-qualified (`g:foo`, `s:Bar`, `a:1`).
    Var(String),
    Option(String),
    Env(String),
    Register(char),
    List(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),
    Unary(&'static str, Box<Expr>),
    /// Binary operator with its case-sensitivity suffix, e.g. `==#`, `=~?`.
    Binary(String, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>),
    /// `dict.key`
    Attr(Box<Expr>, String),
    Call(Box<Expr>, Vec<Expr>),
    /// `expr->Func(args)`
    Method(Box<Expr>, String, Vec<Expr>),
    Lambda {
        params: Vec<String>,
        varargs: bool,
        body: Box<Expr>,
    },
}

pub struct Parser {
    tokens: Vec<Token>,
    /// Character offsets of every token in the source (parallel to `tokens`).
    spans: Vec<(usize, usize)>,
    pos: usize,
}

impl Parser {
    pub fn new(src: &str) -> VimResult<Self> {
        let spanned = Lexer::new(src).tokenize_spans()?;
        let (tokens, spans) = spanned
            .into_iter()
            .map(|(token, start, end)| (token, (start, end)))
            .unzip();
        Ok(Self {
            tokens,
            spans,
            pos: 0,
        })
    }

    /// Character offset just past the tokens consumed so far (i.e. where the
    /// next token starts, skipping white space).
    fn consumed(&self) -> usize {
        self.spans
            .get(self.pos)
            .map(|(start, _)| *start)
            .unwrap_or(0)
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn peek_at(&self, off: usize) -> &Token {
        self.tokens.get(self.pos + off).unwrap_or(&Token::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == token {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: Token) -> VimResult<()> {
        if self.peek() == &token {
            self.bump();
            Ok(())
        } else {
            Err(VimError::new(
                "E15",
                format!(
                    "Invalid expression: expected {token:?}, found {:?}",
                    self.peek()
                ),
            ))
        }
    }

    /// Parse a whole expression; trailing tokens are an error (E488).
    pub fn parse_all(&mut self) -> VimResult<Expr> {
        let expr = self.parse_expr(0)?;
        if !matches!(self.peek(), Token::Eof) {
            return Err(VimError::new(
                "E488",
                format!("Trailing characters: {:?}", self.peek()),
            ));
        }
        Ok(expr)
    }

    pub fn parse_expr(&mut self, min_prec: u8) -> VimResult<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            // Postfix forms bind tightest.
            match self.peek() {
                Token::LParen => {
                    lhs = self.parse_call(lhs)?;
                    continue;
                }
                Token::LBracket => {
                    lhs = self.parse_index(lhs)?;
                    continue;
                }
                Token::Op(op) if op == "." => {
                    // Could be dict member access or string concat; member
                    // access requires an identifier right after the dot.
                    if let Token::Ident(name) = self.peek_at(1).clone() {
                        if min_prec <= 50 {
                            self.bump();
                            self.bump();
                            lhs = Expr::Attr(Box::new(lhs), name);
                            continue;
                        }
                    }
                }
                Token::Arrow => {
                    self.bump();
                    let Token::Ident(name) = self.bump() else {
                        return Err(VimError::new(
                            "E15",
                            "Invalid expression after ->".to_string(),
                        ));
                    };
                    let args = if self.peek() == &Token::LParen {
                        self.parse_arg_list()?
                    } else {
                        Vec::new()
                    };
                    lhs = Expr::Method(Box::new(lhs), name, args);
                    continue;
                }
                _ => {}
            }

            // Infix operators.
            let (prec, right_assoc, op_text) = match self.peek() {
                Token::Op(op) => {
                    let text = op.as_str();
                    if text == "?" {
                        (10u8, true, "?".to_string())
                    } else if text == "||" {
                        (20, false, text.to_string())
                    } else if text == "&&" {
                        (30, false, text.to_string())
                    } else if text == "+" || text == "-" || text == "." || text == ".." {
                        (50, false, text.to_string())
                    } else if text == "*" || text == "/" || text == "%" {
                        (60, false, text.to_string())
                    } else if is_comparison(text) {
                        (40, false, text.to_string())
                    } else {
                        // Assignment/compound operators are statement-level.
                        break;
                    }
                }
                // "is" / "isnot" (with optional #/?) arrive as identifiers
                // because they are spelled like keywords, not operators.
                Token::Ident(name) if is_comparison(name) => (40u8, false, name.clone()),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.bump();
            if op_text == "?" {
                let then_branch = self.parse_expr(0)?;
                self.expect(Token::Colon)?;
                let else_branch = self.parse_expr(10)?;
                lhs = Expr::Ternary(Box::new(lhs), Box::new(then_branch), Box::new(else_branch));
                continue;
            }
            let next_min = if right_assoc { prec } else { prec + 1 };
            let rhs = self.parse_expr(next_min)?;
            lhs = Expr::Binary(op_text, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> VimResult<Expr> {
        match self.peek().clone() {
            Token::Op(op) if op == "!" => {
                self.bump();
                Ok(Expr::Unary("!", Box::new(self.parse_expr(70)?)))
            }
            Token::Op(op) if op == "-" => {
                self.bump();
                Ok(Expr::Unary("-", Box::new(self.parse_expr(70)?)))
            }
            Token::Op(op) if op == "+" => {
                self.bump();
                Ok(Expr::Unary("+", Box::new(self.parse_expr(70)?)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> VimResult<Expr> {
        match self.bump() {
            Token::Number(n) => Ok(Expr::Number(n)),
            Token::Float(f) => Ok(Expr::Float(f)),
            Token::Str(s) => Ok(Expr::Str(s)),
            Token::Blob(b) => Ok(Expr::Blob(b)),
            Token::Option(name) => Ok(Expr::Option(name)),
            Token::Env(name) => Ok(Expr::Env(name)),
            Token::Register(r) => Ok(Expr::Register(r)),
            Token::LParen => {
                let inner = self.parse_expr(0)?;
                self.expect(Token::RParen)?;
                Ok(inner)
            }
            Token::LBracket => {
                let mut items = Vec::new();
                if self.eat(&Token::RBracket) {
                    return Ok(Expr::List(items));
                }
                loop {
                    items.push(self.parse_expr(0)?);
                    if self.eat(&Token::Comma) {
                        if self.peek() == &Token::RBracket {
                            break;
                        }
                        continue;
                    }
                    break;
                }
                self.expect(Token::RBracket)?;
                Ok(Expr::List(items))
            }
            Token::LBrace => self.parse_brace(),
            Token::HashBrace => self.parse_hash_dict(),
            Token::Ident(name) => {
                if name == "v:true" {
                    return Ok(Expr::Bool(true));
                }
                if name == "v:false" {
                    return Ok(Expr::Bool(false));
                }
                if name == "v:null" {
                    return Ok(Expr::Null);
                }
                Ok(Expr::Var(name))
            }
            other => Err(VimError::new(
                "E15",
                format!("Invalid expression: unexpected {other:?}"),
            )),
        }
    }

    /// '#{key: value, ...}': bare keys are literal strings.
    fn parse_hash_dict(&mut self) -> VimResult<Expr> {
        let mut entries = Vec::new();
        if self.eat(&Token::RBrace) {
            return Ok(Expr::Dict(entries));
        }
        loop {
            let key = match self.bump() {
                Token::Ident(name) => name,
                Token::Str(text) => text,
                Token::Number(n) => n.to_string(),
                other => {
                    return Err(VimError::new(
                        "E15",
                        format!("Invalid dictionary key: {other:?}"),
                    ));
                }
            };
            self.expect(Token::Colon)?;
            let value = self.parse_expr(0)?;
            entries.push((Expr::Str(key), value));
            if self.eat(&Token::Comma) {
                if self.peek() == &Token::RBrace {
                    break;
                }
                continue;
            }
            break;
        }
        self.expect(Token::RBrace)?;
        Ok(Expr::Dict(entries))
    }

    /// `{ ... }`: either a dict literal or a lambda.
    fn parse_brace(&mut self) -> VimResult<Expr> {
        // Zero-arg lambda: {-> expr}
        if self.peek() == &Token::Arrow {
            self.bump();
            let body = self.parse_expr(0)?;
            self.expect(Token::RBrace)?;
            return Ok(Expr::Lambda {
                params: Vec::new(),
                varargs: false,
                body: Box::new(body),
            });
        }
        // Lambda with parameters: {a, b -> expr} / {a, ... -> expr}
        if let Token::Ident(_) = self.peek() {
            let mut look = 1usize;
            let mut is_lambda = false;
            loop {
                match self.peek_at(look) {
                    Token::Comma => look += 1,
                    Token::Ident(_) => {
                        look += 1;
                        if self.peek_at(look) == &Token::Arrow {
                            is_lambda = true;
                            break;
                        }
                        if self.peek_at(look) == &Token::Comma {
                            continue;
                        }
                        break;
                    }
                    Token::Semicolon => {
                        // varargs marker: {a, ... ->}
                        look += 1;
                        if self.peek_at(look) == &Token::Op("...".into()) {
                            look += 1;
                        }
                        if self.peek_at(look) == &Token::Arrow {
                            is_lambda = true;
                        }
                        break;
                    }
                    Token::Op(op) if op == "..." => {
                        look += 1;
                        if self.peek_at(look) == &Token::Arrow {
                            is_lambda = true;
                        }
                        break;
                    }
                    Token::Arrow => {
                        is_lambda = true;
                        break;
                    }
                    _ => break,
                }
            }
            if is_lambda {
                return self.parse_lambda_params();
            }
        }
        // Dict literal.
        let mut entries = Vec::new();
        if self.eat(&Token::RBrace) {
            return Ok(Expr::Dict(entries));
        }
        loop {
            let key = self.parse_expr(0)?;
            let key = match key {
                Expr::Str(s) => Expr::Str(s),
                Expr::Number(n) => Expr::Str(n.to_string()),
                other => other,
            };
            self.expect(Token::Colon)?;
            let value = self.parse_expr(0)?;
            entries.push((key, value));
            if self.eat(&Token::Comma) {
                if self.peek() == &Token::RBrace {
                    break;
                }
                continue;
            }
            break;
        }
        self.expect(Token::RBrace)?;
        Ok(Expr::Dict(entries))
    }

    fn parse_lambda_params(&mut self) -> VimResult<Expr> {
        let mut params = Vec::new();
        let mut varargs = false;
        loop {
            match self.bump() {
                Token::Ident(name) => params.push(name),
                Token::Semicolon => {
                    // `{a, ... -> x}`: the semicolon marks the varargs list.
                    varargs = true;
                }
                Token::Op(op) if op == "..." => varargs = true,
                Token::Comma => continue,
                Token::Arrow => break,
                other => {
                    return Err(VimError::new(
                        "E15",
                        format!("Invalid lambda parameters: {other:?}"),
                    ));
                }
            }
            if self.eat(&Token::Comma) {
                continue;
            }
            self.expect(Token::Arrow)?;
            break;
        }
        let body = self.parse_expr(0)?;
        self.expect(Token::RBrace)?;
        Ok(Expr::Lambda {
            params,
            varargs,
            body: Box::new(body),
        })
    }

    fn parse_call(&mut self, callee: Expr) -> VimResult<Expr> {
        let args = self.parse_arg_list()?;
        Ok(Expr::Call(Box::new(callee), args))
    }

    fn parse_arg_list(&mut self) -> VimResult<Vec<Expr>> {
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        if self.eat(&Token::RParen) {
            return Ok(args);
        }
        loop {
            args.push(self.parse_expr(0)?);
            if self.eat(&Token::Comma) {
                continue;
            }
            break;
        }
        self.expect(Token::RParen)?;
        Ok(args)
    }

    fn parse_index(&mut self, target: Expr) -> VimResult<Expr> {
        self.expect(Token::LBracket)?;
        // Empty first index means a slice starting at the beginning.
        if self.peek() == &Token::Colon {
            self.bump();
            let end = if self.peek() == &Token::RBracket {
                None
            } else {
                Some(Box::new(self.parse_expr(0)?))
            };
            self.expect(Token::RBracket)?;
            return Ok(Expr::Slice(Box::new(target), None, end));
        }
        let first = self.parse_expr(0)?;
        if self.eat(&Token::Colon) {
            let end = if self.peek() == &Token::RBracket {
                None
            } else {
                Some(Box::new(self.parse_expr(0)?))
            };
            self.expect(Token::RBracket)?;
            return Ok(Expr::Slice(Box::new(target), Some(Box::new(first)), end));
        }
        self.expect(Token::RBracket)?;
        Ok(Expr::Index(Box::new(target), Box::new(first)))
    }
}

/// Comparison operators (with optional `#`/`?` case suffix).
fn is_comparison(op: &str) -> bool {
    const BASES: &[&str] = &["==", "!=", ">=", "<=", "=~", "!~", "isnot", "is"];
    BASES.iter().any(|base| {
        op.strip_prefix(base)
            .is_some_and(|suffix| suffix.is_empty() || suffix == "#" || suffix == "?")
    }) || op == ">"
        || op == "<"
}

/// Parse a complete expression string.
pub fn parse_expression(src: &str) -> VimResult<Expr> {
    Parser::new(src)?.parse_all()
}

/// Parse a bare expression that may be followed by trailing text; returns the
/// expression and the number of characters consumed (used by `:execute`, which
/// evaluates one expression at a time, eval.c:6699-6757).
pub fn parse_expression_prefix(src: &str) -> VimResult<(Expr, usize)> {
    let mut parser = Parser::new(src)?;
    let expr = parser.parse_expr(0)?;
    Ok((expr, parser.consumed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Expr {
        parse_expression(src).unwrap()
    }

    #[test]
    fn literals_and_containers() {
        assert_eq!(parse("42"), Expr::Number(42));
        assert_eq!(parse("'hi'"), Expr::Str("hi".into()));
        assert_eq!(parse("v:true"), Expr::Bool(true));
        assert_eq!(parse("v:null"), Expr::Null);
        assert_eq!(
            parse("[1, 'a', [2]]"),
            Expr::List(vec![
                Expr::Number(1),
                Expr::Str("a".into()),
                Expr::List(vec![Expr::Number(2)])
            ])
        );
        assert_eq!(
            parse("{'k': 1, 'j': 2}"),
            Expr::Dict(vec![
                (Expr::Str("k".into()), Expr::Number(1)),
                (Expr::Str("j".into()), Expr::Number(2))
            ])
        );
    }

    #[test]
    fn operator_precedence_matches_vim() {
        // 1 + 2 * 3
        assert_eq!(
            parse("1 + 2 * 3"),
            Expr::Binary(
                "+".into(),
                Box::new(Expr::Number(1)),
                Box::new(Expr::Binary(
                    "*".into(),
                    Box::new(Expr::Number(2)),
                    Box::new(Expr::Number(3))
                ))
            )
        );
        // || is looser than &&
        assert_eq!(
            parse("a && b || c"),
            Expr::Binary(
                "||".into(),
                Box::new(Expr::Binary(
                    "&&".into(),
                    Box::new(Expr::Var("a".into())),
                    Box::new(Expr::Var("b".into()))
                )),
                Box::new(Expr::Var("c".into()))
            )
        );
        // ternary is loosest
        match parse("a ? b : c") {
            Expr::Ternary(..) => {}
            other => panic!("expected ternary, got {other:?}"),
        }
    }

    #[test]
    fn calls_index_slice_attr_and_methods() {
        assert_eq!(
            parse("len(x)"),
            Expr::Call(
                Box::new(Expr::Var("len".into())),
                vec![Expr::Var("x".into())]
            )
        );
        assert_eq!(
            parse("l[1]"),
            Expr::Index(Box::new(Expr::Var("l".into())), Box::new(Expr::Number(1)))
        );
        assert_eq!(
            parse("l[1:3]"),
            Expr::Slice(
                Box::new(Expr::Var("l".into())),
                Some(Box::new(Expr::Number(1))),
                Some(Box::new(Expr::Number(3)))
            )
        );
        assert_eq!(
            parse("l[:2]"),
            Expr::Slice(
                Box::new(Expr::Var("l".into())),
                None,
                Some(Box::new(Expr::Number(2)))
            )
        );
        assert_eq!(
            parse("d.key"),
            Expr::Attr(Box::new(Expr::Var("d".into())), "key".into())
        );
        assert_eq!(
            parse("x->trim()"),
            Expr::Method(Box::new(Expr::Var("x".into())), "trim".into(), Vec::new())
        );
    }

    #[test]
    fn option_env_register_and_scoped_vars() {
        assert_eq!(parse("&compatible"), Expr::Option("compatible".into()));
        assert_eq!(parse("$HOME"), Expr::Env("HOME".into()));
        assert_eq!(parse("@a"), Expr::Register('a'));
        assert_eq!(parse("g:foo"), Expr::Var("g:foo".into()));
        assert_eq!(parse("a:1"), Expr::Var("a:1".into()));
        assert_eq!(
            parse("s:Helper()"),
            Expr::Call(Box::new(Expr::Var("s:Helper".into())), vec![])
        );
    }

    #[test]
    fn lambdas_parse() {
        match parse("{x -> x + 1}") {
            Expr::Lambda { params, body, .. } => {
                assert_eq!(params, vec!["x".to_string()]);
                assert!(matches!(*body, Expr::Binary(..)));
            }
            other => panic!("expected lambda, got {other:?}"),
        }
        match parse("{-> 42}") {
            Expr::Lambda { params, body, .. } => {
                assert!(params.is_empty());
                assert_eq!(*body, Expr::Number(42));
            }
            other => panic!("expected zero-arg lambda, got {other:?}"),
        }
    }

    #[test]
    fn regex_operators_carry_case_flags() {
        match parse("x =~? 'foo'") {
            Expr::Binary(op, ..) => assert_eq!(op, "=~?"),
            other => panic!("got {other:?}"),
        }
        match parse("x !~# 'foo'") {
            Expr::Binary(op, ..) => assert_eq!(op, "!~#"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn invalid_expressions_are_errors() {
        assert!(parse_expression("1 +").is_err());
        assert!(parse_expression("(1").is_err());
        assert!(parse_expression("'unterminated").is_err());
    }
}
