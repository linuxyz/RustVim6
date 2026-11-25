//! Builtin functions, group f-l (M3 coverage batch).
//!
//! See `docs/m3_builtins_plan.md` for the candidate list, semantics rules and
//! the skip list (subsystems that are not built yet).
//!
//! Implemented here are the functions whose semantics can be provided by the
//! language/`std` alone:
//!
//! * filesystem queries: `file_readable`, `filewritable`, `getfperm`,
//!   `getfsize`, `getftime`, `getftype`, `findfile`, `finddir`, `glob`,
//!   `globpath`, `glob2regpat`, `isabsolutepath`;
//! * lists/blobs: `flatten`, `flattennew`, `list2blob`, `list2str`;
//! * JSON: `json_encode`, `json_decode`, `js_encode`, `js_decode`;
//! * misc: `funcref`, `garbagecollect`, `getenv`, `getfontname`, `getimstatus`,
//!   `getpid`, `getreg`, `gettext`, `hostname`, `islocked`.
//!
//! Names that need an unbuilt subsystem (folds, windows, tabs, quickfix,
//! command-line, input, jobs, listeners, history, highlight groups, mappings,
//! dynamic libraries, Lua, ...) are intentionally left out of [`FUNCS`].
//!
//! Known simplifications (documented next to the implementation):
//! * `getfontname()` returns an empty string (terminal build, no GUI font);
//!   `getimstatus()` returns 0 (no input-method support).
//! * `iconv()` is not provided because `std` has no iconv-equivalent; the
//!   reference `f_iconv()` returns the input unchanged when no converter is
//!   available, but supporting only a fake subset would be misleading.
//! * `glob()`/`findfile()` understand `*`, `?`, `[...]`, `**`, `**N`, `~`,
//!   `$VAR` and upward `;` searches, but not backtick expressions or
//!   `'wildignore'`/`'suffixes'` filtering.
//! * `js_decode()` maps `v:none` (empty JS array items) to `v:null` because
//!   `Value` has no `v:none`.

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rvim_core::{VimError, VimResult};

use crate::builtins::check_arity;
use crate::env::{Env, Host};
use crate::value::{Value, ValueResult, VimDict};

/// Names implemented in this module (coverage source of truth).
pub const FUNCS: &[&str] = &[
    "file_readable",
    "filewritable",
    "finddir",
    "findfile",
    "flatten",
    "flattennew",
    "funcref",
    "garbagecollect",
    "getenv",
    "getfontname",
    "getfperm",
    "getfsize",
    "getftime",
    "getftype",
    "getimstatus",
    "getpid",
    "getreg",
    "gettext",
    "glob",
    "glob2regpat",
    "globpath",
    "hostname",
    "isabsolutepath",
    "islocked",
    "js_decode",
    "js_encode",
    "json_decode",
    "json_encode",
    "list2blob",
    "list2str",
];

/// Dispatch a call; returns None when the name is not handled here.
pub fn call(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> Option<ValueResult> {
    if !FUNCS.contains(&name) {
        return None;
    }
    Some((|| {
        check_arity(name, args)?;
        dispatch(name, args, env, host)
    })())
}

fn dispatch(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    match name {
        // ------------------------------------------------------ filesystem
        "file_readable" => {
            let path = string_arg(args, 0)?;
            Ok(Value::Number(i64::from(file_readable(&path))))
        }
        "filewritable" => {
            let path = string_arg(args, 0)?;
            Ok(Value::Number(file_writable(Path::new(&path))))
        }
        "getfperm" => {
            let path = string_arg(args, 0)?;
            Ok(Value::String(file_perm(Path::new(&path))))
        }
        "getfsize" => {
            let path = string_arg(args, 0)?;
            Ok(Value::Number(file_size(Path::new(&path))))
        }
        "getftime" => {
            let path = string_arg(args, 0)?;
            Ok(Value::Number(file_time(Path::new(&path))))
        }
        "getftype" => {
            let path = string_arg(args, 0)?;
            Ok(Value::String(file_type_name(Path::new(&path))))
        }
        "isabsolutepath" => {
            let path = string_arg(args, 0)?;
            Ok(Value::Number(i64::from(is_absolute_path(&path))))
        }
        "findfile" | "finddir" => {
            let want_dir = name == "finddir";
            let file = string_arg(args, 0)?;
            find_file(file, args, want_dir, env, host)
        }
        "glob" => do_glob(args, host),
        "globpath" => do_globpath(args, host),
        "glob2regpat" => {
            let pat = string_arg(args, 0)?;
            Ok(Value::String(glob_to_regpat(&pat)))
        }

        // ------------------------------------------------- lists / blobs
        "flatten" => {
            let list = list_arg(args, 0, "flatten()")?;
            let maxdepth = flatten_depth(args)?;
            let mut items = list.borrow().clone();
            flatten_values(&mut items, maxdepth, &mut Vec::new());
            *list.borrow_mut() = items;
            Ok(arg(args, 0))
        }
        "flattennew" => {
            let list = list_arg(args, 0, "flattennew()")?;
            let maxdepth = flatten_depth(args)?;
            let mut items = list.borrow().clone();
            flatten_values(&mut items, maxdepth, &mut Vec::new());
            Ok(Value::list(items))
        }
        "list2blob" => {
            let items = list_items(args, 0, "list2blob()")?;
            let mut bytes = Vec::with_capacity(items.len());
            for item in &items {
                let n = item.as_number()?;
                if !(0..=255).contains(&n) {
                    return Err(err("E1239", format!("Invalid value for blob: {n}")));
                }
                bytes.push(n as u8);
            }
            Ok(Value::blob(bytes))
        }
        "list2str" => {
            let items = list_items(args, 0, "list2str()")?;
            // RustVim strings are always UTF-8, so the optional {utf8} flag
            // does not change how numbers are turned into characters.
            let mut out = String::new();
            for item in &items {
                let n = item.to_number();
                if n >= 0 {
                    if let Some(c) = char::from_u32(n as u32) {
                        out.push(c);
                    }
                }
            }
            Ok(Value::String(out))
        }

        // ---------------------------------------------------------- JSON
        "json_encode" => Ok(Value::String(json_encode_value(&arg(args, 0), false)?)),
        "js_encode" => Ok(Value::String(json_encode_value(&arg(args, 0), true)?)),
        "json_decode" => {
            let text = string_arg(args, 0)?;
            json_decode_value(&text, false)
        }
        "js_decode" => {
            let text = string_arg(args, 0)?;
            json_decode_value(&text, true)
        }

        // ----------------------------------------------------------- misc
        "funcref" => funcref(args),
        "garbagecollect" => Ok(Value::Number(0)),
        "getenv" => Ok(getenv_value(&string_arg(args, 0)?)),
        "getfontname" => {
            // Terminal build: there is no GUI font, Vim returns "" as well.
            Ok(Value::String(String::new()))
        }
        "getimstatus" => {
            // No input-method support: the IM is never active.
            Ok(Value::Number(0))
        }
        "getpid" => Ok(Value::Number(i64::from(std::process::id()))),
        "getreg" => getreg(args, host),
        "gettext" => {
            let text = string_arg(args, 0)?;
            if text.is_empty() {
                return Err(err("E475", "Invalid argument: "));
            }
            Ok(Value::String(text))
        }
        "hostname" => Ok(Value::String(host_name())),
        "islocked" => {
            let name = string_arg(args, 0)?;
            if name.is_empty() {
                return Err(err("E475", "Invalid argument: "));
            }
            // `:lockvar` is not implemented by the Ex layer, so no variable or
            // container can be locked.  A variable that does not exist yields
            // -1, everything else (including list/dict items) yields 0.
            if var_exists(&name, env, host) || looks_like_container_item(&name) {
                Ok(Value::Number(0))
            } else {
                Ok(Value::Number(-1))
            }
        }
        _ => Err(err(
            "E117",
            format!("builtin {name} is listed but not dispatched in f-l group"),
        )),
    }
}

// ------------------------------------------------------------------ helpers

fn err(code: &'static str, msg: impl Into<String>) -> VimError {
    VimError::new(code, msg.into())
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Null)
}

fn narg(args: &[Value], i: usize) -> i64 {
    arg(args, i).to_number()
}

fn bopt(args: &[Value], i: usize) -> bool {
    args.get(i).is_some_and(Value::truthy)
}

