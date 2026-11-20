//! A small reader for Vim's syntax/*.vim scripts (reference: syntax.c
//! syn_cmd_keyword/syn_cmd_match/syn_cmd_region).
//!
//! Vim executes these files with the script interpreter.  RustVim does not need
//! the whole interpreter to consume a syntax file, so this module understands
//! the declarative subset directly: syn keyword/match/region, syn cluster,
//! syn case, syn sync and hi def link.  Anything else (conditionals,
//! let b:current_syntax, runtime!, ...) is ignored, which is exactly what a
//! syntax file contributes to the highlighter.

use crate::{Item, ItemKind, ItemOptions, SyncSpec, SyntaxDef};

/// A parsed syntax script: definitions plus highlight links.
#[derive(Clone, Debug, Default)]
pub struct ParsedSyntax {
    pub def: SyntaxDef,
    pub links: Vec<(String, String)>,
}

/// Parse a syntax script.  Unknown commands are ignored.
pub fn parse(name: &str, text: &str) -> ParsedSyntax {
    let mut out = ParsedSyntax {
        def: SyntaxDef {
            name: name.to_string(),
            ..SyntaxDef::default()
        },
        links: Vec::new(),
    };
    // Join backslash continuations first (syntax files use them heavily).
    let mut logical: Vec<String> = Vec::new();
    let mut pending = String::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_end();
        let continuation = trimmed.ends_with('\\')
            && !trimmed.ends_with("\\\\")
            && !trimmed.trim_start().starts_with('"');
        if continuation {
            pending.push_str(trimmed.trim_end_matches('\\'));
            pending.push(' ');
            continue;
        }
        if pending.is_empty() {
            logical.push(line.to_string());
        } else {
            pending.push_str(line);
            logical.push(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        logical.push(pending);
    }
    for line in logical {
        let line = line.trim();
        if line.is_empty() || line.starts_with('"') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("syntax") || lower.starts_with("syn ") || lower == "syn" {
            parse_syntax_line(&mut out, line);
        } else if lower.starts_with("highlight") || lower.starts_with("hi ") || lower == "hi" {
            parse_highlight_line(&mut out, line);
        }
    }
    out
}

fn next_id(def: &SyntaxDef) -> i64 {
    def.items.iter().map(|item| item.id).max().unwrap_or(0) + 1
}

/// Split a command line into the command word and the rest.
fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim_start()),
        None => (text, ""),
    }
}

fn parse_syntax_line(out: &mut ParsedSyntax, line: &str) {
    let (_, rest) = split_word(line);
    let (sub, rest) = split_word(rest);
    match sub.to_ascii_lowercase().as_str() {
        "keyword" => {
            let (group, rest) = split_word(rest);
            if group.is_empty() {
                return;
            }
            let (words, options) = split_words_and_options(rest);
            if words.is_empty() {
                return;
            }
            let id = next_id(&out.def);
            out.def.items.push(Item {
                id,
                group: group.to_string(),
                kind: ItemKind::Keyword { words },
            });
            if !options.is_default() {
                out.def.item_options.insert(id, options);
            }
        }
        "match" => {
            let (group, rest) = split_word(rest);
            if group.is_empty() {
                return;
            }
            let Some((pattern, rest)) = take_pattern(rest) else {
                return;
            };
            let (options, contains) = parse_options(&rest);
            let id = next_id(&out.def);
            out.def.items.push(Item {
                id,
                group: group.to_string(),
                kind: ItemKind::Match { pattern, contains },
            });
            if !options.is_default() {
                out.def.item_options.insert(id, options);
            }
        }
        "region" => {
            let (group, rest) = split_word(rest);
            if group.is_empty() {
                return;
            }
            let Some((start, rest)) = extract_pattern(rest, "start=") else {
                return;
            };
            let Some((end, rest)) = extract_pattern(&rest, "end=") else {
                return;
            };
            let (options, contains) = parse_options(&rest);
            let id = next_id(&out.def);
            out.def.items.push(Item {
                id,
                group: group.to_string(),
                kind: ItemKind::Region {
                    start,
                    end,
                    contains,
                },
            });
            if !options.is_default() {
                out.def.item_options.insert(id, options);
            }
        }
        "cluster" => {
            let (name, rest) = split_word(rest);
            if name.is_empty() {
                return;
            }
            let rest = rest.trim();
            let list = rest.strip_prefix("add=").unwrap_or(rest);
            let entry = out.def.clusters.entry(name.to_string()).or_default();
            for group in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if !entry.iter().any(|g| g == group) {
                    entry.push(group.to_string());
                }
            }
        }
        "case" => match split_word(rest).0.to_ascii_lowercase().as_str() {
            "ignore" => out.def.ignore_case = true,
            "match" => out.def.ignore_case = false,
            _ => {}
        },
        "sync" => parse_sync(&mut out.def.sync, rest),
        "include" => { /* handled by the caller: it needs a runtime path */ }
        _ => {}
    }
}

