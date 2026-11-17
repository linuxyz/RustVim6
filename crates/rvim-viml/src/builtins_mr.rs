//! Builtin functions, group m-r (M3 coverage batch).
//!
//! See `docs/m3_builtins_plan.md` for the candidate list, semantics rules and
//! the skip list (subsystems that are not built yet).
//!
//! Implemented here: `mapnew`, `matchfuzzy`, `matchfuzzypos`, `matchlist`,
//! `matchstrpos`, `mkdir`, `nextnonblank`, `pathshorten`, `prevnonblank`,
//! `pum_getpos`, `pumvisible`, `rand`, `readblob`, `readdir`, `readdirex`,
//! `readfile`, `reduce`, `reg_executing`, `reg_recording`, `reltime`,
//! `reltimefloat`, `reltimestr`, `rename`, `resolve`.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rvim_core::{VimError, VimResult};

use crate::env::{Env, Host};
use crate::eval::{call_value, eval};
use crate::expr::parse_expression;
use crate::value::{Value, ValueResult, VimDict};

/// Names implemented in this module (coverage source of truth).
pub const FUNCS: &[&str] = &[
    "mapnew",
    "matchfuzzy",
    "matchfuzzypos",
    "matchlist",
    "matchstrpos",
    "mkdir",
    "nextnonblank",
    "pathshorten",
    "prevnonblank",
    "pum_getpos",
    "pumvisible",
    "rand",
    "readblob",
    "readdir",
    "readdirex",
    "readfile",
    "reduce",
    "reg_executing",
    "reg_recording",
    "reltime",
    "reltimefloat",
    "reltimestr",
    "rename",
    "resolve",
];

fn err(code: &'static str, msg: impl Into<String>) -> VimError {
    VimError::new(code, msg.into())
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Null)
}

fn sarg(args: &[Value], i: usize) -> String {
    arg(args, i).to_string_value()
}

fn narg(args: &[Value], i: usize) -> i64 {
    arg(args, i).to_number()
}

/// Dispatch a call; returns None when the name is not handled here.
pub fn call(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> Option<ValueResult> {
    if !FUNCS.contains(&name) {
        return None;
    }
    Some((|| {
        crate::builtins::check_arity(name, args)?;
        dispatch(name, args, env, host)
    })())
}

fn dispatch(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    match name {
        "mapnew" => mapnew(args, env, host),
        "matchfuzzy" => fuzzymatch(args, false, env, host),
        "matchfuzzypos" => fuzzymatch(args, true, env, host),
        "matchlist" => find_some_match(args, MatchKind::List),
        "matchstrpos" => find_some_match(args, MatchKind::Pos),
        "mkdir" => mkdir(args),
        "nextnonblank" => nonblank(args, true, env, host),
        "prevnonblank" => nonblank(args, false, env, host),
        "pathshorten" => pathshorten(args),
        "pum_getpos" => Ok(Value::dict(VimDict::new())),
        "pumvisible" => Ok(Value::Number(0)),
        "rand" => rand_fn(args),
        "readblob" => readblob(args),
        "readdir" => readdir(args, false, env, host),
        "readdirex" => readdir(args, true, env, host),
        "readfile" => readfile(args),
        "reduce" => reduce(args, env, host),
        "reg_executing" | "reg_recording" => Ok(Value::string("")),
        "reltime" => reltime(args),
        "reltimefloat" => reltimefloat(args),
        "reltimestr" => reltimestr(args),
        "rename" => rename_fn(args),
        "resolve" => resolve_fn(args),
        other => Err(err(
            "E117",
            format!("builtin {other} is listed but not dispatched"),
        )),
    }
}

// ---------------------------------------------------------------- mapnew()

/// `mapnew({list}, {expr})` / `mapnew({dict}, {expr})`: like `map()` but the
/// original container is left untouched (`list_filter_map` / FILTERMAP_MAPNEW).
fn mapnew(args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    let target = arg(args, 0);
    let func = arg(args, 1);
    if !matches!(func, Value::String(_) | Value::Funcref(_)) {
        return Err(err(
            "E731",
            "Second argument of mapnew() must be a Funcref or a string",
        ));
    }
    match &target {
        Value::List(list) => {
            let items = list.borrow().clone();
            let mut out = Vec::with_capacity(items.len());
            for (idx, item) in items.iter().enumerate() {
                let mapped = apply_mapper(&func, item, Value::Number(idx as i64), env, host)?;
                out.push(mapped);
            }
            Ok(Value::list(out))
        }
        Value::Dict(dict) => {
            let entries: Vec<(String, Value)> = dict
                .borrow()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut out = VimDict::new();
            for (key, value) in entries {
                let mapped = apply_mapper(&func, &value, Value::String(key.clone()), env, host)?;
                out.insert(key, mapped);
            }
            Ok(Value::dict(out))
        }
        other => Err(err(
            "E896",
            format!(
                "Argument of mapnew() must be a List or Dictionary, not {}",
                other.type_name()
            ),
        )),
    }
}

/// Evaluate the map callback for one item (mirrors `filter_map_one()`).
fn apply_mapper(
    func: &Value,
    value: &Value,
    key: Value,
    env: &mut Env,
    host: &mut dyn Host,
) -> ValueResult {
    match func {
        Value::String(src) => {
            let saved_val = env.vvars.get("val").cloned();
            let saved_key = env.vvars.get("key").cloned();
            env.vvars.insert("val".to_string(), value.clone());
            env.vvars.insert("key".to_string(), key);
            let result = parse_expression(src).and_then(|expr| eval(&expr, env, host));
            match saved_val {
                Some(v) => {
                    env.vvars.insert("val".to_string(), v);
                }
                None => {
                    env.vvars.remove("val");
                }
            }
            match saved_key {
                Some(v) => {
                    env.vvars.insert("key".to_string(), v);
                }
                None => {
                    env.vvars.remove("key");
                }
            }
            result
        }
        other => call_value(other, vec![key, value.clone()], env, host),
    }
}

// ------------------------------------------------------- matchlist()/matchstrpos()

#[derive(Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    /// `matchlist()`: `[matched, \(group)...]`, empty list when no match.
    List,
    /// `matchstrpos()`: `[matched, start, end]` (plus the index for a List).
    Pos,
}

/// Shared implementation of `matchlist()` and `matchstrpos()` (reference:
/// evalfunc.c `find_some_match()` with MATCH_LIST / MATCH_POS).
fn find_some_match(args: &[Value], kind: MatchKind) -> ValueResult {
    let pat = sarg(args, 1);
    let start = if args.len() > 2 {
        narg(args, 2).max(0)
    } else {
        0
    };
    let nth = if args.len() > 3 {
        narg(args, 3).max(1) as usize
    } else {
        1
    };
    match arg(args, 0) {
        Value::String(text) => {
            let byte_start = {
                let mut b = start as usize;
                if b > text.len() {
                    b = text.len();
                }
                while b < text.len() && !text.is_char_boundary(b) {
                    b += 1;
                }
                b
            };
            let start_char = text[..byte_start].chars().count();
            let chars: Vec<char> = text.chars().collect();
            match find_nth_match(&pat, &chars, start_char, nth) {
                Some(m) => Ok(pos_or_list_result(&chars, 0, None, &m, kind)),
                None => Ok(no_match_result(kind, None)),
            }
        }
        Value::List(list) => {
            let items = list.borrow().clone();
            let mut first = start as usize;
            if first > items.len() {
                first = items.len();
            }
            let mut remaining = nth;
            for (idx, item) in items.iter().enumerate().skip(first) {
                let text = item.to_string_value();
                let chars: Vec<char> = text.chars().collect();
                if let Some(m) = find_nth_match(&pat, &chars, 0, 1) {
                    remaining -= 1;
                    if remaining == 0 {
                        return Ok(pos_or_list_result(&chars, 0, Some(idx), &m, kind));
                    }
                }
            }
            Ok(no_match_result(kind, Some(0)))
        }
        other => Err(err(
            "E714",
            format!(
                "First argument of matchlist()/matchstrpos() must be a String or List, not {}",
                other.type_name()
            ),
        )),
    }
}

