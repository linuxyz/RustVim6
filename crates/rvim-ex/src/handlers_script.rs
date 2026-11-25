//! Script-ecosystem Ex commands: `:autocmd`, `:augroup`, `:command`,
//! `:source`, `:runtime`, `:finish`, `:doautocmd`.

use std::path::{Path, PathBuf};

use rvim_core::{VimError, VimResult};
use rvim_editor::autocmd::canonical_event;
use rvim_editor::{Editor, Nargs, UserCommand};

use crate::exec::ExOutcome;
use crate::parser::ParsedEx;
use crate::script::{self, ScriptState};
use rvim_viml::Env;

/// Guard against runaway `:source` recursion.
const MAX_SCRIPT_DEPTH: usize = 50;

// ---------------------------------------------------------------- :augroup

pub fn cmd_augroup(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let name = p.args.trim().to_string();
    if p.bang {
        match name.as_str() {
            "" => {
                if let Some(current) = ed.autocmds.current_group().map(str::to_string) {
                    ed.autocmds.delete_group(&current);
                } else {
                    return Err(VimError::new(
                        "E367",
                        "No current augroup to delete".to_string(),
                    ));
                }
            }
            other => ed.autocmds.delete_group(other),
        }
        return Ok(ExOutcome::Continue);
    }
    if name.is_empty() {
        for g in ed.autocmds.groups() {
            ed.messages.info(g.clone());
        }
        return Ok(ExOutcome::Continue);
    }
    if name.eq_ignore_ascii_case("END") {
        ed.autocmds.end_group();
    } else {
        ed.autocmds.define_group(&name);
    }
    Ok(ExOutcome::Continue)
}

// --------------------------------------------------------------- :autocmd

/// Split off the next whitespace-delimited token; returns (token, rest).
fn next_token(text: &str) -> (&str, &str) {
    let t = text.trim_start();
    match t.find([' ', '\t']) {
        Some(idx) => (&t[..idx], t[idx..].trim_start()),
        None => (t, ""),
    }
}

fn parse_events(spec: &str) -> VimResult<Vec<String>> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part == "*" || part.eq_ignore_ascii_case("all") {
            out.push("*".to_string());
        } else {
            let canonical = canonical_event(part)
                .ok_or_else(|| VimError::new("E216", format!("No such event: {part}")))?;
            out.push(canonical.to_string());
        }
    }
    if out.is_empty() {
        return Err(VimError::new("E216", "No such event".to_string()));
    }
    Ok(out)
}

pub fn cmd_autocmd(
    ed: &mut Editor,
    _env: &mut Env,
    _state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let mut rest = p.args.trim_start();
    // A bare ":autocmd" lists everything; ":autocmd!" removes every autocommand
    // of the *current* augroup (or the ungrouped ones when no augroup is active,
    // autocmd.c:1072-1088 findgroup = current_augroup).
    let current_group = ed.autocmds.current_group().map(str::to_string);
    if rest.is_empty() {
        if p.bang {
            let group = current_group.unwrap_or_default();
            ed.autocmds.remove(Some(&group), None, None);
            return Ok(ExOutcome::Continue);
        }
        for line in ed.autocmds.list() {
            ed.messages.info(line);
        }
        return Ok(ExOutcome::Continue);
    }

    // Optional group name: the first token when it matches a defined group.
    let mut group: Option<String> = None;
    let (first, after_first) = next_token(rest);
    if ed.autocmds.groups().iter().any(|g| g == first) {
        group = Some(first.to_string());
        rest = after_first;
    }

    // Event list, then pattern, then ++flags, then the command text.
    let mut events: Vec<String> = Vec::new();
    if !rest.is_empty() {
        let (event_spec, after_events) = next_token(rest);
        if !event_spec.is_empty() {
            match parse_events(event_spec) {
                Ok(list) => {
                    events = list;
                    rest = after_events;
                }
                Err(_e) => {
                    // autocmd.c:702-708: without a group the token can be a
                    // misspelled group name, so report the whole remainder.
                    return Err(if group.is_some() {
                        VimError::new("E216", format!("No such event: {event_spec}"))
                    } else {
                        VimError::new("E216", format!("No such group or event: {rest}"))
                    });
                }
            }
        }
    }
    let mut pattern = String::new();
    if !rest.is_empty() {
        let (pat, after_pat) = next_token(rest);
        pattern = pat.to_string();
        rest = after_pat;
    }
    let mut nested = false;
    let mut once = false;
    loop {
        let (tok, after) = next_token(rest);
        match tok {
            "++nested" => {
                nested = true;
                rest = after;
            }
            "++once" => {
                once = true;
                rest = after;
            }
            _ => break,
        }
    }
    let command = rest.trim().to_string();
    // When no augroup is active the "current group" is the empty (ungrouped)
    // one; vim's AUGROUP_ALL entries (autocmd.c:1072-1088).
    let remove_group = group.clone().or(current_group).unwrap_or_default();

    if p.bang {
        // ":autocmd! ..." removes matching entries; with a command text it
        // removes and then defines the replacement (vim's replace semantics).
        let event_filters: Vec<Option<String>> = if events.is_empty() {
            vec![None]
        } else {
            events.iter().map(|e| Some(e.clone())).collect()
        };
        for ev in &event_filters {
            ed.autocmds.remove(
                Some(&remove_group),
                ev.as_deref(),
                if pattern.is_empty() {
                    None
                } else {
                    Some(&pattern)
                },
            );
        }
        if command.is_empty() {
            return Ok(ExOutcome::Continue);
        }
    }

    if command.is_empty() {
        // Listing form: `:autocmd [group] {event} [pat]`.
        for line in ed.autocmds.list() {
            ed.messages.info(line);
        }
        return Ok(ExOutcome::Continue);
    }

    let events = if events.is_empty() {
        vec!["*".to_string()]
    } else {
        events
    };
    ed.autocmds
        .add(group.as_deref(), &events, &pattern, nested, once, &command)?;
    Ok(ExOutcome::Continue)
}