fn parse_sync(sync: &mut SyncSpec, rest: &str) {
    for word in rest.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        match lower.as_str() {
            "fromstart" => sync.from_start = true,
            "ccomment" => sync.ccomment = true,
            _ => {
                if let Some(n) = lower.strip_prefix("minlines=").and_then(|n| n.parse().ok()) {
                    sync.minlines = Some(n);
                } else if let Some(n) = lower.strip_prefix("maxlines=").and_then(|n| n.parse().ok())
                {
                    sync.maxlines = Some(n);
                } else if let Some(n) = lower
                    .strip_prefix("linebreaks=")
                    .and_then(|n| n.parse().ok())
                {
                    sync.linebreaks = Some(n);
                }
            }
        }
    }
}

/// Split keywords from their trailing options.
fn split_words_and_options(rest: &str) -> (Vec<String>, ItemOptions) {
    let mut words = Vec::new();
    let mut options = ItemOptions::default();
    let mut contains = Vec::new();
    for word in rest.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        if is_option_word(&lower) {
            apply_option(&mut options, word, &mut contains);
        } else {
            words.push(word.to_string());
        }
    }
    (words, options)
}

fn is_option_word(lower: &str) -> bool {
    matches!(
        lower,
        "contained"
            | "transparent"
            | "display"
            | "conceal"
            | "skipwhite"
            | "skipnl"
            | "skipempty"
            | "keepend"
            | "extend"
            | "excludenl"
            | "fold"
            | "oneline"
    ) || lower.starts_with("nextgroup=")
        || lower.starts_with("skip=")
        || lower.starts_with("contains=")
        || lower.starts_with("cchar=")
}

fn parse_options(rest: &str) -> (ItemOptions, Vec<String>) {
    let mut options = ItemOptions::default();
    let mut contains = Vec::new();
    for word in rest.split_whitespace() {
        if let Some(list) = word.strip_prefix("contains=") {
            contains.extend(
                list.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            );
        } else {
            apply_option(&mut options, word, &mut contains);
        }
    }
    (options, contains)
}

fn apply_option(options: &mut ItemOptions, word: &str, contains: &mut Vec<String>) {
    let lower = word.to_ascii_lowercase();
    match lower.as_str() {
        "contained" => options.contained = true,
        "transparent" => options.transparent = true,
        "display" => options.display = true,
        "conceal" => options.conceal = true,
        "fold" => options.fold = true,
        "skipwhite" => options.skip = Some("[ \\t]*".to_string()),
        "skipnl" | "skipempty" => options.skip = Some("\\n".to_string()),
        _ => {
            if let Some(list) = word.strip_prefix("nextgroup=") {
                options.nextgroup.extend(
                    list.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                );
            } else if let Some(value) = word.strip_prefix("skip=") {
                if let Some((pat, _)) = take_pattern(value) {
                    options.skip = Some(pat);
                }
            } else if let Some(list) = word.strip_prefix("contains=") {
                contains.extend(
                    list.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                );
            }
        }
    }
}

/// Read a delimited pattern at the start of `text`.  The delimiter is the first
/// non-space character (usually `/` or `"`).  Returns the pattern and the rest.
fn take_pattern(text: &str) -> Option<(String, String)> {
    let text = text.trim_start();
    let delim = text.chars().next()?;
    let body = &text[delim.len_utf8()..];
    let mut out = String::new();
    let mut escaped = false;
    for (index, ch) in body.char_indices() {
        if escaped {
            if ch != delim {
                out.push('\\');
            }
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == delim {
            return Some((out, body[index + ch.len_utf8()..].to_string()));
        } else {
            out.push(ch);
        }
    }
    None
}

/// Find `key` followed by a delimited pattern anywhere in `text`, remove the
/// assignment and return the pattern plus the remaining text.
fn extract_pattern(text: &str, key: &str) -> Option<(String, String)> {
    let mut base = 0usize;
    while let Some(index) = text[base..].find(key) {
        let abs = base + index;
        // Require a word boundary before the key.
        if abs == 0 || text.as_bytes()[abs - 1].is_ascii_whitespace() {
            let after = &text[abs + key.len()..];
            if let Some((pattern, rest)) = take_pattern(after) {
                let mut remaining = String::new();
                remaining.push_str(text[..abs].trim_end());
                if !remaining.is_empty() {
                    remaining.push(' ');
                }
                remaining.push_str(rest.trim_start());
                return Some((pattern, remaining));
            }
        }
        base = abs + key.len();
    }
    None
}

fn parse_highlight_line(out: &mut ParsedSyntax, line: &str) {
    let mut words = line.split_whitespace();
    let _ = words.next();
    let mut rest: Vec<&str> = words.collect();
    while matches!(rest.first().copied(), Some("def" | "default")) {
        rest.remove(0);
    }
    if rest.first().copied() == Some("link") {
        rest.remove(0);
        if rest.len() >= 2 {
            out.links.push((rest[0].to_string(), rest[1].to_string()));
        }
    }
}
