//! Legacy VimL script execution: statements, control flow, functions.
//!
//! In Vim, control flow *is* a set of Ex commands (`:if`, `:while`, `:for`,
//! `:try`, `:function`) handled by `do_cmdline()` with a command stack. This
//! module mirrors that: it parses a sourced script into a small statement tree
//! (control-flow keywords become nodes, everything else stays an Ex line) and
//! executes it, while [`crate::exec`] keeps handling individual commands.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_viml::Env;
use rvim_viml::env::Host;
use rvim_viml::expr::parse_expression;
use rvim_viml::value::Value;

use crate::exec::ExOutcome;
use crate::parser::parse_line;
use crate::table::lookup;

pub use rvim_viml::eval::call_function;

/// A user-defined function (`:function`).
#[derive(Clone, Debug)]
pub struct UserFunc {
    pub name: String,
    pub params: Vec<String>,
    /// Default expression for each parameter (parallel to 'params').
    pub defaults: Vec<Option<String>>,
    pub varargs: bool,
    pub body: Vec<String>,
    pub stmts: Vec<Stmt>,
    pub sid: usize,
    /// `dict` attribute: the function gets a `self` dictionary.
    pub dict: Option<String>,
    /// `abort` attribute: stop at the first error and return -1
    /// (eval.txt:2726-2728 *:func-abort*).
    pub abort: bool,
}

/// Script-level state kept across sourced files.
#[derive(Debug, Default)]
pub struct ScriptState {
    pub(crate) colors_loading: bool,
    pub funcs: HashMap<String, UserFunc>,
    /// Buffer-local variables keyed by buffer number.
    pub bufvars: HashMap<i32, HashMap<String, Value>>,
    pub winvars: HashMap<String, Value>,
    pub tabvars: HashMap<String, Value>,
    /// Stack of currently sourced files (for `<sfile>`).
    pub script_files: Vec<PathBuf>,
    /// Autoload scripts already sourced.
    pub autoload: HashSet<String>,
    /// Current autocommand `<amatch>` value (empty when not in an event).
    pub amatch: String,
    /// Single-window/tab state (`winsaveview`, jump list, tag stack).
    pub win: crate::host_win::WinState,
    /// Metadata state (popups, text properties, signs, matches, history).
    pub meta: crate::host_meta::MetaState,
    /// Folds, highlight groups, command-line state.
    pub more: crate::host_more::MoreState,
    /// Jobs, channels, timers and terminal buffers (M7).
    pub m7: crate::m7::M7State,
    /// Small states used by the remaining builtins (M3 extras).
    pub extra: crate::host_extra::ExtraState,
}

/// Evaluation context: variables plus script state.
#[derive(Debug, Default)]
pub struct Ctx {
    pub env: Env,
    pub state: ScriptState,
}

impl Ctx {
    pub fn new() -> Self {
        Self {
            env: Env::new(),
            state: ScriptState::default(),
        }
    }
}

/// Control-flow outcome inside a statement list.
#[derive(Debug)]
pub enum Flow {
    Next,
    Break,
    Continue,
    Return(Value),
    Throw(Value),
    Quit(ExOutcome),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// An ordinary Ex command line (may contain `|`).
    Line(String),
    If {
        branches: Vec<(String, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
    },
    While {
        cond: String,
        body: Vec<Stmt>,
    },
    For {
        target: String,
        expr: String,
        body: Vec<Stmt>,
    },
    Try {
        body: Vec<Stmt>,
        catches: Vec<CatchClause>,
        finally: Vec<Stmt>,
    },
    FuncDef(Box<UserFunc>),
    /// A ':def' block compiled by the Vim9 engine (M9).
    Vim9FuncDef(Box<rvim_viml::vim9::Vim9Func>),
    Return(Option<String>),
    Throw(String),
    Break,
    Continue,
}

#[derive(Debug, Clone)]
pub struct CatchClause {
    /// `:catch /pat/` or `None` for a bare `:catch` (catches everything).
    pub pattern: Option<String>,
    pub body: Vec<Stmt>,
}

/// Structural keywords recognised while building the statement tree.
fn structural_command(line: &str) -> Option<String> {
    let trimmed = strip_modifiers(line);
    let parsed = parse_line(trimmed, 1).ok()?;
    let canonical = lookup(&parsed.command)?.name;
    // 'enddef: value' is a dictionary key, not a command. The same guard keeps
    // 'var: 1' and similar expression lines out of the statement parser.
    if trimmed[canonical.len().min(trimmed.len())..]
        .trim_start()
        .starts_with(':')
    {
        return None;
    }
    Some(canonical.to_string())
}

/// Drop `:silent[!]`, `:verbose N`, `:noautocmd` and leading `:` prefixes so the
/// structural keyword can be recognised.
fn strip_modifiers(line: &str) -> &str {
    let mut rest = line.trim_start();
    if let Some(after) = rest.strip_prefix(':') {
        rest = after.trim_start();
    }
    loop {
        let mut advanced = false;
        for prefix in ["silent!", "silent", "noautocmd", "verbose"] {
            let Some(head) = rest.get(..prefix.len()) else {
                continue;
            };
            if head.eq_ignore_ascii_case(prefix) {
                let after = &rest[prefix.len()..];
                if after.starts_with(' ') || after.starts_with('\t') {
                    rest = after.trim_start();
                    if prefix == "verbose" {
                        rest = rest
                            .split_once(' ')
                            .map(|(_, r)| r.trim_start())
                            .unwrap_or("");
                    }
                    advanced = true;
                    break;
                }
            }
        }
        if !advanced {
            return rest;
        }
    }
}
struct BlockParser<'a> {
    lines: &'a [String],
    pos: usize,
    sid: usize,
}

impl<'a> BlockParser<'a> {
    fn parse_block(&mut self, stop: &[&str]) -> VimResult<(Vec<Stmt>, Option<String>)> {
        let mut out = Vec::new();
        while self.pos < self.lines.len() {
            let raw = self.lines[self.pos].clone();
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('"') {
                self.pos += 1;
                continue;
            }
            let Some(keyword) = structural_command(&raw) else {
                out.push(Stmt::Line(raw));
                self.pos += 1;
                continue;
            };
            if stop.contains(&keyword.as_str()) {
                return Ok((out, Some(keyword)));
            }
            self.pos += 1;
            match keyword.as_str() {
                "if" => out.push(self.parse_if()?),
                "while" => out.push(self.parse_while()?),
                "for" => out.push(self.parse_for()?),
                "try" => out.push(self.parse_try()?),
                "function" => out.push(self.parse_function(&raw)?),
                "def" => out.push(self.parse_vim9_def(&raw)?),
                "return" => out.push(Stmt::Return(nonempty_args(&raw))),
                "throw" => {
                    out.push(Stmt::Throw(args_of(&raw)));
                }
                "break" => out.push(Stmt::Break),
                "continue" => out.push(Stmt::Continue),
                "else" | "elseif" => {
                    // A stray branch keyword outside :if.
                    return Err(VimError::new("E580", ":else without :if".to_string()));
                }
                "endif" | "endwhile" | "endfor" | "endtry" | "endfunction" => {
                    return Err(VimError::new(
                        "E580",
                        format!(
                            ":{keyword} without matching opener at line {}: {}",
                            self.pos + 1,
                            raw.trim()
                        ),
                    ));
                }
                _ => out.push(Stmt::Line(raw)),
            }
        }
        Ok((out, None))
    }

    fn parse_if(&mut self) -> VimResult<Stmt> {
        let mut branches = Vec::new();
        let mut cond = args_of(&self.lines[self.pos - 1]);
        loop {
            let (body, terminator) = self.parse_block(&["elseif", "else", "endif"])?;
            branches.push((cond.clone(), body));
            match terminator.as_deref() {
                Some("elseif") => {
                    cond = args_of(&self.lines[self.pos]);
                    self.pos += 1;
                    continue;
                }
                Some("else") => {
                    self.pos += 1;
                    let (else_body, _) = self.parse_block(&["endif"])?;
                    if self.pos >= self.lines.len() {
                        return Err(VimError::new("E171", "Missing :endif".to_string()));
                    }
                    self.pos += 1; // :endif
                    return Ok(Stmt::If {
                        branches,
                        else_body: Some(else_body),
                    });
                }
                Some("endif") => {
                    self.pos += 1;
                    return Ok(Stmt::If {
                        branches,
                        else_body: None,
                    });
                }
                _ => return Err(VimError::new("E171", "Missing :endif".to_string())),
            }
        }
    }

    fn parse_while(&mut self) -> VimResult<Stmt> {
        let cond = args_of(&self.lines[self.pos - 1]);
        let (body, terminator) = self.parse_block(&["endwhile"])?;
        if terminator.as_deref() != Some("endwhile") {
            return Err(VimError::new("E170", "Missing :endwhile".to_string()));
        }
        self.pos += 1;
        Ok(Stmt::While { cond, body })
    }

    fn parse_for(&mut self) -> VimResult<Stmt> {
        let spec = args_of(&self.lines[self.pos - 1]);
        let Some((target, expr)) = split_for_spec(&spec) else {
            return Err(VimError::new("E690", "Missing :in".to_string()));
        };
        let (body, terminator) = self.parse_block(&["endfor"])?;
        if terminator.as_deref() != Some("endfor") {
            return Err(VimError::new("E170", "Missing :endfor".to_string()));
        }
        self.pos += 1;
        Ok(Stmt::For { target, expr, body })
    }

    fn parse_try(&mut self) -> VimResult<Stmt> {
        let (body, terminator) = self.parse_block(&["catch", "finally", "endtry"])?;
        let mut catches = Vec::new();
        let mut finally = Vec::new();
        let mut term = terminator;
        loop {
            match term.as_deref() {
                Some("catch") => {
                    let spec = args_of(&self.lines[self.pos]);
                    self.pos += 1;
                    let pattern = parse_catch_pattern(&spec);
                    let (catch_body, next) = self.parse_block(&["catch", "finally", "endtry"])?;
                    catches.push(CatchClause {
                        pattern,
                        body: catch_body,
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
                    });
                }
                _ => return Err(VimError::new("E600", "Missing :endtry".to_string())),
            }
        }
    }

    fn parse_function(&mut self, header: &str) -> VimResult<Stmt> {
        let spec = args_of(header);
        // Collect body lines until :endfunction (nested :function not allowed).
        let start_line = self.pos + 1;
        let mut body = Vec::new();
        let mut found_end = false;
        while self.pos < self.lines.len() {
            let line = &self.lines[self.pos];
            self.pos += 1;
            if structural_command(line).as_deref() == Some("endfunction") {
                found_end = true;
                break;
            }
            let marker = rvim_viml::vim9::parser::heredoc_marker(line);
            body.push(line.clone());
            if let Some(marker) = marker {
                // Heredoc text is data: it may contain 'endfunction'.
                while self.pos < self.lines.len() {
                    let next = self.lines[self.pos].clone();
                    let done = next.trim() == marker;
                    self.pos += 1;
                    body.push(next);
                    if done {
                        break;
                    }
                }
            }
        }
        if !found_end {
            return Err(VimError::new(
                "E126",
                format!("Missing :endfunction (body starts at line {start_line})"),
            ));
        }
        let (name, params, defaults, varargs, dict, abort) = parse_func_signature(&spec, self.sid)?;
        // Embedded-language heredocs (python << EOF ...) inside a legacy
        // :function body are data, not Ex commands; skip them when parsing.
        let parse_body = strip_embedded_heredocs(&body);
        let stmts = parse_script_lines(&parse_body, self.sid)?;
        Ok(Stmt::FuncDef(Box::new(UserFunc {
            name,
            params,
            defaults,
            varargs,
            body,
            stmts,
            sid: self.sid,
            dict,
            abort,
        })))
    }