pub fn cmd_doautocmd(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let mut rest = p.args.trim_start();
    let (first, after) = next_token(rest);
    let mut event = first.to_string();
    rest = after;
    // Optional group name before the event.
    if ed.autocmds.groups().iter().any(|g| g == first) {
        let (ev, after2) = next_token(rest);
        event = ev.to_string();
        rest = after2;
    }
    if event.is_empty() {
        return Err(VimError::new("E216", "Invalid event name".to_string()));
    }
    let canonical = canonical_event(&event)
        .ok_or_else(|| VimError::new("E216", format!("Invalid event name: {event}")))?;
    let name = rest.trim();
    // Execute through the shared dispatcher so `++once`, `++nested` and the
    // "autocommands are busy" suppression behave the same as for real events
    // (vim's :doautocmd applies the event with force = FALSE).
    crate::autodispatch::fire(ed, env, state, canonical, name);
    Ok(ExOutcome::Continue)
}

// --------------------------------------------------------------- :command

fn parse_command_def(args: &str) -> VimResult<Option<UserCommand>> {
    let mut rest = args.trim_start();
    if rest.is_empty() {
        return Ok(None);
    }
    let mut def = UserCommand {
        name: String::new(),
        bang: false,
        bar: false,
        register: false,
        range: false,
        count: None,
        nargs: Nargs::Zero,
        complete: String::new(),
        expr: false,
        command: String::new(),
    };
    loop {
        let (tok, after) = next_token(rest);
        if tok.is_empty() {
            return Ok(None);
        }
        if !tok.starts_with('-') {
            def.name = tok.to_string();
            rest = after;
            break;
        }
        rest = after;
        if let Some(v) = tok.strip_prefix("-nargs=") {
            def.nargs = Nargs::parse(v)?;
        } else if tok == "-nargs" {
            let (v, after2) = next_token(rest);
            def.nargs = Nargs::parse(v)?;
            rest = after2;
        } else if tok == "-bang" {
            def.bang = true;
        } else if tok == "-expr" {
            def.expr = true;
        } else if tok == "-bar" {
            def.bar = true;
        } else if tok == "-register" {
            def.register = true;
        } else if tok == "-buffer" {
            // Buffer-local commands are global in the M2 single-window model.
        } else if tok == "-range" {
            def.range = true;
        } else if let Some(v) = tok.strip_prefix("-range=") {
            def.range = true;
            if v == "%" {
                // EX_DFLALL: default range is the whole file (1,$). The
                // UserCommand struct has no dedicated flag, so use a sentinel
                // for the "count" slot (it is never a real count for %).
                def.count = Some(u32::MAX);
            } else {
                def.count = v.parse().ok();
            }
        } else if let Some(v) = tok.strip_prefix("-count=") {
            // -count implies a range whose default value is N (usercmd.c:891).
            def.range = true;
            def.count = v.parse().ok();
        } else if tok == "-count" {
            def.range = true;
            def.count = Some(0);
        } else if let Some(v) = tok.strip_prefix("-complete=") {
            def.complete = v.to_string();
        } else if tok == "-complete" {
            let (v, after2) = next_token(rest);
            def.complete = v.to_string();
            rest = after2;
        } else {
            return Err(VimError::new(
                "E175",
                format!("Invalid argument for :command: {tok}"),
            ));
        }
    }
    def.command = rest.trim().to_string();
    Ok(Some(def))
}