fn pos_or_list_result(
    chars: &[char],
    base: usize,
    item_idx: Option<usize>,
    m: &ReMatch,
    kind: MatchKind,
) -> Value {
    let full = chars[m.start..m.end].iter().collect::<String>();
    match kind {
        MatchKind::List => {
            let mut out = Vec::with_capacity(10);
            for i in 0..10 {
                let text = m
                    .caps
                    .get(i)
                    .and_then(|c| *c)
                    .map(|(s, e)| chars[s..e].iter().collect::<String>())
                    .unwrap_or_default();
                out.push(Value::String(text));
            }
            Value::list(out)
        }
        MatchKind::Pos => {
            let start_byte = base + byte_len(chars, m.start);
            let end_byte = base + byte_len(chars, m.end);
            let mut out = vec![Value::String(full)];
            if let Some(idx) = item_idx {
                out.push(Value::Number(idx as i64));
            }
            out.push(Value::Number(start_byte as i64));
            out.push(Value::Number(end_byte as i64));
            Value::list(out)
        }
    }
}

fn no_match_result(kind: MatchKind, item_idx: Option<usize>) -> Value {
    match kind {
        MatchKind::List => Value::list(Vec::new()),
        MatchKind::Pos => {
            let mut out = vec![Value::string("")];
            if item_idx.is_some() {
                out.push(Value::Number(-1));
            }
            out.push(Value::Number(-1));
            out.push(Value::Number(-1));
            Value::list(out)
        }
    }
}

fn byte_len(chars: &[char], char_idx: usize) -> usize {
    chars[..char_idx].iter().map(|c| c.len_utf8()).sum()
}

// ------------------------------------------------ tiny regex with () captures

// `regex_mvp` exposes only whole-match positions, but `matchlist()` must return
// the `\(...\)` sub-matches.  This is a small backtracking matcher that keeps
// capture spans; it supports the same "magic" subset needed by Vim scripts.

#[derive(Clone, Debug)]
enum ReClass {
    Char(char),
    Range(char, char),
    Digit,
    NotDigit,
    Word,
    NotWord,
    Space,
    NotSpace,
}

#[derive(Clone, Debug)]
enum ReNode {
    Char(char),
    Any,
    Class(Vec<ReClass>, bool),
    Start,
    End,
    Group(Vec<Vec<ReNode>>, usize),
    Repeat(Box<ReNode>, u32, u32),
}

struct ReParser {
    chars: Vec<char>,
    pos: usize,
    groups: usize,
}

impl ReParser {
    fn parse(pattern: &str) -> (Vec<Vec<ReNode>>, usize) {
        let mut parser = ReParser {
            chars: pattern.chars().collect(),
            pos: 0,
            groups: 0,
        };
        let alt = parser.alt();
        (alt, parser.groups)
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

    fn alt(&mut self) -> Vec<Vec<ReNode>> {
        let mut branches = vec![self.concat()];
        while self.peek() == Some('\\') && self.peek_at(1) == Some('|') {
            self.pos += 2;
            branches.push(self.concat());
        }
        branches
    }

    fn concat(&mut self) -> Vec<ReNode> {
        let mut nodes = Vec::new();
        loop {
            match (self.peek(), self.peek_at(1)) {
                (None, _) => break,
                (Some(')'), _) => break,
                (Some('\\'), Some('|')) | (Some('\\'), Some(')')) => break,
                _ => {}
            }
            let Some(atom) = self.atom() else {
                break;
            };
            nodes.push(self.quantifiers(atom));
        }
        nodes
    }

    fn atom(&mut self) -> Option<ReNode> {
        let c = self.bump()?;
        Some(match c {
            '.' => ReNode::Any,
            '^' => ReNode::Start,
            '$' => ReNode::End,
            '[' => self.class(),
            '\\' => match self.bump()? {
                '(' => {
                    self.groups += 1;
                    let idx = self.groups;
                    let branches = self.alt();
                    if self.peek() == Some('\\') && self.peek_at(1) == Some(')') {
                        self.pos += 2;
                    }
                    ReNode::Group(branches, idx)
                }
                'd' => ReNode::Class(vec![ReClass::Digit], false),
                'D' => ReNode::Class(vec![ReClass::NotDigit], false),
                'w' => ReNode::Class(vec![ReClass::Word], false),
                'W' => ReNode::Class(vec![ReClass::NotWord], false),
                's' => ReNode::Class(vec![ReClass::Space], false),
                'S' => ReNode::Class(vec![ReClass::NotSpace], false),
                'n' => ReNode::Char('\n'),
                't' => ReNode::Char('\t'),
                'r' => ReNode::Char('\r'),
                other => ReNode::Char(other),
            },
            other => ReNode::Char(other),
        })
    }

    fn class(&mut self) -> ReNode {
        let mut negated = false;
        if self.peek() == Some('^') {
            self.pos += 1;
            negated = true;
        }
        let mut items = Vec::new();
        let mut first = true;
        while let Some(c) = self.bump() {
            if c == ']' && !first {
                break;
            }
            first = false;
            if c == '\\' {
                match self.bump() {
                    Some('d') => items.push(ReClass::Digit),
                    Some('D') => items.push(ReClass::NotDigit),
                    Some('w') => items.push(ReClass::Word),
                    Some('W') => items.push(ReClass::NotWord),
                    Some('s') => items.push(ReClass::Space),
                    Some('S') => items.push(ReClass::NotSpace),
                    Some('n') => items.push(ReClass::Char('\n')),
                    Some('t') => items.push(ReClass::Char('\t')),
                    Some(other) => items.push(ReClass::Char(other)),
                    None => break,
                }
                continue;
            }
            // A range such as "a-z"; a leading/trailing '-' is a literal.
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|n| n != ']') {
                self.pos += 1;
                let Some(hi) = self.bump() else {
                    break;
                };
                items.push(ReClass::Range(c, hi));
                continue;
            }
            items.push(ReClass::Char(c));
        }
        ReNode::Class(items, negated)
    }

