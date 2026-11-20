//! `:syntax` and `:highlight` (reference: syntax.c ex_syntax, highlight.c).
//!
//! The commands edit the editor's [rvim_editor::SyntaxState]: `:syntax on`
//! loads the built-in definition for the buffer's 'filetype' (or file name),
//! and the `keyword`/`match`/`region`/`cluster`/`case` sub-commands add items
//! on top, exactly like a hand-written `syntax/<ft>.vim`.

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_editor::syntax::lang;
use rvim_syntax::highlight::{HlAttr, HlColor, parse_attrs, parse_color};
use rvim_syntax::{Item, ItemKind};

use crate::exec::ExOutcome;
use crate::parser::ParsedEx;

/// Split the first word off an argument string.
fn split_word(args: &str) -> (&str, &str) {
    let args = args.trim_start();
    match args.find(char::is_whitespace) {
        Some(index) => (&args[..index], args[index..].trim_start()),
        None => (args, ""),
    }
}

/// `:syntax [subcommand] [args]`.
pub fn cmd_syntax(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let (sub, rest) = split_word(&p.args);
    let sub = sub.to_ascii_lowercase();
    match sub.as_str() {
        "" => {
            let state = if !ed.syntax.enabled {
                "OFF"
            } else if ed.syntax.name.is_empty() {
                "ON (no syntax for this buffer)"
            } else {
                "ON"
            };
            let name = ed.syntax.name.clone();
            ed.messages.info(format!(
                "syntax: {state}{}",
                if name.is_empty() {
                    String::new()
                } else {
                    format!(" ({name})")
                }
            ));
        }
        "on" | "enable" => {
            let name = rest.trim();
            if name.is_empty() {
                ed.set_syntax_enabled(true);
            } else {
                ed.syntax.enabled = true;
                if !ed.set_syntax_language(name) {
                    ed.messages
                        .info(format!("E28: No such highlight group or syntax: {name}"));
                }
            }
        }
        "off" => ed.set_syntax_enabled(false),
        "reset" => {
            ed.syntax.clear_engine();
            ed.ensure_syntax();
        }
        "clear" => {
            // `:syntax clear [Group]`: drop every item (or just one group's).
            let group = rest.trim();
            if group.is_empty() {
                let name = ed.syntax.name.clone();
                ed.syntax.load(rvim_syntax::SyntaxDef {
                    name,
                    ..Default::default()
                });
            } else {
                let def = ed.syntax.def_mut();
                def.items.retain(|item| item.group != group);
            }
        }
        "keyword" => {
            let (group, words) = split_word(rest);
            if group.is_empty() {
                return Err(VimError::new("E471", "Argument required"));
            }
            let words: Vec<&str> = words.split_whitespace().collect();
            if words.is_empty() {
                return Err(VimError::new("E471", "Argument required"));
            }
            ed.syntax.add_item(
                group,
                ItemKind::Keyword {
                    words: words.iter().map(|w| w.to_string()).collect(),
                },
            );
        }
        "match" => {
            let (group, rest) = split_word(rest);
            let (pattern, options) = take_pattern(rest)?;
            ed.syntax.add_item(
                group,
                ItemKind::Match {
                    pattern,
                    contains: parse_contains(options),
                },
            );
        }
        "region" => {
            let (group, rest) = split_word(rest);
            let (start, rest) = take_start(rest)?;
            let (end, options) = take_end(rest)?;
            ed.syntax.add_item(
                group,
                ItemKind::Region {
                    start,
                    end,
                    contains: parse_contains(options),
                },
            );
        }
        "cluster" => {
            let (name, rest) = split_word(rest);
            let rest = rest.trim();
            let groups = rest
                .strip_prefix("add=")
                .unwrap_or(rest)
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            ed.syntax.cluster_add(name, &groups);
        }
        "case" => match rest.trim() {
            "ignore" => ed.syntax.set_ignore_case(true),
            "match" => ed.syntax.set_ignore_case(false),
            _ => {}
        },
        "list" => list_syntax(ed, rest.trim()),
        // sync/include/contained/... are accepted but not modelled.
        _ => {}
    }
    Ok(ExOutcome::Continue)
}

fn list_syntax(ed: &mut Editor, filter: &str) {
    let Some(engine) = ed.syntax.engine() else {
        ed.messages.info("No syntax items defined".to_string());
        return;
    };
    let mut lines: Vec<String> = Vec::new();
    for item in engine.items() {
        if !filter.is_empty() && item.group != filter {
            continue;
        }
        let text = match &item.kind {
            ItemKind::Keyword { words } => format!("keyword {} {}", item.group, words.join(" ")),
            ItemKind::Match { pattern, .. } => format!("match {} /{pattern}/", item.group),
            ItemKind::Region { start, end, .. } => {
                format!("region {} start=/{start}/ end=/{end}/", item.group)
            }
        };
        lines.push(text);
    }
    if lines.is_empty() {
        ed.messages.info("No syntax items defined".to_string());
    }
    for line in lines {
        ed.messages.info(line);
    }
}