/// Convert an argument to a string, reporting Vim's E729/E730/E731/E976.
fn string_arg(args: &[Value], i: usize) -> VimResult<String> {
    match args.get(i) {
        None => Ok(String::new()),
        Some(Value::List(_)) => Err(err("E730", "Using a List as a String")),
        Some(Value::Dict(_)) => Err(err("E731", "Using a Dictionary as a String")),
        Some(Value::Funcref(_)) => Err(err("E729", "Using a Funcref as a String")),
        Some(Value::Blob(_)) => Err(err("E976", "Using a Blob as a String")),
        Some(v) => Ok(v.to_string_value()),
    }
}

fn list_arg(args: &[Value], i: usize, func: &str) -> VimResult<Rc<std::cell::RefCell<Vec<Value>>>> {
    match args.get(i) {
        Some(Value::List(l)) => Ok(l.clone()),
        Some(other) => Err(err(
            "E686",
            format!(
                "Argument of {func} must be a List, not {}",
                other.type_name()
            ),
        )),
        None => Err(err("E686", format!("Argument of {func} must be a List"))),
    }
}

fn list_items(args: &[Value], i: usize, func: &str) -> VimResult<Vec<Value>> {
    let list = list_arg(args, i, func)?;
    let items = list.borrow().clone();
    Ok(items)
}

// ------------------------------------------------------- filesystem queries

fn file_readable(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let p = Path::new(path);
    match fs::metadata(p) {
        Ok(meta) if !meta.is_dir() => fs::File::open(p).is_ok(),
        _ => false,
    }
}

/// 0 = not writable, 1 = writable file, 2 = writable directory (reference:
/// `filewritable()` in misc2.c).
fn file_writable(path: &Path) -> i64 {
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    let is_dir = meta.is_dir();
    #[cfg(unix)]
    let writable = {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o222 != 0
    };
    // Win32 semantics: a readonly directory is still writable (reference:
    // `mch_writable()` in os_win32.c).
    #[cfg(not(unix))]
    let writable = is_dir || !meta.permissions().readonly();
    if !writable {
        0
    } else if is_dir {
        2
    } else {
        1
    }
}

fn file_perm(path: &Path) -> String {
    let Ok(meta) = fs::metadata(path) else {
        return String::new();
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() as u32;
        let flags = ['r', 'w', 'x'];
        let mut out = String::with_capacity(9);
        for i in 0..9 {
            if mode & (1 << (8 - i)) != 0 {
                out.push(flags[i % 3]);
            } else {
                out.push('-');
            }
        }
        out
    }
    #[cfg(not(unix))]
    {
        // MSVC's stat() reports rw for everything that is not FILE_ATTRIBUTE_
        // READONLY, which is what Vim on MS-Windows shows as well.
        if meta.permissions().readonly() {
            "r--r--r--".to_string()
        } else {
            "rw-rw-rw-".to_string()
        }
    }
}

fn file_size(path: &Path) -> i64 {
    let Ok(meta) = fs::metadata(path) else {
        return -1;
    };
    if meta.is_dir() {
        return 0;
    }
    i64::try_from(meta.len()).unwrap_or(-2)
}

fn file_time(path: &Path) -> i64 {
    let Ok(meta) = fs::metadata(path) else {
        return -1;
    };
    let Ok(modified) = meta.modified() else {
        return -1;
    };
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(-2),
        Err(_) => -1,
    }
}

fn file_type_name(path: &Path) -> String {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return String::new();
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        return "link".to_string();
    }
    if ft.is_dir() {
        return "dir".to_string();
    }
    if ft.is_file() {
        return "file".to_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if ft.is_fifo() {
            return "fifo".to_string();
        }
        if ft.is_socket() {
            return "socket".to_string();
        }
        if ft.is_char_device() {
            return "cdev".to_string();
        }
        if ft.is_block_device() {
            return "bdev".to_string();
        }
    }
    "other".to_string()
}

fn is_absolute_path(path: &str) -> bool {
    let chars: Vec<char> = path.chars().collect();
    #[cfg(windows)]
    {
        // Reference: mch_isFullName() in os_mswin.c.
        if chars.is_empty() {
            return false;
        }
        if chars[0].is_ascii_alphabetic()
            && chars.len() >= 3
            && chars[1] == ':'
            && (chars[2] == '/' || chars[2] == '\\')
        {
            return true;
        }
        chars.len() >= 2 && chars[0] == chars[1] && (chars[0] == '/' || chars[0] == '\\')
    }
    #[cfg(not(windows))]
    {
        // Reference: mch_isFullName() in os_unix.c.
        matches!(chars.first(), Some('/') | Some('~'))
    }
}

// ----------------------------------------------------------- glob machinery

fn home_dir() -> Option<String> {
    #[cfg(windows)]
    let names = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let names = ["HOME"];
    for name in names {
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Expand a leading `~` and `$VAR` / `${VAR}` references (reference:
/// `expand_env_esc()`); unknown variables are left untouched.
fn expand_env(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    if chars.peek() == Some(&'~') {
        let mut clone = chars.clone();
        clone.next();
        let sep = clone.peek().copied();
        if sep.is_none() || sep == Some('/') || sep == Some('\\') {
            if let Some(home) = home_dir() {
                chars.next();
                out.push_str(&home);
            }
        }
    }
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let mut var = String::new();
        if chars.peek() == Some(&'{') {
            let mut clone = chars.clone();
            clone.next();
            let mut found = false;
            for ch in clone {
                if ch == '}' {
                    found = true;
                    break;
                }
                var.push(ch);
            }
            if !found {
                out.push('$');
                continue;
            }
            // consume "{...}"
            chars.next();
            for _ in 0..var.chars().count() {
                chars.next();
            }
            if let Some(ch) = chars.next() {
                debug_assert_eq!(ch, '}');
            }
        } else {
            while let Some(&ch) = chars.peek() {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    var.push(ch);
                    chars.next();
                } else {
                    break;
                }
            }
        }
        if var.is_empty() {
            out.push('$');
            continue;
        }
        match std::env::var(&var) {
            Ok(value) => out.push_str(&value),
            Err(_) => {
                out.push('$');
                out.push_str(&var);
            }
        }
    }
    out
}

/// Normalize path separators: on Windows a backslash is a path separator, so
/// it is turned into `/`.  On Unix it stays an escape character.
fn normalize_separators(text: &str) -> String {
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.to_string()
    }
}

fn is_glob_meta(c: char) -> bool {
    matches!(c, '*' | '?' | '[')
}

fn has_wildcard(text: &str) -> bool {
    text.chars().any(is_glob_meta)
}