pub fn cmd_command(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    match parse_command_def(&p.args)? {
        None => {
            for cmd in ed.usercmds.entries() {
                let line = cmd.listing();
                ed.messages.info(line);
            }
            Ok(ExOutcome::Continue)
        }
        Some(def) => {
            ed.usercmds.define(def, p.bang)?;
            Ok(ExOutcome::Continue)
        }
    }
}

pub fn cmd_delcommand(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let name = p.args.trim();
    if name.is_empty() {
        return Err(VimError::range_required());
    }
    ed.usercmds.delete(name)?;
    Ok(ExOutcome::Continue)
}

pub fn cmd_comclear(ed: &mut Editor, _p: &ParsedEx) -> VimResult<ExOutcome> {
    let names: Vec<String> = ed
        .usercmds
        .entries()
        .iter()
        .map(|c| c.name.clone())
        .collect();
    for name in names {
        ed.usercmds.delete(&name)?;
    }
    Ok(ExOutcome::Continue)
}

// -------------------------------------------------------- :source / :runtime

/// List of 'runtimepath' entries, each expanded relative to the cwd.
fn runtime_paths(ed: &Editor) -> Vec<PathBuf> {
    ed.options
        .str_or("runtimepath", "")
        .split(',')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Source one script file. `:finish` inside the script stops only that script.
pub fn source_file(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    path: &Path,
) -> VimResult<ExOutcome> {
    if ed.script_depth >= MAX_SCRIPT_DEPTH {
        return Err(VimError::new(
            "E169",
            "Too deep recursion while sourcing scripts".to_string(),
        ));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| VimError::new("E484", format!("Can't open file {}: {e}", path.display())))?;
    if is_vim9_script(&text) {
        return source_vim9_script(ed, env, state, path, &text);
    }
    let sid = env.push_script();
    state.script_files.push(path.to_path_buf());
    ed.script_depth += 1;
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    let result = script::parse_script_lines(&lines, sid)
        .and_then(|stmts| script::exec_stmts(ed, env, state, &stmts));
    ed.script_depth -= 1;
    state.script_files.pop();
    env.pop_script();
    match result {
        Ok(script::Flow::Quit(ExOutcome::Finish)) => Ok(ExOutcome::Continue),
        Ok(script::Flow::Quit(other)) => Ok(other),
        // An exception thrown by the sourced script must reach the caller so a
        // surrounding :try can catch it (ex_docmd.c:1284-1285 need_rethrow,
        // ex_eval.c:1537/1567). The E605 error text carries the value; script.rs
        // turns it back into a thrown value for :catch.
        Ok(script::Flow::Throw(value)) => {
            let text = value.to_string_value();
            env.vvars.insert("exception".to_string(), value.clone());
            Err(VimError::aborting(
                "E605",
                format!("Exception not caught: {text}"),
            ))
        }
        Ok(_) => Ok(ExOutcome::Continue),
        Err(e) => Err(e),
    }
}
/// Source a Vim9 script: parse/compile the whole file, register its
/// functions and execute the top-level statements. Reference:
/// vim9script.c:335-438 (compile_script/compile_one_script).
fn source_vim9_script(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    path: &Path,
    text: &str,
) -> VimResult<ExOutcome> {
    let sid = env.push_script();
    state.script_files.push(path.to_path_buf());
    ed.script_depth += 1;
    let prefix = autoload_prefix(ed, path);
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    let parsed =
        rvim_viml::vim9::parser::parse_script(&lines, &path.to_string_lossy(), sid, &prefix);
    let result = match parsed {
        Ok(script) => {
            let mut host = crate::script::make_host(ed, state);
            rvim_viml::vim9::run_script(&script, env, &mut host)
        }
        Err(error) => Err(error),
    };
    ed.script_depth -= 1;
    state.script_files.pop();
    env.pop_script();
    result?;
    Ok(ExOutcome::Continue)
}

/// Canonical autoload prefix for a script path (dist/ft.vim -> dist#ft).
fn autoload_prefix(ed: &Editor, path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for base in runtime_paths(ed) {
        let base = std::fs::canonicalize(&base).unwrap_or(base);
        let Ok(relative) = canonical.strip_prefix(&base) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if let Some(rest) = relative.strip_prefix("autoload/") {
            if let Some(stem) = rest.strip_suffix(".vim") {
                return stem.replace('/', "#");
            }
        }
    }
    String::new()
}

/// Source the current buffer as a script (used by ':source' in Vim9 test
/// helpers and by interactive editing).
fn source_current_buffer(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
) -> VimResult<ExOutcome> {
    let text: String = {
        let buffer = ed.current_buffer();
        let mut lines = Vec::new();
        for i in 0..buffer.line_count() {
            lines.push(buffer.line_str(i)?.to_string());
        }
        lines.join("\n")
    };
    let path = std::path::PathBuf::from("<buffer>");
    if is_vim9_script(&text) {
        return source_vim9_script(ed, env, state, &path, &text);
    }
    let sid = env.push_script();
    state.script_files.push(path);
    ed.script_depth += 1;
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    let result = script::parse_script_lines(&lines, sid)
        .and_then(|stmts| script::exec_stmts(ed, env, state, &stmts));
    ed.script_depth -= 1;
    state.script_files.pop();
    env.pop_script();
    match result {
        Ok(script::Flow::Quit(ExOutcome::Finish)) => Ok(ExOutcome::Continue),
        Ok(script::Flow::Quit(other)) => Ok(other),
        Ok(script::Flow::Throw(value)) => Err(VimError::aborting(
            "E605",
            format!("Exception not caught: {}", value.to_string_value()),
        )),
        Ok(_) => Ok(ExOutcome::Continue),
        Err(error) => Err(error),
    }
}

pub fn cmd_source(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let arg = p.args.trim();
    if arg.is_empty() {
        // ':source' without an argument sources the current buffer
        // (scriptfile.c: do_source() with fname == NULL).
        return source_current_buffer(ed, env, state);
    }
    // ":source" does NOT search 'runtimepath' (scriptfile.c:1150-1175: the
    // name is used as-is, with only the usual special-character expansion).
    let path = PathBuf::from(script::expand_special(ed, state, arg));
    if !path.exists() {
        return Err(VimError::new("E484", format!("Can't open file {arg}")));
    }
    source_file(ed, env, state, &path)
}

/// `:runtime` search flags (scriptfile.c:244-277): the optional START/OPT/PACK/
/// ALL word selects extra package directories, `!` sources every match.
#[derive(Clone, Copy, Debug)]
struct RuntimeFlags {
    rtp: bool,
    start: bool,
    opt: bool,
}

fn pack_paths(ed: &Editor) -> Vec<PathBuf> {
    let value = ed.options.str_or("packpath", "");
    let paths: Vec<PathBuf> = value
        .split(',')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        runtime_paths(ed)
    } else {
        paths
    }
}

