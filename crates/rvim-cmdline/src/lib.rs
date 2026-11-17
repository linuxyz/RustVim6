//! rvim-cmdline: Ex command-line state, history, completion and wildmenu.
//!
//! Headless model of Vim's command line (`ex_getln.c`, `cmdexpand.c`,
//! `cmdhist.c`). The M4 TUI renders and edits the line; everything here is
//! deterministic and unit-testable:
//!
//! * [`CmdHistory`] — `:` and `/` history with prefix recall (`:h` up/down).
//! * [`complete_command`] — Ex command-name completion from the 589-entry table
//!   plus user-defined commands.
//! * [`complete_filename`] — file-name completion using the same wildcard
//!   matcher as `:runtime` (`*`, `?`, `[...]`).
//! * [`WildMenu`] — the cycling completion list behind 'wildmenu'.

use std::path::{Path, PathBuf};

use rvim_ex::table::{COMMANDS, lookup};
use rvim_ex::wildcard;

/// Kind of command line being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdlineKind {
    Ex,
    SearchForward,
    SearchBackward,
    Expression,
}

impl CmdlineKind {
    /// History list identifier used by Vim (`histtype`).
    pub fn history_id(self) -> char {
        match self {
            CmdlineKind::Ex => ':',
            CmdlineKind::SearchForward | CmdlineKind::SearchBackward => '/',
            CmdlineKind::Expression => '=',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub kind: char,
    pub text: String,
}

/// Command-line history, honouring 'history' limits and duplicate suppression.
#[derive(Debug)]
pub struct CmdHistory {
    entries: Vec<HistoryEntry>,
    max: usize,
    /// Cursor while recalling: index into `entries` from the end.
    recall: Option<usize>,
}

impl Default for CmdHistory {
    fn default() -> Self {
        Self::new(50)
    }
}

impl CmdHistory {
    pub fn new(max: usize) -> Self {
        Self {
            entries: Vec::new(),
            max,
            recall: None,
        }
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add an entry (Vim does not store an entry equal to the previous one).
    pub fn add(&mut self, kind: CmdlineKind, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self
            .entries
            .last()
            .is_some_and(|e| e.kind == kind.history_id() && e.text == text)
        {
            return;
        }
        self.entries.push(HistoryEntry {
            kind: kind.history_id(),
            text: text.to_string(),
        });
        // 'history' limits each history list independently in Vim.
        let id = kind.history_id();
        while self.entries.iter().filter(|e| e.kind == id).count() > self.max {
            if let Some(pos) = self.entries.iter().position(|e| e.kind == id) {
                self.entries.remove(pos);
            } else {
                break;
            }
        }
        self.recall = None;
    }

    /// Entries of one kind, newest last.
    pub fn for_kind(&self, kind: CmdlineKind) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.kind == kind.history_id())
            .map(|e| e.text.as_str())
            .collect()
    }

    /// Recall upwards (older) starting with `prefix`. Pass the current line as
    /// `current` so the first step moves before it.
    pub fn older(&mut self, kind: CmdlineKind, prefix: &str) -> Option<String> {
        let id = kind.history_id();
        let mut idx = match self.recall {
            Some(i) => i,
            None => self.entries.len(),
        };
        while idx > 0 {
            idx -= 1;
            let e = &self.entries[idx];
            if e.kind == id && e.text.starts_with(prefix) {
                self.recall = Some(idx);
                return Some(e.text.clone());
            }
        }
        None
    }

    /// Recall downwards (newer); returns None when past the newest entry.
    pub fn newer(&mut self, kind: CmdlineKind, prefix: &str) -> Option<String> {
        let id = kind.history_id();
        let start = self.recall?;
        let mut idx = start + 1;
        while idx < self.entries.len() {
            let e = &self.entries[idx];
            if e.kind == id && e.text.starts_with(prefix) {
                self.recall = Some(idx);
                return Some(e.text.clone());
            }
            idx += 1;
        }
        self.recall = None;
        None
    }

    /// Number of entries in one history list (`:history` output).
    pub fn count(&self, kind: CmdlineKind) -> usize {
        self.for_kind(kind).len()
    }

    pub fn clear(&mut self, kind: Option<CmdlineKind>) {
        match kind {
            Some(k) => self.entries.retain(|e| e.kind != k.history_id()),
            None => self.entries.clear(),
        }
        self.recall = None;
    }
}

/// Ex command-name completion for `:prefi<Tab>`.
///
/// Matches the table's command names and any user-defined command names,
/// sorted, deduplicated, and without Vim's "don't complete a name to itself"
/// special cases (those belong to the interactive layer).
pub fn complete_command(prefix: &str, user_commands: &[String], max: usize) -> Vec<String> {
    let prefix = prefix.trim_start_matches(':');
    let mut out: Vec<String> = Vec::new();
    for cmd in COMMANDS.iter() {
        if cmd.name.starts_with(prefix) && !out.iter().any(|n| n == cmd.name) {
            out.push(cmd.name.to_string());
        }
    }
    for name in user_commands {
        if name.starts_with(prefix) && !out.iter().any(|n| n == name) {
            out.push(name.clone());
        }
    }
    out.sort();
    out.truncate(max);
    out
}

/// What to complete after a completed command: nothing, a filename, or a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    None,
    File,
    Buffer,
    Option,
    Word,
}

