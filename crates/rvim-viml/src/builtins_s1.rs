//! Builtin functions, group s..sh (M3 coverage batch).
//!
//! See `docs/m3_builtins_plan.md` for the candidate list, semantics rules and
//! the skip list (subsystems that are not built yet).
//!
//! Implemented here: the pure-ish remainder of the `s..sh` group
//! (`sha256`, `shellescape`, `shiftwidth`, `simplify`, `sinh`, `slice`,
//! `srand`, `str*`, `system`/`systemlist`, `tanh`, `tempname`, `setfperm`).
//! Everything else in the group needs a subsystem that does not exist yet
//! (screen, buffer search, `sign_*`, syntax, spell, sound, quickfix, tags,
//! terminal, cursor/command-line state) or belongs to the Ex layer's host
//! builtins (`setbufline`, `setbufvar`, `setpos`, `setreg`).

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rvim_core::{VimError, VimResult};

use crate::env::{Env, Host};
use crate::value::{Value, ValueResult};

/// Names implemented in this module (coverage source of truth).
pub const FUNCS: &[&str] = &[
    "setfperm",
    "sha256",
    "shellescape",
    "shiftwidth",
    "simplify",
    "sinh",
    "slice",
    "srand",
    "str2list",
    "strcharlen",
    "strdisplaywidth",
    "strgetchar",
    "strtrans",
    "system",
    "systemlist",
    "tanh",
    "tempname",
];

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

fn dispatch(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    match name {
        // ------------------------------------------------------- hashing
        "sha256" => Ok(Value::String(sha256_hex(sarg(args, 0).as_bytes()))),

        // ------------------------------------------------------- shell
        "shellescape" => {
            let text = sarg(args, 0);
            let special = args.get(1).is_some_and(Value::truthy);
            let shell = host
                .option_get("shell")
                .map(|v| v.to_string_value())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(default_shell_name);
            let shellslash = host
                .option_get("shellslash")
                .map(|v| v.truthy())
                .unwrap_or(false);
            Ok(Value::String(shellescape(
                &text, special, &shell, shellslash,
            )))
        }
        "system" | "systemlist" => {
            let cmd = sarg(args, 0);
            let input = shell_input(args)?;
            let (shell, flag) = shell_spec(host);
            let (bytes, code) = run_shell(&shell, &flag, &cmd, input.as_deref())?;
            env.set_var("v:shell_error", Value::Number(i64::from(code)))?;
            if name == "systemlist" {
                Ok(Value::list(
                    system_list(&bytes).into_iter().map(Value::String).collect(),
                ))
            } else {
                Ok(Value::String(system_output(&bytes)))
            }
        }

        // ------------------------------------------------------- options
        "shiftwidth" => {
            if !args.is_empty() && narg(args, 0) < 0 {
                // Vim reports a type error for the column and leaves 0.
                return Ok(Value::Number(0));
            }
            let mut sw = host
                .option_get("shiftwidth")
                .map(|v| v.to_number())
                .unwrap_or(0);
            if sw == 0 {
                sw = host
                    .option_get("tabstop")
                    .map(|v| v.to_number())
                    .filter(|n| *n > 0)
                    .unwrap_or(8);
            }
            Ok(Value::Number(sw))
        }

        // ------------------------------------------------------- file names
        "simplify" => Ok(Value::String(simplify_path(&sarg(args, 0)))),
        "tempname" => Ok(Value::String(tempname())),
        "setfperm" => {
            let fname = sarg(args, 0);
            let mode = sarg(args, 1);
            let chars: Vec<char> = mode.chars().collect();
            if chars.len() != 9 {
                return Err(err("E474", format!("Invalid argument: {mode}")));
            }
            let mut bits = 0o000u32;
            for (i, c) in chars.iter().enumerate() {
                if *c != '-' {
                    bits |= 1 << (8 - i);
                }
            }
            Ok(Value::Number(i64::from(set_file_mode(&fname, bits))))
        }

        // ------------------------------------------------------- floats
        "sinh" => Ok(Value::Float(arg(args, 0).to_float()?.sinh())),
        "tanh" => Ok(Value::Float(arg(args, 0).to_float()?.tanh())),

        // ------------------------------------------------------- list/string slicing
        "slice" => {
            let start = arg(args, 1).as_number()?;
            let end = if args.len() > 2 {
                Some(arg(args, 2).as_number()?)
            } else {
                None
            };
            slice_value(&arg(args, 0), start, end)
        }

        // ------------------------------------------------------- random seed
        "srand" => {
            let mut x = if args.is_empty() {
                entropy_seed()
            } else {
                arg(args, 0).as_number()? as u32
            };
            let mut out = Vec::with_capacity(4);
            for _ in 0..4 {
                out.push(Value::Number(i64::from(splitmix32(&mut x))));
            }
            Ok(Value::list(out))
        }

        // ------------------------------------------------------- strings
        "str2list" => {
            // The project's strings are always UTF-8, so `{utf8}` (which Vim
            // uses to force UTF-8 instead of 'encoding') does not change the
            // result; composing characters are kept as separate values.
            let text = sarg(args, 0);
            Ok(Value::list(
                text.chars().map(|c| Value::Number(c as i64)).collect(),
            ))
        }
        "strcharlen" => {
            let text = sarg(args, 0);
            let mut count = 0i64;
            for c in text.chars() {
                // Composing characters do not start a new character.
                if count > 0 && is_composing(c) {
                    continue;
                }
                count += 1;
            }
            Ok(Value::Number(count))
        }
        "strdisplaywidth" => {
            let text = sarg(args, 0);
            let start_col = if args.len() > 1 { narg(args, 1) } else { 0 };
            let tabstop = host
                .option_get("tabstop")
                .map(|v| v.to_number())
                .filter(|n| *n > 0)
                .unwrap_or(8);
            let mut col = start_col;
            for c in text.chars() {
                if c == '\t' {
                    col += tabstop - col.rem_euclid(tabstop);
                } else {
                    col += char_cells(c);
                }
            }
            Ok(Value::Number(col - start_col))
        }
        "strgetchar" => {
            let text = sarg(args, 0);
            let wanted = narg(args, 1);
            let mut index = 0i64;
            for c in text.chars() {
                if index > 0 && is_composing(c) {
                    continue;
                }
                if index == wanted {
                    return Ok(Value::Number(c as i64));
                }
                index += 1;
            }
            Ok(Value::Number(-1))
        }
        "strtrans" => Ok(Value::String(transstr(&sarg(args, 0)))),

        other => Err(err(
            "E117",
            format!("builtin {other} is listed but not dispatched"),
        )),
    }
}