    fn parse_vim9_def(&mut self, header: &str) -> VimResult<Stmt> {
        let def_line = self.pos + 1;
        let mut header = rvim_viml::vim9::parser::strip_comment(
            strip_modifiers(header).trim_start_matches(':').trim(),
        );
        // A Vim9 signature may span several lines and carry # comments.
        let mut balance = paren_balance(&header);
        while balance > 0 && self.pos < self.lines.len() {
            let next = rvim_viml::vim9::parser::strip_comment(&self.lines[self.pos]);
            self.pos += 1;
            header.push(' ');
            header.push_str(next.trim());
            balance += paren_balance(&next);
        }
        let mut body = Vec::new();
        let mut depth = 1usize;
        let mut found_end = false;
        while self.pos < self.lines.len() {
            let line = self.lines[self.pos].clone();
            let keyword = structural_command(&line);
            if keyword.as_deref() == Some("enddef") {
                depth -= 1;
                if depth == 0 {
                    self.pos += 1;
                    found_end = true;
                    break;
                }
            } else if keyword.as_deref() == Some("def")
                && line
                    .split_once("def")
                    .is_some_and(|(_, rest)| rest.contains('('))
            {
                depth += 1;
            }
            self.pos += 1;
            let marker = rvim_viml::vim9::parser::heredoc_marker(&line);
            body.push(line);
            if let Some(marker) = marker {
                // Skip a heredoc body; it may contain ':enddef' text.
                while self.pos < self.lines.len() {
                    let line = self.lines[self.pos].clone();
                    let done = line.trim() == marker;
                    self.pos += 1;
                    body.push(line);
                    if done {
                        break;
                    }
                }
            }
        }
        if !found_end {
            return Err(VimError::new(
                "E126",
                format!("Missing :enddef for def at line {def_line}"),
            ));
        }
        let func = rvim_viml::vim9::parse_function(&header, &body, self.sid, "", self.pos)
            .map_err(|e| VimError {
                code: e.code,
                message: format!("{} (def {header} at line {def_line})", e.message),
                abort: e.abort,
            })?;
        Ok(Stmt::Vim9FuncDef(Box::new((*func).clone())))
    }
}

/// Remove embedded-language heredoc bodies (python << EOF ... EOF) from a
/// legacy function body before parsing its statements.
fn strip_embedded_heredocs(body: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(body.len());
    let mut skip: Option<String> = None;
    for line in body {
        if let Some(marker) = &skip {
            if line.trim() == marker {
                skip = None;
            }
            continue;
        }
        if let Some(marker) = embedded_heredoc_marker(line) {
            skip = Some(marker);
            continue;
        }
        out.push(line.clone());
    }
    out
}

fn embedded_heredoc_marker(line: &str) -> Option<String> {
    let text = line.trim_start();
    let lower = text.to_ascii_lowercase();
    if !(lower.starts_with("python") || lower.starts_with("execute") || lower.starts_with("py3")) {
        return None;
    }
    let idx = text.find("<<")?;
    let rest = text[idx + 2..].trim_start();
    let marker: String = rest
        .trim_matches(|c: char| c == '"' || c == '\'' || c == ' ')
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if marker.is_empty() {
        None
    } else {
        Some(marker)
    }
}

/// Arguments of an Ex command line (text after the command word).
fn args_of(line: &str) -> String {
    let trimmed = strip_modifiers(line);
    match parse_line(trimmed, 1) {
        Ok(p) => p.args,
        Err(_) => trimmed.to_string(),
    }
}

fn nonempty_args(line: &str) -> Option<String> {
    let a = args_of(line);
    if a.trim().is_empty() { None } else { Some(a) }
}

fn split_for_spec(spec: &str) -> Option<(String, String)> {
    // `x in expr` or `[a, b] in expr`
    let idx = spec.find(" in ")?;
    let target = spec[..idx].trim().to_string();
    let expr = spec[idx + 4..].trim().to_string();
    Some((target, expr))
}

fn parse_catch_pattern(spec: &str) -> Option<String> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    // ":catch /pat/" or ":catch pat"
    if let Some(rest) = s.strip_prefix('/') {
        let end = rest.rfind('/').unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }
    Some(s.to_string())
}

/// Parts of a `:function` signature: name, named params, "..." marker, `self`
/// dict attribute and `abort` attribute.
type FuncSignature = (
    String,
    Vec<String>,
    Vec<Option<String>>,
    bool,
    Option<String>,
    bool,
);

/// Parse `s:Foo(a, b, ...) dict abort range` into its parts.
fn parse_func_signature(spec: &str, _sid: usize) -> VimResult<FuncSignature> {
    let spec = spec.trim();
    let Some(paren) = spec.find('(') else {
        return Err(VimError::new(
            "E124",
            format!("Invalid argument for :function: {spec}"),
        ));
    };
    let name = spec[..paren].trim().to_string();
    let close = spec[paren..]
        .find(')')
        .map(|i| i + paren)
        .ok_or_else(|| VimError::new("E124", format!("Invalid argument for :function: {spec}")))?;
    let params_raw = &spec[paren + 1..close];
    let mut params = Vec::new();
    let mut defaults = Vec::new();
    for p in params_raw.split(',') {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        if p == "..." {
            // Varargs marker.
            continue;
        }
        match p.find('=') {
            Some(pos) => {
                params.push(p[..pos].trim().to_string());
                defaults.push(Some(p[pos + 1..].trim().to_string()));
            }
            None => {
                params.push(p.to_string());
                defaults.push(None);
            }
        }
    }
    let varargs = params_raw.contains("...");
    let attrs = spec[close + 1..].trim();
    let attr_words: Vec<&str> = attrs.split_whitespace().collect();
    let dict = if attr_words.contains(&"dict") {
        Some("self".to_string())
    } else {
        None
    };
    let abort = attr_words.contains(&"abort");
    Ok((name, params, defaults, varargs, dict, abort))
}

/// Parse a script body into statements.
pub fn parse_script_lines(lines: &[String], sid: usize) -> VimResult<Vec<Stmt>> {
    let expanded = expand_heredocs(lines);
    let joined = join_continuations(&expanded);
    let mut parser = BlockParser {
        lines: &joined,
        pos: 0,
        sid,
    };
    let (stmts, terminator) = parser.parse_block(&[])?;
    if terminator.is_some() {
        return Err(VimError::new(
            "E580",
            "Unexpected block terminator".to_string(),
        ));
    }
    Ok(stmts)
}

/// Expand ':let x =<< trim END' heredocs into a list literal so the statement
/// parser sees a single line. Reference: eval.c heredoc handling in
/// getline()-style assignments (`:help let-heredoc`).
fn expand_heredocs(lines: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    while i < lines.len() {
        let line = &lines[i];
        let Some(idx) = rvim_viml::vim9::parser::find_outside_quotes(line, "=<<") else {
            out.push(line.clone());
            i += 1;
            continue;
        };
        let prefix = line[..idx].trim_end().to_string();
        let spec = line[idx + 3..].trim();
        let mut words: Vec<&str> = spec.split_whitespace().collect();
        let Some(marker) = words.pop() else {
            out.push(line.clone());
            i += 1;
            continue;
        };
        let trim = words.iter().any(|w| w.eq_ignore_ascii_case("trim"));
        let mut content = Vec::new();
        let mut found = false;
        i += 1;
        while i < lines.len() {
            let text = lines[i].clone();
            if text.trim() == marker {
                found = true;
                break;
            }
            content.push(text);
            i += 1;
        }
        if !found {
            // An unterminated heredoc is an error the caller reports; do not
            // swallow the rest of the file while scanning.
            out.push(line.clone());
            i -= content.len();
            i += 1;
            continue;
        }
        if trim {
            let indent = content
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.len() - l.trim_start().len())
                .min()
                .unwrap_or(0);
            for text in &mut content {
                if text.len() >= indent {
                    *text = text[indent..].to_string();
                }
            }
        }
        let items: Vec<String> = content
            .iter()
            .map(|text| format!("'{}'", text.replace('\'', "''")))
            .collect();
        let separator = if prefix.contains('=') { "" } else { " = " };
        out.push(format!("{prefix}{separator}[{}]", items.join(", ")));
        i += 1;
    }
    out
}

/// Join legacy `\` continuation lines onto the previous line.
pub fn join_continuations(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('\\') {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(rest.trim_start());
                continue;
            }
        }
        out.push(line.clone());
    }
    out
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// The host implementation bridging VimL to the editor and the command layer.
pub struct ExHost<'a> {
    ed: &'a mut Editor,
    state: &'a mut ScriptState,
}

/// Build the VimL host over the editor and script state.
pub fn make_host<'a>(ed: &'a mut Editor, state: &'a mut ScriptState) -> ExHost<'a> {
    ExHost { ed, state }
}

