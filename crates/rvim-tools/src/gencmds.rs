//! Generate the Rust Ex command table from the vendored reference source.
//!
//! Source: `reference/vim/src/ex_cmds.h` (v8.2.5172), whose `cmdnames[]` array
//! holds 589 `EXCMD(enum, "name", func, flags, addr)` entries **in a curated
//! order**: it is sorted on the shortest working abbreviation, so that a prefix
//! match always resolves to the intended command ("r" is :read, not :rewind;
//! "w" is :write, not :wq). That order is part of Vim's observable behaviour and
//! is therefore preserved verbatim here.

use std::fmt::Write as _;
use std::path::Path;

/// EX_* flag values copied from ex_cmds.h lines 30-65.
const EX_FLAGS: &[(&str, u32)] = &[
    ("EX_RANGE", 0x001),
    ("EX_BANG", 0x002),
    ("EX_EXTRA", 0x004),
    ("EX_XFILE", 0x008),
    ("EX_NOSPC", 0x010),
    ("EX_DFLALL", 0x020),
    ("EX_WHOLEFOLD", 0x040),
    ("EX_NEEDARG", 0x080),
    ("EX_TRLBAR", 0x100),
    ("EX_REGSTR", 0x200),
    ("EX_COUNT", 0x400),
    ("EX_NOTRLCOM", 0x800),
    ("EX_ZEROR", 0x1000),
    ("EX_CTRLV", 0x2000),
    ("EX_CMDARG", 0x4000),
    ("EX_BUFNAME", 0x8000),
    ("EX_BUFUNL", 0x10000),
    ("EX_ARGOPT", 0x20000),
    ("EX_SBOXOK", 0x40000),
    ("EX_CMDWIN", 0x80000),
    ("EX_MODIFY", 0x100000),
    ("EX_FLAGS", 0x200000),
    ("EX_RESTRICT", 0x400000),
    ("EX_EXPAND", 0x800000),
    ("EX_LOCK_OK", 0x1000000),
    ("EX_NONWHITE_OK", 0x2000000),
    ("EX_KEEPSCRIPT", 0x4000000),
    ("EX_EXPR_ARG", 0x8000000),
    ("EX_WHOLE", 0x10000000),
];

/// Composite macros defined by ex_cmds.h.
fn composite(name: &str) -> Option<u32> {
    let bits = |names: &[&str]| -> u32 { names.iter().map(|n| flag_value(n).unwrap()).sum() };
    match name {
        "EX_FILES" => Some(bits(&["EX_XFILE", "EX_EXTRA"])),
        "EX_FILE1" => Some(bits(&["EX_XFILE", "EX_EXTRA", "EX_NOSPC"])),
        "EX_WORD1" => Some(bits(&["EX_EXTRA", "EX_NOSPC"])),
        _ => None,
    }
}

fn flag_value(name: &str) -> Option<u32> {
    EX_FLAGS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
        .or_else(|| composite(name))
}

/// Address types from ex_cmds.h lines 71-84.
fn addr_variant(name: &str) -> Option<&'static str> {
    Some(match name {
        "ADDR_LINES" => "Lines",
        "ADDR_WINDOWS" => "Windows",
        "ADDR_ARGUMENTS" => "Arguments",
        "ADDR_LOADED_BUFFERS" => "LoadedBuffers",
        "ADDR_BUFFERS" => "Buffers",
        "ADDR_TABS" => "Tabs",
        "ADDR_TABS_RELATIVE" => "TabsRelative",
        "ADDR_QUICKFIX_VALID" => "QuickfixValid",
        "ADDR_QUICKFIX" => "Quickfix",
        "ADDR_UNSIGNED" => "Unsigned",
        "ADDR_OTHER" => "Other",
        "ADDR_NONE" => "None",
        _ => return None,
    })
}

#[derive(Debug)]
struct Entry {
    enum_name: String,
    name: String,
    func: String,
    flags: u32,
    addr: String,
}

