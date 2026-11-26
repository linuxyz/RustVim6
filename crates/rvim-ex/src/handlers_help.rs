//! Offline help navigation using runtime/doc/tags and the bundled text files.

use crate::{ExOutcome, script::ScriptState};
use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::{Editor, OptValue, SetScope};
use rvim_viml::Env;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
struct HelpTag {
    name: String,
    file: PathBuf,
}

fn doc_dirs(ed: &Editor) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    ed.options
        .str_or("runtimepath", "")
        .split(',')
        .filter(|part| !part.is_empty())
        .map(|part| ed.cwd.join(part).join("doc"))
        .filter(|dir| dir.is_dir())
        .filter_map(|dir| dir.canonicalize().ok())
        .filter(|dir| seen.insert(dir.clone()))
        .collect()
}

fn help_tags(ed: &Editor) -> Vec<HelpTag> {
    let mut tags = Vec::new();
    for dir in doc_dirs(ed) {
        let Ok(contents) = std::fs::read_to_string(dir.join("tags")) else {
            continue;
        };
        for line in contents.lines().filter(|line| !line.starts_with("!_TAG_")) {
            let mut fields = line.split('\t');
            if let (Some(name), Some(file)) = (fields.next(), fields.next()) {
                if !name.is_empty() && !file.is_empty() {
                    tags.push(HelpTag {
                        name: name.to_string(),
                        file: dir.join(file),
                    });
                }
            }
        }
    }
    tags
}

fn lookup(ed: &Editor, topic: &str) -> VimResult<(HelpTag, Pos)> {
    let topic = if topic.trim().is_empty() || topic.trim() == "help" {
        "help.txt"
    } else {
        topic.trim()
    };
    let topic = topic.strip_suffix("@en").unwrap_or(topic);
    let tags = help_tags(ed);
    let aliases = [
        topic.to_string(),
        format!("'{topic}'"),
        format!(":{topic}"),
        format!("{topic}()"),
    ];
    let entry = aliases
        .iter()
        .find_map(|alias| {
            tags.iter()
                .find(|tag| tag.name == *alias && tag.file.is_file())
        })
        .or_else(|| {
            tags.iter()
                .find(|tag| tag.name.eq_ignore_ascii_case(topic) && tag.file.is_file())
        })
        .ok_or_else(|| VimError::new("E149", format!("Sorry, no help for {topic}")))?;
    let contents = std::fs::read_to_string(&entry.file).map_err(|error| {
        VimError::new(
            "E484",
            format!("Can't open help file {}: {error}", entry.file.display()),
        )
    })?;
    // Tags are literal *markers*, not executable Ex commands from the index.
    let marker = format!("*{}*", entry.name);
    let pos = contents
        .lines()
        .enumerate()
        .find_map(|(line, text)| text.find(&marker).map(|col| Pos::new(line, col)))
        .ok_or_else(|| VimError::new("E434", format!("Can't find tag pattern: {}", entry.name)))?;
    Ok((entry.clone(), pos))
}

pub fn cmd_help(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    topic: &str,
) -> VimResult<ExOutcome> {
    // Resolve before changing any windows, so a failed lookup is harmless.
    let (target, pos) = lookup(ed, topic)?;
    ed.push_jump();
    let existing = ed.windows.ids().into_iter().find(|id| {
        ed.windows
            .get(*id)
            .and_then(|win| ed.buffer(win.bufnr).ok())
            .is_some_and(|buf| buf.is_help())
    });
    let previous = if let Some(id) = existing {
        ed.windows.goto(id)?;
        Some((ed.current_bufnr(), ed.cursor()))
    } else {
        ed.split_window(false)?;
        // Help defaults must not inherit a source window's editing controls.
        let win = ed.windows.current_mut();
        for option in ["buftype", "filetype", "modifiable", "readonly"] {
            win.local.remove(option);
        }
        ed.set_option("number", OptValue::Bool(false), SetScope::Local)?;
        ed.set_option("relativenumber", OptValue::Bool(false), SetScope::Local)?;
        None
    };
    crate::autodispatch::edit_file(ed, env, state, &target.file)?;
    let buffer = ed.current_buffer_mut();
    buffer.set_help(true);
    buffer.set_readonly(true);
    ed.set_cursor(pos);
    ed.windows.current_mut().top_line = pos.line.saturating_sub(2);
    if let Some((bufnr, from)) = previous {
        crate::handlers_tags::push_tag_stack(
            state,
            crate::host_win::TagStackEntry {
                tagname: target.name,
                matchnr: 1,
                bufnr: bufnr as i64,
                from: [bufnr as i64, from.line as i64 + 1, from.col as i64, 0],
                user_data: None,
            },
        );
    }
    Ok(ExOutcome::Continue)
}

