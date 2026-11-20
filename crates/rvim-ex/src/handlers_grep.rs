//! `:vimgrep` / `:lvimgrep` (reference: quickfix.c ex_vimgrep).
//!
//! The search itself is line based and uses the shared Vim regexp engine.  File
//! arguments may contain `*`/`?`/`[...]` wildcards in their last component;
//! without arguments the argument list is searched, like Vim.

use std::path::{Path, PathBuf};

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_viml::Env;
use rvim_viml::regex_mvp;
use rvim_viml::value::{Value, VimDict};

use crate::exec::ExOutcome;
use crate::parser::ParsedEx;
use crate::script::ScriptState;

/// Dispatch `:vimgrep`/`:lvimgrep` and their `add` forms.
pub fn cmd_vimgrep(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    let args = p.args.trim_start();
    let mut chars = args.char_indices();
    let Some((_, delim)) = chars.next() else {
        return Err(VimError::new("E471", "Argument required"));
    };
    let rest = &args[delim.len_utf8()..];
    let mut pattern = String::new();
    let mut escaped = false;
    let mut end_index = None;
    for (index, ch) in rest.char_indices() {
        if escaped {
            if ch != delim {
                pattern.push('\\');
            }
            pattern.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == delim {
            end_index = Some(index + ch.len_utf8());
            break;
        } else {
            pattern.push(ch);
        }
    }
    let Some(end_index) = end_index else {
        return Err(VimError::new("E146", format!("Invalid argument: {args}")));
    };
    let tail = rest[end_index..].trim_start();
    // Flags: g (all matches per line), j (do not jump), a (add to the list).
    let (flags, files) = split_flags(tail);
    let all_matches = flags.contains('g');
    let no_jump = flags.contains('j');
    let append = flags.contains('a') || name.ends_with("add");
    let mut paths = if files.is_empty() {
        ed.arglist.clone()
    } else {
        expand_files(ed, files)
    };
    paths.retain(|path| path.is_file());
    let mut items: Vec<Value> = Vec::new();
    let mut first: Option<(PathBuf, usize, usize)> = None;
    let mut files_with_hits = 0usize;
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut hit_file = false;
        for (lnum, line) in text.lines().enumerate() {
            let mut from = 0usize;
            while let Some((start, end)) = regex_mvp::find(&pattern, &line[from..], false) {
                let abs = from + start;
                let mut dict = VimDict::new();
                dict.insert("filename", Value::string(path.display().to_string()));
                dict.insert("lnum", Value::Number(lnum as i64 + 1));
                dict.insert("col", Value::Number(abs as i64 + 1));
                dict.insert("text", Value::string(line.to_string()));
                items.push(Value::dict(dict));
                hit_file = true;
                if first.is_none() {
                    first = Some((path.clone(), lnum, abs));
                }
                if !all_matches {
                    break;
                }
                let step = (end - start).max(1);
                from += start + step;
                if from > line.len() {
                    break;
                }
            }
        }
        if hit_file {
            files_with_hits += 1;
        }
    }
    if items.is_empty() {
        return Err(VimError::new(
            "E480",
            format!("No match found for {pattern}"),
        ));
    }
    let count = items.len();
    let list = Value::list(items);
    let which = if name.starts_with('l') { 1 } else { 0 };
    let win = if which == 1 {
        Some(ed.current_win_id())
    } else {
        None
    };
    if append {
        // ':vimgrepadd' appends to the current list (action 'a').
        crate::handlers_qf::builtin_setqflist(ed, &[list, Value::string("a")], which, win)
            .map_err(|e| VimError::new(e.code, e.message))?;
    } else {
        crate::handlers_qf::builtin_setqflist(ed, &[list], which, win)
            .map_err(|e| VimError::new(e.code, e.message))?;
    }
    ed.messages.info(format!(
        "{count} match{} in {files_with_hits} file{}",
        if count == 1 { "" } else { "es" },
        if files_with_hits == 1 { "" } else { "s" }
    ));
    if !no_jump {
        if let Some((path, lnum, col)) = first {
            crate::autodispatch::edit_file(ed, env, state, &path)?;
            let pos = rvim_core::Pos::new(lnum, col);
            ed.set_cursor(pos);
        }
    }
    Ok(ExOutcome::Continue)
}

/// Flags end at the first whitespace or `:` (Vim accepts `/pat/gj` and
/// `/pat/g file`).
fn split_flags(tail: &str) -> (String, &str) {
    let cut = tail
        .find(|c: char| c.is_whitespace() || c == ':')
        .unwrap_or(tail.len());
    let flags: String = tail[..cut]
        .chars()
        .filter(|c| matches!(c, 'g' | 'j' | 'a'))
        .collect();
    let rest = tail[cut..].trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
    (flags, rest)
}

/// Expand the last path component of every argument (simple `*`/`?`/class
/// wildcards); arguments without wildcards are kept as-is.
fn expand_files(ed: &Editor, files: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for word in files.split_whitespace() {
        let word = word.trim_matches(|c| c == '\'' || c == '"');
        let path = PathBuf::from(word);
        let has_wildcard = word.contains(['*', '?', '[']);
        if !has_wildcard {
            out.push(if path.is_absolute() {
                path
            } else {
                ed.cwd.join(path)
            });
            continue;
        }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
        let dir = match parent {
            Some(parent) => {
                if parent.is_absolute() {
                    parent.to_path_buf()
                } else {
                    ed.cwd.join(parent)
                }
            }
            None => ed.cwd.clone(),
        };
        let pattern = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(word);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if crate::wildcard::matches(pattern, name) {
                out.push(entry.path());
            }
        }
    }
    out
}

#[allow(dead_code)]
fn _path(p: &Path) -> &Path {
    p
}