/// Take a `/pattern/` argument, returning the pattern and the rest.
fn take_pattern(args: &str) -> VimResult<(String, &str)> {
    let args = args.trim_start();
    let Some(rest) = args.strip_prefix('/') else {
        return Err(VimError::new("E475", "Argument required"));
    };
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\\' => {
                if let Some((_, next)) = chars.next() {
                    if next == '/' {
                        out.push('/');
                    } else {
                        out.push('\\');
                        out.push(next);
                    }
                }
            }
            '/' => return Ok((out, &rest[index + 1..])),
            other => out.push(other),
        }
    }
    Ok((out, ""))
}

/// `start=/re/` within a `:syntax region` argument.
fn take_start(args: &str) -> VimResult<(String, &str)> {
    let args = args.trim_start();
    let Some(rest) = args.strip_prefix("start=") else {
        return Err(VimError::new("E475", "Argument required"));
    };
    take_pattern(rest)
}

/// `end=/re/` within a `:syntax region` argument.
fn take_end(args: &str) -> VimResult<(String, &str)> {
    let args = args.trim_start();
    let Some(rest) = args.strip_prefix("end=") else {
        return Err(VimError::new("E475", "Argument required"));
    };
    take_pattern(rest)
}

/// `contains=a,@b` from the trailing options.
fn parse_contains(options: &str) -> Vec<String> {
    for word in options.split_whitespace() {
        if let Some(list) = word.strip_prefix("contains=") {
            return list
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

/// `:highlight [clear|link|Group [key=value...]]`.
pub fn cmd_highlight(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let args = p.args.trim();
    if args.is_empty() {
        list_highlights(ed);
        return Ok(ExOutcome::Continue);
    }
    let (first, rest) = split_word(args);
    match first {
        "clear" => {
            let group = rest.trim();
            ed.syntax
                .highlights
                .clear(if group.is_empty() { None } else { Some(group) });
            return Ok(ExOutcome::Continue);
        }
        "link" | "def" => {
            let rest = rest.trim();
            let rest = rest.strip_prefix("link").unwrap_or(rest).trim();
            let (from, to) = split_word(rest);
            if from.is_empty() || to.is_empty() {
                return Err(VimError::new("E412", "Not enough arguments for :highlight"));
            }
            ed.syntax.highlights.link(from, to.trim());
            return Ok(ExOutcome::Continue);
        }
        _ => {}
    }
    // `Group` alone prints; `Group key=value...` sets attributes.
    let group = first;
    let mut attr = HlAttr::default();
    let mut touched = false;
    for word in rest.split_whitespace() {
        let Some((key, value)) = word.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "guifg" | "ctermfg" => {
                attr.fg = if value.eq_ignore_ascii_case("NONE") {
                    None
                } else {
                    parse_color(value)
                };
                touched = true;
            }
            "guibg" | "ctermbg" => {
                attr.bg = if value.eq_ignore_ascii_case("NONE") {
                    None
                } else {
                    parse_color(value)
                };
                touched = true;
            }
            "gui" | "cterm" => {
                let parsed = parse_attrs(value);
                attr.bold |= parsed.bold;
                attr.italic |= parsed.italic;
                attr.underline |= parsed.underline;
                attr.reverse |= parsed.reverse;
                touched = true;
            }
            _ => {}
        }
    }
    if touched {
        let mut merged = ed.syntax.highlights.resolve(group).unwrap_or_default();
        merged.merge(&attr);
        ed.syntax.highlights.set(group, merged);
        return Ok(ExOutcome::Continue);
    }
    match ed.syntax.highlights.resolve(group) {
        Some(attr) => {
            let fg = describe(attr.fg);
            let bg = describe(attr.bg);
            let mut flags = Vec::new();
            if attr.bold {
                flags.push("bold");
            }
            if attr.italic {
                flags.push("italic");
            }
            if attr.underline {
                flags.push("underline");
            }
            if attr.reverse {
                flags.push("reverse");
            }
            ed.messages
                .info(format!("{group} fg={fg} bg={bg} gui={}", flags.join(",")));
        }
        None => ed
            .messages
            .info(format!("E411: Highlight group not found: {group}")),
    }
    Ok(ExOutcome::Continue)
}

fn describe(color: Option<HlColor>) -> String {
    match color {
        Some(HlColor::Rgb(r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
        Some(HlColor::Indexed(index)) => index.to_string(),
        None => "NONE".to_string(),
    }
}

fn list_highlights(ed: &mut Editor) {
    let names: Vec<String> = ed
        .syntax
        .highlights
        .groups()
        .into_iter()
        .map(str::to_string)
        .collect();
    for name in names {
        ed.messages.info(name);
    }
}

/// `:syntax` name completion helper (used by tests and `:syntax list`).
pub fn syntax_names() -> &'static [&'static str] {
    lang::NAMES
}

/// One parsed highlight-group definition (exposed for tests).
pub fn group_item(group: &str, pattern: &str) -> Item {
    Item {
        id: 0,
        group: group.to_string(),
        kind: ItemKind::Match {
            pattern: pattern.to_string(),
            contains: Vec::new(),
        },
    }
}
