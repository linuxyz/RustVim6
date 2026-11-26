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
use rvim_syntax::{Item, ItemKind, ItemOptions, SyncSpec};

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
                let live: Vec<i64> = def.items.iter().map(|item| item.id).collect();
                def.item_options.retain(|id, _| live.contains(id));
            }
        }
        "keyword" => {
            let (group, words) = split_word(rest);
            if group.is_empty() {
                return Err(VimError::new("E471", "Argument required"));
            }
            let (words, options) = split_words_and_options(words);
            if words.is_empty() {
                return Err(VimError::new("E471", "Argument required"));
            }
            ed.syntax
                .add_item_with_options(group, ItemKind::Keyword { words }, options);
        }
        "match" => {
            let (group, rest) = split_word(rest);
            let (pattern, options) = take_pattern(rest)?;
            let (flags, contains) = parse_item_options(options);
            ed.syntax
                .add_item_with_options(group, ItemKind::Match { pattern, contains }, flags);
        }
        "region" => {
            let (group, rest) = split_word(rest);
            let (start, rest) = take_start(rest)?;
            let (end, options) = take_end(rest)?;
            let (flags, contains) = parse_item_options(options);
            ed.syntax.add_item_with_options(
                group,
                ItemKind::Region {
                    start,
                    end,
                    contains,
                },
                flags,
            );
        }
        "sync" => {
            let mut sync = SyncSpec::default();
            for word in rest.split_whitespace() {
                let lower = word.to_ascii_lowercase();
                if lower == "fromstart" {
                    sync.from_start = true;
                } else if lower == "ccomment" {
                    sync.ccomment = true;
                } else if let Some(n) = lower.strip_prefix("minlines=").and_then(|n| n.parse().ok())
                {
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
            ed.syntax.set_sync(sync);
        }
        "include" => {
            let mut rest = rest.trim();
            let mut cluster = None;
            if let Some(remain) = rest.strip_prefix('@') {
                let (name, after) = split_word(remain);
                cluster = Some(name.to_string());
                rest = after.trim();
            }
            if rest.is_empty() {
                return Err(VimError::new("E471", "Argument required"));
            }
            let path = resolve_syntax_file(ed, rest)
                .ok_or_else(|| VimError::new("E484", format!("Can't open file: {rest}")))?;
            let bytes = std::fs::read(&path).map_err(|_| {
                VimError::new("E484", format!("Can't open file: {}", path.display()))
            })?;
            let text = String::from_utf8_lossy(&bytes);
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("include")
                .to_string();
            ed.syntax.include_script(&name, &text, cluster.as_deref());
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
        let mut text = match &item.kind {
            ItemKind::Keyword { words } => format!("keyword {} {}", item.group, words.join(" ")),
            ItemKind::Match { pattern, .. } => format!("match {} /{pattern}/", item.group),
            ItemKind::Region { start, end, .. } => {
                format!("region {} start=/{start}/ end=/{end}/", item.group)
            }
        };
        let flags = engine.options_of(item.id);
        if flags.contained {
            text.push_str(" contained");
        }
        if flags.transparent {
            text.push_str(" transparent");
        }
        if !flags.nextgroup.is_empty() {
            text.push_str(&format!(" nextgroup={}", flags.nextgroup.join(",")));
        }
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

/// Trailing options of one item: flags plus the `contains=` list.
fn parse_item_options(options: &str) -> (ItemOptions, Vec<String>) {
    let mut flags = ItemOptions::default();
    let mut contains = Vec::new();
    for word in options.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        match lower.as_str() {
            "contained" => flags.contained = true,
            "transparent" => flags.transparent = true,
            "display" => flags.display = true,
            "conceal" => flags.conceal = true,
            "fold" => flags.fold = true,
            "skipwhite" => flags.skip = Some("[ \t]*".to_string()),
            "skipnl" | "skipempty" => flags.skip = Some("\n".to_string()),
            _ => {
                if let Some(list) = word.strip_prefix("contains=") {
                    contains.extend(
                        list.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                    );
                } else if let Some(list) = word.strip_prefix("nextgroup=") {
                    flags.nextgroup.extend(
                        list.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                    );
                } else if let Some(value) = word.strip_prefix("skip=") {
                    if let Ok((pattern, _)) = take_pattern(value) {
                        flags.skip = Some(pattern);
                    }
                }
            }
        }
    }
    (flags, contains)
}

/// Keywords are followed by flags (`contained`, `nextgroup=...`); tell the
/// two apart the way `syntax.c syn_cmd_keyword` does.
fn split_words_and_options(text: &str) -> (Vec<String>, ItemOptions) {
    let mut words = Vec::new();
    let mut flags = ItemOptions::default();
    for word in text.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        let is_option = matches!(
            lower.as_str(),
            "contained"
                | "transparent"
                | "display"
                | "conceal"
                | "fold"
                | "skipwhite"
                | "skipnl"
                | "skipempty"
        ) || lower.starts_with("nextgroup=")
            || lower.starts_with("skip=")
            || lower.starts_with("contains=");
        if !is_option {
            words.push(word.to_string());
            continue;
        }
        if let Some(list) = word.strip_prefix("nextgroup=") {
            flags.nextgroup.extend(
                list.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            );
        } else if lower == "contained" {
            flags.contained = true;
        } else if lower == "transparent" {
            flags.transparent = true;
        } else if lower == "display" {
            flags.display = true;
        } else if lower == "conceal" {
            flags.conceal = true;
        } else if lower == "fold" {
            flags.fold = true;
        } else if lower == "skipwhite" {
            flags.skip = Some("[ \t]*".to_string());
        } else if lower == "skipnl" || lower == "skipempty" {
            flags.skip = Some("\\n".to_string());
        } else if let Some(value) = word.strip_prefix("skip=") {
            if let Ok((pattern, _)) = take_pattern(value) {
                flags.skip = Some(pattern);
            }
        }
    }
    (words, flags)
}

/// A `:syntax include` argument: an existing path or `syntax/<name>.vim` on
/// 'runtimepath'.
fn resolve_syntax_file(ed: &Editor, name: &str) -> Option<std::path::PathBuf> {
    let direct = std::path::PathBuf::from(name);
    if direct.is_file() {
        return Some(direct);
    }
    let rtp = ed.options.str_or("runtimepath", "");
    for dir in rtp.split(',').filter(|d| !d.is_empty()) {
        let path = std::path::Path::new(dir).join(name);
        if path.is_file() {
            return Some(path);
        }
        let path = std::path::Path::new(dir).join("syntax").join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// `:highlight [clear|link|Group [key=value...]]`.
pub fn cmd_highlight(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let args = p.args.trim();
    if args.is_empty() {
        list_highlights(ed);
        return Ok(ExOutcome::Continue);
    }
    let (mut first, mut rest) = split_word(args);
    let default = matches!(first, "default" | "def");
    if default {
        (first, rest) = split_word(rest);
    }
    match first {
        "clear" => {
            let group = rest.trim();
            if group.is_empty() {
                // `:hi clear` resets every group to Vim's defaults
                // (highlight.c highlight_clear()).
                ed.syntax.highlights = rvim_syntax::highlight::HighlightTable::with_defaults();
            } else {
                ed.syntax.highlights.clear(Some(group));
            }
            return Ok(ExOutcome::Continue);
        }
        "link" => {
            let rest = rest.trim();
            let (from, to) = split_word(rest);
            if from.is_empty() || to.is_empty() {
                return Err(VimError::new("E412", "Not enough arguments for :highlight"));
            }
            if p.bang || !ed.syntax.highlights.exists(from) {
                if to.trim().eq_ignore_ascii_case("NONE") {
                    ed.syntax.highlights.clear(Some(from));
                } else {
                    ed.syntax.highlights.force_link(from, to.trim());
                }
            }
            return Ok(ExOutcome::Continue);
        }
        _ => {}
    }
    // `Group` alone prints; `Group key=value...` sets attributes.
    let group = first;
    if default && ed.syntax.highlights.exists(group) {
        return Ok(ExOutcome::Continue);
    }
    if rest.eq_ignore_ascii_case("NONE") {
        ed.syntax.highlights.clear(Some(group));
        ed.syntax.highlights.set(group, HlAttr::default());
        ed.syntax.highlights.set_cterm(group, HlAttr::default());
        return Ok(ExOutcome::Continue);
    }
    // GUI and terminal definitions coexist: later cterm lines in a runtime
    // scheme must not overwrite its RGB colors. NONE explicitly clears a field.
    for gui in [true, false] {
        let mut attr = ed
            .syntax
            .highlights
            .attributes(group, gui)
            .unwrap_or_default();
        let mut touched = false;
        for word in rest.split_whitespace() {
            let Some((key, value)) = word.split_once('=') else {
                continue;
            };
            let value = value.trim();
            if (gui && !key.starts_with("gui")) || (!gui && !key.starts_with("cterm")) {
                continue;
            }
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
                    attr.bold = parsed.bold;
                    attr.italic = parsed.italic;
                    attr.underline = parsed.underline;
                    attr.reverse = parsed.reverse;
                    touched = true;
                }
                _ => {}
            }
        }
        if touched {
            if gui {
                ed.syntax.highlights.set(group, attr);
            } else {
                ed.syntax.highlights.set_cterm(group, attr);
            }
        }
    }
    if rest.contains('=') {
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