/// Remove backslashes used to escape characters in a literal path component.
fn unescape_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn join_path(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "." {
        name.to_string()
    } else if dir.ends_with('/') || dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Split a path pattern into a root directory and path components.
fn split_pattern(pattern: &str) -> (String, Vec<String>) {
    let mut root = String::new();
    let mut rest = pattern;
    #[cfg(windows)]
    {
        let chars: Vec<char> = rest.chars().collect();
        if chars.len() >= 2 && chars[0].is_ascii_alphabetic() && chars[1] == ':' {
            root = format!("{}:/", chars[0]);
            rest = &rest[2..];
            while rest.starts_with('/') {
                rest = &rest[1..];
            }
        } else if rest.starts_with("//") {
            // UNC path: keep //server/share as the root.
            let mut parts = rest[2..].splitn(3, '/');
            let server = parts.next().unwrap_or_default();
            let share = parts.next().unwrap_or_default();
            root = format!("//{server}/{share}");
            rest = parts.next().unwrap_or_default();
        }
    }
    if rest.starts_with('/') {
        root = "/".to_string();
        while rest.starts_with('/') {
            rest = &rest[1..];
        }
    }
    while rest.starts_with("./") {
        rest = &rest[2..];
    }
    let components: Vec<String> = rest
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(str::to_string)
        .collect();
    if root.is_empty() {
        root = ".".to_string();
    }
    (root, components)
}

fn clean_glob_result(path: &str) -> String {
    let mut out = path.replace('\\', "/");
    while out.starts_with("./") {
        out = out[2..].to_string();
    }
    out
}

/// True when a `**`-style component starts a directory-tree wildcard.
fn starstar_limit(component: &str, strict: bool) -> VimResult<Option<usize>> {
    let Some(rest) = component.strip_prefix("**") else {
        return Ok(None);
    };
    if rest.is_empty() {
        return Ok(Some(30));
    }
    if rest.chars().all(|c| c.is_ascii_digit()) {
        let n = rest.parse::<usize>().unwrap_or(30).min(100);
        return Ok(Some(n));
    }
    if strict {
        return Err(err("E343", "Invalid **".to_string()));
    }
    Ok(None)
}

fn read_dir_entries(dir: &str) -> Vec<(String, bool)> {
    let mut entries = Vec::new();
    let read = fs::read_dir(if dir.is_empty() { "." } else { dir });
    if let Ok(iter) = read {
        for entry in iter.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push((name, is_dir));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

fn char_eq(a: char, b: char, ignore_case: bool) -> bool {
    if ignore_case {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

/// Match one path component against one file name.  Supports `*`, `?`,
/// `[...]` (with `^`/`!` negation and ranges) and backslash escapes.
fn wild_match(pattern: &[char], name: &[char], ignore_case: bool) -> bool {
    let mut pi = 0;
    let mut ni = 0;
    let mut star: Option<(usize, usize)> = None;
    loop {
        if pi < pattern.len() {
            match pattern[pi] {
                '*' => {
                    star = Some((pi, ni));
                    pi += 1;
                    continue;
                }
                '?' => {
                    if ni < name.len() {
                        pi += 1;
                        ni += 1;
                        continue;
                    }
                }
                '[' => {
                    if ni < name.len() {
                        if let Some((matched, next)) =
                            match_class(&pattern[pi..], name[ni], ignore_case)
                        {
                            if matched {
                                pi = next;
                                ni += 1;
                                continue;
                            }
                        }
                    }
                }
                '\\' if pi + 1 < pattern.len() => {
                    if ni < name.len() && char_eq(pattern[pi + 1], name[ni], ignore_case) {
                        pi += 2;
                        ni += 1;
                        continue;
                    }
                }
                c => {
                    if ni < name.len() && char_eq(c, name[ni], ignore_case) {
                        pi += 1;
                        ni += 1;
                        continue;
                    }
                }
            }
        } else if ni == name.len() {
            return true;
        }
        match star {
            Some((spi, sni)) if sni < name.len() => {
                star = Some((spi, sni + 1));
                pi = spi + 1;
                ni = sni + 1;
            }
            _ => return false,
        }
    }
}

/// Parse a `[...]` class at the start of `pattern`; returns whether the
/// character matched and the pattern index just past the closing `]`.
fn match_class(pattern: &[char], c: char, ignore_case: bool) -> Option<(bool, usize)> {
    let mut i = 1;
    let mut negate = false;
    if pattern.len() <= i {
        return None;
    }
    if i < pattern.len() && (pattern[i] == '^' || pattern[i] == '!') {
        negate = true;
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pattern.len() {
        if pattern[i] == ']' && !first {
            let hit = if negate { !matched } else { matched };
            return Some((hit, i + 1));
        }
        first = false;
        let lo = if pattern[i] == '\\' && i + 1 < pattern.len() {
            i += 1;
            pattern[i]
        } else {
            pattern[i]
        };
        if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
            let hi = pattern[i + 2];
            let (lo, hi) = if ignore_case {
                (lo.to_ascii_lowercase(), hi.to_ascii_lowercase())
            } else {
                (lo, hi)
            };
            let cc = if ignore_case {
                c.to_ascii_lowercase()
            } else {
                c
            };
            if lo <= cc && cc <= hi {
                matched = true;
            }
            i += 3;
            continue;
        }
        if char_eq(lo, c, ignore_case) {
            matched = true;
        }
        i += 1;
    }
    None
}

/// Recursively expand wildcard path components below `dir`.
fn glob_walk(
    dir: &str,
    components: &[String],
    dirs_only: bool,
    ignore_case: bool,
    strict_starstar: bool,
    depth: usize,
    out: &mut Vec<String>,
) -> VimResult<()> {
    let Some(component) = components.first() else {
        return Ok(());
    };
    let rest = &components[1..];
    if let Some(limit) = starstar_limit(component, strict_starstar)? {
        if rest.is_empty() {
            if dirs_only {
                out.push(dir.to_string());
            }
            if depth >= limit {
                return Ok(());
            }
            for (name, is_dir) in read_dir_entries(dir) {
                if name.starts_with('.') {
                    continue;
                }
                let child = join_path(dir, &name);
                if is_dir {
                    out.push(child.clone());
                    glob_walk(
                        &child,
                        components,
                        dirs_only,
                        ignore_case,
                        strict_starstar,
                        depth + 1,
                        out,
                    )?;
                } else if !dirs_only {
                    out.push(child);
                }
            }
            return Ok(());
        }
        // `**` can also match zero directories.
        glob_walk(
            dir,
            rest,
            dirs_only,
            ignore_case,
            strict_starstar,
            depth,
            out,
        )?;
        if depth >= limit {
            return Ok(());
        }
        for (name, is_dir) in read_dir_entries(dir) {
            if !is_dir || name.starts_with('.') {
                continue;
            }
            let child = join_path(dir, &name);
            glob_walk(
                &child,
                components,
                dirs_only,
                ignore_case,
                strict_starstar,
                depth + 1,
                out,
            )?;
        }
        return Ok(());
    }
    if has_wildcard(component) {
        let pattern: Vec<char> = component.chars().collect();
        for (name, is_dir) in read_dir_entries(dir) {
            if name.starts_with('.') && !component.starts_with('.') {
                continue;
            }
            let name_chars: Vec<char> = name.chars().collect();
            if !wild_match(&pattern, &name_chars, ignore_case) {
                continue;
            }
            let child = join_path(dir, &name);
            if rest.is_empty() {
                if !dirs_only || is_dir {
                    out.push(child);
                }
            } else if is_dir {
                glob_walk(
                    &child,
                    rest,
                    dirs_only,
                    ignore_case,
                    strict_starstar,
                    depth,
                    out,
                )?;
            }
        }
        return Ok(());
    }
    let literal = unescape_component(component);
    let child = join_path(dir, &literal);
    if rest.is_empty() {
        let path = Path::new(&child);
        let matched = if dirs_only {
            path.is_dir()
        } else {
            path.exists()
        };
        if matched {
            out.push(child);
        }
    } else {
        glob_walk(
            &child,
            rest,
            dirs_only,
            ignore_case,
            strict_starstar,
            depth,
            out,
        )?;
    }
    Ok(())
}

/// Expand a path pattern (used by glob() and by the path search of
/// findfile()/finddir()).  Results are cleaned of leading `./`.
fn expand_pattern(
    pattern: &str,
    dirs_only: bool,
    ignore_case: bool,
    strict_starstar: bool,
) -> VimResult<Vec<String>> {
    let expanded = normalize_separators(&expand_env(pattern));
    if expanded.is_empty() {
        return Ok(Vec::new());
    }
    let (root, components) = split_pattern(&expanded);
    let mut matches = Vec::new();
    if components.is_empty() {
        let path = Path::new(&root);
        let matched = if dirs_only {
            path.is_dir()
        } else {
            path.exists()
        };
        if matched {
            matches.push(clean_glob_result(&root));
        }
        return Ok(matches);
    }
    if !components
        .iter()
        .any(|c| has_wildcard(c) || c.starts_with("**"))
    {
        let mut literal = root.clone();
        for component in &components {
            literal = join_path(&literal, &unescape_component(component));
        }
        let path = Path::new(&literal);
        let matched = if dirs_only {
            path.is_dir()
        } else {
            path.exists()
        };
        if matched {
            matches.push(clean_glob_result(&literal));
        }
        return Ok(matches);
    }
    glob_walk(
        &root,
        &components,
        dirs_only,
        ignore_case,
        strict_starstar,
        0,
        &mut matches,
    )?;
    Ok(matches.into_iter().map(|m| clean_glob_result(&m)).collect())
}

fn host_ignore_case(host: &mut dyn Host) -> bool {
    if cfg!(windows) {
        return true;
    }
    host.option_get("wildignorecase")
        .map(|v| v.truthy())
        .unwrap_or(false)
}

// --------------------------------------------------------------------- glob

fn glob_result(matches: Vec<String>, as_list: bool) -> Value {
    if as_list {
        Value::list(matches.into_iter().map(Value::String).collect())
    } else {
        Value::String(matches.join("\n"))
    }
}

fn do_glob(args: &[Value], host: &mut dyn Host) -> ValueResult {
    let pattern = string_arg(args, 0)?;
    let as_list = bopt(args, 2);
    let ignore_case = host_ignore_case(host);
    let matches = expand_pattern(&pattern, false, ignore_case, false)?;
    Ok(glob_result(matches, as_list))
}

/// Split a path option/list on commas, honouring `\,` escapes.
fn split_path_list(text: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    if next == ',' {
                        current.push(',');
                    } else {
                        current.push('\\');
                        current.push(next);
                    }
                } else {
                    current.push('\\');
                }
            }
            ',' => {
                entries.push(current.clone());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    entries.push(current);
    entries
}

fn do_globpath(args: &[Value], host: &mut dyn Host) -> ValueResult {
    let path = string_arg(args, 0)?;
    let expr = string_arg(args, 1)?;
    let as_list = bopt(args, 3);
    let ignore_case = host_ignore_case(host);
    let mut matches = Vec::new();
    for dir in split_path_list(&path) {
        let pattern = if dir.is_empty() {
            expr.clone()
        } else {
            join_path(&dir, &expr)
        };
        matches.extend(expand_pattern(&pattern, false, ignore_case, false)?);
    }
    Ok(glob_result(matches, as_list))
}

/// Convert a file glob into a Vim search pattern (reference:
/// `file_pat_to_reg_pat()` in fileio.c).
fn glob_to_regpat(pattern: &str) -> String {
    if pattern.is_empty() {
        return "^$".to_string();
    }
    let mut out = String::new();
    let mut pat: &str = pattern;
    // A leading '*' already matches anything: no '^' anchor.
    if pat.starts_with('*') {
        while pat.starts_with('*') && pat.len() > 1 {
            pat = &pat[1..];
        }
    } else {
        out.push('^');
    }
    let mut chars: Vec<char> = pat.chars().collect();
    let mut add_dollar = true;
    // A trailing '*' makes the '$' anchor pointless.
    while chars.len() > 1 && chars[chars.len() - 1] == '*' {
        chars.pop();
        add_dollar = false;
    }
    if chars.last() == Some(&'*') {
        add_dollar = false;
        chars.pop();
    }
    let mut nested = 0i32;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                out.push_str(".*");
                while i + 1 < chars.len() && chars[i + 1] == '*' {
                    i += 1;
                }
            }
            '?' => out.push('.'),
            '.' | '~' => {
                out.push('\\');
                out.push(chars[i]);
            }
            '\\' => {
                if i + 1 < chars.len() {
                    i += 1;
                    let c = chars[i];
                    // Path separators may be written with either slash; on
                    // Windows both forms mean the same thing.
                    if cfg!(windows) && (c == '/' || c == '\\') {
                        out.push_str("[/\\\\]");
                    } else if matches!(c, ',' | '%' | '#' | '{' | '}') || c.is_whitespace() {
                        out.push(c);
                    } else {
                        out.push('\\');
                        out.push(c);
                    }
                }
            }
            '/' if cfg!(windows) => out.push_str("[/\\\\]"),
            '{' => {
                out.push_str("\\(");
                nested += 1;
            }
            '}' => {
                out.push_str("\\)");
                nested -= 1;
            }
            ',' if nested > 0 => out.push_str("\\|"),
            c => out.push(c),
        }
        i += 1;
    }
    if add_dollar {
        out.push('$');
    }
    out
}

// ----------------------------------------------------- findfile()/finddir()

fn path_matches(candidate: &str, want_dir: bool) -> bool {
    match fs::metadata(candidate) {
        Ok(meta) => meta.is_dir() == want_dir,
        Err(_) => false,
    }
}

fn is_self_relative(path: &str) -> bool {
    path == "."
        || path == ".."
        || path.starts_with("./")
        || path.starts_with("../")
        || path.starts_with(".\\")
        || path.starts_with("..\\")
}

fn absolute_path(path: &str) -> PathBuf {
    std::path::absolute(Path::new(path)).unwrap_or_else(|_| PathBuf::from(path))
}

fn path_same(a: &Path, b: &Path) -> bool {
    let (Ok(ca), Ok(cb)) = (fs::canonicalize(a), fs::canonicalize(b)) else {
        return a == b;
    };
    ca == cb
}

fn find_in_upward(start: &str, file: &str, want_dir: bool, stopdirs: &[String]) -> Vec<String> {
    let mut current = absolute_path(start);
    let stops: Vec<PathBuf> = stopdirs
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| absolute_path(s))
        .collect();
    let mut found = Vec::new();
    loop {
        let dir = current.to_string_lossy().replace('\\', "/");
        let candidate = join_path(&dir, file);
        if path_matches(&candidate, want_dir) {
            found.push(candidate);
        }
        if stops.iter().any(|stop| path_same(&current, stop)) {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    found
}

/// Directory candidates for one `'path'` entry.
fn path_entry_dirs(entry: &str, ignore_case: bool) -> VimResult<Vec<String>> {
    if entry.is_empty() {
        return Ok(vec![".".to_string()]);
    }
    let mut dirs = expand_pattern(entry, true, ignore_case, true)?;
    if dirs.is_empty() {
        // Upward search from a not-yet-expanded literal directory still works
        // when the directory exists (checked by expand_pattern).
        return Ok(dirs);
    }
    dirs.dedup();
    Ok(dirs)
}

fn default_search_path(host: &mut dyn Host) -> String {
    match host.option_get("path") {
        Some(value) => {
            let text = value.to_string_value();
            if text.is_empty() {
                ".".to_string()
            } else {
                text
            }
        }
        None => ".".to_string(),
    }
}

fn find_file(
    file: String,
    args: &[Value],
    want_dir: bool,
    env: &mut Env,
    host: &mut dyn Host,
) -> ValueResult {
    let _ = env;
    if file.is_empty() {
        return Ok(Value::String(String::new()));
    }
    let ignore_case = host_ignore_case(host);
    let explicit_path = args.len() > 1 && !string_arg(args, 1)?.is_empty();
    let path_spec = if explicit_path {
        string_arg(args, 1)?
    } else {
        default_search_path(host)
    };
    let count = if args.len() > 2 { narg(args, 2) } else { 1 };

    let mut matches: Vec<String> = Vec::new();
    let search_whole_path;
    if is_absolute_path(&file) || is_self_relative(&file) {
        // Absolute/`.`-relative names bypass the path search.
        search_whole_path = false;
        let candidate = clean_glob_result(&file);
        if path_matches(&candidate, want_dir) {
            matches.push(candidate);
        }
    } else {
        search_whole_path = true;
    }
    if search_whole_path {
        for entry in split_path_option(&path_spec) {
            if entry.contains(';') {
                let mut parts = entry.split(';');
                let start = parts.next().unwrap_or_default();
                let stopdirs: Vec<String> = parts.map(str::to_string).collect();
                let start = if start.is_empty() { "." } else { start };
                let dirs = path_entry_dirs(start, ignore_case)?;
                for dir in dirs {
                    matches.extend(find_in_upward(&dir, &file, want_dir, &stopdirs));
                }
            } else {
                for dir in path_entry_dirs(&entry, ignore_case)? {
                    let candidate = clean_glob_result(&join_path(&dir, &file));
                    if path_matches(&candidate, want_dir) {
                        matches.push(candidate);
                    }
                }
            }
        }
    }
    if count < 0 {
        Ok(Value::list(
            matches.into_iter().map(Value::String).collect(),
        ))
    } else {
        let wanted = count.max(1) as usize;
        Ok(Value::String(
            matches.get(wanted - 1).cloned().unwrap_or_default(),
        ))
    }
}

/// Split a `'path'`-style option into entries (`,` and space separate).
fn split_path_option(text: &str) -> Vec<String> {
    text.split([',', ' '])
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

// ------------------------------------------------------- flatten() helpers

fn flatten_depth(args: &[Value]) -> VimResult<i64> {
    if args.len() < 2 {
        return Ok(-1);
    }
    let depth = narg(args, 1);
    if depth < 0 {
        return Err(err("E900", "maxdepth must be non-negative number"));
    }
    Ok(depth)
}

/// Flatten `items` up to `maxdepth` levels (`-1` = unlimited).  Only the
/// structure of this list is changed; nested lists are spliced in by value.
fn flatten_values(items: &mut Vec<Value>, maxdepth: i64, stack: &mut Vec<usize>) {
    if maxdepth == 0 {
        return;
    }
    let snapshot = std::mem::take(items);
    let mut out = Vec::with_capacity(snapshot.len());
    for item in snapshot {
        if let Value::List(sub) = &item {
            let key = Rc::as_ptr(sub) as usize;
            if !stack.contains(&key) {
                stack.push(key);
                let mut nested = sub.borrow().clone();
                flatten_values(&mut nested, maxdepth - 1, stack);
                stack.pop();
                out.extend(nested);
                continue;
            }
        }
        out.push(item);
    }
    *items = out;
}

// ---------------------------------------------------------- funcref()/misc

fn funcref(args: &[Value]) -> ValueResult {
    let first = arg(args, 0);
    let (name, mut partial, mut dict, lambda) = match &first {
        Value::Funcref(f) => (
            f.name.clone(),
            f.partial.clone(),
            f.dict.clone(),
            f.lambda.clone(),
        ),
        other => (other.to_string_value(), Vec::new(), None, None),
    };
    if name.is_empty() {
        return Err(err("E475", "Invalid argument: "));
    }
    if !matches!(first, Value::Funcref(_))
        && name.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return Err(err("E475", format!("Invalid argument: {name}")));
    }
    if let Some(second) = args.get(1) {
        match second {
            Value::List(list) => {
                partial.extend(list.borrow().iter().cloned());
                if args.len() > 2 {
                    match &args[2] {
                        Value::Dict(_) => dict = Some(args[2].clone()),
                        _ => return Err(err("E922", "Expected a dict")),
                    }
                }
            }
            Value::Dict(_) => {
                if args.len() > 2 {
                    return Err(err(
                        "E923",
                        "Second argument of function() must be a list or a dict",
                    ));
                }
                dict = Some(second.clone());
            }
            _ => {
                return Err(err(
                    "E923",
                    "Second argument of function() must be a list or a dict",
                ));
            }
        }
    }
    Ok(Value::Funcref(Rc::new(crate::value::FuncRef {
        name,
        partial,
        dict,
        lambda,
        vim9_lambda: None,
    })))
}

fn getenv_value(name: &str) -> Value {
    if name.eq_ignore_ascii_case("HOME") {
        if let Some(home) = home_dir() {
            return Value::String(home);
        }
    }
    match std::env::var(name) {
        // An empty variable is the same as an unset one (reference:
        // `vim_getenv()` in misc1.c).
        Ok(value) if !value.is_empty() => Value::String(value),
        _ => Value::Null,
    }
}

fn host_name() -> String {
    for name in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    String::new()
}

fn var_exists(name: &str, env: &mut Env, host: &mut dyn Host) -> bool {
    if env.var_exists(name) {
        return true;
    }
    if let Some(rest) = name.strip_prefix("b:") {
        return host.buffer_var_get(rest).is_some();
    }
    if let Some(rest) = name.strip_prefix("w:") {
        return host.window_var_get(rest).is_some();
    }
    if let Some(rest) = name.strip_prefix("t:") {
        return host.tab_var_get(rest).is_some();
    }
    false
}

fn looks_like_container_item(name: &str) -> bool {
    name.chars().any(|c| matches!(c, '[' | '.'))
}

fn getreg(args: &[Value], host: &mut dyn Host) -> ValueResult {
    let regname = if args.is_empty() {
        '"'
    } else {
        let text = string_arg(args, 0)?;
        match text.chars().next() {
            None => '"',
            Some('@') => '"',
            Some(c) => c,
        }
    };
    if args.len() > 1 {
        // The second argument only selects expression-register source in the
        // reference implementation; the host returns a plain string either way.
        let _ = narg(args, 1);
    }
    let as_list = args.len() > 2 && arg(args, 2).truthy();
    let contents = host
        .host_register(regname)
        .transpose()?
        .unwrap_or_else(|| Value::String(String::new()));
    match contents {
        Value::String(text) => {
            if as_list {
                let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
                if text.ends_with('\n') {
                    lines.pop();
                }
                if text.is_empty() {
                    lines.clear();
                }
                Ok(Value::list(lines.into_iter().map(Value::String).collect()))
            } else {
                Ok(Value::String(text))
            }
        }
        Value::List(list) => Ok(Value::list(list.borrow().clone())),
        other => Ok(Value::String(other.to_string_value())),
    }
}

// --------------------------------------------------------------- JSON

fn json_encode_value(value: &Value, js: bool) -> VimResult<String> {
    let mut out = String::new();
    let mut stack: Vec<usize> = Vec::new();
    json_encode_into(value, js, &mut out, &mut stack)?;
    Ok(out)
}

fn json_encode_into(
    value: &Value,
    js: bool,
    out: &mut String,
    stack: &mut Vec<usize>,
) -> VimResult<()> {
    match value {
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::Float(f) => out.push_str(&json_float(*f)),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Null => out.push_str("null"),
        Value::String(text) => json_write_string(text, out),
        Value::Blob(bytes) => {
            out.push('[');
            for (i, byte) in bytes.borrow().iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&byte.to_string());
            }
            out.push(']');
        }
        Value::Funcref(_) => {
            return Err(err("E1161", "Cannot json encode a function"));
        }
        Value::List(list) => {
            let key = Rc::as_ptr(list) as usize;
            if stack.contains(&key) {
                out.push_str("[]");
                return Ok(());
            }
            stack.push(key);
            let items = list.borrow().clone();
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_encode_into(item, js, out, stack)?;
            }
            out.push(']');
            stack.pop();
        }
        Value::Dict(dict) => {
            let key = Rc::as_ptr(dict) as usize;
            if stack.contains(&key) {
                out.push_str("{}");
                return Ok(());
            }
            stack.push(key);
            let entries: Vec<(String, Value)> = dict
                .borrow()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if js && is_simple_json_key(k) {
                    out.push_str(k);
                } else {
                    json_write_string(k, out);
                }
                out.push(':');
                json_encode_into(v, js, out, stack)?;
            }
            out.push('}');
            stack.pop();
        }
    }
    Ok(())
}

