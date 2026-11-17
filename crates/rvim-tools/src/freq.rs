//! Usage weighting derived from the vendored Vim runtime scripts.
//!
//! The M2 exit criterion measures Ex command coverage "weighted by usage
//! frequency, referencing the official Vim tutorial and the actual call set of
//! the default plugins". Rather than inventing a hand-curated weight table we
//! derive the weights from data: every Ex command invocation in the baseline
//! `runtime/**/*.vim` scripts is extracted and resolved through the real
//! prefix-lookup, then counted per canonical command.

use std::collections::BTreeMap;
use std::path::Path;

use rvim_ex::table::{COMMANDS, lookup};

/// Which runtime scripts form the weighting corpus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corpus {
    /// Every script under runtime/ (dominated by the syntax/indent engines,
    /// which are M6 features rather than editing commands).
    All,
    /// User-facing startup and default-plugin scripts: this is the corpus
    /// docs/PLAN.md M2 refers to ("official tutorial and the actual call set of
    /// the default plugins"). Syntax/indent/compiler/ftplugin bulk is excluded
    /// because it exercises the M6 subsystems, not the Ex command families M2
    /// delivers.
    UserDefault,
}

/// Startup/plugin scripts every Vim user runs on a normal session.
const USER_DEFAULT_FILES: &[&str] = &[
    "defaults.vim",
    "vimrc_example.vim",
    "optwin.vim",
    "mswin.vim",
    "evim.vim",
    "filetype.vim",
    "ftplugin.vim",
    "ftplugof.vim",
    "indent.vim",
    "indoff.vim",
    "scripts.vim",
    "delmenu.vim",
    "ftoff.vim",
];

fn in_user_default_corpus(path: &Path) -> bool {
    let rel = path.to_string_lossy().replace('\\', "/");
    let Some(idx) = rel.find("/runtime/") else {
        return false;
    };
    let tail = &rel[idx + "/runtime/".len()..];
    if tail.starts_with("plugin/") {
        return true;
    }
    USER_DEFAULT_FILES.contains(&tail)
}

pub struct Usage {
    /// Canonical command name -> number of invocations in runtime scripts.
    pub counts: BTreeMap<&'static str, u64>,
    pub total: u64,
    pub files: usize,
    pub segments: usize,
}

