//! Autocommands (reference: autocmd.c).
//!
//! M2 ships the declaration model: augroups, event/pattern matching and
//! `:autocmd` listing/removal all behave, so runtime scripts such as
//! `filetype.vim` and `ftplugin.vim` can be sourced without errors. Firing
//! events requires the script executor (M3) and is registered in
//! docs/compat_deviations.md as a known M2 boundary.

use rvim_core::{VimError, VimResult};

/// Event names from the baseline `event_names[]` table (autocmd.c), in
/// reference order. 114 events.
pub const EVENTS: &[&str] = &[
    "BufAdd",
    "BufCreate",
    "BufDelete",
    "BufEnter",
    "BufFilePost",
    "BufFilePre",
    "BufHidden",
    "BufLeave",
    "BufNew",
    "BufNewFile",
    "BufRead",
    "BufReadCmd",
    "BufReadPost",
    "BufReadPre",
    "BufUnload",
    "BufWinEnter",
    "BufWinLeave",
    "BufWipeout",
    "BufWrite",
    "BufWritePost",
    "BufWritePre",
    "BufWriteCmd",
    "CmdlineChanged",
    "CmdlineEnter",
    "CmdlineLeave",
    "CmdwinEnter",
    "CmdwinLeave",
    "CmdUndefined",
    "ColorScheme",
    "ColorSchemePre",
    "CompleteChanged",
    "CompleteDone",
    "CompleteDonePre",
    "CursorHold",
    "CursorHoldI",
    "CursorMoved",
    "CursorMovedI",
    "DiffUpdated",
    "DirChanged",
    "DirChangedPre",
    "EncodingChanged",
    "ExitPre",
    "FileEncoding",
    "FileAppendPost",
    "FileAppendPre",
    "FileAppendCmd",
    "FileChangedShell",
    "FileChangedShellPost",
    "FileChangedRO",
    "FileReadPost",
    "FileReadPre",
    "FileReadCmd",
    "FileType",
    "FileWritePost",
    "FileWritePre",
    "FileWriteCmd",
    "FilterReadPost",
    "FilterReadPre",
    "FilterWritePost",
    "FilterWritePre",
    "FocusGained",
    "FocusLost",
    "FuncUndefined",
    "GUIEnter",
    "GUIFailed",
    "InsertChange",
    "InsertEnter",
    "InsertLeave",
    "InsertLeavePre",
    "InsertCharPre",
    "MenuPopup",
    "ModeChanged",
    "OptionSet",
    "QuickFixCmdPost",
    "QuickFixCmdPre",
    "QuitPre",
    "RemoteReply",
    "SafeState",
    "SafeStateAgain",
    "SessionLoadPost",
    "ShellCmdPost",
    "ShellFilterPost",
    "SourceCmd",
    "SourcePre",
    "SourcePost",
    "SpellFileMissing",
    "StdinReadPost",
    "StdinReadPre",
    "SwapExists",
    "Syntax",
    "TabNew",
    "TabClosed",
    "TabEnter",
    "TabLeave",
    "TermChanged",
    "TerminalOpen",
    "TerminalWinOpen",
    "TermResponse",
    "TextChanged",
    "TextChangedI",
    "TextChangedP",
    "User",
    "VimEnter",
    "VimLeave",
    "VimLeavePre",
    "WinNew",
    "WinClosed",
    "WinEnter",
    "WinLeave",
    "WinScrolled",
    "VimResized",
    "TextYankPost",
    "VimSuspend",
    "VimResume",
];

/// Case-insensitive event-name lookup (`:autocmd` accepts any case).
pub fn canonical_event(name: &str) -> Option<&'static str> {
    EVENTS
        .iter()
        .copied()
        .find(|e| e.eq_ignore_ascii_case(name))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoCmd {
    pub group: String,
    /// Canonical event names the command fires on.
    pub events: Vec<String>,
    /// File/pattern match; empty means "every file".
    pub pattern: String,
    pub nested: bool,
    pub once: bool,
    /// Ex command text without the leading ':'.
    pub command: String,
}

#[derive(Debug, Default)]
pub struct AutocmdStore {
    entries: Vec<AutoCmd>,
    groups: Vec<String>,
    /// Group newly registered autocommands belong to.
    current_group: Option<String>,
}