    fn quantifiers(&mut self, mut node: ReNode) -> ReNode {
        loop {
            match (self.peek(), self.peek_at(1)) {
                (Some('*'), _) => {
                    self.pos += 1;
                    node = ReNode::Repeat(Box::new(node), 0, u32::MAX);
                }
                (Some('\\'), Some('+')) => {
                    self.pos += 2;
                    node = ReNode::Repeat(Box::new(node), 1, u32::MAX);
                }
                (Some('\\'), Some('=')) | (Some('\\'), Some('?')) => {
                    self.pos += 2;
                    node = ReNode::Repeat(Box::new(node), 0, 1);
                }
                (Some('\\'), Some('{')) => {
                    let save = self.pos;
                    self.pos += 2;
                    if self.peek() == Some('-') {
                        // Non-greedy variant: treat as greedy.
                        self.pos += 1;
                    }
                    let mut min_text = String::new();
                    while let Some(c) = self.peek() {
                        if c.is_ascii_digit() {
                            min_text.push(c);
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    let min = min_text.parse::<u32>().unwrap_or(0);
                    let mut max = min;
                    if self.peek() == Some(',') {
                        self.pos += 1;
                        let mut max_text = String::new();
                        while let Some(c) = self.peek() {
                            if c.is_ascii_digit() {
                                max_text.push(c);
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                        max = if max_text.is_empty() {
                            u32::MAX
                        } else {
                            max_text.parse::<u32>().unwrap_or(min)
                        };
                    }
                    if self.peek() == Some('\\') && self.peek_at(1) == Some('}') {
                        self.pos += 2;
                        node = ReNode::Repeat(Box::new(node), min, max.max(min));
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
}

fn class_matches(items: &[ReClass], negated: bool, c: char) -> bool {
    let hit = items.iter().any(|item| match item {
        ReClass::Char(x) => *x == c,
        ReClass::Range(a, b) => *a <= c && c <= *b,
        ReClass::Digit => c.is_ascii_digit(),
        ReClass::NotDigit => !c.is_ascii_digit(),
        ReClass::Word => c.is_alphanumeric() || c == '_',
        ReClass::NotWord => !(c.is_alphanumeric() || c == '_'),
        ReClass::Space => c.is_whitespace(),
        ReClass::NotSpace => !c.is_whitespace(),
    });
    hit != negated
}

/// One successful match: character spans of the whole match and of the groups.
struct ReMatch {
    start: usize,
    end: usize,
    /// Index 0 is the whole match, `caps[i]` is `\(...\)` group `i`.
    caps: Vec<Option<(usize, usize)>>,
}

struct ReMatcher<'a> {
    text: &'a [char],
    search_start: usize,
    steps: usize,
    limit: usize,
}

/// Search `pat` in `chars` starting at character index `start`, returning the
/// leftmost match.
fn re_search(pattern: &str, chars: &[char], start: usize) -> Option<ReMatch> {
    let (alt, groups) = ReParser::parse(pattern);
    let begin = start.min(chars.len());
    for at in begin..=chars.len() {
        let mut caps: Vec<Option<(usize, usize)>> = vec![None; groups + 1];
        let mut matcher = ReMatcher {
            text: chars,
            search_start: at,
            steps: 0,
            limit: 200_000,
        };
        if let Some(end) = matcher.alt(&alt, at, &mut caps) {
            caps[0] = Some((at, end));
            return Some(ReMatch {
                start: at,
                end,
                caps,
            });
        }
        if matcher.steps >= matcher.limit {
            return None;
        }
    }
    None
}

struct Rep<'a> {
    inner: &'a ReNode,
    min: u32,
    max: u32,
    count: u32,
    rest: &'a [ReNode],
}

impl ReMatcher<'_> {
    fn alt(
        &mut self,
        branches: &[Vec<ReNode>],
        pos: usize,
        caps: &mut Vec<Option<(usize, usize)>>,
    ) -> Option<usize> {
        let saved = caps.clone();
        for branch in branches {
            *caps = saved.clone();
            if let Some(end) = self.seq(branch, pos, caps) {
                return Some(end);
            }
        }
        *caps = saved;
        None
    }

    fn seq(
        &mut self,
        nodes: &[ReNode],
        pos: usize,
        caps: &mut Vec<Option<(usize, usize)>>,
    ) -> Option<usize> {
        self.steps += 1;
        if self.steps > self.limit {
            return None;
        }
        let Some(first) = nodes.first() else {
            return Some(pos);
        };
        let rest = &nodes[1..];
        match first {
            ReNode::Group(branches, idx) => {
                let start = pos;
                let saved = caps.clone();
                for branch in branches {
                    *caps = saved.clone();
                    if let Some(mid) = self.seq(branch, pos, caps) {
                        let old = caps[*idx];
                        caps[*idx] = Some((start, mid));
                        if let Some(end) = self.seq(rest, mid, caps) {
                            return Some(end);
                        }
                        caps[*idx] = old;
                    }
                }
                *caps = saved;
                None
            }
            ReNode::Repeat(inner, min, max) => self.repeat(
                &Rep {
                    inner,
                    min: *min,
                    max: *max,
                    count: 0,
                    rest,
                },
                pos,
                caps,
            ),
            ReNode::Char(c) => {
                if self.text.get(pos).is_some_and(|t| t == c) {
                    self.seq(rest, pos + 1, caps)
                } else {
                    None
                }
            }
            ReNode::Any => {
                if pos < self.text.len() {
                    self.seq(rest, pos + 1, caps)
                } else {
                    None
                }
            }
            ReNode::Class(items, negated) => match self.text.get(pos) {
                Some(c) if class_matches(items, *negated, *c) => self.seq(rest, pos + 1, caps),
                _ => None,
            },
            ReNode::Start => {
                if pos == self.search_start {
                    self.seq(rest, pos, caps)
                } else {
                    None
                }
            }
            ReNode::End => {
                if pos == self.text.len() {
                    self.seq(rest, pos, caps)
                } else {
                    None
                }
            }
        }
    }

    fn repeat(
        &mut self,
        rep: &Rep<'_>,
        pos: usize,
        caps: &mut Vec<Option<(usize, usize)>>,
    ) -> Option<usize> {
        self.steps += 1;
        if self.steps > self.limit {
            return None;
        }
        if rep.count < rep.max {
            let saved = caps.clone();
            if let Some(next) = self.seq(std::slice::from_ref(rep.inner), pos, caps) {
                // Zero-width repetitions cannot make progress; stop growing.
                if next > pos {
                    let advanced = Rep {
                        inner: rep.inner,
                        min: rep.min,
                        max: rep.max,
                        count: rep.count + 1,
                        rest: rep.rest,
                    };
                    if let Some(end) = self.repeat(&advanced, next, caps) {
                        return Some(end);
                    }
                }
            }
            *caps = saved;
        }
        if rep.count >= rep.min {
            self.seq(rep.rest, pos, caps)
        } else {
            None
        }
    }
}

/// Find the `nth` match of `pat` in `chars` starting at character index `start`.
fn find_nth_match(pattern: &str, chars: &[char], start: usize, nth: usize) -> Option<ReMatch> {
    let mut cursor = start;
    let mut remaining = nth.max(1);
    loop {
        let m = re_search(pattern, chars, cursor)?;
        remaining -= 1;
        if remaining == 0 {
            return Some(m);
        }
        cursor = m.start + 1;
        if cursor > chars.len() {
            return None;
        }
    }
}

// ------------------------------------------------------------- matchfuzzy()

fn eq_ci(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// Vim's `fuzzy_match_compute_score()`.
fn fuzzy_score(text: &[char], positions: &[usize]) -> i64 {
    let mut score = 100i64;
    let leading = -5 * positions[0] as i64;
    score += leading.max(-15);
    let unmatched = text.len() as i64 - positions.len() as i64;
    score -= unmatched;
    for (i, &curr) in positions.iter().enumerate() {
        if i > 0 {
            let prev = positions[i - 1];
            if curr == prev + 1 {
                score += 40;
            } else {
                score -= 2 * (curr - prev) as i64;
            }
        }
        if curr > 0 {
            let neighbor = text[curr - 1];
            let current = text[curr];
            if neighbor.is_lowercase() && current.is_uppercase() {
                score += 30;
            }
            if neighbor == '/' || neighbor == '\\' {
                score += 30;
            } else if neighbor == ' ' || neighbor == '_' {
                score += 25;
            }
        } else {
            score += 15;
        }
    }
    score
}

/// Greedy best-effort fuzzy match of `pat` inside `text`, returning the score
/// and the character positions of the matched pattern characters.
fn fuzzy_positions(pat: &[char], text: &[char]) -> Option<(i64, Vec<usize>)> {
    if pat.is_empty() || pat.len() > text.len() {
        return None;
    }
    let mut best: Option<(i64, Vec<usize>)> = None;
    for start in 0..text.len() {
        if !eq_ci(text[start], pat[0]) {
            continue;
        }
        let mut positions = vec![start];
        let mut cursor = start + 1;
        let mut ok = true;
        for pc in &pat[1..] {
            let mut found = None;
            let mut i = cursor;
            while i < text.len() {
                if eq_ci(text[i], *pc) {
                    found = Some(i);
                    break;
                }
                i += 1;
            }
            match found {
                Some(i) => {
                    positions.push(i);
                    cursor = i + 1;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let score = fuzzy_score(text, &positions);
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, positions));
        }
    }
    best
}

fn fuzzy_match_text(pat: &[char], text: &[char], matchseq: bool) -> Option<(i64, Vec<usize>)> {
    if pat.is_empty() {
        return None;
    }
    if matchseq {
        return fuzzy_positions(pat, text);
    }
    let mut total = 0i64;
    let mut positions = Vec::new();
    for word in pat.split(|c| c.is_whitespace()).filter(|w| !w.is_empty()) {
        let (score, p) = fuzzy_positions(word, text)?;
        total += score;
        positions.extend(p);
    }
    Some((total, positions))
}

fn fuzzymatch(
    args: &[Value],
    retmatchpos: bool,
    env: &mut Env,
    host: &mut dyn Host,
) -> ValueResult {
    let fname = if retmatchpos {
        "matchfuzzypos()"
    } else {
        "matchfuzzy()"
    };
    let Value::List(list) = arg(args, 0) else {
        return Err(err(
            "E714",
            format!("First argument of {fname} must be a List"),
        ));
    };
    let pattern = sarg(args, 1);
    let mut matchseq = false;
    let mut limit = 0i64;
    let mut key: Option<String> = None;
    let mut text_cb: Option<Value> = None;
    if args.len() > 2 {
        let Value::Dict(opts) = arg(args, 2) else {
            return Err(err("E715", "Dictionary required"));
        };
        let (has_key, key_value) = {
            let d = opts.borrow();
            match d.get("key") {
                Some(v) => (true, Some(v.to_string_value())),
                None => (false, None),
            }
        };
        if has_key {
            key = Some(key_value.unwrap_or_default());
        } else {
            let cb_value = opts.borrow().get("text_cb").cloned();
            if let Some(v) = cb_value {
                if !matches!(v, Value::Funcref(_) | Value::String(_)) {
                    return Err(err(
                        "E475",
                        format!(
                            "Invalid value for argument \"text_cb\": {}",
                            v.to_string_value()
                        ),
                    ));
                }
                text_cb = Some(v);
            }
        }
        if let Some(v) = opts.borrow().get("limit") {
            if !matches!(v, Value::Number(_)) {
                return Err(err(
                    "E475",
                    format!(
                        "Invalid value for argument \"limit\": {}",
                        v.to_string_value()
                    ),
                ));
            }
            limit = v.to_number();
        }
        matchseq = opts.borrow().get("matchseq").is_some();
    }
    let pat_chars: Vec<char> = pattern.chars().collect();
    let items = list.borrow().clone();
    let mut scored: Vec<(i64, usize, Value, Vec<usize>)> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        if limit > 0 && scored.len() as i64 >= limit {
            break;
        }
        let text = match item {
            Value::String(s) => Some(s.clone()),
            Value::Dict(d) => {
                if let Some(k) = &key {
                    Some(
                        d.borrow()
                            .get(k)
                            .map(Value::to_string_value)
                            .unwrap_or_default(),
                    )
                } else if let Some(cb) = &text_cb {
                    match call_value(cb, vec![item.clone()], env, host)? {
                        Value::String(s) => Some(s),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(text) = text else { continue };
        let chars: Vec<char> = text.chars().collect();
        let Some((score, positions)) = fuzzy_match_text(&pat_chars, &chars, matchseq) else {
            continue;
        };
        scored.push((score, idx, item.clone(), positions));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    if !retmatchpos {
        return Ok(Value::list(
            scored.into_iter().map(|(_, _, v, _)| v).collect(),
        ));
    }
    let matches = Value::list(scored.iter().map(|(_, _, v, _)| v.clone()).collect());
    let positions = Value::list(
        scored
            .iter()
            .map(|(_, _, _, p)| Value::list(p.iter().map(|&x| Value::Number(x as i64)).collect()))
            .collect(),
    );
    let scores = Value::list(
        scored
            .iter()
            .map(|(s, _, _, _)| Value::Number(*s))
            .collect(),
    );
    Ok(Value::list(vec![matches, positions, scores]))
}

// -------------------------------------------------------------- nonblank()

fn host_line_count(host: &mut dyn Host, env: &mut Env) -> Option<i64> {
    host.call_host_builtin("line", vec![Value::string("$")], env)
        .and_then(|r| r.ok())
        .map(|v| v.to_number())
}

fn host_getline(host: &mut dyn Host, env: &mut Env, lnum: i64) -> Option<String> {
    host.call_host_builtin("getline", vec![Value::Number(lnum)], env)
        .and_then(|r| r.ok())
        .map(|v| v.to_string_value())
}

/// `nextnonblank({lnum})` / `prevnonblank({lnum})` (evalfunc.c).
fn nonblank(args: &[Value], next: bool, env: &mut Env, host: &mut dyn Host) -> ValueResult {
    let lnum = narg(args, 0);
    let Some(count) = host_line_count(host, env) else {
        // No buffer access through this host: there are no lines.
        return Ok(Value::Number(0));
    };
    if next {
        if lnum < 0 {
            return Ok(Value::Number(0));
        }
        let mut current = if lnum == 0 { 1 } else { lnum };
        loop {
            if current > count {
                return Ok(Value::Number(0));
            }
            let line = host_getline(host, env, current).unwrap_or_default();
            if !line.trim_start().is_empty() {
                return Ok(Value::Number(current));
            }
            current += 1;
        }
    } else {
        if lnum < 1 || lnum > count {
            return Ok(Value::Number(0));
        }
        let mut current = lnum;
        while current >= 1 {
            let line = host_getline(host, env, current).unwrap_or_default();
            if !line.trim_start().is_empty() {
                break;
            }
            current -= 1;
        }
        Ok(Value::Number(current.max(0)))
    }
}

// ------------------------------------------------------------- pathshorten()

fn is_path_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Shorten each directory component of a path to `trim_len` characters, like
/// Vim's `shorten_dir_len()`; the file name itself is kept intact.
fn shorten_dir(text: &str, trim_len: usize) -> String {
    let trim_len = trim_len.max(1);
    let chars: Vec<char> = text.chars().collect();
    let tail_start = chars
        .iter()
        .rposition(|c| is_path_sep(*c))
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut out = String::with_capacity(text.len());
    let mut skip = false;
    let mut chunk_len = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        if i >= tail_start {
            out.push(c);
        } else if is_path_sep(c) {
            out.push(c);
            skip = false;
            chunk_len = 0;
        } else if !skip {
            out.push(c);
            if c != '~' && c != '.' {
                chunk_len += 1;
                if chunk_len >= trim_len {
                    skip = true;
                }
            }
        }
    }
    out
}

fn pathshorten(args: &[Value]) -> ValueResult {
    let trim_len = if args.len() > 1 {
        let n = narg(args, 1);
        if n < 1 { 1 } else { n as usize }
    } else {
        1
    };
    Ok(Value::String(shorten_dir(&sarg(args, 0), trim_len)))
}

// ------------------------------------------------------------------ mkdir()

fn mkdir(args: &[Value]) -> ValueResult {
    let mut name = sarg(args, 0);
    while name.len() > 1 && (name.ends_with('/') || name.ends_with('\\')) {
        name.pop();
    }
    if name.is_empty() {
        return Ok(Value::Number(0));
    }
    let path = Path::new(&name);
    let parents = args.len() > 1 && sarg(args, 1) == "p";
    if parents {
        if path.is_dir() {
            return Ok(Value::Number(1));
        }
        Ok(Value::Number(i64::from(fs::create_dir_all(path).is_ok())))
    } else {
        Ok(Value::Number(i64::from(fs::create_dir(path).is_ok())))
    }
}

// ------------------------------------------------------------------- rand()

thread_local! {
    /// Global xoshiro128** state used by `rand()` without an argument.
    static RAND_STATE: Cell<Option<[u32; 4]>> = const { Cell::new(None) };
}

fn splitmix32(state: &mut u32) -> u32 {
    *state = state.wrapping_add(0x9e37_79b9);
    let mut z = *state;
    z = (z ^ (z >> 16)).wrapping_mul(0x85eb_ca6b);
    z = (z ^ (z >> 13)).wrapping_mul(0xc2b2_ae35);
    z ^ (z >> 16)
}

fn xoshiro_next(state: &mut [u32; 4]) -> u32 {
    let result = state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
    let t = state[1] << 9;
    state[2] ^= state[0];
    state[3] ^= state[1];
    state[1] ^= state[2];
    state[0] ^= state[3];
    state[2] ^= t;
    state[3] = state[3].rotate_left(11);
    result
}

fn global_rand() -> i64 {
    RAND_STATE.with(|cell| {
        let mut seed_state = cell.get();
        let state = match &mut seed_state {
            Some(state) => state,
            None => {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
                    .unwrap_or(0);
                let mut seed = nanos;
                let mut fresh = [
                    splitmix32(&mut seed),
                    splitmix32(&mut seed),
                    splitmix32(&mut seed),
                    splitmix32(&mut seed),
                ];
                let result = xoshiro_next(&mut fresh);
                cell.set(Some(fresh));
                return i64::from(result);
            }
        };
        let result = xoshiro_next(state);
        let copy = *state;
        cell.set(Some(copy));
        i64::from(result)
    })
}

fn rand_fn(args: &[Value]) -> ValueResult {
    let invalid = || {
        err(
            "E475",
            "Invalid argument: rand() requires no argument or a List of four Numbers",
        )
    };
    if args.is_empty() {
        return Ok(Value::Number(global_rand()));
    }
    let Value::List(list) = arg(args, 0) else {
        return Err(invalid());
    };
    let numbers = {
        let items = list.borrow();
        if items.len() != 4 {
            return Err(invalid());
        }
        let mut numbers = [0u32; 4];
        for (i, item) in items.iter().enumerate() {
            match item {
                Value::Number(n) => numbers[i] = *n as u32,
                _ => return Err(invalid()),
            }
        }
        numbers
    };
    let mut state = numbers;
    let result = xoshiro_next(&mut state);
    {
        let mut items = list.borrow_mut();
        for (i, value) in state.iter().enumerate() {
            items[i] = Value::Number(i64::from(*value));
        }
    }
    Ok(Value::Number(i64::from(result)))
}

// ---------------------------------------------------------- file functions

fn read_file_bytes(fname: &str) -> VimResult<Vec<u8>> {
    if fname.is_empty() {
        return Err(err("E484", "Can't open file <empty>"));
    }
    let path = Path::new(fname);
    if path.is_dir() {
        return Err(err("E17", format!("\"{fname}\" is a directory")));
    }
    fs::read(path).map_err(|_| err("E484", format!("Can't open file {fname}")))
}

fn readblob(args: &[Value]) -> ValueResult {
    Ok(Value::blob(read_file_bytes(&sarg(args, 0))?))
}

/// `readfile({fname} [, {type} [, {max}]])` (reference: filepath.c
/// `read_file_or_blob()`); `{type}` "b" keeps CRs, "B" returns a Blob.
fn readfile(args: &[Value]) -> ValueResult {
    let fname = sarg(args, 0);
    let typ = if args.len() > 1 && !matches!(arg(args, 1), Value::Null) {
        sarg(args, 1)
    } else {
        String::new()
    };
    let binary = typ == "b";
    if typ == "B" {
        return Ok(Value::blob(read_file_bytes(&fname)?));
    }
    let max = if args.len() > 2 && !matches!(arg(args, 2), Value::Null) {
        Some(narg(args, 2))
    } else {
        None
    };
    if let Some(m) = max {
        if m == 0 {
            return Ok(Value::list(Vec::new()));
        }
    }
    let bytes = read_file_bytes(&fname)?;
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    for &b in &bytes {
        if b == b'\n' {
            lines.push(std::mem::take(&mut current));
        } else if b == 0 {
            // NUL bytes become newlines inside the line (Vim quirk).
            current.push(b'\n');
        } else {
            current.push(b);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    } else if binary && (bytes.ends_with(b"\n") || bytes.is_empty()) {
        lines.push(Vec::new());
    }
    if !binary {
        for line in &mut lines {
            while line.last() == Some(&b'\r') {
                line.pop();
            }
        }
    }
    if let Some(max) = max {
        if max > 0 {
            lines.truncate(max as usize);
        } else if max < 0 {
            let keep = (-max) as usize;
            if lines.len() > keep {
                let extra = lines.len() - keep;
                lines.drain(..extra);
            }
        }
    }
    Ok(Value::list(
        lines
            .into_iter()
            .map(|line| Value::String(String::from_utf8_lossy(&line).into_owned()))
            .collect(),
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortMode {
    None,
    Byte,
    IgnoreCase,
}

fn sort_mode_from_args(args: &[Value]) -> VimResult<SortMode> {
    if args.len() <= 2 {
        return Ok(SortMode::Byte);
    }
    let Value::Dict(dict) = &args[2] else {
        return Err(err("E715", "Dictionary required"));
    };
    let sort_value = dict.borrow().get("sort").map(Value::to_string_value);
    let Some(sort_value) = sort_value else {
        return Err(err("E475", "Dictionary key 'sort' required"));
    };
    Ok(match sort_value.as_str() {
        "none" => SortMode::None,
        "icase" => SortMode::IgnoreCase,
        _ => SortMode::Byte,
    })
}

fn sort_names(names: &mut [String], mode: SortMode) {
    match mode {
        SortMode::None => {}
        SortMode::Byte => names.sort(),
        SortMode::IgnoreCase => names.sort_by(|a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        }),
    }
}

fn read_dir_names(path: &str) -> VimResult<Vec<String>> {
    let entries = fs::read_dir(path).map_err(|_| err("E484", format!("Can't open file {path}")))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| err("E484", format!("Can't open file {path}")))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// Evaluate the `readdir()`/`readdirex()` filter expression with `v:val` bound.
fn eval_dir_expr(src: &str, value: &Value, env: &mut Env, host: &mut dyn Host) -> VimResult<i64> {
    let saved = env.vvars.get("val").cloned();
    env.vvars.insert("val".to_string(), value.clone());
    let parsed = parse_expression(src).and_then(|expr| eval(&expr, env, host));
    match saved {
        Some(v) => {
            env.vvars.insert("val".to_string(), v);
        }
        None => {
            env.vvars.remove("val");
        }
    }
    Ok(parsed?.to_number())
}

fn mtime_seconds(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn file_perm(is_dir: bool, readonly: bool, name: &str) -> String {
    let executable = is_dir || {
        let lower = name.to_ascii_lowercase();
        [".exe", ".com", ".cmd", ".bat"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    };
    let bits = format!(
        "{}{}{}",
        'r',
        if readonly { '-' } else { 'w' },
        if executable { 'x' } else { '-' }
    );
    format!("{bits}{bits}{bits}")
}

/// Build one `readdirex()` item dictionary (reference: `create_readdirex_item`).
fn readdirex_item(dir: &Path, name: &str) -> VimDict {
    let full = dir.join(name);
    let mut dict = VimDict::new();
    dict.insert("name", Value::string(name));
    let mut size = 0i64;
    let mut time = 0i64;
    let mut file_type = "file".to_string();
    let mut perm = "---------".to_string();
    if let Ok(meta) = fs::symlink_metadata(&full) {
        time = mtime_seconds(&meta);
        if meta.file_type().is_symlink() {
            match fs::metadata(&full) {
                Ok(target) => {
                    file_type = if target.is_dir() { "linkd" } else { "link" }.to_string();
                    size = if target.is_dir() {
                        0
                    } else {
                        target.len() as i64
                    };
                    time = mtime_seconds(&target);
                    perm = file_perm(target.is_dir(), target.permissions().readonly(), name);
                }
                Err(_) => file_type = "link".to_string(),
            }
        } else if meta.is_dir() {
            file_type = "dir".to_string();
            perm = file_perm(true, meta.permissions().readonly(), name);
        } else {
            size = meta.len() as i64;
            perm = file_perm(false, meta.permissions().readonly(), name);
        }
    }
    dict.insert("size", Value::Number(size));
    dict.insert("time", Value::Number(time));
    dict.insert("type", Value::String(file_type));
    dict.insert("perm", Value::String(perm));
    dict.insert("user", Value::string(""));
    dict.insert("group", Value::string(""));
    dict
}

/// `readdir({dirname} [, {expr} [, {dict}]])` and
/// `readdirex({dirname} [, {expr} [, {dict}]])` (reference: filepath.c).
fn readdir(args: &[Value], withattr: bool, env: &mut Env, host: &mut dyn Host) -> ValueResult {
    let dir = sarg(args, 0);
    let sort_mode = sort_mode_from_args(args)?;
    let expr = if args.len() > 1 {
        Some(sarg(args, 1))
    } else {
        None
    };
    let mut names = read_dir_names(&dir)?;
    sort_names(&mut names, sort_mode);
    let dir_path = PathBuf::from(&dir);
    let mut result = Vec::new();
    for name in names {
        let item = if withattr {
            Value::dict(readdirex_item(&dir_path, &name))
        } else {
            Value::String(name)
        };
        if let Some(expr) = &expr {
            let keep = eval_dir_expr(expr, &item, env, host)?;
            if keep < 0 {
                break;
            }
            if keep == 0 {
                continue;
            }
        }
        result.push(item);
    }
    Ok(Value::list(result))
}

fn rename_fn(args: &[Value]) -> ValueResult {
    let from = sarg(args, 0);
    let to = sarg(args, 1);
    if from.is_empty() || to.is_empty() {
        return Ok(Value::Number(-1));
    }
    let from_path = Path::new(&from);
    let to_path = Path::new(&to);
    if !from_path.exists() {
        return Ok(Value::Number(-1));
    }
    if let (Ok(a), Ok(b)) = (fs::canonicalize(from_path), fs::canonicalize(to_path)) {
        if a == b {
            return Ok(Value::Number(0));
        }
    }
    if to_path.is_file() {
        let _ = fs::remove_file(to_path);
    }
    match fs::rename(from_path, to_path) {
        Ok(()) => Ok(Value::Number(0)),
        Err(_) => Ok(Value::Number(-1)),
    }
}

/// Strip the Windows `\\?\` verbatim prefix from a canonicalized path.
fn strip_verbatim(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    }
}

fn resolve_fn(args: &[Value]) -> ValueResult {
    let path = sarg(args, 0);
    if path.is_empty() {
        return Ok(Value::string(""));
    }
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::canonicalize(&path) {
                return Ok(Value::String(strip_verbatim(&target)));
            }
        }
    }
    Ok(Value::String(path))
}

// ---------------------------------------------------------------- reduce()

/// `reduce({object}, {func} [, {initial}])` for List, String and Blob
/// (reference: list.c `list_reduce()`, strings.c `string_reduce()`, blob.c
/// `blob_reduce()`).
fn reduce(args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    let target = arg(args, 0);
    let func = arg(args, 1);
    if !matches!(func, Value::Funcref(_) | Value::String(_)) {
        return Err(err("E475", "Missing function argument"));
    }
    let has_initial = args.len() > 2;
    let empty_error = |kind: &str| {
        err(
            "E998",
            format!("Reduce of an empty {kind} with no initial value"),
        )
    };
    match &target {
        Value::List(list) => {
            let items = list.borrow().clone();
            let (mut acc, start) = if has_initial {
                (arg(args, 2), 0)
            } else {
                if items.is_empty() {
                    return Err(empty_error("List"));
                }
                (items[0].clone(), 1)
            };
            for item in items.iter().skip(start) {
                acc = call_value(&func, vec![acc, item.clone()], env, host)?;
            }
            Ok(acc)
        }
        Value::String(text) => {
            if has_initial && !matches!(arg(args, 2), Value::String(_)) {
                return Err(err("E475", "String expected for argument 3"));
            }
            let chars: Vec<char> = text.chars().collect();
            let (mut acc, start) = if has_initial {
                (arg(args, 2), 0)
            } else {
                let Some(first) = chars.first() else {
                    return Err(empty_error("String"));
                };
                (Value::String(first.to_string()), 1)
            };
            for c in chars.iter().skip(start) {
                acc = call_value(&func, vec![acc, Value::String(c.to_string())], env, host)?;
            }
            Ok(acc)
        }
        Value::Blob(blob) => {
            let bytes = blob.borrow().clone();
            if has_initial && !matches!(arg(args, 2), Value::Number(_)) {
                return Err(err("E475", "Number expected for argument 3"));
            }
            let (mut acc, start) = if has_initial {
                (arg(args, 2), 0)
            } else {
                let Some(first) = bytes.first() else {
                    return Err(empty_error("Blob"));
                };
                (Value::Number(i64::from(*first)), 1)
            };
            for b in bytes.iter().skip(start) {
                acc = call_value(&func, vec![acc, Value::Number(i64::from(*b))], env, host)?;
            }
            Ok(acc)
        }
        other => Err(err(
            "E475",
            format!(
                "String, List or Blob required for reduce(), not {}",
                other.type_name()
            ),
        )),
    }
}

// ------------------------------------------------------- time functions

fn now_micros() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64 * 1_000_000 + i64::from(d.subsec_micros()),
        Err(_) => 0,
    }
}

fn proftime_micros(value: &Value) -> Option<i64> {
    let Value::List(list) = value else {
        return None;
    };
    let items = list.borrow();
    if items.len() != 2 {
        return None;
    }
    let sec = match items.first() {
        Some(Value::Number(n)) => *n,
        _ => return None,
    };
    let usec = match items.get(1) {
        Some(Value::Number(n)) => *n,
        _ => return None,
    };
    Some(sec.saturating_mul(1_000_000).saturating_add(usec))
}

fn split_micros(total: i64) -> (i64, i64) {
    (total.div_euclid(1_000_000), total.rem_euclid(1_000_000))
}

/// `reltime([{start} [, {end}]])` returns a `[seconds, microseconds]` pair.
fn reltime(args: &[Value]) -> ValueResult {
    let invalid = || {
        err(
            "E474",
            "Invalid argument: reltime() requires a List with two Numbers",
        )
    };
    let total = match args.len() {
        0 => now_micros(),
        1 => {
            let Some(start) = proftime_micros(&arg(args, 0)) else {
                return Err(invalid());
            };
            now_micros().saturating_sub(start)
        }
        _ => {
            let Some(start) = proftime_micros(&arg(args, 0)) else {
                return Err(invalid());
            };
            let Some(end) = proftime_micros(&arg(args, 1)) else {
                return Err(invalid());
            };
            end.saturating_sub(start)
        }
    };
    let (sec, usec) = split_micros(total);
    Ok(Value::list(vec![Value::Number(sec), Value::Number(usec)]))
}

fn reltimefloat(args: &[Value]) -> ValueResult {
    match proftime_micros(&arg(args, 0)) {
        Some(micros) => Ok(Value::Float(micros as f64 / 1_000_000.0)),
        None => Ok(Value::Float(0.0)),
    }
}

fn reltimestr(args: &[Value]) -> ValueResult {
    match proftime_micros(&arg(args, 0)) {
        Some(micros) => {
            let (sec, usec) = split_micros(micros);
            Ok(Value::String(format!("{sec:>3}.{usec:06}")))
        }
        None => Ok(Value::string("")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct H;

    impl Host for H {}

    /// A host that backs `getline()` / `line()` with an in-memory buffer.
    struct LineHost {
        lines: Vec<String>,
    }

    impl Host for LineHost {
        fn call_host_builtin(
            &mut self,
            name: &str,
            args: Vec<Value>,
            _env: &mut Env,
        ) -> Option<ValueResult> {
            match name {
                "getline" => {
                    let lnum = args.first().map(Value::to_number).unwrap_or(0);
                    let idx = lnum.saturating_sub(1).max(0) as usize;
                    Some(Ok(Value::String(
                        self.lines.get(idx).cloned().unwrap_or_default(),
                    )))
                }
                "line" => {
                    if args.first().map(Value::to_string_value).as_deref() == Some("$") {
                        Some(Ok(Value::Number(self.lines.len() as i64)))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
    }

    fn hcall(name: &str, args: &[Value]) -> ValueResult {
        let mut env = Env::new();
        let mut host = H;
        super::call(name, args, &mut env, &mut host).expect("function should be handled")
    }

    fn line_served(name: &str, args: &[Value], lines: &[&str]) -> ValueResult {
        let mut env = Env::new();
        let mut host = LineHost {
            lines: lines.iter().map(|s| (*s).to_string()).collect(),
        };
        super::call(name, args, &mut env, &mut host).expect("function should be handled")
    }

    fn list_items(value: &Value) -> Vec<Value> {
        match value {
            Value::List(list) => list.borrow().clone(),
            other => panic!("expected a List, got {}", other.type_name()),
        }
    }

    fn eval_value(src: &str) -> Value {
        let mut env = Env::new();
        let mut host = H;
        let expr = parse_expression(src).expect("expression parses");
        eval(&expr, &mut env, &mut host).expect("expression evaluates")
    }

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(tag: &str) -> PathBuf {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("rvim_mr_{}_{}_{}", std::process::id(), tag, n))
    }

    #[test]
    fn funcs_are_dispatched_and_registered() {
        assert!(!FUNCS.is_empty());
        for name in FUNCS {
            assert!(
                crate::func_table::FUNCTIONS.iter().any(|d| d.name == *name),
                "{name} is missing from the generated function table"
            );
            assert!(crate::builtins::is_builtin(name), "{name} not registered");
            let mut env = Env::new();
            let mut host = H;
            assert!(
                super::call(name, &[], &mut env, &mut host).is_some(),
                "{name} is in FUNCS but not dispatched"
            );
        }
        let mut env = Env::new();
        let mut host = H;
        assert!(super::call("not_a_real_builtin", &[], &mut env, &mut host).is_none());
    }

    #[test]
    fn mapnew_list_leaves_original_untouched() {
        let original = Value::list(vec![Value::Number(1), Value::Number(2)]);
        let mapped = hcall("mapnew", &[original.clone(), Value::string("v:val * 2")]).unwrap();
        assert_eq!(mapped.to_string_value(), "[2, 4]");
        assert_eq!(original.to_string_value(), "[1, 2]");
        // The key is available as v:key (the list index).
        let keyed = hcall("mapnew", &[original, Value::string("v:val + v:key")]).unwrap();
        assert_eq!(keyed.to_string_value(), "[1, 3]");
    }

    #[test]
    fn mapnew_dict_and_lambda() {
        let mut dict = VimDict::new();
        dict.insert("a", Value::Number(1));
        dict.insert("b", Value::Number(2));
        let add = eval_value("{k, v -> v + strlen(k)}");
        let mapped = hcall("mapnew", &[Value::dict(dict), add]).unwrap();
        assert_eq!(mapped.to_string_value(), "{'a': 2, 'b': 3}");
    }

    #[test]
    fn mapnew_rejects_bad_arguments() {
        let too_few = hcall("mapnew", &[Value::list(Vec::new())]).unwrap_err();
        assert_eq!(too_few.code, "E119");
        let not_callable =
            hcall("mapnew", &[Value::list(Vec::new()), Value::Number(3)]).unwrap_err();
        assert_eq!(not_callable.code, "E731");
    }

    #[test]
    fn matchlist_returns_groups() {
        let result = hcall(
            "matchlist",
            &[Value::string("testing"), Value::string(r"\(t\)e\(st\)")],
        )
        .unwrap();
        let items = list_items(&result);
        assert_eq!(items.len(), 10);
        assert_eq!(items[0].to_string_value(), "test");
        assert_eq!(items[1].to_string_value(), "t");
        assert_eq!(items[2].to_string_value(), "st");
        assert_eq!(items[3].to_string_value(), "");
    }

    #[test]
    fn matchlist_no_match_and_nth() {
        let none = hcall("matchlist", &[Value::string("abc"), Value::string("zzz")]).unwrap();
        assert!(list_items(&none).is_empty());
        let second = hcall(
            "matchlist",
            &[
                Value::string("abcabc"),
                Value::string("abc"),
                Value::Number(0),
                Value::Number(2),
            ],
        )
        .unwrap();
        assert_eq!(list_items(&second)[0].to_string_value(), "abc");
        assert_eq!(list_items(&second).len(), 10);
    }

    #[test]
    fn matchstrpos_returns_positions() {
        let found = hcall(
            "matchstrpos",
            &[Value::string("testing"), Value::string("ing")],
        )
        .unwrap();
        assert_eq!(found.to_string_value(), "['ing', 4, 7]");
        let none = hcall(
            "matchstrpos",
            &[Value::string("testing"), Value::string("zzz")],
        )
        .unwrap();
        assert_eq!(none.to_string_value(), "['', -1, -1]");
        let from_start = hcall(
            "matchstrpos",
            &[Value::string("aXbXc"), Value::string("X"), Value::Number(2)],
        )
        .unwrap();
        assert_eq!(from_start.to_string_value(), "['X', 3, 4]");
    }

    #[test]
    fn matchstrpos_searches_lists() {
        let list = Value::list(vec![
            Value::string("alpha"),
            Value::string("beta"),
            Value::string("gamma"),
        ]);
        let found = hcall("matchstrpos", &[list, Value::string("a")]).unwrap();
        assert_eq!(found.to_string_value(), "['a', 0, 0, 1]");
    }

    #[test]
    fn matchfuzzy_filters_and_orders() {
        let list = Value::list(vec![
            Value::string("hello"),
            Value::string("world"),
            Value::string("hero"),
        ]);
        let matched = hcall("matchfuzzy", &[list.clone(), Value::string("hlo")]).unwrap();
        assert_eq!(matched.to_string_value(), "['hello']");
        let limited = hcall(
            "matchfuzzy",
            &[
                list,
                Value::string("h"),
                Value::dict({
                    let mut d = VimDict::new();
                    d.insert("limit", Value::Number(1));
                    d
                }),
            ],
        )
        .unwrap();
        assert_eq!(list_items(&limited).len(), 1);
    }

    #[test]
    fn matchfuzzypos_returns_positions_and_scores() {
        let list = Value::list(vec![Value::string("foobar")]);
        let result = hcall("matchfuzzypos", &[list, Value::string("fb")]).unwrap();
        let outer = list_items(&result);
        assert_eq!(outer.len(), 3);
        assert_eq!(list_items(&outer[0])[0].to_string_value(), "foobar");
        let positions = list_items(&outer[1]);
        assert_eq!(positions[0].to_string_value(), "[0, 3]");
        let scores = list_items(&outer[2]);
        assert_eq!(scores[0].to_number(), 105);
    }

    #[test]
    fn matchfuzzy_matchseq() {
        let list = Value::list(vec![Value::string("one two"), Value::string("two one")]);
        let mut opts = VimDict::new();
        opts.insert("matchseq", Value::Number(1));
        let matched = hcall(
            "matchfuzzy",
            &[list, Value::string("two one"), Value::dict(opts)],
        )
        .unwrap();
        assert_eq!(matched.to_string_value(), "['two one']");
    }

    #[test]
    fn mkdir_creates_recursively() {
        let base = temp_path("mkdir");
        let nested = base.join("a").join("b");
        let result = hcall(
            "mkdir",
            &[
                Value::String(nested.display().to_string()),
                Value::string("p"),
            ],
        )
        .unwrap();
        assert_eq!(result.to_number(), 1);
        assert!(nested.is_dir());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn pathshorten_shortens_components() {
        assert_eq!(
            hcall("pathshorten", &[Value::string("~/foo/../.bar/fname")])
                .unwrap()
                .to_string_value(),
            "~/f/../.b/fname"
        );
        assert_eq!(
            hcall(
                "pathshorten",
                &[Value::string("/alpha/beta/gamma.txt"), Value::Number(2)],
            )
            .unwrap()
            .to_string_value(),
            "/al/be/gamma.txt"
        );
    }

    #[test]
    fn nextnonblank_scans_forward() {
        let lines = ["", "first", "", "second"];
        assert_eq!(
            line_served("nextnonblank", &[Value::Number(1)], &lines)
                .unwrap()
                .to_number(),
            2
        );
        assert_eq!(
            line_served("nextnonblank", &[Value::Number(3)], &lines)
                .unwrap()
                .to_number(),
            4
        );
        assert_eq!(
            line_served("nextnonblank", &[Value::Number(0)], &lines)
                .unwrap()
                .to_number(),
            2
        );
        assert_eq!(
            line_served("nextnonblank", &[Value::Number(5)], &lines)
                .unwrap()
                .to_number(),
            0
        );
    }

    #[test]
    fn prevnonblank_scans_backward() {
        let lines = ["", "first", "", "second"];
        assert_eq!(
            line_served("prevnonblank", &[Value::Number(4)], &lines)
                .unwrap()
                .to_number(),
            4
        );
        assert_eq!(
            line_served("prevnonblank", &[Value::Number(3)], &lines)
                .unwrap()
                .to_number(),
            2
        );
        assert_eq!(
            line_served("prevnonblank", &[Value::Number(1)], &lines)
                .unwrap()
                .to_number(),
            0
        );
    }

    #[test]
    fn pum_helpers_report_no_popup_menu() {
        assert_eq!(hcall("pumvisible", &[]).unwrap().to_number(), 0);
        let pos = hcall("pum_getpos", &[]).unwrap();
        match pos {
            Value::Dict(d) => assert!(d.borrow().is_empty()),
            other => panic!("expected a Dict, got {}", other.type_name()),
        }
    }

    #[test]
    fn rand_is_xoshiro128starstar() {
        let seed = Value::list(vec![
            Value::Number(1),
            Value::Number(2),
            Value::Number(3),
            Value::Number(4),
        ]);
        assert_eq!(
            hcall("rand", std::slice::from_ref(&seed))
                .unwrap()
                .to_number(),
            11520
        );
        // The seed list was advanced in place by one xoshiro128** step.
        assert_eq!(seed.to_string_value(), "[7, 0, 1026, 12288]");
        let random = hcall("rand", &[]).unwrap().to_number();
        assert!(
            (0..=0xffff_ffff).contains(&random),
            "rand() out of range: {random}"
        );
        let bad = hcall("rand", &[Value::Number(1)]).unwrap_err();
        assert_eq!(bad.code, "E475");
    }

    #[test]
    fn readfile_splits_lines_and_honours_flags() {
        let path = temp_path("readfile");
        fs::write(&path, b"a\r\nb\n\nc\n").unwrap();
        let name = Value::String(path.display().to_string());
        let text = hcall("readfile", std::slice::from_ref(&name)).unwrap();
        assert_eq!(text.to_string_value(), "['a', 'b', '', 'c']");
        let binary = hcall("readfile", &[name.clone(), Value::string("b")]).unwrap();
        let binary_lines = list_items(&binary);
        assert_eq!(binary_lines[0].to_string_value(), "a\r");
        assert_eq!(binary_lines[1].to_string_value(), "b");
        assert_eq!(binary_lines[2].to_string_value(), "");
        assert_eq!(binary_lines[3].to_string_value(), "c");
        assert_eq!(binary_lines[4].to_string_value(), "");
        let tail = hcall(
            "readfile",
            &[name.clone(), Value::string(""), Value::Number(-2)],
        )
        .unwrap();
        assert_eq!(tail.to_string_value(), "['', 'c']");
        let blob_mode = hcall("readfile", &[name, Value::string("B")]).unwrap();
        match blob_mode {
            Value::Blob(b) => assert_eq!(*b.borrow(), b"a\r\nb\n\nc\n".to_vec()),
            other => panic!("expected a Blob, got {}", other.type_name()),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn readfile_missing_file_is_an_error() {
        let missing = temp_path("no_such_file").display().to_string();
        let result = hcall("readfile", &[Value::String(missing)]);
        assert_eq!(result.unwrap_err().code, "E484");
    }

    #[test]
    fn readblob_reads_raw_bytes() {
        let path = temp_path("readblob");
        fs::write(&path, [0u8, 1, 255]).unwrap();
        let blob = hcall("readblob", &[Value::String(path.display().to_string())]).unwrap();
        match blob {
            Value::Blob(b) => assert_eq!(*b.borrow(), vec![0u8, 1, 255]),
            other => panic!("expected a Blob, got {}", other.type_name()),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn readdir_sorts_and_filters() {
        let dir = temp_path("readdir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.txt"), b"b").unwrap();
        fs::write(dir.join("a.txt"), b"a").unwrap();
        fs::write(dir.join("c.log"), b"c").unwrap();
        let path = Value::String(dir.display().to_string());
        let names = hcall("readdir", std::slice::from_ref(&path)).unwrap();
        assert_eq!(names.to_string_value(), "['a.txt', 'b.txt', 'c.log']");
        let filtered = hcall(
            "readdir",
            &[path.clone(), Value::string("v:val =~ '\\.txt$'")],
        )
        .unwrap();
        assert_eq!(filtered.to_string_value(), "['a.txt', 'b.txt']");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn readdirex_reports_attributes() {
        let dir = temp_path("readdirex");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("file.txt"), b"hello").unwrap();
        let items = hcall("readdirex", &[Value::String(dir.display().to_string())]).unwrap();
        let entries = list_items(&items);
        assert_eq!(entries.len(), 2);
        let first = match &entries[0] {
            Value::Dict(d) => d.borrow().clone(),
            other => panic!("expected a Dict, got {}", other.type_name()),
        };
        assert_eq!(
            first
                .get("name")
                .map(Value::to_string_value)
                .unwrap_or_default(),
            "file.txt"
        );
        assert_eq!(first.get("size").map(Value::to_number).unwrap_or(-1), 5);
        assert_eq!(
            first
                .get("type")
                .map(Value::to_string_value)
                .unwrap_or_default(),
            "file"
        );
        let second = match &entries[1] {
            Value::Dict(d) => d.borrow().clone(),
            other => panic!("expected a Dict, got {}", other.type_name()),
        };
        assert_eq!(
            second
                .get("type")
                .map(Value::to_string_value)
                .unwrap_or_default(),
            "dir"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reduce_list_string_blob() {
        let add = eval_value("{a, b -> a + b}");
        assert_eq!(
            hcall(
                "reduce",
                &[
                    Value::list(vec![Value::Number(1), Value::Number(2), Value::Number(3)]),
                    add.clone()
                ]
            )
            .unwrap()
            .to_number(),
            6
        );
        assert_eq!(
            hcall(
                "reduce",
                &[
                    Value::list(vec![Value::Number(1), Value::Number(2)]),
                    add.clone(),
                    Value::Number(10),
                ],
            )
            .unwrap()
            .to_number(),
            13
        );
        // A string function name is called like a Funcref.  add() appends its
        // second argument, so the accumulator grows by one nested list each step.
        let lists = Value::list(vec![
            Value::list(vec![Value::Number(1)]),
            Value::list(vec![Value::Number(2)]),
            Value::list(vec![Value::Number(3)]),
        ]);
        assert_eq!(
            hcall("reduce", &[lists, Value::string("add")])
                .unwrap()
                .to_string_value(),
            "[1, [2], [3]]"
        );
        let concat = eval_value("{a, b -> a . b}");
        assert_eq!(
            hcall("reduce", &[Value::string("abc"), concat, Value::string("")])
                .unwrap()
                .to_string_value(),
            "abc"
        );
        let blob = eval_value("0z0102");
        assert_eq!(hcall("reduce", &[blob, add]).unwrap().to_number(), 3);
    }

    #[test]
    fn reduce_empty_without_initial_is_an_error() {
        let result = hcall("reduce", &[Value::list(Vec::new()), Value::string("add")]);
        assert_eq!(result.unwrap_err().code, "E998");
    }

    #[test]
    fn register_state_helpers_are_empty() {
        assert_eq!(hcall("reg_executing", &[]).unwrap().to_string_value(), "");
        assert_eq!(hcall("reg_recording", &[]).unwrap().to_string_value(), "");
    }

    #[test]
    fn reltime_helpers() {
        let current = hcall("reltime", &[]).unwrap();
        let items = list_items(&current);
        assert_eq!(items.len(), 2);
        assert!(items[0].to_number() > 0);
        let start = Value::list(vec![Value::Number(0), Value::Number(0)]);
        let diff = hcall(
            "reltime",
            &[
                start.clone(),
                Value::list(vec![Value::Number(1), Value::Number(250_000)]),
            ],
        )
        .unwrap();
        assert_eq!(diff.to_string_value(), "[1, 250000]");
        let since = hcall("reltime", &[start]).unwrap();
        assert!(list_items(&since)[0].to_number() > 0);
        assert_eq!(
            hcall(
                "reltimefloat",
                &[Value::list(vec![Value::Number(1), Value::Number(500_000)])],
            )
            .unwrap()
            .to_string_value(),
            "1.5"
        );
        assert_eq!(
            hcall(
                "reltimestr",
                &[Value::list(vec![Value::Number(1), Value::Number(500_000)])],
            )
            .unwrap()
            .to_string_value(),
            "  1.500000"
        );
        assert_eq!(
            hcall(
                "reltimefloat",
                &[Value::list(vec![Value::Number(0), Value::Number(1)])],
            )
            .unwrap()
            .to_string_value(),
            // A Float goes through vim's "%g": 0.000001 is outside
            // [0.001, 1e7) so it prints in exponent form.
            "1.0e-6"
        );
    }

    #[test]
    fn rename_moves_files() {
        let from = temp_path("rename_from");
        let to = temp_path("rename_to");
        fs::write(&from, b"x").unwrap();
        let result = hcall(
            "rename",
            &[
                Value::String(from.display().to_string()),
                Value::String(to.display().to_string()),
            ],
        )
        .unwrap();
        assert_eq!(result.to_number(), 0);
        assert!(!from.exists());
        assert!(to.exists());
        let missing = hcall(
            "rename",
            &[
                Value::String(from.display().to_string()),
                Value::String(to.display().to_string()),
            ],
        )
        .unwrap();
        assert_eq!(missing.to_number(), -1);
        let _ = fs::remove_file(&to);
    }

    #[test]
    fn resolve_is_a_no_op_for_plain_paths() {
        let missing = temp_path("resolve_missing").display().to_string();
        assert_eq!(
            hcall("resolve", &[Value::String(missing.clone())])
                .unwrap()
                .to_string_value(),
            missing
        );
        let file = temp_path("resolve_file");
        fs::write(&file, b"x").unwrap();
        let plain = file.display().to_string();
        assert_eq!(
            hcall("resolve", &[Value::String(plain.clone())])
                .unwrap()
                .to_string_value(),
            plain
        );
        let _ = fs::remove_file(&file);
    }
}