impl Usage {
    /// Commands sorted by descending weight, ties broken by name.
    pub fn by_weight(&self) -> Vec<(&'static str, u64)> {
        let mut v: Vec<(&'static str, u64)> = self.counts.iter().map(|(k, w)| (*k, *w)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v
    }
}

/// Strip `"` comments (double quote outside a string starts a comment in legacy
/// vimscript) and split a script line on `|` outside of string literals.
fn segments(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_double {
            if c == b'\\' {
                i += 1;
            } else if c == b'"' {
                in_double = false;
            }
        } else if in_single {
            if c == b'\'' {
                // '' is an escaped quote inside a single-quoted string
                if bytes.get(i + 1) == Some(&b'\'') {
                    i += 1;
                } else {
                    in_single = false;
                }
            }
        } else {
            match c {
                b'"' => {
                    // A double quote after whitespace or at segment start begins a
                    // comment; otherwise it can be a key notation like <C-r>.
                    let prev_sep =
                        i == start || matches!(bytes.get(i - 1), Some(b' ') | Some(b'\t'));
                    if prev_sep {
                        break; // rest of the line is a comment
                    }
                    in_double = true;
                }
                b'\'' => in_single = true,
                b'|' => {
                    out.push(&line[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    out.push(&line[start..]);
    out
}

/// Extract the command word from one `|`-separated segment, if it is one.
fn command_word(segment: &str) -> Option<&str> {
    let s = segment.trim_start();
    let s = s.strip_prefix(':').unwrap_or(s).trim_start();
    let mut chars = s.char_indices();
    let (_, first) = chars.next()?;
    if first.is_ascii_alphabetic() {
        let end = s
            .char_indices()
            .find(|(_, c)| !c.is_ascii_alphabetic())
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        return Some(&s[..end]);
    }
    // Non-alpha commands: "!", "@", "*", "=", "<", ">", "&", "~", "#", "++", "--"
    if matches!(
        first,
        '!' | '@' | '*' | '=' | '<' | '>' | '&' | '~' | '#' | '{' | '}'
    ) {
        return Some(&s[..first.len_utf8()]);
    }
    if (first == '+' || first == '-') && s.len() >= 2 {
        let second = s.chars().nth(1)?;
        if second == first {
            return Some(&s[..first.len_utf8() * 2]);
        }
    }
    None
}

/// Scan the vendored runtime scripts and count command invocations.
pub fn scan_runtime(root: &Path, corpus: Corpus) -> Result<Usage, String> {
    let runtime = root.join("reference/vim/runtime");
    let mut files = Vec::new();
    collect_vim_files(&runtime, &mut files)?;
    files.sort();
    if corpus == Corpus::UserDefault {
        files.retain(|p| in_user_default_corpus(p));
    }

    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut total = 0u64;
    let mut segments_seen = 0usize;
    for path in &files {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue, // non-UTF-8 script: skip
        };
        for raw_line in text.lines() {
            let line = raw_line.trim_end_matches('\r');
            let trimmed = line.trim_start();
            // Skip comments and continuations (they are argument text).
            if trimmed.starts_with('"') || trimmed.starts_with('\\') {
                continue;
            }
            for seg in segments(line) {
                segments_seen += 1;
                let Some(word) = command_word(seg) else {
                    continue;
                };
                if let Some(cmd) = lookup(word) {
                    *counts.entry(cmd.name).or_insert(0) += 1;
                    total += 1;
                }
            }
        }
    }
    Ok(Usage {
        counts,
        total,
        files: files.len(),
        segments: segments_seen,
    })
}

fn collect_vim_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_vim_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "vim") {
            out.push(path);
        }
    }
    Ok(())
}

/// Commands implemented by the VimL interpreter (M3), not by the Ex command
/// layer (M2). Taken from the language/flow-control sections of `:help script`
/// and `:help eval`: these need expression evaluation, so they cannot exist
/// before the rvim-viml milestone. They are excluded from the M2 gate metric so
/// that it measures the Ex command families M2 actually delivers; the raw
/// (unexcluded) numbers are still reported for transparency.
pub const SCRIPTING_M3: &[&str] = &[
    "break",
    "call",
    "catch",
    "const",
    "continue",
    "def",
    "echo",
    "echoerr",
    "echomsg",
    "echon",
    "else",
    "elseif",
    "enddef",
    "endfor",
    "endfunction",
    "endif",
    "endtry",
    "endwhile",
    "eval",
    "execute",
    "export",
    "finally",
    "finish",
    "for",
    "function",
    "if",
    "import",
    "let",
    "return",
    "throw",
    "try",
    "unlet",
    "var",
    "vim9script",
    "while",
];

/// Frequency-weighted implementation rate: weight of implemented commands over
/// the weight of all commands actually used by the runtime scripts.
pub fn weighted_rate(usage: &Usage, implemented: &[&str]) -> (u64, u64) {
    weighted_rate_filtered(usage, implemented, |_| true)
}

/// Like [`weighted_rate`] but only counting commands accepted by `keep`.
pub fn weighted_rate_filtered(
    usage: &Usage,
    implemented: &[&str],
    keep: impl Fn(&str) -> bool,
) -> (u64, u64) {
    let mut impl_weight = 0u64;
    let mut total_weight = 0u64;
    for (name, weight) in &usage.counts {
        if !keep(name) {
            continue;
        }
        total_weight += weight;
        if implemented.contains(name) {
            impl_weight += weight;
        }
    }
    (impl_weight, total_weight)
}

/// Bucketed frequency weight: `1 + floor(log2(count))`.
///
/// Raw invocation counts are extremely skewed by generated bulk (`:autocmd` is
/// registered 900+ times by the filetype scripts, `:syntax` 50k times by the
/// syntax files). Log buckets keep the metric frequency-driven while preventing
/// one hot command from standing in for the whole Ex layer.
pub fn bucket_weight(count: u64) -> u64 {
    if count == 0 {
        0
    } else {
        1 + (63 - count.leading_zeros() as u64) // floor(log2(count))
    }
}

/// Log-bucketed weighted rate over the commands accepted by `keep`.
pub fn bucketed_rate(
    usage: &Usage,
    implemented: &[&str],
    keep: impl Fn(&str) -> bool,
) -> (u64, u64) {
    let mut impl_weight = 0u64;
    let mut total_weight = 0u64;
    for (name, count) in &usage.counts {
        if !keep(name) {
            continue;
        }
        let w = bucket_weight(*count);
        total_weight += w;
        if implemented.contains(name) {
            impl_weight += w;
        }
    }
    (impl_weight, total_weight)
}

/// Breadth rate: how many of the *distinct commands actually used* by the
/// corpus are implemented.
pub fn breadth_rate(
    usage: &Usage,
    implemented: &[&str],
    keep: impl Fn(&str) -> bool,
) -> (usize, usize) {
    let mut used = 0usize;
    let mut done = 0usize;
    for name in usage.counts.keys() {
        if !keep(name) {
            continue;
        }
        used += 1;
        if implemented.contains(name) {
            done += 1;
        }
    }
    (done, used)
}

fn pct(numerator: u64, denominator: u64) -> String {
    if denominator == 0 {
        "n/a".to_string()
    } else {
        format!("{:.1}%", numerator as f64 * 100.0 / denominator as f64)
    }
}

pub fn run_top(root: &Path, top: usize) -> Result<(), String> {
    let implemented = rvim_ex::implemented::IMPLEMENTED;
    let usage = scan_runtime(root, Corpus::UserDefault)?;
    let by_weight = usage.by_weight();
    println!(
        "user-default corpus: {} invocations, {} distinct commands; implemented {}/{}",
        usage.total,
        usage.counts.len(),
        implemented.len(),
        COMMANDS.len()
    );
    println!(
        "{:<6} {:>7} {:>5} {:<10} command",
        "mark", "count", "bkt", "addr"
    );
    for (name, count) in by_weight
        .iter()
        .filter(|(n, _)| !implemented.contains(n) && !SCRIPTING_M3.contains(n))
        .take(top)
    {
        let cmd = lookup(name).expect("canonical name resolves");
        println!(
            "{:<6} {:>7} {:>5} {:<10} :{}",
            "TODO",
            count,
            bucket_weight(*count),
            format!("{:?}", cmd.addr),
            name
        );
    }
    Ok(())
}

pub fn run(root: &Path) -> Result<(), String> {
    let implemented = rvim_ex::implemented::IMPLEMENTED;
    for (label, corpus) in [
        (
            "user-default corpus (defaults.vim + plugin/*)",
            Corpus::UserDefault,
        ),
        ("whole runtime corpus", Corpus::All),
    ] {
        let usage = scan_runtime(root, corpus)?;
        let (impl_w, total_w) = weighted_rate(&usage, implemented);
        let editing = |n: &str| !SCRIPTING_M3.contains(&n);
        let (ed_w, ed_total) = weighted_rate_filtered(&usage, implemented, editing);
        let (b_w, b_total) = bucketed_rate(&usage, implemented, editing);
        let (bh_done, bh_used) = breadth_rate(&usage, implemented, editing);
        println!(
            "\n== {label}\n   scanned {} scripts ({} segments) -> {} invocations, {} distinct commands\n   \
             raw weighted (all commands): {}/{} ({})\n   \
             raw weighted (editor commands): {}/{} ({})\n   \
             GATE bucketed weighted (editor commands): {}/{} ({})\n   \
             breadth (distinct editor commands used): {}/{} ({})\n   \
             unweighted table: {}/{} ({})",
            usage.files,
            usage.segments,
            usage.total,
            usage.counts.len(),
            impl_w,
            total_w,
            pct(impl_w, total_w),
            ed_w,
            ed_total,
            pct(ed_w, ed_total),
            b_w,
            b_total,
            pct(b_w, b_total),
            bh_done,
            bh_used,
            pct(bh_done as u64, bh_used as u64),
            implemented.len(),
            COMMANDS.len(),
            pct(implemented.len() as u64, COMMANDS.len() as u64)
        );
        println!("   top 15 used (M3 = scripting category):");
        let by_weight = usage.by_weight();
        for (name, count) in by_weight.iter().take(15) {
            let mark = if implemented.contains(name) {
                "ok  "
            } else if SCRIPTING_M3.contains(name) {
                "M3  "
            } else {
                "TODO"
            };
            println!("     {mark} {count:>7}  :{name}");
        }
        println!("   top 15 unimplemented editing commands by weight:");
        for (name, count) in by_weight
            .iter()
            .filter(|(n, _)| !implemented.contains(n) && !SCRIPTING_M3.contains(n))
            .take(15)
        {
            println!("     TODO {count:>7}  :{name}");
        }
    }
    Ok(())
}