/// Candidate files in 'runtimepath' walk order (scriptfile.c:438-500): for each
/// root in order, for each whitespace-separated name, wildcard matches sorted.
fn runtime_candidates(ed: &Editor, flags: RuntimeFlags, names: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let collect = |out: &mut Vec<PathBuf>, bases: &[PathBuf], prefix: &str| {
        for base in bases {
            for name in names {
                let pattern = if prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{prefix}{name}")
                };
                let mut matches = expand_glob(base, &pattern);
                matches.sort();
                matches.dedup();
                out.extend(matches);
            }
        }
    };
    if flags.rtp {
        collect(&mut out, &runtime_paths(ed), "");
    }
    if flags.start {
        collect(&mut out, &pack_paths(ed), "pack/*/start/*/");
    }
    if flags.opt {
        collect(&mut out, &pack_paths(ed), "pack/*/opt/*/");
    }
    out
}

pub fn cmd_runtime(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let mut rest = p.args.trim();
    if rest.is_empty() {
        return Err(VimError::range_required());
    }
    let mut flags = RuntimeFlags {
        rtp: true,
        start: false,
        opt: false,
    };
    let (first, after) = next_token(rest);
    if !first.is_empty() {
        let upper = first.to_ascii_uppercase();
        let keyword = ["START", "OPT", "PACK", "ALL"]
            .into_iter()
            .find(|kw| kw.starts_with(&upper));
        match keyword {
            Some("START") => {
                flags = RuntimeFlags {
                    rtp: false,
                    start: true,
                    opt: false,
                };
                rest = after;
            }
            Some("OPT") => {
                flags = RuntimeFlags {
                    rtp: false,
                    start: false,
                    opt: true,
                };
                rest = after;
            }
            Some("PACK") => {
                flags = RuntimeFlags {
                    rtp: false,
                    start: true,
                    opt: true,
                };
                rest = after;
            }
            Some("ALL") => {
                flags = RuntimeFlags {
                    rtp: true,
                    start: true,
                    opt: true,
                };
                rest = after;
            }
            _ => {}
        }
    }
    let names: Vec<&str> = rest.split_whitespace().collect();
    if names.is_empty() {
        return Err(VimError::range_required());
    }
    let candidates = runtime_candidates(ed, flags, &names);
    if p.bang {
        // ":runtime!" sources every match in 'runtimepath' order
        // (scriptfile.c:440, DIP_ALL).
        for file in candidates {
            match source_file(ed, env, state, &file)? {
                ExOutcome::Continue => {}
                other => return Ok(other),
            }
        }
    } else if let Some(file) = candidates.first() {
        // Without "!" only the first match found is sourced.
        match source_file(ed, env, state, file)? {
            ExOutcome::Continue => {}
            other => return Ok(other),
        }
    }
    Ok(ExOutcome::Continue)
}