impl Host for ExHost<'_> {
    fn option_get(&mut self, name: &str) -> Option<Value> {
        let global = name.starts_with("g:");
        let name = name
            .strip_prefix("l:")
            .or_else(|| name.strip_prefix("g:"))
            .unwrap_or(name);
        if name.starts_with("t_") {
            return Some(Value::Number(if name == "t_Co" { 256 } else { 0 }));
        }
        // 'modified' is not a real option: it is the buffer's changed flag.
        if name == "modified" {
            return Some(Value::Number(i64::from(
                self.ed.current_buffer().is_changed(),
            )));
        }
        let value = if global {
            self.ed.options.get(name).ok()?.clone()
        } else {
            self.ed.wopt(name).ok()?
        };
        Some(match value {
            rvim_editor::OptValue::Bool(b) => Value::Number(i64::from(b)),
            rvim_editor::OptValue::Num(n) => Value::Number(n),
            rvim_editor::OptValue::Str(s) => Value::String(s),
        })
    }

    fn option_set(&mut self, name: &str, value: &Value) -> Option<rvim_core::VimResult<()>> {
        let name = name
            .strip_prefix("l:")
            .or_else(|| name.strip_prefix("g:"))
            .unwrap_or(name);
        // `:set nomodified` clears the buffer changed flag (option.c); the
        // value is otherwise not a settable option.
        if name == "modified" {
            self.ed.set_modified(value.truthy());
            return Some(Ok(()));
        }
        let opt = match value {
            Value::Number(n) => rvim_editor::OptValue::Num(*n),
            Value::String(s) => rvim_editor::OptValue::Str(s.clone()),
            Value::Bool(b) => rvim_editor::OptValue::Bool(*b),
            other => rvim_editor::OptValue::Str(other.to_string_value()),
        };
        Some(self.ed.options.set(name, opt))
    }

    fn buffer_var_get(&mut self, name: &str) -> Option<Value> {
        let bufnr = self.ed.current_bufnr();
        self.state
            .bufvars
            .get(&bufnr)
            .and_then(|m| m.get(name))
            .cloned()
            .or_else(|| builtin_buffer_var(self.ed, name))
    }

    fn buffer_var_set(&mut self, name: &str, value: Value) -> Option<rvim_core::VimResult<()>> {
        let bufnr = self.ed.current_bufnr();
        self.state
            .bufvars
            .entry(bufnr)
            .or_default()
            .insert(name.to_string(), value);
        Some(Ok(()))
    }

    fn window_var_get(&mut self, name: &str) -> Option<Value> {
        self.state.winvars.get(name).cloned()
    }

    fn window_var_set(&mut self, name: &str, value: Value) -> Option<rvim_core::VimResult<()>> {
        self.state.winvars.insert(name.to_string(), value);
        Some(Ok(()))
    }

    fn tab_var_get(&mut self, name: &str) -> Option<Value> {
        self.state.tabvars.get(name).cloned()
    }

    fn tab_var_set(&mut self, name: &str, value: Value) -> Option<rvim_core::VimResult<()>> {
        self.state.tabvars.insert(name.to_string(), value);
        Some(Ok(()))
    }

    fn call_user_function(
        &mut self,
        name: &str,
        args: Vec<Value>,
        env: &mut Env,
    ) -> Option<rvim_viml::value::ValueResult> {
        let resolved = resolve_function_name(name, self.state, env.current_sid);
        if let Some(func) = self.state.funcs.get(&resolved).cloned() {
            return Some(call_user_func(self.ed, env, self.state, &func, args));
        }
        // Vim9 functions (M9) are looked up by canonical name; an unexported
        // function of the current script is reached through <SNR>.
        if let Some(result) = self.call_vim9_named(name, args.clone(), env) {
            return Some(result);
        }
        // Autoload: `foo#bar#baz()` loads `autoload/foo/bar.vim` on first use.
        if name.contains('#') && try_autoload(self.ed, env, self.state, name) {
            let resolved = resolve_function_name(name, self.state, env.current_sid);
            if let Some(func) = self.state.funcs.get(&resolved).cloned() {
                return Some(call_user_func(
                    self.ed,
                    env,
                    self.state,
                    &func,
                    args.clone(),
                ));
            }
            if let Some(result) = self.call_vim9_named(name, args, env) {
                return Some(result);
            }
        }
        None
    }

    fn source_script(
        &mut self,
        path: &std::path::Path,
        env: &mut Env,
    ) -> Option<rvim_core::VimResult<()>> {
        Some(crate::handlers_script::source_file(self.ed, env, self.state, path).map(|_| ()))
    }

    fn define_legacy_function(
        &mut self,
        header: &str,
        body: &[String],
        sid: usize,
        env: &mut Env,
    ) -> Option<rvim_core::VimResult<()>> {
        Some(crate::script::define_legacy_function(
            self.ed, env, self.state, header, body, sid,
        ))
    }

    fn echo_message(&mut self, text: &str, err: bool) -> Option<rvim_core::VimResult<()>> {
        if err {
            Some(Err(VimError::new("E0", text.to_string())))
        } else {
            self.ed.messages.info(text.to_string());
            Some(Ok(()))
        }
    }

    fn call_host_builtin(
        &mut self,
        name: &str,
        args: Vec<Value>,
        env: &mut Env,
    ) -> Option<rvim_viml::value::ValueResult> {
        if HOST_BUILTINS.contains(&name) {
            if let Some(result) = host_builtin(self.ed, env, self.state, name, args.clone()) {
                return Some(result);
            }
        }
        if let Some(result) = crate::host_extra::call(name, &args, self.ed, env, self.state) {
            return Some(result);
        }
        if let Some(result) = crate::host_win::call(name, &args, self.ed, env, self.state) {
            return Some(result);
        }
        if let Some(result) = crate::host_meta::call(name, &args, self.ed, env, self.state) {
            return Some(result);
        }
        crate::host_more::call(name, &args, self.ed, env, self.state)
    }

    /// Cheap name check so `call_function` can skip the big dispatchers for
    /// ordinary (non-builtin) calls. See [`Host::is_host_builtin`].
    fn is_host_builtin(&self, name: &str) -> bool {
        HOST_BUILTINS.contains(&name)
            || crate::host_extra::FUNCS.contains(&name)
            || crate::host_win::FUNCS.contains(&name)
            || crate::host_meta::FUNCS.contains(&name)
            || crate::host_more::FUNCS.contains(&name)
    }

    fn maxfuncdepth(&self) -> usize {
        funcdepth_limit(self.ed)
    }

    /// `b:` as a dictionary snapshot (evalvars.c:3203-3236 returns the buffer
    /// scope dictitem for the empty variable name).
    fn buffer_var_dict(&mut self) -> Option<Value> {
        let bufnr = self.ed.current_bufnr();
        let mut dict = rvim_viml::VimDict::new();
        if let Some(vars) = self.state.bufvars.get(&bufnr) {
            for (k, v) in vars {
                dict.insert(k.clone(), v.clone());
            }
        }
        dict.insert(
            "changedtick".to_string(),
            Value::Number(self.ed.changedtick() as i64),
        );
        Some(Value::dict(dict))
    }

    fn window_var_dict(&mut self) -> Option<Value> {
        let mut dict = rvim_viml::VimDict::new();
        for (k, v) in &self.state.winvars {
            dict.insert(k.clone(), v.clone());
        }
        Some(Value::dict(dict))
    }

    fn tab_var_dict(&mut self) -> Option<Value> {
        let mut dict = rvim_viml::VimDict::new();
        for (k, v) in &self.state.tabvars {
            dict.insert(k.clone(), v.clone());
        }
        Some(Value::dict(dict))
    }

    fn exists_check(&mut self, what: &str) -> Option<bool> {
        host_exists(self.ed, self.state, what)
    }

    fn has_feature(&self, feature: &str) -> bool {
        host_has(self.ed, feature)
    }

    fn execute_command(&mut self, line: &str, env: &mut Env) -> Option<rvim_core::VimResult<()>> {
        Some(run_line_parts(self.ed, env, self.state, line).map(|_| ()))
    }
}

impl ExHost<'_> {
    /// Dispatch a call to a compiled Vim9 function, if one matches.
    fn call_vim9_named(
        &mut self,
        name: &str,
        args: Vec<Value>,
        env: &mut Env,
    ) -> Option<rvim_viml::value::ValueResult> {
        let mut candidates = vec![name.to_string()];
        if !name.starts_with('<') && !name.contains('#') {
            candidates.push(format!("<SNR>{}_{}", env.current_sid, name));
        }
        for candidate in candidates {
            if let Some(func) = env.vim9.function(&candidate) {
                return Some(rvim_viml::vim9::call_vim9(&func, args, env, self));
            }
        }
        None
    }
}

fn builtin_buffer_var(ed: &Editor, name: &str) -> Option<Value> {
    match name {
        // b:changedtick is the buffer's monotonic change counter (change.c).
        "changedtick" => Some(Value::Number(ed.changedtick() as i64)),
        _ => None,
    }
}

/// Resolve `s:Name` / `<SID>` to the script-specific stored name.
pub fn resolve_function_name(name: &str, state: &ScriptState, sid: usize) -> String {
    if let Some(rest) = name.strip_prefix("s:") {
        let key = format!("<SNR>{sid}_{rest}");
        if state.funcs.contains_key(&key) {
            return key;
        }
        return key;
    }
    if let Some(rest) = name.strip_prefix("<SID>") {
        return format!("<SNR>{sid}_{rest}");
    }
    if name.starts_with("<SNR>") {
        return name.to_string();
    }
    name.to_string()
}

fn host_has(ed: &Editor, feature: &str) -> bool {
    if feature == "gui_running" {
        return ed.gui_running;
    }
    if feature == "termguicolors" {
        return true;
    }
    let feature = feature.to_ascii_lowercase();
    let _ = ed;
    if let Some(patch) = feature.strip_prefix("patch-") {
        // The baseline is pinned to v8.2.5172; only that release line and its
        // earlier patches are reported.
        let Some(version) = patch.strip_prefix("8.2.") else {
            return false;
        };
        return version.parse::<u32>().is_ok_and(|number| number <= 5172);
    }
    match feature.as_str() {
        "eval" | "multi_byte" | "syntax" | "autocmd" | "listcmds" | "user_commands"
        | "cmdline_compl" | "cmdline_hist" | "quickfix" | "folding" | "diff" | "windows"
        | "tabpages" | "spell" | "tag_binary" | "crypt" | "cryptv" | "crypt-blowfish"
        | "crypt-blowfish2" | "crypt-zip" | "job" | "channel" | "terminal" | "timers"
        | "vim9script" | "float" | "vim_starting" => true,
        // Platforms and explicitly unsupported subsystems.
        "win32" | "win64" => cfg!(windows),
        "unix" => cfg!(unix),
        "gui" | "xterm_clipboard" | "netbeans_intg" | "cscope" | "persistent_undo"
        | "libsodium" | "xchacha20" => false,
        _ => false,
    }
}

fn host_exists(ed: &Editor, state: &ScriptState, what: &str) -> Option<bool> {
    if let Some(rest) = what.strip_prefix(':') {
        // Command existence.
        let word = rest
            .chars()
            .take_while(|c| c.is_ascii_alphabetic() || *c == '!' || *c == '#')
            .collect::<String>();
        return Some(lookup(&word).is_some() || ed.usercmds.get(&word).is_some());
    }
    if let Some(rest) = what.strip_prefix('+') {
        return Some(ed.options.get(rest).is_ok());
    }
    if let Some(rest) = what.strip_prefix('&') {
        if rest.starts_with("t_") {
            return Some(true);
        }
        return Some(ed.options.get(rest).is_ok());
    }
    if let Some(rest) = what.strip_prefix('*') {
        let resolved = resolve_function_name(rest, state, 0);
        return Some(state.funcs.contains_key(&resolved) || rvim_viml::builtins::is_builtin(rest));
    }
    if let Some(rest) = what.strip_prefix('$') {
        return Some(std::env::var(rest).is_ok());
    }
    None
}

/// Evaluate a VimL expression in the current context.
pub fn eval_expr(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    src: &str,
) -> VimResult<Value> {
    let expr = parse_expression(src)?;
    let mut host = ExHost { ed, state };
    rvim_viml::eval(&expr, env, &mut host)
}

/// Execute one Ex line with script context (no multi-line constructs).
pub fn run_line_parts(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    line: &str,
) -> VimResult<ExOutcome> {
    crate::exec::run_line_with(ed, env, state, line)
}