/// Decide what the command line wants completed.
///
/// `line` is the text before the cursor. Only the first command word of the
/// line is classified; arguments after `:edit`/`:write` are filenames, after
/// `:set` are options, and after `:buffer` are buffers.
pub fn completion_kind(line: &str) -> CompletionKind {
    let body = line.trim_start_matches(':');
    let word = body
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
        .collect::<String>();
    if word.is_empty() {
        return CompletionKind::None;
    }
    // After the command word and one space, arguments are being typed.
    let has_arg = body.trim_start().len() > word.len();
    if !has_arg {
        return CompletionKind::None;
    }
    let Some(cmd) = lookup(&word) else {
        return CompletionKind::None;
    };
    use rvim_ex::table::ExFlags;
    if cmd.has(ExFlags::FILES) || matches!(cmd.name, "edit" | "write" | "read" | "source" | "cd") {
        CompletionKind::File
    } else if cmd.name.starts_with("buffer") || cmd.name == "bdelete" {
        CompletionKind::Buffer
    } else if cmd.name.starts_with("set") {
        CompletionKind::Option
    } else {
        CompletionKind::Word
    }
}

/// Complete a file name against the filesystem.
///
/// Supports a directory prefix (`dir/par`), `~` is *not* expanded (headless
/// portability), and the wildcard matcher handles `*`, `?` and `[...]`.
pub fn complete_filename(cwd: &Path, prefix: &str, max: usize) -> Vec<String> {
    let (dir_part, name_part, base) = match prefix.rfind('/') {
        Some(idx) => (
            &prefix[..=idx],
            &prefix[idx + 1..],
            cwd.join(&prefix[..idx]),
        ),
        None => ("", prefix, cwd.to_path_buf()),
    };
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name_part.is_empty() || wildcard::matches(&format!("{name_part}*"), &name) {
                let mut candidate = format!("{dir_part}{name}");
                if entry.path().is_dir() {
                    candidate.push('/');
                }
                out.push(candidate);
            }
        }
    }
    out.sort();
    out.truncate(max);
    out
}

/// Option-name completion for `:set `.
pub fn complete_option(prefix: &str, max: usize, names: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| n.starts_with(prefix))
        .map(|n| n.to_string())
        .collect();
    out.sort();
    out.dedup();
    out.truncate(max);
    out
}

/// Wildmenu state: the completion list plus the current selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WildMenu {
    pub matches: Vec<String>,
    pub index: Option<usize>,
    pub original: String,
}

impl WildMenu {
    pub fn new(matches: Vec<String>, original: &str) -> Self {
        let index = if matches.is_empty() { None } else { Some(0) };
        Self {
            matches,
            index,
            original: original.to_string(),
        }
    }

    pub fn is_active(&self) -> bool {
        !self.matches.is_empty()
    }

    /// Current selection, or the original text when inactive.
    pub fn current(&self) -> &str {
        match self.index.and_then(|i| self.matches.get(i)) {
            Some(m) => m,
            None => &self.original,
        }
    }

    /// Advance the selection, wrapping around.
    pub fn cycle_next(&mut self) -> &str {
        if self.matches.is_empty() {
            return &self.original;
        }
        let i = match self.index {
            Some(i) => (i + 1) % self.matches.len(),
            None => 0,
        };
        self.index = Some(i);
        &self.matches[i]
    }

    /// Move the selection backwards, wrapping around.
    pub fn cycle_previous(&mut self) -> &str {
        if self.matches.is_empty() {
            return &self.original;
        }
        let i = match self.index {
            Some(0) | None => self.matches.len() - 1,
            Some(i) => i - 1,
        };
        self.index = Some(i);
        &self.matches[i]
    }

    pub fn cancel(&mut self) {
        self.index = None;
    }
}

/// Expand `~` and environment variables in a file name (used by `:cd`, `:e`).
pub fn expand_filename(input: &str, home: Option<&str>) -> String {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = home {
            return format!("{home}/{rest}");
        }
    }
    if let Some(rest) = input.strip_prefix("$HOME/") {
        if let Some(home) = home {
            return format!("{home}/{rest}");
        }
    }
    input.to_string()
}