/// Vim9 scripts begin with a `vim9script` command on their first code line.
///
/// The M3 interpreter implements legacy VimL only, and only a handful of the
/// v8.2.5172 runtime files are Vim9 (`autoload/dist/ft.vim`,
/// `autoload/dist/script.vim`, `autoload/ccomplete.vim`).
fn is_vim9_script(text: &str) -> bool {
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('"'))
        .is_some_and(|line| {
            let lower = line.to_ascii_lowercase();
            lower == "vim9script" || lower.starts_with("vim9script ")
        })
}

/// ':vim9cmd {cmd}' and the Ex forms of :var/:const/:final.
pub fn cmd_vim9_line(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    line: &str,
) -> VimResult<ExOutcome> {
    let path = state
        .script_files
        .last()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut host = crate::script::make_host(ed, state);
    rvim_viml::vim9::run_command_line(line, env.current_sid, &path, "", env, &mut host)?;
    Ok(ExOutcome::Continue)
}

/// ':import' outside a vim9script file (legacy tests import the vim9.vim
/// helper module).
pub fn cmd_import(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let path = state
        .script_files
        .last()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let line = format!("import {}", p.args);
    let mut host = crate::script::make_host(ed, state);
    rvim_viml::vim9::run_import_line(&line, env.current_sid, &path, "", env, &mut host)?;
    Ok(ExOutcome::Continue)
}

pub(crate) fn find_in_runtime(ed: &Editor, name: &str) -> Option<PathBuf> {
    for base in runtime_paths(ed) {
        let candidate = base.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Expand an Ex wildcard pattern (`*`, `?`, `**`, `[...]`) below `base`.
pub fn expand_glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut current = vec![base.to_path_buf()];
    for component in pattern.split('/').filter(|c| !c.is_empty()) {
        let mut next = Vec::new();
        for dir in &current {
            if component == "**" {
                collect_recursive(dir, &mut next);
            } else if component.contains('*') || component.contains('?') || component.contains('[')
            {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if crate::wildcard::matches(component, &name) {
                            next.push(entry.path());
                        }
                    }
                }
            } else {
                let candidate = dir.join(component);
                if candidate.exists() {
                    next.push(candidate);
                }
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current.into_iter().filter(|p| p.is_file()).collect()
}

fn collect_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(path.clone());
            if path.is_dir() {
                collect_recursive(&path, out);
            }
        }
    }
}