/// Evaluate one expression at the start of `src` and report how many *bytes* it
/// consumed, so `:execute` can evaluate each argument separately and join the
/// results with spaces (eval.c:6699-6757).
pub fn eval_expr_prefix(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    src: &str,
) -> VimResult<(Value, usize)> {
    let (expr, consumed_chars) = rvim_viml::expr::parse_expression_prefix(src)?;
    let mut host = ExHost { ed, state };
    let value = rvim_viml::eval(&expr, env, &mut host)?;
    let consumed_bytes = src
        .char_indices()
        .nth(consumed_chars)
        .map(|(byte, _)| byte)
        .unwrap_or(src.len());
    Ok((value, consumed_bytes))
}

/// Run a single line given a [`Ctx`].
pub fn run_line_ctx(ed: &mut Editor, ctx: &mut Ctx, line: &str) -> VimResult<ExOutcome> {
    let Ctx { env, state } = ctx;
    run_line_parts(ed, env, state, line)
}

/// Execute a parsed statement list.
///
/// Legacy VimL keeps running after an ordinary error (unless the function has
/// the `abort` attribute), so top-level script execution records the error and
/// continues; `:try` bodies use the strict variant so they can catch errors.
pub fn exec_stmts(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stmts: &[Stmt],
) -> VimResult<Flow> {
    exec_stmts_impl(ed, env, state, stmts, false)
}

fn exec_stmts_strict(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stmts: &[Stmt],
) -> VimResult<Flow> {
    exec_stmts_impl(ed, env, state, stmts, true)
}

fn exec_stmts_impl(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stmts: &[Stmt],
    strict: bool,
) -> VimResult<Flow> {
    for stmt in stmts {
        let flow = match exec_stmt(ed, env, state, stmt) {
            Ok(flow) => flow,
            Err(e) => {
                // A `:throw` that crossed a function-call boundary was parked
                // in `pending_throw` by `call_user_func`; re-throw the original
                // value (it must not be re-wrapped as E605, eval.txt:2726).
                // At the top of a sourced script this makes source_file()
                // abort the script with E605 (ex_docmd.c:1284-1285).
                if let Some(exc) = env.pending_throw.take() {
                    return Ok(Flow::Throw(exc));
                }
                // An exception that left `:source`/a function arrives as an
                // aborting E605 carrying its value (handlers_script.rs
                // source_file() convention). Turn it back into the original
                // thrown value so `:catch` sees "boom", not the E605 text.
                if let Some(exc) = thrown_value_from_error(&e) {
                    env.vvars.insert("exception".to_string(), exc.clone());
                    if strict || env.try_depth > 0 || !env.frames.is_empty() {
                        return Ok(Flow::Throw(exc));
                    }
                    // Uncaught at script level: keep unwinding the sourcing
                    // chain with the aborting error (need_rethrow).
                    return Err(e);
                }
                // Non-continuable errors (VimError::aborting, e.g. an uncaught
                // throw escaping `:source`) abort the current command sequence
                // instead of being reported and skipped (ex_docmd.c:1284-1285).
                if e.abort {
                    return Err(e);
                }
                // While a `:try` is active an error becomes an exception with
                // the "Vim({command}):" prefix (ex_eval.c:144 cause_errthrow()).
                if strict || env.try_depth > 0 {
                    return Ok(Flow::Throw(Value::String(format!(
                        "Vim({}):{e}",
                        stmt_command_name(stmt)
                    ))));
                }
                env.vvars
                    .insert("errmsg".to_string(), Value::String(e.to_string()));
                ed.emsg(e.to_string());
                // ":func-abort": stop at the first error; the caller turns the
                // aborted call into -1 (eval.txt:2726-2728).
                if env.aborting_func {
                    return Err(e);
                }
                continue;
            }
        };
        if !matches!(flow, Flow::Next) {
            return Ok(flow);
        }
    }
    Ok(Flow::Next)
}

/// Recover the thrown value from the `E605 "Exception not caught: value"` error
/// that [`call_user_func`] and `source_file()` produce when an exception leaves
/// a function or a sourced script (ex_eval.c:1537-1567 `do_throw`/`need_rethrow`).
///
/// Both use the convention "E605: Exception not caught: {tv_get_string(value)}"
/// so an enclosing `:try` can turn the error back into the original value.
fn thrown_value_from_error(e: &VimError) -> Option<Value> {
    if e.code != "E605" {
        return None;
    }
    e.message
        .strip_prefix("Exception not caught: ")
        .map(|text| Value::String(text.to_string()))
}

/// Command name used in the `Vim(cmd):` exception prefix for an error raised by
/// `stmt` (`eap->cmd` at the throw point, ex_eval.c:144-260).
fn stmt_command_name(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::Line(line) => {
            let trimmed = strip_modifiers(line);
            match parse_line(trimmed, 1) {
                Ok(p) => lookup(&p.command).map(|c| c.name).unwrap_or("call"),
                Err(_) => "call",
            }
        }
        Stmt::If { .. } => "if",
        Stmt::While { .. } => "while",
        Stmt::For { .. } => "for",
        Stmt::Try { .. } => "try",
        Stmt::FuncDef(_) => "function",
        Stmt::Vim9FuncDef(_) => "def",
        Stmt::Return(_) => "return",
        Stmt::Throw(_) => "throw",
        Stmt::Break => "break",
        Stmt::Continue => "continue",
    }
}

fn exec_stmt(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    stmt: &Stmt,
) -> VimResult<Flow> {
    match stmt {
        Stmt::Line(line) => match crate::exec::run_line_plain(ed, env, state, line) {
            Ok(ExOutcome::Continue) => Ok(Flow::Next),
            Ok(ExOutcome::Finish) => Ok(Flow::Quit(ExOutcome::Finish)),
            Ok(other) => Ok(Flow::Quit(other)),
            Err(e) => Err(e),
        },
        Stmt::If {
            branches,
            else_body,
        } => {
            for (cond, body) in branches {
                let value = eval_expr(ed, env, state, cond)?;
                if value.truthy() {
                    return exec_stmts(ed, env, state, body);
                }
            }
            match else_body {
                Some(body) => exec_stmts(ed, env, state, body),
                None => Ok(Flow::Next),
            }
        }
        Stmt::While { cond, body } => {
            loop {
                let value = eval_expr(ed, env, state, cond)?;
                if !value.truthy() {
                    break;
                }
                match exec_stmts(ed, env, state, body)? {
                    Flow::Next | Flow::Continue => continue,
                    Flow::Break => break,
                    other => return Ok(other),
                }
            }
            Ok(Flow::Next)
        }
        Stmt::For { target, expr, body } => {
            let value = eval_expr(ed, env, state, expr)?;
            let items = iterate_value(&value)?;
            for item in items {
                assign_target_simple(ed, env, state, target, item)?;
                match exec_stmts(ed, env, state, body)? {
                    Flow::Next | Flow::Continue => continue,
                    Flow::Break => break,
                    other => return Ok(other),
                }
            }
            Ok(Flow::Next)
        }
        Stmt::Try {
            body,
            catches,
            finally,
        } => {
            // Errors raised while a try conditional is active become exceptions
            // (ex_eval.c:144); this guard covers the body, the catches and the
            // finally clause alike.
            env.try_depth += 1;
            let result = (|| -> VimResult<Flow> {
                let mut flow = match exec_stmts_strict(ed, env, state, body) {
                    Ok(flow) => flow,
                    Err(e) => {
                        if let Some(exc) = env.pending_throw.take() {
                            Flow::Throw(exc)
                        } else if let Some(exc) = thrown_value_from_error(&e) {
                            env.vvars.insert("exception".to_string(), exc.clone());
                            Flow::Throw(exc)
                        } else {
                            Flow::Throw(Value::String(format!("Vim:{e}")))
                        }
                    }
                };
                // A thrown value (explicit `:throw` or a converted error) goes
                // through the catch clauses.
                if let Flow::Throw(exc) = &flow {
                    env.vvars.insert("exception".to_string(), exc.clone());
                    let text = exc.to_string_value();
                    let mut handled = None;
                    for clause in catches {
                        let hit = match &clause.pattern {
                            None => true,
                            Some(pat) => rvim_viml::regex_mvp::is_match(pat, &text, false),
                        };
                        if hit {
                            handled = Some(clause);
                            break;
                        }
                    }
                    if let Some(clause) = handled {
                        flow = exec_stmts(ed, env, state, &clause.body)?;
                    }
                }
                // The finally clause always runs. A `:return`/`:break`/
                // `:continue`/`:finish` or a new exception from it discards any
                // pending exception/command (eval.txt:4000-4008).
                if !finally.is_empty() {
                    let finally_flow = exec_stmts(ed, env, state, finally)?;
                    if !matches!(finally_flow, Flow::Next) {
                        flow = finally_flow;
                    }
                }
                Ok(flow)
            })();
            env.try_depth = env.try_depth.saturating_sub(1);
            result
        }
        Stmt::FuncDef(func) => {
            let key = func_key(func);
            state.funcs.insert(key, (**func).clone());
            Ok(Flow::Next)
        }
        Stmt::Vim9FuncDef(func) => {
            env.vim9.add_function(std::rc::Rc::new((**func).clone()));
            Ok(Flow::Next)
        }
        Stmt::Return(expr) => {
            if env.frames.is_empty() {
                return Err(VimError::new(
                    "E133",
                    ":return not inside a function".to_string(),
                ));
            }
            let value = match expr {
                Some(src) => eval_expr(ed, env, state, src)?,
                None => Value::Number(0),
            };
            Ok(Flow::Return(value))
        }
        Stmt::Throw(expr) => {
            let value = eval_expr(ed, env, state, expr)?;
            Ok(Flow::Throw(value))
        }
        Stmt::Break => Ok(Flow::Break),
        Stmt::Continue => Ok(Flow::Continue),
    }
}