/// Pattern matching for autocommand patterns: `*` matches any run of
/// characters, `?` matches one; comma-separated alternatives are supported.
///
/// Reference: `match_file_pat()` (fileio.c:5386-5397). A pattern that contains
/// no path separator matches the *tail* of the file name, so `au BufRead
/// picked.txt` fires for an absolute path ending in `/picked.txt`. A pattern
/// with a path separator matches the full path.
pub fn pattern_matches(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let tail = name.rsplit(['/', '\\']).next().unwrap_or(name);
    pattern.split(',').filter(|p| !p.is_empty()).any(|alt| {
        if alt.contains('/') || alt.contains('\\') {
            wildcard_match(alt.as_bytes(), name.as_bytes())
        } else {
            wildcard_match(alt.as_bytes(), tail.as_bytes())
        }
    })
}

/// Minimal glob matcher (`*`, `?`) over bytes; shared with `:autocmd` patterns.
fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(sp) = star {
            p = sp + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

impl AutocmdStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[AutoCmd] {
        &self.entries
    }

    pub fn groups(&self) -> &[String] {
        &self.groups
    }

    pub fn current_group(&self) -> Option<&str> {
        self.current_group.as_deref()
    }

    /// `:augroup NAME`: create (if needed) and enter a group.
    pub fn define_group(&mut self, name: &str) {
        if !self.groups.iter().any(|g| g == name) {
            self.groups.push(name.to_string());
        }
        self.current_group = Some(name.to_string());
    }

    /// `:augroup END`: leave the current group.
    pub fn end_group(&mut self) {
        self.current_group = None;
    }

    /// `:augroup NAME`: delete a group and its autocommands.
    pub fn delete_group(&mut self, name: &str) {
        self.groups.retain(|g| g != name);
        self.entries.retain(|e| e.group != name);
        if self.current_group.as_deref() == Some(name) {
            self.current_group = None;
        }
    }

    /// `:autocmd {event} {pat} {cmd}`; the group defaults to the current one.
    pub fn add(
        &mut self,
        group: Option<&str>,
        events: &[String],
        pattern: &str,
        nested: bool,
        once: bool,
        command: &str,
    ) -> VimResult<()> {
        let group = group
            .map(str::to_string)
            .or_else(|| self.current_group.clone())
            .unwrap_or_default();
        if let Some(g) = group.strip_prefix("END") {
            let _ = g;
        }
        let mut canonical = Vec::new();
        for e in events {
            let name = if e == "*" || e.eq_ignore_ascii_case("all") {
                "*"
            } else {
                canonical_event(e)
                    .ok_or_else(|| VimError::new("E216", format!("Invalid event name: {e}")))?
            };
            canonical.push(name.to_string());
        }
        self.entries.push(AutoCmd {
            group,
            events: canonical,
            pattern: pattern.to_string(),
            nested,
            once,
            command: command.to_string(),
        });
        Ok(())
    }

    /// Remove entries: all, or limited by group/event/pattern filters.
    pub fn remove(&mut self, group: Option<&str>, event: Option<&str>, pattern: Option<&str>) {
        let group = group.map(str::to_string);
        self.entries.retain(|e| {
            let group_ok = group.as_ref().is_none_or(|g| &e.group == g);
            let event_ok = event.is_none_or(|ev| {
                e.events
                    .iter()
                    .any(|x| x == "*" || x.eq_ignore_ascii_case(ev))
            });
            let pattern_ok = pattern.is_none_or(|pt| e.pattern == pt);
            !(group_ok && event_ok && pattern_ok)
        });
    }

    /// Drop entries by index. Used when `++once` autocommands have fired; the
    /// indices must be ascending and are removed back-to-front.
    pub fn remove_indices(&mut self, indices: &[usize]) {
        for &i in indices.iter().rev() {
            if i < self.entries.len() {
                self.entries.remove(i);
            }
        }
    }

    /// Entries that would fire for `event` on `name`.
    pub fn matching(&self, event: &str, name: &str) -> Vec<&AutoCmd> {
        self.entries
            .iter()
            .filter(|e| {
                e.events
                    .iter()
                    .any(|x| x == "*" || x.eq_ignore_ascii_case(event))
                    && pattern_matches(&e.pattern, name)
            })
            .collect()
    }

    /// Text lines for `:autocmd` listing, in Vim's layout.
    pub fn list(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|e| {
                let flags = match (e.once, e.nested) {
                    (true, true) => "++once ++nested ",
                    (true, false) => "++once ",
                    (false, true) => "++nested ",
                    (false, false) => "",
                };
                let events = e.events.join(",");
                format!(
                    "{}  {}  {}  {}\t{}{}",
                    if e.group.is_empty() { "---" } else { &e.group },
                    events,
                    if e.pattern.is_empty() {
                        "*"
                    } else {
                        &e.pattern
                    },
                    flags,
                    "",
                    e.command
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_baseline_event_name_is_known() {
        assert_eq!(EVENTS.len(), 114);
        assert!(canonical_event("BufRead").is_some());
        assert!(canonical_event("bufwrite").is_some());
        assert!(canonical_event("NotARealEvent").is_none());
    }

    #[test]
    fn pattern_matching_uses_glob_rules() {
        assert!(pattern_matches("*.c", "foo.c"));
        assert!(!pattern_matches("*.c", "foo.h"));
        assert!(pattern_matches("*.c,*.h", "foo.h"));
        assert!(pattern_matches("", "anything"));
        assert!(pattern_matches("a?c", "abc"));
    }

    #[test]
    fn pattern_without_slash_matches_the_file_name_tail() {
        // match_file_pat(): no '/' in the pattern => match the tail only.
        assert!(pattern_matches("picked.txt", "/tmp/a/b/picked.txt"));
        assert!(pattern_matches("*.txt", "C:\\tmp\\picked.txt"));
        assert!(!pattern_matches("picked.txt", "/tmp/picked.txt.bak"));
        // A pattern containing a separator matches the full path.
        assert!(pattern_matches("/tmp/a/*.txt", "/tmp/a/x.txt"));
        assert!(!pattern_matches("/tmp/a/*.txt", "/other/a/x.txt"));
    }

    #[test]
    fn augroups_scope_registration_and_deletion() {
        let mut store = AutocmdStore::new();
        store.define_group("mygrp");
        store
            .add(
                None,
                &["BufRead".to_string()],
                "*.rs",
                false,
                false,
                "setlocal et",
            )
            .unwrap();
        store.end_group();
        store
            .add(
                None,
                &["BufWrite".to_string()],
                "*",
                false,
                false,
                "echo 'w'",
            )
            .unwrap();
        assert_eq!(store.entries().len(), 2);
        assert_eq!(store.entries()[0].group, "mygrp");
        assert_eq!(store.entries()[1].group, "");
        store.delete_group("mygrp");
        assert_eq!(store.entries().len(), 1);
        assert_eq!(store.groups(), &Vec::<String>::new());
    }

    #[test]
    fn invalid_event_is_rejected() {
        let mut store = AutocmdStore::new();
        let err = store
            .add(None, &["Bogus".to_string()], "*", false, false, "echo")
            .unwrap_err();
        assert_eq!(err.code, "E216");
    }

    #[test]
    fn matching_filters_by_event_and_pattern() {
        let mut store = AutocmdStore::new();
        store
            .add(None, &["BufRead".to_string()], "*.rs", false, false, "one")
            .unwrap();
        store
            .add(None, &["*".to_string()], "*.txt", false, false, "two")
            .unwrap();
        let hits = store.matching("BufRead", "main.rs");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "one");
        // Wildcard events match every event.
        let hits = store.matching("VimEnter", "notes.txt");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "two");
    }

    #[test]
    fn removal_filters_work() {
        let mut store = AutocmdStore::new();
        store
            .add(None, &["BufRead".to_string()], "*.rs", false, false, "one")
            .unwrap();
        store
            .add(None, &["BufRead".to_string()], "*.txt", false, false, "two")
            .unwrap();
        store.remove(None, Some("BufRead"), Some("*.rs"));
        assert_eq!(store.entries().len(), 1);
        store.remove(None, None, None);
        assert!(store.entries().is_empty());
    }
}