pub fn cmd_finish(_ed: &mut Editor, _p: &ParsedEx) -> VimResult<ExOutcome> {
    Ok(ExOutcome::Finish)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::run_line;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rvim_ex_script_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn slash(p: &Path) -> String {
        p.display().to_string().replace('\\', "/")
    }

    #[test]
    fn throw_from_source_is_caught_by_the_caller() {
        // ex_docmd.c:1284-1285 need_rethrow: a :throw inside a sourced script
        // must surface to a surrounding :try.
        let dir = temp_dir("throw");
        let inner = dir.join("inner.vim");
        std::fs::write(&inner, "throw 'boom'\n").unwrap();
        let state = dir.join("state.txt");
        let mut ed = Editor::new();
        let cmd = format!(
            "try | source {} | catch | call writefile(['caught '.v:exception], '{}') | endtry",
            slash(&inner),
            slash(&state)
        );
        run_line(&mut ed, &cmd).unwrap();
        assert_eq!(
            std::fs::read_to_string(&state).unwrap().trim(),
            "caught boom"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uncaught_throw_from_source_is_an_error() {
        let dir = temp_dir("throw2");
        let inner = dir.join("inner.vim");
        std::fs::write(&inner, "throw 'boom'\n").unwrap();
        let mut ed = Editor::new();
        let err = run_line(&mut ed, &format!("source {}", slash(&inner))).unwrap_err();
        assert_eq!(err.code, "E605");
        assert!(err.message.contains("boom"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autocmd_bang_only_clears_the_current_group() {
        // autocmd.c:1072-1088: with no group argument the command works on the
        // current augroup; outside an augroup only the ungrouped entries go.
        let mut ed = Editor::new();
        run_line(&mut ed, "augroup G1").unwrap();
        run_line(&mut ed, "au BufWritePre * let g:g1 = 1").unwrap();
        run_line(&mut ed, "augroup END").unwrap();
        run_line(&mut ed, "augroup G2").unwrap();
        run_line(&mut ed, "au!").unwrap();
        run_line(&mut ed, "augroup END").unwrap();
        assert_eq!(ed.autocmds.entries().len(), 1, "G1 survives G2's au!");
        assert_eq!(ed.autocmds.entries()[0].group, "G1");

        // "au!" outside any group removes the ungrouped autocommands only.
        run_line(&mut ed, "au BufWritePre * let g:top = 1").unwrap();
        assert_eq!(ed.autocmds.entries().len(), 2);
        run_line(&mut ed, "au!").unwrap();
        assert_eq!(ed.autocmds.entries().len(), 1);
        assert_eq!(ed.autocmds.entries()[0].group, "G1");
    }

    #[test]
    fn runtime_appends_every_match_in_runtimepath_order() {
        let dir = temp_dir("runtime");
        let first = dir.join("zfirst");
        let second = dir.join("asecond");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let order = dir.join("order.txt");
        std::fs::write(
            first.join("r.vim"),
            format!("call writefile(['1'], '{}', 'a')\n", slash(&order)),
        )
        .unwrap();
        std::fs::write(
            second.join("r.vim"),
            format!("call writefile(['2'], '{}', 'a')\n", slash(&order)),
        )
        .unwrap();
        let mut ed = Editor::new();
        run_line(
            &mut ed,
            &format!("set runtimepath={},{}", slash(&first), slash(&second)),
        )
        .unwrap();
        run_line(&mut ed, "runtime! r.vim").unwrap();
        assert_eq!(std::fs::read_to_string(&order).unwrap(), "1\n2\n");
        // Without "!" only the first match is sourced.
        std::fs::remove_file(&order).unwrap();
        run_line(&mut ed, "runtime r.vim").unwrap();
        assert_eq!(std::fs::read_to_string(&order).unwrap(), "1\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_does_not_search_runtimepath() {
        // scriptfile.c:1150-1175: :source uses the name as-is.
        let dir = temp_dir("source");
        let rtp = dir.join("rt");
        std::fs::create_dir_all(&rtp).unwrap();
        std::fs::write(rtp.join("r.vim"), "let g:o = 1\n").unwrap();
        let mut ed = Editor::new();
        run_line(&mut ed, &format!("set runtimepath={}", slash(&rtp))).unwrap();
        let err = run_line(&mut ed, "source r.vim").unwrap_err();
        assert_eq!(err.code, "E484");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