fn is_simple_json_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// JSON string escaping, matching `write_string()` in json.c.
fn json_write_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Format a float like Vim's `%g` handling in `vim_snprintf()`: a decimal
/// point is kept, trailing zeroes are removed.
fn json_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "-Infinity".to_string()
        } else {
            "Infinity".to_string()
        };
    }
    let abs = f.abs();
    if abs == 0.0 || (0.001..10_000_000.0).contains(&abs) {
        let text = format!("{f:.6}");
        return trim_json_float(&text);
    }
    let text = format!("{f:.6e}");
    let (mantissa, exponent) = text.split_once('e').unwrap_or((text.as_str(), "0"));
    let mantissa = trim_json_float(mantissa);
    let exp: i32 = exponent.parse().unwrap_or(0);
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exp.abs())
}

fn trim_json_float(text: &str) -> String {
    if !text.contains('.') {
        return format!("{text}.0");
    }
    let mut out = text.to_string();
    while out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.push('0');
    }
    out
}

/// Public JSON encoder used by the channel layer (M7).
pub fn json_encode_public(value: &Value) -> VimResult<String> {
    json_encode_value(value, false)
}

/// Public JSON decoder used by the channel layer (M7).
pub fn json_decode_public(text: &str) -> ValueResult {
    json_decode_value(text, false)
}