/// Strip C comments, keeping string literals intact.
fn strip_comments(src: &str) -> String {
    let bytes: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(next) = bytes.get(i + 1) {
                    out.push(*next);
                    i += 1;
                }
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
        } else if c == '/' && bytes.get(i + 1) == Some(&'/') {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && bytes.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Split `text` on top-level commas (outside parentheses/strings).
fn split_args(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_str => {
                in_str = true;
                cur.push(c);
            }
            '"' if in_str => {
                in_str = false;
                cur.push(c);
            }
            '\\' if in_str => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '(' if !in_str => {
                depth += 1;
                cur.push(c);
            }
            ')' if !in_str => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            ',' if !in_str && depth == 0 => {
                parts.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

/// Parse all EXCMD(...) entries in source order.
fn parse_entries(src: &str) -> Result<Vec<Entry>, String> {
    let clean = strip_comments(src);
    let mut entries = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = clean[search_from..].find("\nEXCMD(") {
        let start = search_from + rel + 1 + "EXCMD(".len();
        // Find the matching close paren; `end` is its byte offset (excluded).
        let mut depth = 1usize;
        let mut in_str = false;
        let mut end = None;
        let mut escaped = false;
        for (off, c) in clean[start..].char_indices() {
            if in_str {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + off);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.ok_or_else(|| format!("unbalanced EXCMD at offset {start}"))?;
        let text = clean[start..end].to_string();
        search_from = end + 1;
        let parts = split_args(&text);
        if parts.len() != 5 {
            return Err(format!("expected 5 args, got {}: {text}", parts.len()));
        }
        let name = parts[1].trim().trim_matches('"').to_string();
        if name.is_empty() {
            return Err(format!("empty command name in: {text}"));
        }
        let mut flags = 0u32;
        for bit in parts[3].split('|').map(str::trim).filter(|s| !s.is_empty()) {
            let v = flag_value(bit).ok_or_else(|| format!("unknown flag {bit} in: {text}"))?;
            flags |= v;
        }
        let addr = addr_variant(parts[4].trim())
            .ok_or_else(|| format!("unknown addr type {}", parts[4]))?
            .to_string();
        entries.push(Entry {
            enum_name: parts[0].trim().to_string(),
            name,
            func: parts[2].trim().to_string(),
            flags,
            addr,
        });
    }
    Ok(entries)
}

fn render(entries: &[Entry]) -> String {
    let mut out = String::new();
    out.push_str(
        "//! GENERATED FILE - do not edit by hand.\n\
         //!\n\
         //! The Ex command table mirrored from the baseline `reference/vim/src/ex_cmds.h`\n\
         //! (v8.2.5172): 589 EXCMD entries, in the reference's curated order. Vim sorts\n\
         //! this table on the *shortest working abbreviation*, so a prefix match resolves\n\
         //! to the intended command (\":r\" is :read not :rewind; \":w\" is :write not :wq).\n\
         //!\n\
         //! Regenerate with: `cargo run -p rvim-tools -- gen-cmds`\n\n\
         use crate::table::AddrKind;\n\n\
         /// One Ex command declaration, aligned with Vim's `struct cmdname`.\n\
         #[derive(Clone, Copy, Debug)]\n\
         pub struct Cmd {\n\
         \x20   /// Command name as typed (canonical/longest form).\n\
         \x20   pub name: &'static str,\n\
         \x20   /// C handler in the reference implementation (documentation only).\n\
         \x20   pub func: &'static str,\n\
         \x20   /// EX_* flag bits from ex_cmds.h.\n\
         \x20   pub flags: u32,\n\
         \x20   /// Address type the command accepts.\n\
         \x20   pub addr: AddrKind,\n\
         }\n\n",
    );
    let _ = writeln!(
        out,
        "/// Number of commands in the baseline table (fixed denominator for coverage).\npub const COMMAND_COUNT: usize = {};\n",
        entries.len()
    );
    let _ = writeln!(out, "pub static COMMANDS: [Cmd; COMMAND_COUNT] = [");
    for e in entries {
        let _ = writeln!(
            out,
            "    Cmd {{ name: {:?}, func: {:?}, flags: 0x{:08X}, addr: AddrKind::{} }},",
            e.name, e.func, e.flags, e.addr
        );
    }
    out.push_str("];\n");
    let _ = writeln!(out, "\n// {} entries parsed from ex_cmds.h", entries.len());
    let _ = entries;
    out
}

pub fn run(root: &Path) -> Result<(), String> {
    let src_path = root.join("reference/vim/src/ex_cmds.h");
    let src = std::fs::read_to_string(&src_path)
        .map_err(|e| format!("cannot read {}: {e}", src_path.display()))?;
    let entries = parse_entries(&src)?;
    if entries.len() != 589 {
        return Err(format!(
            "expected 589 commands in the baseline, parsed {}",
            entries.len()
        ));
    }
    // Invariants of the curated order that Vim's abbreviation resolution relies
    // on; these are also asserted by unit tests in rvim-ex.
    let pos = |name: &str| entries.iter().position(|e| e.name == name);
    let invalid_order = [
        ("write", "wq"),
        ("print", "previous"),
        ("quit", "qall"),
        ("read", "rewind"),
        ("delete", "delmarks"),
    ];
    for (a, b) in invalid_order {
        match (pos(a), pos(b)) {
            (Some(ia), Some(ib)) if ia < ib => {}
            (Some(_), Some(_)) => {
                return Err(format!("table order violated: {a} must precede {b}"));
            }
            _ => return Err(format!("missing command in table: {a} or {b}")),
        }
    }
    let out_path = root.join("crates/rvim-ex/src/cmd_table.rs");
    std::fs::write(&out_path, render(&entries))
        .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
    println!(
        "wrote {} ({} commands, {} unique enum names)",
        out_path.display(),
        entries.len(),
        entries
            .iter()
            .map(|e| e.enum_name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        strip_comments(
            r#"
/* block comment */
EXCMD(CMD_append, "append", ex_append,
    EX_BANG|EX_RANGE|EX_TRLBAR,
    ADDR_LINES),
EXCMD(CMD_write, "write", ex_write,
    EX_FILES|EX_TRLBAR|EX_DFLALL,
    ADDR_OTHER),
EXCMD(CMD_bang, "!", ex_bang, EX_BANG, ADDR_LINES), // trailing
"#,
        )
    }

    #[test]
    fn parses_entries_in_order() {
        let entries = parse_entries(&sample()).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "append");
        assert_eq!(entries[0].func, "ex_append");
        assert_eq!(entries[0].addr, "Lines");
        assert_eq!(entries[0].flags, 0x002 | 0x001 | 0x100);
        // EX_FILES expands to EX_XFILE|EX_EXTRA.
        assert_eq!(entries[1].flags, 0x008 | 0x004 | 0x100 | 0x020);
        assert_eq!(entries[2].name, "!");
    }

    #[test]
    fn flag_table_matches_reference_values() {
        assert_eq!(flag_value("EX_RANGE"), Some(0x001));
        assert_eq!(flag_value("EX_TRLBAR"), Some(0x100));
        assert_eq!(flag_value("EX_WHOLE"), Some(0x10000000));
        assert_eq!(flag_value("EX_FILE1"), Some(0x008 | 0x004 | 0x010));
        assert_eq!(flag_value("EX_NOPE"), None);
    }
}