pub fn cmd_helpclose(ed: &mut Editor) -> VimResult<ExOutcome> {
    let help = ed.windows.ids().into_iter().find(|id| {
        ed.windows
            .get(*id)
            .and_then(|win| ed.buffer(win.bufnr).ok())
            .is_some_and(|buf| buf.is_help())
    });
    if let Some(id) = help {
        ed.windows.goto(id)?;
        ed.close_current_window(false)?;
    }
    Ok(ExOutcome::Continue)
}

/// Preserve punctuation in |links|, *definitions*, option names and key names.
pub fn topic_at_cursor(ed: &Editor) -> VimResult<String> {
    let text = ed.current_line();
    let col = rvim_core::text::snap_byte(&text, ed.cursor().col);
    for delimiter in ['|', '*'] {
        let mut start = 0;
        while let Some(open) = text[start..].find(delimiter).map(|at| start + at) {
            let Some(close) = text[open + 1..].find(delimiter).map(|at| open + 1 + at) else {
                break;
            };
            if open <= col && col <= close && close > open + 1 {
                return Ok(text[open + 1..close].to_string());
            }
            start = close + 1;
        }
    }
    let start = text[..rvim_core::text::snap_byte(&text, col)]
        .rfind(char::is_whitespace)
        .map(|at| at + 1)
        .unwrap_or(0);
    let end = text[col..]
        .find(char::is_whitespace)
        .map(|at| col + at)
        .unwrap_or(text.len());
    let topic = text[start..end].trim_matches(['|', '*']);
    if topic.is_empty() {
        Err(VimError::new("E349", "No identifier under cursor"))
    } else {
        Ok(topic.to_string())
    }
}

pub fn complete_topics(ed: &Editor, prefix: &str, max: usize) -> Vec<String> {
    let mut names = help_tags(ed)
        .into_iter()
        .map(|tag| tag.name)
        .filter(|name| {
            name.starts_with(prefix) || name.trim_start_matches(['\'', ':']).starts_with(prefix)
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names.truncate(max);
    names
}

fn collect_docs(dir: &Path, files: &mut Vec<PathBuf>) -> VimResult<()> {
    let entries = std::fs::read_dir(dir).map_err(|error| {
        VimError::new("E150", format!("Cannot open {}: {error}", dir.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| VimError::new("E150", error.to_string()))?;
        // Do not follow directory symlinks: avoid loops in plugin doc trees.
        let kind = entry
            .file_type()
            .map_err(|error| VimError::new("E150", error.to_string()))?;
        if kind.is_dir() {
            collect_docs(&entry.path(), files)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "txt") {
            files.push(entry.path());
        }
    }
    Ok(())
}

pub fn generate_tags(dir: &Path) -> VimResult<()> {
    let mut files = Vec::new();
    collect_docs(dir, &mut files)?;
    files.sort();
    let mut tags = BTreeMap::new();
    for file in files {
        let contents = std::fs::read_to_string(&file)
            .map_err(|error| VimError::new("E150", error.to_string()))?;
        let relative = file
            .strip_prefix(dir)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        for line in contents.lines() {
            let mut start = 0;
            while let Some(open) = line[start..].find('*').map(|at| start + at) {
                let Some(close) = line[open + 1..].find('*').map(|at| open + 1 + at) else {
                    break;
                };
                let tag = &line[open + 1..close];
                let before = open == 0
                    || line[..open]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_whitespace);
                let after = close + 1 == line.len()
                    || line[close + 1..]
                        .chars()
                        .next()
                        .is_some_and(char::is_whitespace);
                if before
                    && after
                    && !tag.is_empty()
                    && !tag.chars().any(char::is_whitespace)
                    && tags.insert(tag.to_string(), relative.clone()).is_some()
                {
                    return Err(VimError::new("E154", format!("Duplicate tag {tag}")));
                }
                start = close + 1;
            }
        }
    }
    let mut output = String::new();
    for (tag, file) in tags {
        output.push_str(&format!("{tag}\t{file}\t/*{tag}*\n"));
    }
    std::fs::write(dir.join("tags"), output)
        .map_err(|error| VimError::new("E152", format!("Cannot write help tags: {error}")))
}

pub fn cmd_helptags(ed: &Editor, arg: &str) -> VimResult<ExOutcome> {
    if arg.trim() == "ALL" {
        for dir in doc_dirs(ed) {
            generate_tags(&dir)?;
        }
    } else if !arg.trim().is_empty() {
        generate_tags(&ed.cwd.join(arg.trim()))?;
    } else {
        return Err(VimError::new("E471", "Argument required"));
    }
    Ok(ExOutcome::Continue)
}