// ---------------------------------------------------------------------------
// sha256()
// ---------------------------------------------------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of `data`, rendered as 64 lowercase hex characters.
fn sha256_hex(data: &[u8]) -> String {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = Vec::with_capacity(data.len() + 72);
    message.extend_from_slice(data);
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let b = &chunk[i * 4..i * 4 + 4];
            *word = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for (k, wi) in SHA256_K.iter().zip(w.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(*wi);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut out = String::with_capacity(64);
    for word in state {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

// ---------------------------------------------------------------------------
// shellescape()
// ---------------------------------------------------------------------------

fn default_shell_name() -> String {
    if cfg!(windows) {
        "cmd.exe".to_string()
    } else {
        "sh".to_string()
    }
}

/// Last path component of a shell name (`/bin/bash -x` -> `bash -x`).
fn shell_tail(shell: &str) -> &str {
    shell.rsplit(['/', '\\']).next().unwrap_or(shell)
}

fn shellescape(text: &str, special: bool, shell: &str, shellslash: bool) -> String {
    let tail = shell_tail(shell);
    let csh_like = tail.contains("csh");
    let fish_like = tail.contains("fish");
    let powershell = tail.contains("pwsh") || (cfg!(windows) && tail.contains("powershell"));
    // MS-Windows uses double quotes unless PowerShell or 'shellslash' is set;
    // elsewhere single quotes are used.
    let double_quotes = cfg!(windows) && !powershell && !shellslash;

    let mut out = String::with_capacity(text.len() + 2);
    out.push(if double_quotes { '"' } else { '\'' });
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (byte_idx, c) = chars[i];
        if double_quotes && c == '"' {
            out.push('"');
            out.push('"');
            i += 1;
            continue;
        }
        if !double_quotes && c == '\'' {
            if powershell {
                out.push('\'');
                out.push('\'');
            } else {
                out.push('\'');
                out.push('\\');
                out.push('\'');
                out.push('\'');
            }
            i += 1;
            continue;
        }
        if c == '\n' && (csh_like || special) {
            out.push('\\');
            if csh_like && special {
                out.push('\\');
            }
            out.push('\n');
            i += 1;
            continue;
        }
        if c == '!' && (csh_like || special) {
            out.push('\\');
            if csh_like && special {
                out.push('\\');
            }
            out.push('!');
            i += 1;
            continue;
        }
        if special {
            if let Some(len) = cmdline_var_len(&text[byte_idx..]) {
                out.push('\\');
                for (_, vc) in chars.iter().skip(i).take(len) {
                    out.push(*vc);
                }
                i += len;
                continue;
            }
        }
        if c == '\\' && fish_like {
            out.push('\\');
            out.push('\\');
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out.push(if double_quotes { '"' } else { '\'' });
    out
}

/// Length in bytes (== chars, these are ASCII) of a `:!` command-line
/// variable starting at `rest`, if any.
fn cmdline_var_len(rest: &str) -> Option<usize> {
    const VARS: &[&str] = &[
        "<cword>", "<cWORD>", "<cexpr>", "<cfile>", "<sfile>", "<slnum>", "<stack>", "<script>",
        "<afile>", "<abuf>", "<amatch>", "<sflnum>", "<SID>", "<client>", "%", "#",
    ];
    VARS.iter()
        .find(|name| rest.starts_with(**name))
        .map(|name| name.len())
}

// ---------------------------------------------------------------------------
// system() / systemlist()
// ---------------------------------------------------------------------------

/// The `{input}` argument of `system()`/`systemlist()`.
fn shell_input(args: &[Value]) -> VimResult<Option<String>> {
    match args.get(1) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(Value::List(items)) => {
            // Like writefile() with "b": NUL between items, newlines inside an
            // item become NUL.
            let lines: Vec<String> = items
                .borrow()
                .iter()
                .map(|v| v.to_string_value().replace('\n', "\0"))
                .collect();
            Ok(Some(lines.join("\n")))
        }
        Some(Value::Number(n)) => Err(err(
            "E474",
            format!("Invalid argument: system() from buffer {n} is not supported"),
        )),
        Some(other) => Ok(Some(other.to_string_value())),
    }
}

fn shell_spec(host: &mut dyn Host) -> (String, String) {
    let shell = host
        .option_get("shell")
        .map(|v| v.to_string_value())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMSPEC").ok().filter(|s| !s.is_empty()))
        .or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(default_shell_name);
    let flag = host
        .option_get("shellcmdflag")
        .map(|v| v.to_string_value())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_shell_flag(&shell));
    (shell, flag)
}

fn default_shell_flag(shell: &str) -> String {
    let tail = shell_tail(shell).to_ascii_lowercase();
    if tail.contains("powershell") || tail.contains("pwsh") {
        "-Command".to_string()
    } else if tail.contains("cmd") {
        "/C".to_string()
    } else {
        "-c".to_string()
    }
}

/// Run `cmd` through the shell, returning (stdout+stderr, exit status).
fn run_shell(shell: &str, flag: &str, cmd: &str, input: Option<&str>) -> VimResult<(Vec<u8>, i32)> {
    let mut command = Command::new(shell);
    command
        .arg(flag)
        .arg(cmd)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| err("E379", format!("Cannot run shell: {e}")))?;
    if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
        // The command may exit without reading all input; Vim feeds a temp
        // file, so a broken pipe is not an error for us either.
        let _ = stdin.write_all(data.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|e| err("E379", format!("Cannot run shell: {e}")))?;
    let mut bytes = output.stdout;
    // Vim's 'shellredir' merges stderr into stdout.
    bytes.extend_from_slice(&output.stderr);
    let code = if output.status.success() {
        0
    } else {
        output.status.code().unwrap_or(-1)
    };
    Ok((bytes, code))
}

fn system_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut chars = text.chars().peekable();
    let mut out = String::with_capacity(text.len());
    while let Some(c) = chars.next() {
        if cfg!(windows) && c == '\r' && chars.peek() == Some(&'\n') {
            continue;
        }
        if c == '\0' {
            out.push('\u{1}');
        } else {
            out.push(c);
        }
    }
    out
}