/// Resolve a possibly-relative path against the current directory.
pub fn resolve_path(cwd: &Path, input: &str) -> PathBuf {
    let expanded = expand_filename(input, std::env::var("HOME").ok().as_deref());
    let path = PathBuf::from(expanded);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_adds_dedups_and_limits() {
        let mut h = CmdHistory::new(3);
        h.add(CmdlineKind::Ex, "set nu");
        h.add(CmdlineKind::Ex, "set nu"); // duplicate of previous: ignored
        h.add(CmdlineKind::Ex, "set et");
        h.add(CmdlineKind::SearchForward, "foo");
        h.add(CmdlineKind::Ex, "set ts=4");
        h.add(CmdlineKind::Ex, "e x.txt");
        assert_eq!(h.count(CmdlineKind::Ex), 3);
        // Oldest ex entry was dropped by the limit.
        assert_eq!(
            h.for_kind(CmdlineKind::Ex),
            vec!["set et", "set ts=4", "e x.txt"]
        );
        // Search history is independent.
        assert_eq!(h.for_kind(CmdlineKind::SearchForward), vec!["foo"]);
    }

    #[test]
    fn history_recall_honours_prefix() {
        let mut h = CmdHistory::new(50);
        h.add(CmdlineKind::Ex, "set nu");
        h.add(CmdlineKind::Ex, "edit a.txt");
        h.add(CmdlineKind::Ex, "set et");
        assert_eq!(h.older(CmdlineKind::Ex, "set").as_deref(), Some("set et"));
        assert_eq!(h.older(CmdlineKind::Ex, "set").as_deref(), Some("set nu"));
        assert_eq!(h.older(CmdlineKind::Ex, "set"), None);
        assert_eq!(h.newer(CmdlineKind::Ex, "set").as_deref(), Some("set et"));
        assert_eq!(h.newer(CmdlineKind::Ex, "set"), None);
        // Prefix filter that matches nothing.
        let mut h2 = CmdHistory::new(50);
        h2.add(CmdlineKind::Ex, "set nu");
        assert_eq!(h2.older(CmdlineKind::Ex, "edit"), None);
    }

    #[test]
    fn command_completion_matches_table_and_users() {
        let users = vec!["MyCmd".to_string(), "MyOther".to_string()];
        let m = complete_command("se", &users, 100);
        assert!(m.contains(&"set".to_string()));
        assert!(m.contains(&"setlocal".to_string()));
        let m = complete_command("My", &users, 100);
        assert_eq!(m, vec!["MyCmd", "MyOther"]);
        // A full name still completes to itself (Vim keeps it in the list).
        let m = complete_command("write", &users, 100);
        assert_eq!(m, vec!["write"]);
    }

    #[test]
    fn completion_kind_classifies_arguments() {
        assert_eq!(completion_kind("se"), CompletionKind::None);
        assert_eq!(completion_kind("set nu"), CompletionKind::Option);
        assert_eq!(completion_kind("e src/"), CompletionKind::File);
        assert_eq!(completion_kind("w out.txt"), CompletionKind::File);
        assert_eq!(completion_kind("buffer 3"), CompletionKind::Buffer);
        assert_eq!(completion_kind(""), CompletionKind::None);
    }

    #[test]
    fn filename_completion_lists_directory() {
        let dir = std::env::temp_dir().join("rvim_cmdline_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.txt"), "x").unwrap();
        std::fs::write(dir.join("beta.txt"), "y").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();

        let all = complete_filename(&dir, "", 100);
        assert!(all.contains(&"alpha.txt".to_string()));
        assert!(all.contains(&"beta.txt".to_string()));
        assert!(all.contains(&"sub/".to_string()));

        let filtered = complete_filename(&dir, "al", 100);
        assert_eq!(filtered, vec!["alpha.txt"]);

        let subdir = complete_filename(&dir, "sub/", 100);
        assert!(subdir.is_empty()); // empty subdir

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wildmenu_cycles_and_restores() {
        let mut w = WildMenu::new(
            vec!["set".into(), "setlocal".into(), "setglobal".into()],
            "set",
        );
        assert_eq!(w.current(), "set");
        assert_eq!(w.cycle_next(), "setlocal");
        assert_eq!(w.cycle_next(), "setglobal");
        assert_eq!(w.cycle_next(), "set");
        assert_eq!(w.cycle_previous(), "setglobal");
        w.cancel();
        assert_eq!(w.current(), "set");
        let mut empty = WildMenu::new(Vec::new(), "x");
        assert!(!empty.is_active());
        assert_eq!(empty.cycle_next(), "x");
    }

    #[test]
    fn option_completion_and_path_helpers() {
        let names = ["set", "tabstop", "textwidth", "number"];
        assert_eq!(
            complete_option("t", 10, &names),
            vec!["tabstop", "textwidth"]
        );
        assert_eq!(expand_filename("~/x", Some("/home/u")), "/home/u/x");
        assert_eq!(expand_filename("x", Some("/home/u")), "x");
        let p = resolve_path(Path::new("/base"), "sub/file");
        assert!(p.ends_with("sub/file"));
    }
}