fn json_decode_value(text: &str, js: bool) -> ValueResult {
    let mut parser = JsonParser::new(text, js);
    parser.skip_ws();
    if parser.eof() {
        // An empty message decodes to v:none in Vim; `Value` uses v:null.
        return Ok(Value::Null);
    }
    let value = parser.parse_value()?;
    parser.skip_ws();
    if !parser.eof() {
        let rest: String = parser.chars[parser.pos..].iter().collect();
        return Err(err("E488", format!("Trailing characters: {rest}")));
    }
    Ok(value)
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
    js: bool,
}

impl JsonParser {
    fn new(text: &str, js: bool) -> Self {
        Self {
            chars: text.chars().collect(),
            pos: 0,
            js,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn invalid<T>(&self) -> VimResult<T> {
        Err(err("E474", "Invalid argument"))
    }

    fn parse_value(&mut self) -> VimResult<Value> {
        self.skip_ws();
        let Some(c) = self.peek() else {
            return self.invalid();
        };
        match c {
            '[' => self.parse_array(),
            '{' => self.parse_object(),
            '"' => self.parse_string('"').map(Value::String),
            '\'' if self.js => self.parse_string('\'').map(Value::String),
            ',' if self.js => {
                // Empty JS array item: v:none, represented as v:null.
                self.pos += 1;
                Ok(Value::Null)
            }
            '-' | '0'..='9' => self.parse_number(),
            _ => self.parse_keyword(),
        }
    }

    fn parse_keyword(&mut self) -> VimResult<Value> {
        let rest: String = self.chars[self.pos..].iter().collect();
        let lower = rest.to_ascii_lowercase();
        for (word, value) in [
            ("false", Value::Bool(false)),
            ("true", Value::Bool(true)),
            ("null", Value::Null),
        ] {
            if lower.starts_with(word) {
                self.pos += word.chars().count();
                return Ok(value);
            }
        }
        if lower.starts_with("nan") {
            self.pos += 3;
            return Ok(Value::Float(f64::NAN));
        }
        if lower.starts_with("-infinity") {
            self.pos += 9;
            return Ok(Value::Float(f64::NEG_INFINITY));
        }
        if lower.starts_with("infinity") {
            self.pos += 8;
            return Ok(Value::Float(f64::INFINITY));
        }
        self.invalid()
    }

    fn parse_number(&mut self) -> VimResult<Value> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
            // -Infinity
            let rest: String = self.chars[self.pos..].iter().collect();
            if rest.to_ascii_lowercase().starts_with("infinity") {
                self.pos += 8;
                return Ok(Value::Float(f64::NEG_INFINITY));
            }
        }
        let mut is_float = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.pos += 1;
            } else if c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-' {
                // Only continue when the number really continues.
                let after_exponent = self
                    .chars
                    .get(self.pos.wrapping_sub(1))
                    .is_some_and(|&prev| prev == 'e' || prev == 'E');
                if (c == '+' || c == '-') && !after_exponent {
                    break;
                }
                is_float |= c == '.' || c == 'e' || c == 'E';
                self.pos += 1;
            } else {
                break;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            match text.parse::<f64>() {
                Ok(f) => Ok(Value::Float(f)),
                Err(_) => self.invalid(),
            }
        } else {
            match text.parse::<i64>() {
                Ok(n) => Ok(Value::Number(n)),
                Err(_) => {
                    // Out-of-range integers wrap like Vim's varnumber_T.
                    let f = text.parse::<f64>().unwrap_or(0.0);
                    Ok(Value::Number(f as i64))
                }
            }
        }
    }

    fn parse_string(&mut self, quote: char) -> VimResult<String> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return self.invalid();
            };
            if c == quote {
                self.pos += 1;
                return Ok(out);
            }
            if c != '\\' {
                out.push(c);
                self.pos += 1;
                continue;
            }
            self.pos += 1;
            let Some(escaped) = self.peek() else {
                return self.invalid();
            };
            match escaped {
                '"' => {
                    out.push('"');
                    self.pos += 1;
                }
                '\\' => {
                    out.push('\\');
                    self.pos += 1;
                }
                'b' => {
                    out.push('\u{8}');
                    self.pos += 1;
                }
                't' => {
                    out.push('\t');
                    self.pos += 1;
                }
                'n' => {
                    out.push('\n');
                    self.pos += 1;
                }
                'f' => {
                    out.push('\u{c}');
                    self.pos += 1;
                }
                'r' => {
                    out.push('\r');
                    self.pos += 1;
                }
                'u' => {
                    self.pos += 1;
                    let mut code = self.parse_hex4()?;
                    if (0xd800..=0xdbff).contains(&code) {
                        // Possible surrogate pair: \uD800\uDC00.
                        if self.peek() == Some('\\') && self.chars.get(self.pos + 1) == Some(&'u') {
                            let save = self.pos;
                            self.pos += 2;
                            let low = self.parse_hex4()?;
                            if (0xdc00..=0xdfff).contains(&low) {
                                code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                            } else {
                                self.pos = save;
                            }
                        }
                    }
                    out.push(char::from_u32(code as u32).unwrap_or('\u{fffd}'));
                }
                other => {
                    // Unknown escape: Vim drops the backslash and keeps the
                    // character (this is how "\/" works).
                    out.push(other);
                    self.pos += 1;
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> VimResult<i64> {
        let mut value = 0i64;
        for _ in 0..4 {
            let Some(c) = self.peek() else {
                return self.invalid();
            };
            let Some(digit) = c.to_digit(16) else {
                return self.invalid();
            };
            value = value * 16 + i64::from(digit);
            self.pos += 1;
        }
        Ok(value)
    }

    fn parse_array(&mut self) -> VimResult<Value> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                None => return self.invalid(),
                _ => {}
            }
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                _ => return self.invalid(),
            }
        }
        Ok(Value::list(items))
    }

    fn parse_object(&mut self) -> VimResult<Value> {
        self.pos += 1; // '{'
        let mut dict = VimDict::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                None => return self.invalid(),
                _ => {}
            }
            let key = match self.peek() {
                Some('"') => self.parse_string('"')?,
                Some('\'') if self.js => self.parse_string('\'')?,
                _ if self.js => self.parse_unquoted_key()?,
                _ => return self.invalid(),
            };
            self.skip_ws();
            if self.peek() != Some(':') {
                return self.invalid();
            }
            self.pos += 1;
            let value = self.parse_value()?;
            if dict.get(&key).is_some() {
                return Err(err("E938", format!("Duplicate key in JSON: \"{key}\"")));
            }
            dict.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                _ => return self.invalid(),
            }
        }
        Ok(Value::dict(dict))
    }

    fn parse_unquoted_key(&mut self) -> VimResult<String> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == ':' || c <= ' ' {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return self.invalid();
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Dummy host: all `Host` methods have defaults.
    struct H;
    impl crate::env::Host for H {}

    struct RegHost;
    impl crate::env::Host for RegHost {
        fn host_register(&mut self, _name: char) -> Option<ValueResult> {
            Some(Ok(Value::String("line1\nline2\n".to_string())))
        }
    }

    fn callf(name: &str, args: &[Value]) -> ValueResult {
        let mut env = Env::new();
        let mut host = H;
        call(name, args, &mut env, &mut host).expect("function is in FUNCS")
    }

    fn s(text: &str) -> Value {
        Value::string(text)
    }

    fn num(n: i64) -> Value {
        Value::Number(n)
    }

    fn str_of(v: &Value) -> String {
        v.to_string_value()
    }

    fn list_of(v: &Value) -> Vec<Value> {
        match v {
            Value::List(items) => items.borrow().clone(),
            other => panic!("expected list, got {}", other.type_name()),
        }
    }

    fn temp_tree(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rvim_fl_{}_{}_{}", std::process::id(), tag, n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn as_posix(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn unknown_names_return_none() {
        let mut env = Env::new();
        let mut host = H;
        assert!(call("strlen", &[s("x")], &mut env, &mut host).is_none());
    }

    #[test]
    fn arity_errors_are_reported() {
        let mut env = Env::new();
        let mut host = H;
        let result = call("file_readable", &[s("a"), s("b")], &mut env, &mut host).unwrap();
        assert_eq!(result.unwrap_err().code, "E118");
    }

    #[test]
    fn file_readable_test() {
        let dir = temp_tree("readable");
        let file = dir.join("f.txt");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            str_of(&callf("file_readable", &[s(&file.to_string_lossy())]).unwrap()),
            "1"
        );
        assert_eq!(
            str_of(&callf("file_readable", &[s(&dir.to_string_lossy())]).unwrap()),
            "0"
        );
        assert_eq!(
            str_of(&callf("file_readable", &[s(&dir.join("nope").to_string_lossy())]).unwrap()),
            "0"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn filewritable_test() {
        let dir = temp_tree("writable");
        let file = dir.join("f.txt");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            callf("filewritable", &[s(&file.to_string_lossy())])
                .unwrap()
                .to_number(),
            1
        );
        assert_eq!(
            callf("filewritable", &[s(&dir.to_string_lossy())])
                .unwrap()
                .to_number(),
            2
        );
        assert_eq!(
            callf("filewritable", &[s(&dir.join("nope").to_string_lossy())])
                .unwrap()
                .to_number(),
            0
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn getfperm_test() {
        let dir = temp_tree("perm");
        let file = dir.join("f.txt");
        fs::write(&file, b"x").unwrap();
        let perm = str_of(&callf("getfperm", &[s(&file.to_string_lossy())]).unwrap());
        assert_eq!(perm.chars().count(), 9);
        assert!(perm.chars().all(|c| "-rwx".contains(c)));
        let missing =
            str_of(&callf("getfperm", &[s(&dir.join("nope").to_string_lossy())]).unwrap());
        assert!(missing.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn getfsize_test() {
        let dir = temp_tree("size");
        let file = dir.join("f.txt");
        fs::write(&file, b"hello").unwrap();
        assert_eq!(
            callf("getfsize", &[s(&file.to_string_lossy())])
                .unwrap()
                .to_number(),
            5
        );
        assert_eq!(
            callf("getfsize", &[s(&dir.to_string_lossy())])
                .unwrap()
                .to_number(),
            0
        );
        assert_eq!(
            callf("getfsize", &[s(&dir.join("nope").to_string_lossy())])
                .unwrap()
                .to_number(),
            -1
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn getftime_test() {
        let dir = temp_tree("time");
        let file = dir.join("f.txt");
        fs::write(&file, b"x").unwrap();
        assert!(
            callf("getftime", &[s(&file.to_string_lossy())])
                .unwrap()
                .to_number()
                > 0
        );
        assert_eq!(
            callf("getftime", &[s(&dir.join("nope").to_string_lossy())])
                .unwrap()
                .to_number(),
            -1
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn getftype_test() {
        let dir = temp_tree("type");
        let file = dir.join("f.txt");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            str_of(&callf("getftype", &[s(&file.to_string_lossy())]).unwrap()),
            "file"
        );
        assert_eq!(
            str_of(&callf("getftype", &[s(&dir.to_string_lossy())]).unwrap()),
            "dir"
        );
        assert_eq!(
            str_of(&callf("getftype", &[s(&dir.join("nope").to_string_lossy())]).unwrap()),
            ""
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn isabsolutepath_test() {
        #[cfg(windows)]
        {
            assert_eq!(
                callf("isabsolutepath", &[s("C:\\x")]).unwrap().to_number(),
                1
            );
            assert_eq!(callf("isabsolutepath", &[s("C:x")]).unwrap().to_number(), 0);
            assert_eq!(
                callf("isabsolutepath", &[s("\\\\srv\\share")])
                    .unwrap()
                    .to_number(),
                1
            );
            assert_eq!(
                callf("isabsolutepath", &[s("rel/x")]).unwrap().to_number(),
                0
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(callf("isabsolutepath", &[s("/x")]).unwrap().to_number(), 1);
            assert_eq!(callf("isabsolutepath", &[s("~/x")]).unwrap().to_number(), 1);
            assert_eq!(
                callf("isabsolutepath", &[s("rel/x")]).unwrap().to_number(),
                0
            );
        }
    }

    #[test]
    fn findfile_test() {
        let dir = temp_tree("findfile");
        let file = dir.join("f.txt");
        fs::write(&file, b"x").unwrap();
        let sub = dir.join("a");
        fs::create_dir_all(&sub).unwrap();
        let nested = sub.join("f.txt");
        fs::write(&nested, b"x").unwrap();

        let found = str_of(&callf("findfile", &[s("f.txt"), s(&as_posix(&dir))]).unwrap());
        assert_eq!(found, format!("{}/f.txt", as_posix(&dir)));

        let short = str_of(&callf("findfile", &[s("nope.txt"), s(&as_posix(&dir))]).unwrap());
        assert!(short.is_empty());

        let star = format!("{}/**", as_posix(&dir));
        let many = callf("findfile", &[s("f.txt"), s(&star), num(-1)]).unwrap();
        let many = list_of(&many);
        assert_eq!(many.len(), 2);
        assert_eq!(str_of(&many[0]), format!("{}/f.txt", as_posix(&dir)));
        assert_eq!(str_of(&many[1]), format!("{}/a/f.txt", as_posix(&dir)));

        let second = str_of(&callf("findfile", &[s("f.txt"), s(&star), num(2)]).unwrap());
        assert_eq!(second, format!("{}/a/f.txt", as_posix(&dir)));

        let beyond = str_of(&callf("findfile", &[s("f.txt"), s(&star), num(3)]).unwrap());
        assert!(beyond.is_empty());

        let invalid = call("findfile", &[s("x"), s("**x")], &mut Env::new(), &mut H)
            .unwrap()
            .unwrap_err();
        assert_eq!(invalid.code, "E343");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finddir_test() {
        let dir = temp_tree("finddir");
        let deep = dir.join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        let file = dir.join("notadir");
        fs::write(&file, b"x").unwrap();

        // A literal directory is found, a file is not.
        let found = str_of(&callf("finddir", &[s("a"), s(&as_posix(&dir))]).unwrap());
        assert_eq!(found, format!("{}/a", as_posix(&dir)));
        let not_dir = str_of(&callf("finddir", &[s("notadir"), s(&as_posix(&dir))]).unwrap());
        assert!(not_dir.is_empty());

        let star = format!("{}/**", as_posix(&dir));
        let all = list_of(&callf("finddir", &[s("b"), s(&star), num(-1)]).unwrap());
        assert_eq!(all.len(), 1);
        assert_eq!(str_of(&all[0]), format!("{}/a/b", as_posix(&dir)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flatten_test() {
        let list = Value::list(vec![
            num(1),
            Value::list(vec![num(2), Value::list(vec![num(3), num(4)])]),
            num(5),
        ]);
        let result = callf("flatten", &[list]).unwrap();
        assert_eq!(
            result,
            Value::list(vec![num(1), num(2), num(3), num(4), num(5)])
        );

        let list = Value::list(vec![
            num(1),
            Value::list(vec![num(2), Value::list(vec![num(3)])]),
        ]);
        let result = callf("flatten", &[list, num(1)]).unwrap();
        assert_eq!(
            result,
            Value::list(vec![num(1), num(2), Value::list(vec![num(3)])])
        );

        let bad = callf("flatten", &[num(1)]);
        assert_eq!(bad.unwrap_err().code, "E686");
    }

    #[test]
    fn flattennew_test() {
        let list = Value::list(vec![num(1), Value::list(vec![num(2), num(3)])]);
        let result = callf("flattennew", std::slice::from_ref(&list)).unwrap();
        assert_eq!(result, Value::list(vec![num(1), num(2), num(3)]));
        // The original list is unchanged.
        assert_eq!(
            list,
            Value::list(vec![num(1), Value::list(vec![num(2), num(3)])])
        );
    }

    #[test]
    fn list2blob_test() {
        let blob = callf("list2blob", &[Value::list(vec![num(65), num(66)])]).unwrap();
        let Value::Blob(bytes) = &blob else {
            panic!("expected blob");
        };
        assert_eq!(*bytes.borrow(), vec![65u8, 66]);
        let bad = callf("list2blob", &[Value::list(vec![num(256)])]);
        assert_eq!(bad.unwrap_err().code, "E1239");
    }

    #[test]
    fn list2str_test() {
        assert_eq!(
            str_of(&callf("list2str", &[Value::list(vec![num(65), num(66)])]).unwrap()),
            "AB"
        );
        assert_eq!(
            str_of(&callf("list2str", &[Value::list(vec![num(0x4e2d)])]).unwrap()),
            "中"
        );
        assert_eq!(
            str_of(&callf("list2str", &[Value::list(vec![])]).unwrap()),
            ""
        );
    }

    #[test]
    fn funcref_test() {
        let value = callf("funcref", &[s("strlen")]).unwrap();
        assert_eq!(value.type_name(), "func");
        let Value::Funcref(f) = &value else {
            panic!("expected funcref");
        };
        assert_eq!(f.name, "strlen");
        assert!(f.partial.is_empty());

        let with_args = callf("funcref", &[s("strlen"), Value::list(vec![num(1)])]).unwrap();
        let Value::Funcref(f) = &with_args else {
            panic!("expected funcref");
        };
        assert_eq!(f.partial, vec![num(1)]);

        let dict = Value::dict(VimDict::new());
        let with_dict = callf("funcref", &[s("strlen"), dict.clone()]).unwrap();
        let Value::Funcref(f) = &with_dict else {
            panic!("expected funcref");
        };
        assert!(f.dict.is_some());

        assert_eq!(callf("funcref", &[s("")]).unwrap_err().code, "E475");
        assert_eq!(callf("funcref", &[s("1abc")]).unwrap_err().code, "E475");
        assert_eq!(
            callf("funcref", &[s("strlen"), Value::list(vec![]), num(1)])
                .unwrap_err()
                .code,
            "E922"
        );
    }

    #[test]
    fn garbagecollect_test() {
        assert_eq!(callf("garbagecollect", &[]).unwrap().to_number(), 0);
        assert_eq!(callf("garbagecollect", &[num(1)]).unwrap().to_number(), 0);
    }

    #[test]
    fn getenv_test() {
        let path = callf("getenv", &[s("PATH")]).unwrap();
        assert!(matches!(path, Value::String(_)));
        assert!(matches!(
            callf("getenv", &[s("RVIM_FL_DEFINITELY_NOT_SET_42")]).unwrap(),
            Value::Null
        ));
    }

    #[test]
    fn getfontname_test() {
        assert_eq!(str_of(&callf("getfontname", &[]).unwrap()), "");
        assert_eq!(str_of(&callf("getfontname", &[s("*")]).unwrap()), "");
    }

    #[test]
    fn getimstatus_test() {
        assert_eq!(callf("getimstatus", &[]).unwrap().to_number(), 0);
    }

    #[test]
    fn getpid_test() {
        assert!(callf("getpid", &[]).unwrap().to_number() > 0);
    }

    #[test]
    fn getreg_test() {
        let mut env = Env::new();
        let mut host = RegHost;
        let value = call("getreg", &[], &mut env, &mut host).unwrap().unwrap();
        assert_eq!(str_of(&value), "line1\nline2\n");
        let value = call("getreg", &[s("a"), num(0), num(1)], &mut env, &mut host)
            .unwrap()
            .unwrap();
        assert_eq!(list_of(&value), vec![s("line1"), s("line2")]);

        // No host register support: treated as an empty register.
        let mut env = Env::new();
        let mut host = H;
        let value = call("getreg", &[], &mut env, &mut host).unwrap().unwrap();
        assert_eq!(str_of(&value), "");
    }

    #[test]
    fn gettext_test() {
        assert_eq!(str_of(&callf("gettext", &[s("hello")]).unwrap()), "hello");
        assert_eq!(callf("gettext", &[s("")]).unwrap_err().code, "E475");
    }

    #[test]
    fn hostname_test() {
        assert!(matches!(callf("hostname", &[]).unwrap(), Value::String(_)));
    }

    #[test]
    fn glob2regpat_test() {
        // A leading '*' has no '^' anchor and the stars themselves are
        // consumed (reference: file_pat_to_reg_pat()).
        assert_eq!(str_of(&callf("glob2regpat", &[s("*.c")]).unwrap()), "\\.c$");
        assert_eq!(
            str_of(&callf("glob2regpat", &[s("Make*.mak")]).unwrap()),
            "^Make.*\\.mak$"
        );
        assert_eq!(str_of(&callf("glob2regpat", &[s("")]).unwrap()), "^$");
        assert_eq!(str_of(&callf("glob2regpat", &[s("a?b")]).unwrap()), "^a.b$");
        assert_eq!(str_of(&callf("glob2regpat", &[s("*.c*")]).unwrap()), "\\.c");
    }

    #[test]
    fn glob_test() {
        let dir = temp_tree("glob");
        let base = as_posix(&dir);
        fs::write(dir.join("a.txt"), b"x").unwrap();
        fs::write(dir.join("b.txt"), b"x").unwrap();
        fs::write(dir.join("c.md"), b"x").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("d.txt"), b"x").unwrap();

        let pattern = format!("{base}/*.txt");
        let text = str_of(&callf("glob", &[s(&pattern)]).unwrap());
        assert_eq!(text, format!("{base}/a.txt\n{base}/b.txt"));

        let listed = list_of(&callf("glob", &[s(&pattern), num(0), num(1)]).unwrap());
        assert_eq!(
            listed,
            vec![s(&format!("{base}/a.txt")), s(&format!("{base}/b.txt"))]
        );

        // Non-matching pattern yields an empty string.
        assert_eq!(
            str_of(&callf("glob", &[s(&format!("{base}/*.none"))]).unwrap()),
            ""
        );

        // Recursive "**".
        let recursive =
            list_of(&callf("glob", &[s(&format!("{base}/**/*.txt")), num(0), num(1)]).unwrap());
        assert_eq!(
            recursive,
            vec![
                s(&format!("{base}/a.txt")),
                s(&format!("{base}/b.txt")),
                s(&format!("{base}/sub/d.txt")),
            ]
        );

        // Width wildcard and character class.
        let single = str_of(&callf("glob", &[s(&format!("{base}/?.txt"))]).unwrap());
        assert!(single.contains("a.txt") || single.contains("b.txt"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn globpath_test() {
        let dir = temp_tree("globpath");
        let base = as_posix(&dir);
        fs::create_dir_all(dir.join("one")).unwrap();
        fs::write(dir.join("one").join("x.txt"), b"x").unwrap();
        let expected = format!("{base}/one/x.txt");
        let text = str_of(&callf("globpath", &[s(&format!("{base}/one")), s("*.txt")]).unwrap());
        assert_eq!(text, expected);
        let listed = list_of(
            &callf(
                "globpath",
                &[s(&format!("{base}/one")), s("*.txt"), num(0), num(1)],
            )
            .unwrap(),
        );
        assert_eq!(listed, vec![s(&expected)]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_encode_test() {
        assert_eq!(str_of(&callf("json_encode", &[num(42)]).unwrap()), "42");
        assert_eq!(
            str_of(&callf("json_encode", &[s("a\"b")]).unwrap()),
            "\"a\\\"b\""
        );
        assert_eq!(
            str_of(&callf("json_encode", &[Value::Bool(true)]).unwrap()),
            "true"
        );
        assert_eq!(
            str_of(&callf("json_encode", &[Value::Null]).unwrap()),
            "null"
        );
        assert_eq!(
            str_of(&callf("json_encode", &[Value::Float(1.5)]).unwrap()),
            "1.5"
        );
        assert_eq!(
            str_of(&callf("json_encode", &[Value::Float(2.0)]).unwrap()),
            "2.0"
        );
        assert_eq!(
            str_of(&callf("json_encode", &[Value::list(vec![num(1), s("x")])]).unwrap()),
            "[1,\"x\"]"
        );
        let mut dict = VimDict::new();
        dict.insert("a", num(1));
        dict.insert("b c", s("x"));
        assert_eq!(
            str_of(&callf("json_encode", &[Value::dict(dict.clone())]).unwrap()),
            "{\"a\":1,\"b c\":\"x\"}"
        );
        // js_encode leaves simple keys unquoted.
        assert_eq!(
            str_of(&callf("js_encode", &[Value::dict(dict)]).unwrap()),
            "{a:1,\"b c\":\"x\"}"
        );
        // Functions cannot be encoded.
        let f = callf("funcref", &[s("strlen")]).unwrap();
        assert_eq!(callf("json_encode", &[f]).unwrap_err().code, "E1161");
    }

    #[test]
    fn json_decode_test() {
        let value = callf(
            "json_decode",
            &[s("{\"a\": 1, \"b\": [true, null, \"x\"], \"c\": 1.5}")],
        )
        .unwrap();
        let Value::Dict(dict) = &value else {
            panic!("expected dict");
        };
        let dict = dict.borrow();
        assert_eq!(dict.get("a"), Some(&num(1)));
        assert_eq!(
            dict.get("b"),
            Some(&Value::list(vec![Value::Bool(true), Value::Null, s("x")]))
        );
        assert_eq!(dict.get("c"), Some(&Value::Float(1.5)));
        assert_eq!(
            callf("json_decode", &[s("[]")]).unwrap(),
            Value::list(vec![])
        );
        assert_eq!(callf("json_decode", &[s("123")]).unwrap(), num(123));
        assert!(callf("json_decode", &[s("not json")]).is_err());
        assert!(callf("json_decode", &[s("{} x")]).is_err());
        // Duplicate keys are an error.
        assert_eq!(
            callf("json_decode", &[s("{\"a\":1,\"a\":2}")])
                .unwrap_err()
                .code,
            "E938"
        );
    }

    #[test]
    fn js_decode_test() {
        let value = callf("js_decode", &[s("{a: 1, 'b': 2,}")]).unwrap();
        let Value::Dict(dict) = &value else {
            panic!("expected dict");
        };
        let dict = dict.borrow();
        assert_eq!(dict.get("a"), Some(&num(1)));
        assert_eq!(dict.get("b"), Some(&num(2)));
        assert_eq!(
            callf("js_decode", &[s("[1,2]")]).unwrap(),
            Value::list(vec![num(1), num(2)])
        );
    }

    #[test]
    fn islocked_test() {
        let mut env = Env::new();
        let mut host = H;
        let missing = call("islocked", &[s("nope")], &mut env, &mut host)
            .unwrap()
            .unwrap();
        assert_eq!(missing.to_number(), -1);
        env.globals.insert("locked_var".to_string(), num(1));
        let found = call("islocked", &[s("locked_var")], &mut env, &mut host)
            .unwrap()
            .unwrap();
        assert_eq!(found.to_number(), 0);
        assert_eq!(
            call("islocked", &[s("")], &mut env, &mut host)
                .unwrap()
                .unwrap_err()
                .code,
            "E475"
        );
    }

    #[test]
    fn js_encode_test() {
        assert_eq!(str_of(&callf("js_encode", &[num(7)]).unwrap()), "7");
        let mut dict = VimDict::new();
        dict.insert("key", s("v"));
        assert_eq!(
            str_of(&callf("js_encode", &[Value::dict(dict)]).unwrap()),
            "{key:\"v\"}"
        );
        assert_eq!(
            str_of(&callf("js_encode", &[Value::list(vec![Value::Bool(true)])]).unwrap()),
            "[true]"
        );
    }

    #[test]
    fn every_listed_name_is_dispatched() {
        for name in FUNCS {
            let mut env = Env::new();
            let mut host = H;
            let result = call(name, &[], &mut env, &mut host);
            assert!(result.is_some(), "{name} is in FUNCS but not callable");
            if let Some(Err(error)) = result {
                assert!(
                    !error.message.contains("listed but not dispatched"),
                    "{name} has no dispatch arm"
                );
            }
        }
    }
}