fn system_list(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut items: Vec<String> = text
        .split('\n')
        .map(|line| line.replace('\0', "\n"))
        .collect();
    if items.last().is_some_and(String::is_empty) {
        items.pop();
    }
    items
}

// ---------------------------------------------------------------------------
// simplify()
// ---------------------------------------------------------------------------

fn is_path_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Textual simplification of a file name (Vim's `simplify()`).
///
/// Shortcuts/symbolic links are not resolved. `dir/..` is only stripped when
/// `dir` does not exist as a non-directory entry; that is the same guard Vim
/// uses to avoid changing the meaning of an erroneous name.
fn simplify_path(path: &str) -> String {
    let ends_with_sep = path.chars().last().is_some_and(is_path_sep);
    let sep = path.chars().find(|c| is_path_sep(*c)).unwrap_or('/');

    let mut rest = path;
    let mut prefix = String::new();
    if cfg!(windows) {
        let chars: Vec<char> = rest.chars().collect();
        if chars.len() >= 2 && chars[0].is_ascii_alphabetic() && chars[1] == ':' {
            prefix.push(chars[0]);
            prefix.push(':');
            rest = &rest[2..];
        }
    }

    let leading: Vec<char> = rest.chars().take_while(|c| is_path_sep(*c)).collect();
    let mut root = String::new();
    if !leading.is_empty() {
        if cfg!(windows) {
            root.push(sep);
        } else if leading.len() == 2 {
            // Posix: "//path" is unchanged, "///path" is "/path".
            root.push_str("//");
        } else {
            root.push('/');
        }
    }

    let mut stack: Vec<&str> = Vec::new();
    let mut keep_leading_dot = false;
    for comp in rest.trim_start_matches(is_path_sep).split(is_path_sep) {
        if comp.is_empty() {
            continue;
        }
        if comp == "." {
            if stack.is_empty() && root.is_empty() {
                keep_leading_dot = true;
            }
            continue;
        }
        if comp == ".." {
            if let Some(last) = stack.last() {
                if *last != ".." && can_strip_component(&prefix, &root, &stack, sep) {
                    stack.pop();
                    continue;
                }
            }
            stack.push("..");
            continue;
        }
        stack.push(comp);
    }

    let mut out = prefix;
    out.push_str(&root);
    if keep_leading_dot {
        out.push('.');
        if !stack.is_empty() {
            out.push(sep);
        }
    }
    for (i, comp) in stack.iter().enumerate() {
        if i > 0 {
            out.push(sep);
        }
        out.push_str(comp);
    }
    if ends_with_sep && !out.is_empty() && !out.ends_with(is_path_sep) {
        out.push(sep);
    }
    out
}

fn can_strip_component(prefix: &str, root: &str, stack: &[&str], sep: char) -> bool {
    let mut path = String::from(prefix);
    path.push_str(root);
    for (i, comp) in stack.iter().enumerate() {
        if i > 0 {
            path.push(sep);
        }
        path.push_str(comp);
    }
    match std::fs::metadata(&path) {
        Ok(meta) => meta.is_dir(),
        // A non-existent component is stripped, matching Vim.
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// tempname()
// ---------------------------------------------------------------------------

static TEMPNAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique file name in the temp directory; the file is not created.
fn tempname() -> String {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    for _ in 0..1000 {
        let n = TEMPNAME_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = dir.join(format!("rvim{pid}_{n}"));
        if !candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    dir.join(format!("rvim{pid}_fallback"))
        .to_string_lossy()
        .into_owned()
}

// ---------------------------------------------------------------------------
// setfperm()
// ---------------------------------------------------------------------------

/// Apply a mode built like Vim's: bit 8 = owner read ... bit 0 = other execute.
fn set_file_mode(path: &str, bits: u32) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(bits);
        std::fs::set_permissions(path, perms).is_ok()
    }
    #[cfg(windows)]
    {
        // MS-Windows only has the read-only attribute; _wchmod() looks at the
        // owner write bit.
        let readonly = bits & 0o200 == 0;
        match std::fs::metadata(path) {
            Ok(meta) => {
                let mut perms = meta.permissions();
                perms.set_readonly(readonly);
                std::fs::set_permissions(path, perms).is_ok()
            }
            Err(_) => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, bits);
        false
    }
}

// ---------------------------------------------------------------------------
// slice()
// ---------------------------------------------------------------------------

fn slice_value(target: &Value, start: i64, end: Option<i64>) -> ValueResult {
    match target {
        Value::String(text) => Ok(Value::String(slice_string(text, start, end))),
        Value::Number(n) => Ok(Value::String(slice_string(&n.to_string(), start, end))),
        Value::List(list) => {
            let items = list.borrow();
            match slice_bounds(items.len() as i64, start, end) {
                Some((s, e)) => Ok(Value::list(items[s..e].to_vec())),
                None => Ok(Value::list(Vec::new())),
            }
        }
        Value::Blob(blob) => {
            let bytes = blob.borrow();
            match slice_bounds(bytes.len() as i64, start, end) {
                Some((s, e)) => Ok(Value::blob(bytes[s..e].to_vec())),
                None => Ok(Value::blob(Vec::new())),
            }
        }
        Value::Dict(_) => Err(err("E719", "Cannot slice a Dictionary")),
        other => Err(err(
            "E909",
            format!("Cannot index a {} value", other.type_name()),
        )),
    }
}

/// `slice()` on a string: character indexes, `{end}` is exclusive. Composing
/// characters are counted separately (the project has no composing table for
/// indexing).
fn slice_string(text: &str, start: i64, end: Option<i64>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len() as i64;
    let mut s = if start < 0 { len + start } else { start };
    if s < 0 {
        s = 0;
    }
    let e = match end {
        None => len,
        Some(raw) => {
            if raw < 0 {
                len + raw
            } else {
                raw
            }
        }
    };
    if s >= len || e <= s || e <= 0 {
        return String::new();
    }
    let e = e.min(len);
    chars[s as usize..e as usize].iter().collect()
}

/// `slice()` on a List/Blob: `{end}` is exclusive, negative indexes count from
/// the end. Returns None for an empty result.
fn slice_bounds(len: i64, start: i64, end: Option<i64>) -> Option<(usize, usize)> {
    let mut s = if start < 0 { len + start } else { start };
    if s < 0 || s >= len {
        // A too-small start yields an empty list (legacy script behaviour).
        s = len;
    }
    let e = match end {
        None => len,
        Some(raw) => {
            if raw < 0 {
                len + raw
            } else {
                raw.min(len)
            }
        }
    };
    if s >= len || e <= s || e <= 0 {
        return None;
    }
    Some((s as usize, e as usize))
}

// ---------------------------------------------------------------------------
// srand()
// ---------------------------------------------------------------------------

fn splitmix32(x: &mut u32) -> u32 {
    *x = x.wrapping_add(0x9e37_79b9);
    let mut z = *x;
    z = (z ^ (z >> 16)).wrapping_mul(0x85eb_ca6b);
    z = (z ^ (z >> 13)).wrapping_mul(0xc2b2_ae35);
    z ^ (z >> 16)
}

/// Seed for `srand()` without an argument: time, pid and a counter (Vim reads
/// /dev/urandom, which is not available through `std`).
fn entropy_seed() -> u32 {
    static SEED_COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = SEED_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());
    ((nanos as u32) ^ ((nanos >> 32) as u32))
        ^ (pid as u32)
        ^ (counter as u32).wrapping_mul(0x9e37_79b9)
}

// ---------------------------------------------------------------------------
// Unicode helpers (strcharlen / strdisplaywidth / strgetchar / strtrans)
// ---------------------------------------------------------------------------

/// Rough Unicode combining table (the exact table lives in Vim's mbyte.c).
fn is_composing(c: char) -> bool {
    let u = c as u32;
    matches!(
        u,
        0x0300..=0x036f
            | 0x0483..=0x0489
            | 0x0591..=0x05bd
            | 0x0610..=0x061a
            | 0x064b..=0x065f
            | 0x06d6..=0x06ed
            | 0x0730..=0x074a
            | 0x07a6..=0x07b0
            | 0x0900..=0x0902
            | 0x093c
            | 0x0941..=0x0948
            | 0x094d
            | 0x0951..=0x0957
            | 0x1ab0..=0x1aff
            | 0x1dc0..=0x1dff
            | 0x20d0..=0x20f0
            | 0xfe00..=0xfe0f
            | 0xfe20..=0xfe2f
            | 0xe0100..=0xe01ef
    )
}

/// Display cells of one character (0 for composing characters, 2 for the
/// common double-width ranges).
fn char_cells(c: char) -> i64 {
    if is_composing(c) {
        return 0;
    }
    let u = c as u32;
    let wide = matches!(
        u,
        0x1100..=0x115f
            | 0x2e80..=0x303e
            | 0x3041..=0x33ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xa000..=0xa4cf
            | 0xa960..=0xa97f
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff01..=0xff60
            | 0xffe0..=0xffe6
            | 0x1b000..=0x1b2ff
            | 0x1f000..=0x1f9ff
            | 0x20000..=0x3fffd
    );
    if wide { 2 } else { 1 }
}

/// True when Vim's `vim_isprintc()` would consider the character printable.
fn is_printable(c: char) -> bool {
    let u = c as u32;
    if u < 0x100 {
        return (0x20..=0x7e).contains(&u) || (0xa1..=0xff).contains(&u);
    }
    !matches!(
        u,
        0x070f
            | 0x180b..=0x180e
            | 0x200b..=0x200f
            | 0x202a..=0x202e
            | 0x2060..=0x206f
            | 0xd800..=0xdfff
            | 0xfeff
            | 0xfff9..=0xfffb
            | 0xfffe..=0xffff
    )
}

fn push_translated_hex(out: &mut String, code: u32) {
    if code > 0xff {
        out.push_str(&format!("<{code:04x}>"));
    } else {
        out.push_str(&format!("<{code:02x}>"));
    }
}

/// `strtrans()`: translate unprintable characters like Vim displays them.
fn transstr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let code = c as u32;
        if c == '\n' {
            // Newlines are shown/passed around as NULs.
            out.push_str("^@");
        } else if code < 0x20 || code == 0x7f {
            out.push('^');
            out.push(char::from_u32(code ^ 0x40).unwrap_or('?'));
        } else if is_printable(c) {
            out.push(c);
        } else {
            push_translated_hex(&mut out, code);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;

    /// Host that provides nothing.
    struct H;
    impl Host for H {}

    /// Host with canned option values.
    #[derive(Default)]
    struct OptHost {
        opts: Vec<(&'static str, Value)>,
    }

    impl OptHost {
        fn new(opts: &[(&'static str, Value)]) -> Self {
            Self {
                opts: opts.to_vec(),
            }
        }
    }

    impl Host for OptHost {
        fn option_get(&mut self, name: &str) -> Option<Value> {
            self.opts
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn run(name: &str, args: &[Value], host: &mut dyn Host) -> (Env, ValueResult) {
        let mut env = Env::new();
        let result = call(name, args, &mut env, host).expect("function is handled");
        (env, result)
    }

    fn run_default(name: &str, args: &[Value]) -> (Env, ValueResult) {
        let mut host = H;
        run(name, args, &mut host)
    }

    fn ok(name: &str, args: &[Value]) -> Value {
        run_default(name, args).1.expect("no error")
    }

    fn ok_host(name: &str, args: &[Value], host: &mut dyn Host) -> Value {
        run(name, args, host).1.expect("no error")
    }

    #[test]
    fn all_listed_funcs_are_dispatched() {
        for name in FUNCS {
            assert!(
                call(name, &[], &mut Env::new(), &mut H).is_some(),
                "{name} must be dispatched"
            );
        }
        assert!(call("not_a_builtin", &[], &mut Env::new(), &mut H).is_none());
    }

    #[test]
    fn sha256_matches_reference_vectors() {
        assert_eq!(
            ok("sha256", &[Value::string("")]).to_string_value(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            ok("sha256", &[Value::string("abc")]).to_string_value(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            ok(
                "sha256",
                &[Value::string("The quick brown fox jumps over the lazy dog")]
            )
            .to_string_value(),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn sha256_coerces_numbers_to_strings() {
        assert_eq!(
            ok("sha256", &[Value::Number(123)]).to_string_value(),
            "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3"
        );
    }

    #[test]
    fn shellescape_quotes_for_powershell() {
        // PowerShell is detected by shell name and always uses single quotes,
        // which makes the result platform independent.
        let mut host = OptHost::new(&[("shell", Value::string("pwsh"))]);
        let mut quoted = |text: &str, special: bool| {
            ok_host(
                "shellescape",
                &[
                    Value::string(text),
                    if special {
                        Value::Number(1)
                    } else {
                        Value::Number(0)
                    },
                ],
                &mut host,
            )
            .to_string_value()
        };
        assert_eq!(quoted("abc", false), "'abc'");
        assert_eq!(quoted("a b", false), "'a b'");
        assert_eq!(quoted("a'b", false), "'a''b'");
        assert_eq!(quoted("%", false), "'%'");
        assert_eq!(quoted("%", true), "'\\%'");
        assert_eq!(quoted("a\nb", true), "'a\\\nb'");
    }

    #[test]
    fn shellescape_uses_double_quotes_on_windows_and_single_elsewhere() {
        let mut host = OptHost::new(&[("shell", Value::string("cmd.exe"))]);
        let got = ok_host("shellescape", &[Value::string("a\"b c")], &mut host).to_string_value();
        if cfg!(windows) {
            assert_eq!(got, "\"a\"\"b c\"");
        } else {
            assert_eq!(got, "'a\"b c'");
        }
    }

    #[test]
    fn shellescape_honours_shellslash_on_windows() {
        let mut host = OptHost::new(&[
            ("shell", Value::string("cmd.exe")),
            ("shellslash", Value::Number(1)),
        ]);
        let got = ok_host("shellescape", &[Value::string("a b")], &mut host).to_string_value();
        assert_eq!(got, "'a b'");
    }

    #[test]
    fn shiftwidth_falls_back_to_tabstop() {
        let mut host = OptHost::new(&[
            ("shiftwidth", Value::Number(0)),
            ("tabstop", Value::Number(4)),
        ]);
        assert_eq!(ok_host("shiftwidth", &[], &mut host).to_number(), 4);

        let mut host = OptHost::new(&[
            ("shiftwidth", Value::Number(6)),
            ("tabstop", Value::Number(4)),
        ]);
        assert_eq!(ok_host("shiftwidth", &[], &mut host).to_number(), 6);

        // No host options at all: Vim's defaults are sw=8 and ts=8.
        assert_eq!(ok("shiftwidth", &[]).to_number(), 8);
    }

    #[test]
    fn shiftwidth_with_negative_column_returns_zero() {
        assert_eq!(ok("shiftwidth", &[Value::Number(-1)]).to_number(), 0);
    }

    #[test]
    fn simplify_collapses_dot_and_dotdot_components() {
        assert_eq!(
            ok("simplify", &[Value::string("./zzdir/.././/file/")]).to_string_value(),
            "./file/"
        );
        assert_eq!(
            ok("simplify", &[Value::string("zzdir/./file")]).to_string_value(),
            "zzdir/file"
        );
        assert_eq!(
            ok("simplify", &[Value::string("zzdir//file")]).to_string_value(),
            "zzdir/file"
        );
        assert_eq!(
            ok("simplify", &[Value::string("zzdir/../../x")]).to_string_value(),
            "../x"
        );
        assert_eq!(
            ok("simplify", &[Value::string("/zz_no_such_dir/../c")]).to_string_value(),
            "/c"
        );
        assert_eq!(ok("simplify", &[Value::string(".")]).to_string_value(), ".");
        assert_eq!(
            ok("simplify", &[Value::string("a/")]).to_string_value(),
            "a/"
        );
        assert_eq!(ok("simplify", &[Value::string("/")]).to_string_value(), "/");
    }

    #[cfg(windows)]
    #[test]
    fn simplify_keeps_windows_drive_prefix() {
        assert_eq!(
            ok("simplify", &[Value::string("C:\\zzdir\\..\\file")]).to_string_value(),
            "C:\\file"
        );
    }

    #[test]
    fn sinh_and_tanh_compute_hyperbolic_functions() {
        match ok("sinh", &[Value::Number(0)]) {
            Value::Float(f) => assert_eq!(f, 0.0),
            other => panic!("expected Float, got {other:?}"),
        }
        match ok("sinh", &[Value::Float(0.5)]) {
            Value::Float(f) => assert!((f - 0.521_095_305_493_747_4).abs() < 1e-12),
            other => panic!("expected Float, got {other:?}"),
        }
        match ok("tanh", &[Value::Float(0.5)]) {
            Value::Float(f) => assert!((f - 0.462_117_157_260_009_74).abs() < 1e-12),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn slice_lists_is_end_exclusive() {
        let list = Value::list(vec![
            Value::Number(1),
            Value::Number(2),
            Value::Number(3),
            Value::Number(4),
        ]);
        assert_eq!(
            ok("slice", &[list.clone(), Value::Number(1), Value::Number(3)]),
            Value::list(vec![Value::Number(2), Value::Number(3)])
        );
        assert_eq!(
            ok("slice", &[list.clone(), Value::Number(-2)]),
            Value::list(vec![Value::Number(3), Value::Number(4)])
        );
        assert_eq!(
            ok(
                "slice",
                &[list.clone(), Value::Number(1), Value::Number(-1)]
            ),
            Value::list(vec![Value::Number(2), Value::Number(3)])
        );
        assert_eq!(
            ok("slice", &[list.clone(), Value::Number(9)]),
            Value::list(Vec::new())
        );
        assert_eq!(
            ok("slice", &[list, Value::Number(-9)]),
            Value::list(Vec::new())
        );
    }

    #[test]
    fn slice_strings_use_character_indexes() {
        assert_eq!(
            ok(
                "slice",
                &[Value::string("abcdef"), Value::Number(1), Value::Number(3)]
            )
            .to_string_value(),
            "bc"
        );
        assert_eq!(
            ok("slice", &[Value::string("あいう"), Value::Number(1)]).to_string_value(),
            "いう"
        );
        assert_eq!(
            ok(
                "slice",
                &[Value::string("abc"), Value::Number(1), Value::Number(100)]
            )
            .to_string_value(),
            "bc"
        );
        // A very negative start clamps to zero for strings (unlike lists).
        assert_eq!(
            ok("slice", &[Value::string("abc"), Value::Number(-9)]).to_string_value(),
            "abc"
        );
    }

    #[test]
    fn slice_blobs_and_numbers() {
        assert_eq!(
            ok(
                "slice",
                &[
                    Value::blob(vec![1, 2, 3]),
                    Value::Number(1),
                    Value::Number(2)
                ]
            ),
            Value::blob(vec![2])
        );
        assert_eq!(
            ok(
                "slice",
                &[Value::Number(12345), Value::Number(1), Value::Number(3)]
            )
            .to_string_value(),
            "23"
        );
    }

    #[test]
    fn slice_rejects_dictionaries_and_specials() {
        let dict = Value::dict(crate::value::VimDict::new());
        let e = run_default("slice", &[dict, Value::Number(0)])
            .1
            .unwrap_err();
        assert_eq!(e.code, "E719");
        let e = run_default("slice", &[Value::Number(0), Value::list(Vec::new())])
            .1
            .unwrap_err();
        assert_eq!(e.code, "E745");
    }

    #[test]
    fn srand_matches_vim_reference_values() {
        let seed = ok("srand", &[Value::Number(123_456_789)]);
        assert_eq!(
            seed,
            Value::list(vec![
                Value::Number(1_573_771_921),
                Value::Number(319_883_699),
                Value::Number(2_742_014_374),
                Value::Number(1_324_369_493),
            ])
        );
        // Same seed, same sequence; different seed, different sequence.
        assert_eq!(
            ok("srand", &[Value::Number(42)]),
            ok("srand", &[Value::Number(42)])
        );
        assert_ne!(
            ok("srand", &[Value::Number(42)]),
            ok("srand", &[Value::Number(43)])
        );
    }

    #[test]
    fn srand_without_argument_returns_four_numbers() {
        let first = ok("srand", &[]);
        let Value::List(list) = &first else {
            panic!("srand() must return a List");
        };
        assert_eq!(list.borrow().len(), 4);
        for value in list.borrow().iter() {
            let n = value.as_number().unwrap();
            assert!((0..=u32::MAX as i64).contains(&n), "seed {n} out of range");
        }
        assert_ne!(first, ok("srand", &[]));
    }

    #[test]
    fn srand_rejects_non_numbers() {
        let e = run_default("srand", &[Value::list(vec![Value::Number(1)])])
            .1
            .unwrap_err();
        assert_eq!(e.code, "E745");
    }

    #[test]
    fn str2list_returns_character_codes() {
        assert_eq!(
            ok("str2list", &[Value::string("ABC")]),
            Value::list(vec![
                Value::Number(65),
                Value::Number(66),
                Value::Number(67)
            ])
        );
        assert_eq!(
            ok("str2list", &[Value::string("あ")]),
            Value::list(vec![Value::Number(12354)])
        );
        assert_eq!(
            ok("str2list", &[Value::string("")]),
            Value::list(Vec::new())
        );
        // {utf8} is accepted and does not change anything (strings are UTF-8).
        assert_eq!(
            ok("str2list", &[Value::string("a"), Value::Number(1)]),
            Value::list(vec![Value::Number(97)])
        );
    }

    #[test]
    fn strcharlen_ignores_composing_characters() {
        assert_eq!(ok("strcharlen", &[Value::string("abc")]).to_number(), 3);
        assert_eq!(ok("strcharlen", &[Value::string("あいう")]).to_number(), 3);
        assert_eq!(ok("strcharlen", &[Value::string("")]).to_number(), 0);
        // "a" followed by COMBINING ACUTE ACCENT is one character.
        assert_eq!(
            ok("strcharlen", &[Value::string("a\u{301}")]).to_number(),
            1
        );
    }

    #[test]
    fn strdisplaywidth_expands_tabs_and_wide_characters() {
        assert_eq!(
            ok("strdisplaywidth", &[Value::string("abc")]).to_number(),
            3
        );
        assert_eq!(
            ok("strdisplaywidth", &[Value::string("\tabc")]).to_number(),
            11
        );
        assert_eq!(
            ok(
                "strdisplaywidth",
                &[Value::string("\tabc"), Value::Number(4)]
            )
            .to_number(),
            7
        );
        assert_eq!(
            ok("strdisplaywidth", &[Value::string("あa")]).to_number(),
            3
        );
        assert_eq!(
            ok("strdisplaywidth", &[Value::string("a\u{301}")]).to_number(),
            1
        );
        assert_eq!(ok("strdisplaywidth", &[Value::string("")]).to_number(), 0);
    }

    #[test]
    fn strdisplaywidth_uses_tabstop_option() {
        let mut host = OptHost::new(&[("tabstop", Value::Number(4))]);
        assert_eq!(
            ok_host("strdisplaywidth", &[Value::string("\ta")], &mut host).to_number(),
            5
        );
    }

    #[test]
    fn strgetchar_returns_character_codes() {
        assert_eq!(
            ok("strgetchar", &[Value::string("abc"), Value::Number(1)]).to_number(),
            98
        );
        assert_eq!(
            ok("strgetchar", &[Value::string("abc"), Value::Number(5)]).to_number(),
            -1
        );
        assert_eq!(
            ok("strgetchar", &[Value::string("abc"), Value::Number(-1)]).to_number(),
            -1
        );
        // The combining accent belongs to "a", so index 1 is "b".
        assert_eq!(
            ok(
                "strgetchar",
                &[Value::string("a\u{301}b"), Value::Number(1)]
            )
            .to_number(),
            98
        );
    }

    #[test]
    fn strtrans_escapes_unprintable_characters() {
        assert_eq!(
            ok("strtrans", &[Value::string("abc")]).to_string_value(),
            "abc"
        );
        assert_eq!(
            ok("strtrans", &[Value::string("\n")]).to_string_value(),
            "^@"
        );
        assert_eq!(
            ok("strtrans", &[Value::string("\u{1}")]).to_string_value(),
            "^A"
        );
        assert_eq!(
            ok("strtrans", &[Value::string("a\tb")]).to_string_value(),
            "a^Ib"
        );
        assert_eq!(
            ok("strtrans", &[Value::string("\u{7f}")]).to_string_value(),
            "^?"
        );
        // Printable non-ASCII stays as it is; the non-breaking space is not
        // printable for Vim.
        assert_eq!(ok("strtrans", &[Value::string("é")]).to_string_value(), "é");
        assert_eq!(
            ok("strtrans", &[Value::string("\u{a0}")]).to_string_value(),
            "<a0>"
        );
    }

    #[test]
    fn tempname_returns_unique_nonexistent_names() {
        let first = ok("tempname", &[]).to_string_value();
        let second = ok("tempname", &[]).to_string_value();
        assert!(!first.is_empty());
        assert_ne!(first, second);
        assert!(!std::path::Path::new(&first).exists());
        assert!(!std::path::Path::new(&second).exists());
    }

    #[test]
    fn setfperm_rejects_bad_mode_strings() {
        let e = run_default(
            "setfperm",
            &[Value::string("whatever"), Value::string("rw")],
        )
        .1
        .unwrap_err();
        assert_eq!(e.code, "E474");
    }

    #[test]
    fn setfperm_returns_zero_for_missing_files() {
        let missing = tempname();
        let result = ok(
            "setfperm",
            &[Value::string(&missing), Value::string("rwxrwxrwx")],
        );
        assert_eq!(result.to_number(), 0);
    }

    #[test]
    fn setfperm_applies_permissions() {
        let file = tempname();
        std::fs::write(&file, b"x").expect("create temp file");
        let mode = |text: &str| ok("setfperm", &[Value::string(&file), Value::string(text)]);
        assert_eq!(mode("rw-------").to_number(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&file).expect("metadata").permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);
        }
        #[cfg(windows)]
        {
            assert!(
                !std::fs::metadata(&file)
                    .expect("metadata")
                    .permissions()
                    .readonly()
            );
            assert_eq!(mode("r--------").to_number(), 1);
            assert!(
                std::fs::metadata(&file)
                    .expect("metadata")
                    .permissions()
                    .readonly()
            );
        }
        let _ = std::fs::remove_file(&file);
    }

    /// A per-platform command whose exit code can be checked.
    fn exit_command(code: i64) -> String {
        if cfg!(windows) {
            format!("exit /b {code}")
        } else {
            format!("exit {code}")
        }
    }

    #[test]
    fn system_returns_command_output() {
        let (env, result) = run_default("system", &[Value::string("echo hi")]);
        let text = result.expect("system() succeeds").to_string_value();
        let text = text.trim_end_matches(['\r', '\n']);
        assert_eq!(text, "hi");
        assert_eq!(env.get_var("v:shell_error").unwrap().to_number(), 0);
    }

    #[test]
    fn system_sets_shell_error_for_failing_commands() {
        let cmd = exit_command(3);
        let (env, result) = run_default("system", &[Value::string(cmd)]);
        result.expect("system() returns the output even on failure");
        assert_eq!(env.get_var("v:shell_error").unwrap().to_number(), 3);
    }

    #[test]
    fn system_pipes_input() {
        // Windows `sort` needs CRLF input; Unix `sort` sorts lines as well.
        let input = if cfg!(windows) {
            "b\r\na\r\n"
        } else {
            "b\na\n"
        };
        let result = ok("system", &[Value::string("sort"), Value::string(input)]);
        let text = result.to_string_value();
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        assert_eq!(lines, ["a", "b"]);
    }

    #[test]
    fn system_list_splits_output_lines() {
        let result = ok("systemlist", &[Value::string("echo hi")]);
        let Value::List(items) = result else {
            panic!("systemlist() must return a List");
        };
        let items = items.borrow();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].to_string_value().trim(), "hi");
    }

    #[test]
    fn system_list_of_empty_output_is_empty() {
        // "rem" (cmd) / "true" (sh) produce no output on any platform.
        let cmd = if cfg!(windows) { "rem" } else { "true" };
        assert_eq!(
            ok("systemlist", &[Value::string(cmd)]),
            Value::list(Vec::new())
        );
    }
}