fn paren_balance(text: &str) -> i32 {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == '\\' && q == '"' {
                    chars.next();
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

fn func_key(func: &UserFunc) -> String {
    if let Some(rest) = func.name.strip_prefix("s:") {
        format!("<SNR>{}_{}", func.sid, rest)
    } else {
        func.name.clone()
    }
}

fn iterate_value(value: &Value) -> VimResult<Vec<Value>> {
    Ok(match value {
        Value::List(l) => l.borrow().clone(),
        Value::Blob(b) => b
            .borrow()
            .iter()
            .map(|x| Value::Number(*x as i64))
            .collect(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        other => {
            return Err(VimError::new(
                "E714",
                format!("List required for :for, got {}", other.type_name()),
            ));
        }
    })
}

/// Assign to a simple variable or `[a, b]` destructure target.
fn assign_target_simple(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    target: &str,
    value: Value,
) -> VimResult<()> {
    let target = target.trim();
    if target.starts_with('[') && target.ends_with(']') {
        let inner = &target[1..target.len() - 1];
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        let items = iterate_value(&value)?;
        for (i, part) in parts.iter().enumerate() {
            let item = items.get(i).cloned().unwrap_or(Value::Null);
            assign_var(ed, env, state, part, item)?;
        }
        return Ok(());
    }
    assign_var(ed, env, state, target, value)
}

/// Assign to a variable target (`name`, `s:name`, `&opt`, `@r`, `b:name`).
pub fn assign_var(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    target: &str,
    value: Value,
) -> VimResult<()> {
    let target = target.trim();
    if let Some(opt) = target.strip_prefix('&') {
        let mut host = ExHost { ed, state };
        return match host.option_set(opt, &value) {
            Some(result) => result,
            None => Err(VimError::new("E113", format!("Unknown option: {opt}"))),
        };
    }
    if let Some(reg) = target.strip_prefix('@') {
        // ":let @a = expr": a List becomes a linewise register, anything else a
        // charwise one (eval.c set_reg_var → write_reg_contents()).
        let name = reg
            .chars()
            .next()
            .ok_or_else(|| VimError::new("E354", "Invalid register name".to_string()))?;
        let register = match &value {
            Value::List(items) => {
                let mut text = items
                    .borrow()
                    .iter()
                    .map(Value::to_string_value)
                    .collect::<Vec<_>>()
                    .join("\n");
                text.push('\n');
                rvim_editor::Register::linewise(text)
            }
            other => rvim_editor::Register::charwise(other.to_string_value()),
        };
        ed.registers.put(name, register);
        return Ok(());
    }
    if let Some(rest) = target.strip_prefix("b:") {
        if let Some(v) = state
            .bufvars
            .entry(ed.current_bufnr())
            .or_default()
            .get_mut(rest)
        {
            *v = value;
        } else {
            state
                .bufvars
                .entry(ed.current_bufnr())
                .or_default()
                .insert(rest.to_string(), value);
        }
        return Ok(());
    }
    if let Some(rest) = target.strip_prefix("w:") {
        state.winvars.insert(rest.to_string(), value);
        return Ok(());
    }
    if let Some(rest) = target.strip_prefix("t:") {
        state.tabvars.insert(rest.to_string(), value);
        return Ok(());
    }
    env.set_var(target, value)
}

/// `tv_get_lnum()` (typval.c:2462): a Number is used as-is, a String may be a
/// number, "$", "." or a mark reference "'x".
pub fn tv_get_lnum(ed: &Editor, value: &Value) -> i64 {
    if let Value::String(s) = value {
        let t = s.trim();
        if let Ok(n) = t.parse::<i64>() {
            return n;
        }
        return match t {
            "." => ed.cursor().line as i64 + 1,
            "$" => ed.line_count() as i64,
            _ if t.starts_with('\'') => {
                let name = t.chars().nth(1).unwrap_or('"');
                ed.marks
                    .resolve(ed.current_bufnr(), name)
                    .map(|p| p.line as i64 + 1)
                    .unwrap_or(0)
            }
            _ => 0,
        };
    }
    value.to_number()
}

/// Source the autoload script that should define `name`; true when a script was
/// sourced (and the function may now exist).
fn try_autoload(ed: &mut Editor, env: &mut Env, state: &mut ScriptState, name: &str) -> bool {
    let (func_part, _) = match name.split_once('#') {
        Some(p) => p,
        None => return false,
    };
    let _ = func_part;
    // `foo#bar#baz` -> autoload/foo/bar.vim
    let mut parts: Vec<&str> = name.split('#').collect();
    if parts.len() < 2 {
        return false;
    }
    let last = parts.pop().unwrap();
    let _ = last;
    // `demo#hello` -> autoload/demo.vim, `foo#bar#baz` -> autoload/foo/bar.vim
    let rel = format!("autoload/{}.vim", parts.join("/"));
    if state.autoload.contains(&rel) {
        return false;
    }
    let runtimepath = ed.options.str_or("runtimepath", "");
    for dir in runtimepath.split(',').filter(|d| !d.is_empty()) {
        let candidate = Path::new(dir).join(&rel);
        if candidate.is_file() {
            state.autoload.insert(rel.clone());
            let _ = crate::handlers_script::source_file(ed, env, state, &candidate);
            return true;
        }
    }
    state.autoload.insert(rel);
    false
}
/// Default stack size for the VimL/editor worker thread.
///
/// A nested VimL function call costs tens of KiB of native stack in a debug
/// build (`call_user_func` -> `exec_stmts` -> `eval` -> ...), so the 1 MiB
/// Windows main-thread stack overflows at ~25 levels — long before the
/// `'maxfuncdepth'` (100) guard can report E132. The binary must run the whole
/// session on a thread with this stack size; [`run_with_deep_stack`] does it.
pub const DEEP_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Run `f` on a worker thread with [`DEEP_STACK_SIZE`] of stack and return its
/// result, propagating a panic.
///
/// `bin/rvim` must call this around the whole session setup/run (the `Session`
/// itself is not `Send`, so the thread has to start before it is created):
///
/// ```ignore
/// fn main() -> ExitCode {
///     rvim_ex::script::run_with_deep_stack(run)
/// }
/// fn run() -> ExitCode { /* former main body */ }
/// ```
pub fn run_with_deep_stack<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let handle = std::thread::Builder::new()
        .name("rvim-main".to_string())
        .stack_size(DEEP_STACK_SIZE)
        .spawn(f)
        .expect("rvim: cannot spawn the main worker thread");
    match handle.join() {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Default `'maxfuncdepth'` when the option table has no entry (option.c).
const DEFAULT_MAXFUNCDEPTH: usize = 100;

fn funcdepth_limit(ed: &Editor) -> usize {
    match ed.options.get("maxfuncdepth") {
        Ok(rvim_editor::OptValue::Num(n)) if *n > 0 => *n as usize,
        _ => DEFAULT_MAXFUNCDEPTH,
    }
}

/// JSON encode/decode used by the channel layer (M7).
pub fn json_encode(value: &Value) -> VimResult<String> {
    rvim_viml::builtins_fl::json_encode_public(value)
}

pub fn json_decode(text: &str) -> VimResult<Value> {
    rvim_viml::builtins_fl::json_decode_public(text)
}

/// Invoke a Funcref/lambda/function-name callback on the main thread.
pub fn invoke_callback(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    target: &Value,
    args: Vec<Value>,
) -> VimResult<Value> {
    let mut host = make_host(ed, state);
    rvim_viml::eval::call_value(target, args, env, &mut host)
}

/// Call a user function: bind arguments, execute the body, catch `:return`.
/// Define a legacy ':func ... :endfunc' found inside a Vim9 script.
///
/// The Ex layer owns legacy functions, so the Vim9 engine hands the block back
/// here; we synthesise a ':function' script and run it, which reuses the normal
/// signature parser and registration path.
pub fn define_legacy_function(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    header: &str,
    body: &[String],
    sid: usize,
) -> VimResult<()> {
    let header = header.trim();
    let normalized = if header.starts_with("func ") || header.starts_with("function ") {
        header.replacen("func", "function", 1)
    } else {
        format!("function {header}")
    };
    let mut lines = Vec::with_capacity(body.len() + 2);
    lines.push(normalized);
    lines.extend(body.iter().cloned());
    lines.push("endfunction".to_string());
    let stmts = parse_script_lines(&lines, sid)?;
    exec_stmts(ed, env, state, &stmts)?;
    Ok(())
}

pub fn call_user_func(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    func: &UserFunc,
    args: Vec<Value>,
) -> VimResult<Value> {
    // funcdepth_increment() refuses the call once the depth reaches
    // 'maxfuncdepth' (userfunc.c:2536-2544).
    if env.call_depth >= funcdepth_limit(ed) {
        return Err(VimError::new(
            "E132",
            "Function call depth is higher than 'maxfuncdepth'".to_string(),
        ));
    }
    let mut frame = rvim_viml::env::Frame {
        args: HashMap::new(),
        locals: HashMap::new(),
        sid: func.sid,
    };
    let required = func.defaults.iter().filter(|d| d.is_none()).count();
    if args.len() < required {
        return Err(VimError::new(
            "E119",
            format!("Not enough arguments for function: {}", func.name),
        ));
    }
    if !func.varargs && args.len() > func.params.len() {
        return Err(VimError::new(
            "E118",
            format!("Too many arguments for function: {}", func.name),
        ));
    }
    // Vim always provides a:firstline / a:lastline: the call range, or the
    // current line when the call has no range (userfunc.c:2724-2728).
    let call_line = ed.cursor().line as i64 + 1;
    frame
        .args
        .insert("firstline".to_string(), Value::Number(call_line));
    frame
        .args
        .insert("lastline".to_string(), Value::Number(call_line));
    // Named arguments live in a: under their names (userfunc.c:2735-2769).
    for (i, param) in func.params.iter().enumerate() {
        let value = match args.get(i) {
            Some(value) => value.clone(),
            None => match func.defaults.get(i).and_then(|d| d.as_deref()) {
                Some(expr) => eval_expr(ed, env, state, expr)?,
                None => Value::Null,
            },
        };
        frame.args.insert(param.clone(), value);
    }
    let extra = args.len().saturating_sub(func.params.len());
    frame
        .args
        .insert("0".to_string(), Value::Number(extra as i64));
    frame.args.insert(
        "000".to_string(),
        Value::list(args.iter().skip(func.params.len()).cloned().collect()),
    );
    // Only the varargs are addressable as a:1..a:N; a function without "..."
    // has no numeric argument variables (userfunc.c:2769-2771).
    for (i, value) in args.iter().enumerate().skip(func.params.len()) {
        frame
            .args
            .insert((i - func.params.len() + 1).to_string(), value.clone());
    }
    if let Some(dict_name) = &func.dict {
        let _ = dict_name;
        if let Some(Value::Dict(d)) = args.first() {
            frame
                .locals
                .insert("self".to_string(), Value::Dict(d.clone()));
        }
    }
    env.frames.push(frame);
    env.call_depth += 1;
    let saved_sid = env.current_sid;
    let saved_abort = env.aborting_func;
    env.current_sid = func.sid;
    env.aborting_func = func.abort;
    let flow = exec_stmts(ed, env, state, &func.stmts);
    env.aborting_func = saved_abort;
    env.current_sid = saved_sid;
    env.frames.pop();
    env.call_depth -= 1;
    match flow {
        Ok(Flow::Return(value)) => Ok(value),
        Ok(Flow::Throw(exc)) => {
            // Park the original exception value for the caller: it must travel
            // through the call stack unchanged (eval.txt:2726-2728).
            env.vvars.insert("exception".to_string(), exc.clone());
            env.pending_throw = Some(exc.clone());
            Err(VimError::new(
                "E605",
                format!("Exception not caught: {}", exc.to_string_value()),
            ))
        }
        // The abort attribute returns -1 to the caller after the error was
        // reported by exec_stmts_impl (eval.txt:2726-2728).
        Err(_) if func.abort => Ok(Value::Number(-1)),
        Err(e) => Err(e),
        Ok(Flow::Quit(outcome)) => Err(VimError::new(
            "E168",
            format!("Aborting function unexpectedly: {outcome:?}"),
        )),
        Ok(Flow::Break | Flow::Continue | Flow::Next) => Ok(Value::Number(0)),
    }
}

/// Builtins that need editor access.
pub const HOST_BUILTINS: &[&str] = &[
    "appendbufline",
    "bufadd",
    "bufexists",
    "buflisted",
    "bufloaded",
    "buffer_exists",
    "buffer_name",
    "buffer_number",
    "changenr",
    "cindent",
    "getbufvar",
    "getbufline",
    "getcurpos",
    "getpid",
    "byte2line",
    "line2byte",
    "getbufinfo",
    "getpos",
    "getreg",
    "getregtype",
    "hostname",
    "setbufvar",
    "setbufline",
    "setpos",
    "setreg",
    "visualmode",
    "getline",
    "setline",
    "append",
    "appendbufline",
    "line",
    "col",
    "expand",
    "filereadable",
    "isdirectory",
    "executable",
    "exists",
    "has",
    "did_filetype",
    "fnamemodify",
    "getcwd",
    "bufname",
    "bufnr",
    "mode",
    "strftime",
    "localtime",
    "execute",
    "synID",
    "synIDattr",
    "synIDtrans",
    "synconcealed",
    "synstack",
    "soundfold",
    "spellbadword",
    "spellsuggest",
    // M7 jobs/channels/timers/terminals.
    "job_start",
    "job_status",
    "job_stop",
    "job_getchannel",
    "job_info",
    "job_setoptions",
    "ch_open",
    "ch_close",
    "ch_close_in",
    "ch_sendraw",
    "ch_sendexpr",
    "ch_evalexpr",
    "ch_evalraw",
    "ch_read",
    "ch_readraw",
    "ch_readblob",
    "ch_canread",
    "ch_status",
    "ch_getjob",
    "ch_getbufnr",
    "ch_info",
    "ch_log",
    "ch_logfile",
    "ch_setoptions",
    "timer_start",
    "timer_stop",
    "timer_stopall",
    "timer_pause",
    "timer_info",
    "term_start",
    "term_wait",
    "term_getline",
    "term_getsize",
    "term_getstatus",
    "term_getjob",
    "term_list",
    "term_sendkeys",
    "term_scrape",
    "terminalprops",
];
fn host_builtin(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    name: &str,
    args: Vec<Value>,
) -> Option<VimResult<Value>> {
    // Names this host provides; anything else falls through to the pure
    // builtin table in rvim-viml.

    if !HOST_BUILTINS.contains(&name) {
        return None;
    }
    Some((|| -> VimResult<Value> {
        // Subsystem host functions live in their own modules.
        if let Some(result) = crate::host_syn::call(name, &args, ed, env, state) {
            return result;
        }
        if let Some(result) = crate::host_spell::call(name, &args, ed, env, state) {
            return result;
        }
        if let Some(result) = crate::m7::call(name, &args, ed, env, state) {
            return result;
        }
        let s = |i: usize| args.get(i).map(Value::to_string_value).unwrap_or_default();
        let n = |i: usize| args.get(i).map(Value::to_number).unwrap_or(0);
        match name {
            "getline" => {
                // getline({lnum}) -> String; getline({lnum}, {end}) -> List.
                // {lnum} accepts a Number, "$", "." and "'x" (tv_get_lnum in
                // typval.c:2462); lists clamp into the buffer, a single line
                // outside it is "" (evalbuffer.c:737 get_buffer_lines()).
                let lnum = if args.is_empty() {
                    ed.cursor().line as i64 + 1
                } else {
                    tv_get_lnum(ed, &args[0])
                };
                if args.len() > 1 {
                    let mut start = lnum;
                    let mut end = tv_get_lnum(ed, &args[1]);
                    if end < start {
                        return Ok(Value::list(Vec::new()));
                    }
                    if start < 1 {
                        start = 1;
                    }
                    let max = ed.line_count() as i64;
                    if end > max {
                        end = max;
                    }
                    let mut out = Vec::new();
                    for line in start..=end {
                        if let Ok(text) = ed.current_buffer().line_str(line as usize - 1) {
                            out.push(Value::String(text.to_string()));
                        }
                    }
                    Ok(Value::list(out))
                } else if lnum < 1 || lnum > ed.line_count() as i64 {
                    Ok(Value::String(String::new()))
                } else {
                    Ok(Value::String(
                        ed.current_buffer()
                            .line_str(lnum as usize - 1)
                            .map(str::to_string)
                            .unwrap_or_default(),
                    ))
                }
            }
            "setline" => {
                let lnum = n(0).saturating_sub(1).max(0) as usize;
                match args.get(1) {
                    Some(Value::List(l)) => {
                        let lines: Vec<String> =
                            l.borrow().iter().map(Value::to_string_value).collect();
                        let end = (lnum + lines.len()).min(ed.line_count());
                        if lnum < ed.line_count() {
                            ed.replace_lines(lnum, end, lines)?;
                        } else {
                            ed.replace_lines(ed.line_count(), ed.line_count(), lines)?;
                        }
                        Ok(Value::Number(0))
                    }
                    Some(v) => {
                        ed.set_line_text(lnum, v.to_string_value())?;
                        Ok(Value::Number(0))
                    }
                    None => Ok(Value::Number(0)),
                }
            }
            "line" => {
                let arg = s(0);
                let lnum = match arg.as_str() {
                    "." => ed.cursor().line as i64 + 1,
                    "$" => ed.line_count() as i64,
                    mark if mark.starts_with('\'') => {
                        let name = mark.chars().nth(1).unwrap_or('"');
                        ed.marks
                            .resolve(ed.current_bufnr(), name)
                            .map(|p| p.line as i64 + 1)
                            .unwrap_or(0)
                    }
                    other => other.parse().unwrap_or(0),
                };
                Ok(Value::Number(lnum))
            }
            "col" => Ok(Value::Number(ed.cursor().col as i64 + 1)),
            "expand" => Ok(Value::String(expand_special(ed, state, &s(0)))),
            "filereadable" => Ok(Value::Number(i64::from(Path::new(&s(0)).is_file()))),
            "isdirectory" => Ok(Value::Number(i64::from(Path::new(&s(0)).is_dir()))),
            "executable" => Ok(Value::Number(0)),
            "exists" => {
                let what = s(0);
                // Scope dictionaries always exist when they can be evaluated
                // bare (evalvars.c:3203-3236); l:/a: only inside a function.
                if matches!(what.as_str(), "g:" | "b:" | "w:" | "t:" | "s:" | "v:") {
                    return Ok(Value::Number(1));
                }
                if matches!(what.as_str(), "l:" | "a:") {
                    return Ok(Value::Number(i64::from(!env.frames.is_empty())));
                }
                let mut hit = env.var_exists(&what);
                if !hit {
                    if let Some(rest) = what.strip_prefix("b:") {
                        hit = state
                            .bufvars
                            .get(&ed.current_bufnr())
                            .is_some_and(|m| m.contains_key(rest));
                    } else if let Some(rest) = what.strip_prefix("w:") {
                        hit = state.winvars.contains_key(rest);
                    } else if let Some(rest) = what.strip_prefix("t:") {
                        hit = state.tabvars.contains_key(rest);
                    } else {
                        hit = host_exists(ed, state, &what).unwrap_or(false);
                    }
                }
                Ok(Value::Number(i64::from(hit)))
            }
            "has" => Ok(Value::Number(i64::from(host_has(ed, &s(0))))),
            "did_filetype" => Ok(Value::Number(i64::from(
                !ed.options.str_or("filetype", "").is_empty(),
            ))),
            "expand_special" => Ok(Value::String(expand_special(ed, state, &s(0)))),
            "fnamemodify" => Ok(Value::String(fnamemodify(&s(0), &s(1), &ed.cwd))),
            "getcwd" => Ok(Value::String(ed.cwd.display().to_string())),
            // `winnr()`/`tabpagenr()` are answered by host_win with the real
            // window/tabpage model; these arms must not shadow them.
            "tabpagenr" => Ok(Value::Number(1)),
            "mode" => Ok(Value::String("n".to_string())),
            "strftime" => Ok(Value::String(String::new())),
            "localtime" => Ok(Value::Number(0)),
            "setf" | "setfiletype" => Ok(Value::Number(0)),
            "execute" => {
                // execute({expr} [, {silent}])
                let cmd = s(0);
                run_line_parts(ed, env, state, &cmd)?;
                Ok(Value::String(String::new()))
            }
            "getbufline" => {
                let lnum = n(1).max(1) as usize;
                let end = if args.len() > 2 { n(2) as usize } else { lnum };
                let mut out = Vec::new();
                for line in lnum..=end.max(lnum) {
                    if let Ok(text) = ed.current_buffer().line_str(line - 1) {
                        out.push(Value::String(text.to_string()));
                    }
                }
                Ok(Value::list(out))
            }
            "setbufline" => {
                let lnum = n(1).max(1) as usize;
                let text = match args.get(2) {
                    Some(Value::List(l)) => l
                        .borrow()
                        .iter()
                        .map(Value::to_string_value)
                        .collect::<Vec<_>>()
                        .join("\n"),
                    Some(v) => v.to_string_value(),
                    None => String::new(),
                };
                ed.set_line_text(lnum - 1, text)?;
                Ok(Value::Number(0))
            }
            "appendbufline" | "append" => {
                let lnum = if name == "append" { n(0) } else { n(1) };
                let text_idx = if name == "append" { 1 } else { 2 };
                let text = match args.get(text_idx) {
                    Some(Value::List(l)) => l
                        .borrow()
                        .iter()
                        .map(Value::to_string_value)
                        .collect::<Vec<_>>(),
                    Some(v) => vec![v.to_string_value()],
                    None => Vec::new(),
                };
                let at = (lnum.max(0) as usize).min(ed.line_count());
                ed.replace_lines(at, at, text)?;
                Ok(Value::Number(0))
            }
            "getbufvar" | "setbufvar" => {
                let bname = s(1);
                if name == "getbufvar" {
                    let default = args.get(2).cloned().unwrap_or(Value::Null);
                    let hit = state
                        .bufvars
                        .get(&ed.current_bufnr())
                        .and_then(|m| m.get(&bname))
                        .cloned();
                    Ok(hit.unwrap_or(default))
                } else {
                    let value = args.get(2).cloned().unwrap_or(Value::Null);
                    state
                        .bufvars
                        .entry(ed.current_bufnr())
                        .or_default()
                        .insert(bname, value);
                    Ok(Value::Number(0))
                }
            }
            "bufexists" | "bufadd" | "bufname" | "bufnr" | "buflisted" | "bufloaded"
            | "buffer_exists" | "buffer_name" | "buffer_number" => {
                let target = args.first().cloned().unwrap_or(Value::Null);
                match name {
                    "bufexists" | "buffer_exists" => {
                        let found = match target {
                            Value::Number(id) => ed.all_buffers().iter().any(|b| b.id == id as i32),
                            Value::String(text) => {
                                ed.all_buffers().iter().any(|b| b.display_name() == text)
                            }
                            _ => false,
                        };
                        Ok(Value::Number(i64::from(found)))
                    }
                    "bufadd" => {
                        let path = PathBuf::from(target.to_string_value());
                        let id = ed.new_buffer();
                        if let Ok(buf) = ed.load_buffer_file(id, &path) {
                            ed.replace_buffer(id, buf)?;
                        }
                        Ok(Value::Number(id as i64))
                    }
                    "bufname" | "buffer_name" => {
                        let short = |b: &rvim_buffer::Buffer| -> String {
                            b.name
                                .as_ref()
                                .map(|p| shorten_path(p, &ed.cwd))
                                .unwrap_or_default()
                        };
                        let hit = match target {
                            Value::Number(id) => ed
                                .all_buffers()
                                .iter()
                                .find(|b| b.id == id as i32)
                                .map(short)
                                .unwrap_or_default(),
                            _ => short(ed.current_buffer()),
                        };
                        Ok(Value::String(hit))
                    }
                    "bufnr" | "buffer_number" => {
                        let text = target.to_string_value();
                        let short = |b: &rvim_buffer::Buffer| -> String {
                            b.name
                                .as_ref()
                                .map(|p| shorten_path(p, &ed.cwd))
                                .unwrap_or_default()
                        };
                        let hit = ed
                            .all_buffers()
                            .iter()
                            .find(|b| {
                                short(b) == text
                                    || b.display_name() == text
                                    || b.id.to_string() == text
                            })
                            .map(|b| b.id as i64)
                            .unwrap_or_else(|| ed.current_bufnr() as i64);
                        Ok(Value::Number(hit))
                    }
                    _ => Ok(Value::Number(i64::from(ed.current_buffer().is_changed()))),
                }
            }
            "changenr" => {
                let count = ed
                    .history_for(ed.current_bufnr())
                    .map(|h| h.change_count())
                    .unwrap_or(0);
                Ok(Value::Number(count as i64))
            }
            "cindent" => Ok(Value::Number(rvim_editor::indent::cindent(ed, n(0), n(1)))),
            "getcurpos" => {
                let pos = ed.cursor();
                Ok(Value::list(vec![
                    Value::Number(0),
                    Value::Number(pos.line as i64 + 1),
                    Value::Number(pos.col as i64 + 1),
                    Value::Number(0),
                    Value::Number(pos.col as i64 + 1),
                ]))
            }
            "getpos" => {
                let spec = s(0);
                let pos = if let Some(mark) = spec.strip_prefix('\'') {
                    let name = mark.chars().next().unwrap_or('"');
                    ed.marks.resolve(ed.current_bufnr(), name)
                } else {
                    Some(ed.cursor())
                };
                match pos {
                    Some(pos) => Ok(Value::list(vec![
                        Value::Number(0),
                        Value::Number(pos.line as i64 + 1),
                        Value::Number(pos.col as i64 + 1),
                        Value::Number(0),
                    ])),
                    None => Ok(Value::list(vec![
                        Value::Number(0),
                        Value::Number(0),
                        Value::Number(0),
                        Value::Number(0),
                    ])),
                }
            }
            "setpos" => {
                let list = match args.get(1) {
                    Some(Value::List(l)) => l.borrow().clone(),
                    _ => return Ok(Value::Number(-1)),
                };
                let lnum = list.get(1).map(Value::to_number).unwrap_or(0);
                let col = list.get(2).map(Value::to_number).unwrap_or(0);
                let spec = s(0);
                let pos = rvim_core::Pos::new(
                    lnum.saturating_sub(1).max(0) as usize,
                    col.saturating_sub(1).max(0) as usize,
                );
                if spec == "." {
                    ed.set_cursor(pos);
                } else if let Some(mark) = spec.strip_prefix('\'') {
                    let name = mark.chars().next().unwrap_or('"');
                    let bufnr = ed.current_bufnr();
                    ed.marks.set_local(bufnr, name, pos);
                }
                Ok(Value::Number(0))
            }
            "getreg" | "getregtype" => {
                let reg_name = s(0).chars().next().unwrap_or('"');
                // An unset named register is empty in Vim; it must not fall
                // back to the unnamed register (register.c get_reg_contents()).
                let reg = ed.registers.get(reg_name);
                match name {
                    "getreg" => Ok(Value::String(
                        reg.map(|r| r.text.clone()).unwrap_or_default(),
                    )),
                    _ => Ok(Value::String(
                        reg.map(|r| match r.reg_type {
                            rvim_editor::RegType::LineWise => "V",
                            _ => "v",
                        })
                        .unwrap_or("v")
                        .to_string(),
                    )),
                }
            }
            "setreg" => {
                let reg = s(0).chars().next().unwrap_or('"');
                let raw = args.get(1).cloned().unwrap_or(Value::Null);
                let (text, is_list) = match &raw {
                    Value::List(l) => (
                        l.borrow()
                            .iter()
                            .map(Value::to_string_value)
                            .collect::<Vec<_>>()
                            .join("\n"),
                        true,
                    ),
                    other => (other.to_string_value(), false),
                };
                let linewise = s(2).starts_with('V') || s(2).starts_with('l') || is_list;
                let text = if linewise && !text.ends_with('\n') {
                    format!("{text}\n")
                } else {
                    text
                };
                let reg_value = if linewise {
                    rvim_editor::Register::linewise(text)
                } else {
                    rvim_editor::Register::charwise(text)
                };
                ed.registers.put(reg, reg_value);
                Ok(Value::Number(0))
            }
            "getpid" => Ok(Value::Number(std::process::id() as i64)),
            "byte2line" => {
                let byte = n(0).max(1) as usize;
                let mut offset = 0usize;
                for (i, line) in ed.current_buffer().lines().iter().enumerate() {
                    let len = line.as_str().len() + 1; // include the newline
                    if byte <= offset + len {
                        return Ok(Value::Number(i as i64 + 1));
                    }
                    offset += len;
                }
                Ok(Value::Number(-1))
            }
            "line2byte" => {
                let lnum = n(0).max(1) as usize;
                let mut offset = 0usize;
                for (i, line) in ed.current_buffer().lines().iter().enumerate() {
                    if i + 1 == lnum {
                        return Ok(Value::Number(offset as i64 + 1));
                    }
                    offset += line.as_str().len() + 1;
                }
                Ok(Value::Number(-1))
            }
            "getbufinfo" => {
                let mut info = rvim_viml::VimDict::new();
                info.insert("bufnr", Value::Number(ed.current_bufnr() as i64));
                info.insert("name", Value::String(ed.current_buffer().display_name()));
                info.insert("lnum", Value::Number(ed.cursor().line as i64 + 1));
                info.insert("linecount", Value::Number(ed.line_count() as i64));
                info.insert(
                    "changed",
                    Value::Number(i64::from(ed.current_buffer().is_changed())),
                );
                info.insert("listed", Value::Number(1));
                info.insert("loaded", Value::Number(1));
                Ok(Value::list(vec![Value::dict(info)]))
            }
            "hostname" => Ok(Value::String(
                std::env::var("COMPUTERNAME")
                    .or_else(|_| std::env::var("HOSTNAME"))
                    .unwrap_or_default(),
            )),
            "visualmode" => Ok(Value::String(String::new())),
            _ => Ok(Value::Number(0)),
        }
    })())
}

/// Expand Vim special file names and modifiers: `%`, `#`, `~`, `$ENV`,
/// `<sfile>`, `<amatch>`, `<afile>`, `<cword>`, `<cWORD>`, `<cfile>`, `<slnum>`.
/// Mirrors `expand_special()` in ex_getln.c / `eval_fname_script()`.
pub fn expand_special(ed: &mut Editor, state: &ScriptState, text: &str) -> String {
    // Split off a leading special item from its modifiers: "%:p:h", "~:h".
    let (base, mods) = split_expand_modifiers(text);
    match base {
        "%" => {
            let name = ed.current_buffer().name.clone();
            match name {
                Some(path) => {
                    let shown = shorten_path(&path, &ed.cwd);
                    fnamemodify(&shown, mods, &ed.cwd)
                }
                None => String::new(),
            }
        }
        // "#" is the alternate buffer name; rvim has no alternate-buffer ring
        // yet, so this is empty as in Vim when there is none.
        "#" => String::new(),
        _ => {
            let mut out = text.to_string();
            if out.contains("<sfile>") || out.contains("<amatch>") || out.contains("<slnum>") {
                let sfile = state
                    .script_files
                    .last()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                out = out
                    .replace("<sfile>", &sfile)
                    .replace("<amatch>", &state.amatch)
                    .replace("<slnum>", "0");
            }
            if out.contains("<afile>") {
                out = out.replace("<afile>", &state.amatch);
            }
            if out.contains("<cword>") {
                let word = ed.current_line();
                out = out.replace("<cword>", word.trim());
            }
            if out.contains("<cWORD>") {
                let word = ed.current_line();
                out = out.replace("<cWORD>", word.trim());
            }
            if out.contains("<cfile>") {
                let word = ed.current_line();
                let file = word.split_whitespace().next().unwrap_or("").to_string();
                out = out.replace("<cfile>", &file);
            }
            if out.starts_with('~') {
                out = expand_home(&out);
                if !mods.is_empty() {
                    out = fnamemodify(&out, mods, &ed.cwd);
                }
            } else if out.starts_with('$') {
                out = expand_env_vars(&out);
            }
            out
        }
    }
}

/// Split `"%:p:h"` into (`"%"`, `"p:h"`); `"plain"` yields (`"plain"`, `""`).
fn split_expand_modifiers(text: &str) -> (&str, &str) {
    let specials = [
        "%", "#", "<sfile>", "<amatch>", "<afile>", "<slnum>", "<cword>", "<cWORD>", "<cfile>",
    ];
    for special in specials {
        if let Some(rest) = text.strip_prefix(special) {
            if rest.is_empty() {
                return (special, "");
            }
            if let Some(mods) = rest.strip_prefix(':') {
                return (special, mods);
            }
        }
    }
    (text, "")
}

/// Make `path` relative to `cwd` when it lives underneath it, the way Vim's
/// `b_fname` (short file name) is shown by `bufname('%')`/`expand('%')`.
pub fn shorten_path(path: &Path, cwd: &Path) -> String {
    if path.is_absolute() {
        if let Ok(rel) = path.strip_prefix(cwd) {
            if !rel.as_os_str().is_empty() {
                return rel.display().to_string();
            }
        }
    }
    path.display().to_string()
}

/// Expand a leading `~` to the user's home directory.
fn expand_home(text: &str) -> String {
    let rest = text.strip_prefix('~').unwrap_or(text);
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    if home.is_empty() {
        return text.to_string();
    }
    format!("{home}{rest}")
}

/// Expand `$VAR` / `${VAR}` references; unknown names are left untouched
/// (`expand_env_esc()` in os.c).
fn expand_env_vars(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        let (name, next) = if i < chars.len() && chars[i] == '{' {
            let mut j = i + 1;
            let start = j;
            while j < chars.len() && chars[j] != '}' {
                j += 1;
            }
            (chars[start..j].iter().collect::<String>(), j + 1)
        } else {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            (chars[start..i].iter().collect::<String>(), i)
        };
        if name.is_empty() {
            out.push('$');
            i = next;
            continue;
        }
        match std::env::var(&name) {
            Ok(value) => out.push_str(&value),
            Err(_) => {
                // Keep the reference as-is.
                out.push('$');
                out.push_str(&name);
            }
        }
        i = next;
    }
    out
}

fn fnamemodify(path: &str, mods: &str, cwd: &Path) -> String {
    // Vim works on the path text, not on OS path objects: keep separators as
    // the caller wrote them as much as possible.
    let mut text = path.to_string();
    for m in mods.chars() {
        match m {
            'p' => {
                let p = PathBuf::from(&text);
                if !p.is_absolute() {
                    text = cwd.join(&p).display().to_string();
                }
            }
            'h' => {
                // Remove the last path component; a trailing separator means
                // "the path itself" (vim's :h never returns "" for "a/").
                let trimmed = text.trim_end_matches(['/', '\\']);
                text = match trimmed.rfind(['/', '\\']) {
                    Some(0) => "/".to_string(),
                    Some(pos) => trimmed[..pos].to_string(),
                    None => ".".to_string(),
                };
            }
            't' => {
                // A trailing separator makes the tail empty (vim: "a/b/" -> "").
                if text.ends_with('/') || text.ends_with('\\') {
                    text = String::new();
                } else {
                    text = match text.rfind(['/', '\\']) {
                        Some(pos) => text[pos + 1..].to_string(),
                        None => text,
                    };
                }
            }
            'r' => {
                if let Some(pos) = text.rfind('.') {
                    let slash = text.rfind(['/', '\\']).map(|p| p + 1).unwrap_or(0);
                    if pos > slash {
                        text.truncate(pos);
                    }
                }
            }
            'e' => {
                text = match text.rfind('.') {
                    Some(pos) => {
                        let slash = text.rfind(['/', '\\']).map(|p| p + 1).unwrap_or(0);
                        if pos > slash {
                            text[pos + 1..].to_string()
                        } else {
                            String::new()
                        }
                    }
                    None => String::new(),
                };
            }
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers_viml::cmd_execute;
    use crate::parser::parse_line;

    /// Execute a VimL source string against a fresh editor/context.
    fn exec_source(src: &str) -> (Editor, Ctx) {
        let mut ed = Editor::new();
        let mut ctx = Ctx::new();
        let lines: Vec<String> = src.lines().map(str::to_string).collect();
        let stmts = parse_script_lines(&lines, 0).expect("script parses");
        let _ = exec_stmts(&mut ed, &mut ctx.env, &mut ctx.state, &stmts);
        (ed, ctx)
    }

    fn var(ctx: &Ctx, name: &str) -> Value {
        ctx.env
            .get_var(name)
            .unwrap_or_else(|| panic!("{name} missing"))
    }

    const REC: &str = "function! Rec(n)\n  if a:n <= 0\n    return 0\n  endif\n  return 1 + Rec(a:n-1)\nendfunction\n";

    #[test]
    fn varargs_are_only_the_extra_arguments() {
        // userfunc.c:2729-2813: a:1..a:N address only the "..." arguments.
        let (_ed, ctx) = exec_source(concat!(
            "function! V(args, ...)\n",
            "  return [a:0, a:000, a:1, a:args]\n",
            "endfunction\n",
            "let g:r = V(7, 1, 2)\n",
        ));
        assert_eq!(var(&ctx, "g:r").to_string_value(), "[2, [1, 2], 1, 7]");
    }

    #[test]
    fn numeric_arg_without_varargs_is_undefined() {
        let (_ed, ctx) = exec_source(concat!(
            "function! W(x)\n",
            "  return a:1\n",
            "endfunction\n",
            "try\n",
            "  let g:w = W(5)\n",
            "catch\n",
            "  let g:e = v:exception\n",
            "endtry\n",
        ));
        assert_eq!(
            var(&ctx, "g:e").to_string_value(),
            "Vim(return):E121: Undefined variable: a:1"
        );
    }

    #[test]
    fn thrown_value_crosses_function_calls_unchanged() {
        // eval.txt:2726-2728: the exception value is not re-wrapped as E605.
        let (_ed, ctx) = exec_source(concat!(
            "function! Thrower()\n",
            "  throw 'myerr'\n",
            "endfunction\n",
            "function! Deep()\n",
            "  call Thrower()\n",
            "  return 0\n",
            "endfunction\n",
            "try\n",
            "  call Deep()\n",
            "catch /^myerr$/\n",
            "  let g:e = v:exception\n",
            "endtry\n",
        ));
        assert_eq!(var(&ctx, "g:e").to_string_value(), "myerr");
    }

    #[test]
    fn error_inside_called_function_is_catchable() {
        // ex_eval.c:144: while a try conditional is active an error becomes a
        // Vim({command}): exception for the caller.
        let (_ed, ctx) = exec_source(concat!(
            "function! ErrF()\n",
            "  let x = nosuchvar_xyz\n",
            "  return 42\n",
            "endfunction\n",
            "try\n",
            "  let g:e = ErrF()\n",
            "catch\n",
            "  let g:msg = v:exception\n",
            "endtry\n",
        ));
        assert_eq!(
            var(&ctx, "g:msg").to_string_value(),
            "Vim(let):E121: Undefined variable: nosuchvar_xyz"
        );
    }

    #[test]
    fn abort_function_stops_at_first_error_and_returns_minus_one() {
        let (_ed, ctx) = exec_source(concat!(
            "function! Ab() abort\n",
            "  let x = nosuch_abort\n",
            "  return 42\n",
            "endfunction\n",
            "let g:r = Ab()\n",
            "function! NoAb()\n",
            "  let x = nosuch_plain\n",
            "  return 42\n",
            "endfunction\n",
            "let g:r2 = NoAb()\n",
        ));
        assert_eq!(var(&ctx, "g:r").to_number(), -1);
        assert_eq!(var(&ctx, "g:r2").to_number(), 42);
    }

    #[test]
    fn return_in_finally_discards_pending_exception() {
        // eval.txt:4000-4008.
        let (_ed, ctx) = exec_source(concat!(
            "function! FinRet()\n",
            "  try\n",
            "    throw 'x'\n",
            "  finally\n",
            "    return 'finret'\n",
            "  endtry\n",
            "endfunction\n",
            "try\n",
            "  let g:r = FinRet()\n",
            "catch\n",
            "  let g:e = v:exception\n",
            "endtry\n",
        ));
        assert_eq!(var(&ctx, "g:r").to_string_value(), "finret");
        assert_eq!(ctx.env.get_var("g:e"), None);
    }

    #[test]
    fn exception_from_finally_replaces_pending_one() {
        let (_ed, ctx) = exec_source(concat!(
            "function! FinThrow()\n",
            "  try\n",
            "    throw 'orig'\n",
            "  finally\n",
            "    throw 'fromfin'\n",
            "  endtry\n",
            "endfunction\n",
            "try\n",
            "  call FinThrow()\n",
            "catch\n",
            "  let g:e = v:exception\n",
            "endtry\n",
        ));
        assert_eq!(var(&ctx, "g:e").to_string_value(), "fromfin");
    }

    #[test]
    fn throw_inside_source_is_catchable_with_original_value() {
        // handlers_script::source_file() turns an uncaught throw into an
        // aborting E605; script.rs restores the original value so `:catch`
        // sees "boom" (ex_eval.c:1537-1567 need_rethrow).
        let inner = std::env::temp_dir().join(format!("rvim_src_exc_{}.vim", std::process::id()));
        std::fs::write(&inner, "throw 'boom'\n").unwrap();
        let inner_ex = inner.display().to_string().replace('\\', "/");
        let (_ed, ctx) = exec_source(&format!(
            "try\n  source {inner_ex}\ncatch /^boom$/\n  let g:e = v:exception\nendtry\n"
        ));
        assert_eq!(var(&ctx, "g:e").to_string_value(), "boom");
        // Without a :catch the throwing source aborts the sourcing script.
        let (_ed, ctx) = exec_source(&format!("source {inner_ex}\nlet g:after = 1\n"));
        let _ = std::fs::remove_file(&inner);
        assert_eq!(ctx.env.get_var("g:after"), None);
    }
    #[test]
    fn deep_recursion_raises_e132_on_a_large_stack() {
        // The CLI runs the session on a 64 MiB worker thread
        // (run_with_deep_stack): native frames are too large for the 1 MiB
        // main-thread stack, so E132 must be reported from the depth guard
        // (userfunc.c:2536-2544) instead of a stack-overflow abort.
        let (ok, caught) = run_with_deep_stack(|| {
            let (_ed, ctx) = exec_source(&format!("{REC}let g:r = Rec(30)\n"));
            let ok = var(&ctx, "g:r").to_number();
            let (_ed, ctx) = exec_source(&format!(
                "{REC}try\n  let g:r = Rec(150)\ncatch\n  let g:e = v:exception\nendtry\n"
            ));
            let caught = var(&ctx, "g:e").to_string_value();
            (ok, caught)
        });
        assert_eq!(ok, 30);
        assert_eq!(
            caught,
            "Vim(return):E132: Function call depth is higher than 'maxfuncdepth'"
        );
    }

    #[test]
    fn changedtick_tracks_edits() {
        // b:changedtick must advance on every change (change.c CHANGEDTICK);
        // the absolute start value is owned by rvim-buffer.
        let mut ed = Editor::new();
        let t0 = builtin_buffer_var(&ed, "changedtick").unwrap().to_number();
        ed.set_line_text(0, "one".to_string()).unwrap();
        let t1 = builtin_buffer_var(&ed, "changedtick").unwrap().to_number();
        assert!(t1 > t0, "t1={t1} t0={t0}");
    }

    #[test]
    fn modified_option_reflects_buffer_changed_flag() {
        let mut ed = Editor::new();
        let mut state = ScriptState::default();
        {
            let mut host = make_host(&mut ed, &mut state);
            assert_eq!(host.option_get("modified").unwrap().to_number(), 0);
        }
        ed.set_line_text(0, "x".to_string()).unwrap();
        {
            let mut host = make_host(&mut ed, &mut state);
            assert_eq!(host.option_get("modified").unwrap().to_number(), 1);
            host.option_set("modified", &Value::Number(0))
                .unwrap()
                .unwrap();
            assert_eq!(host.option_get("modified").unwrap().to_number(), 0);
        }
    }

    #[test]
    fn execute_joins_separate_expressions_with_spaces() {
        // eval.c:6699-6757: `execute 'let g:x =' 1 + 2` runs `let g:x = 3`.
        let mut ed = Editor::new();
        let mut env = Env::new();
        let mut state = ScriptState::default();
        let p = parse_line("execute 'let g:x =' 1 + 2", 1).unwrap();
        cmd_execute(&mut ed, &mut env, &mut state, &p).unwrap();
        assert_eq!(env.get_var("g:x").unwrap().to_number(), 3);
    }
}
