//! Abbreviations (reference: `:abbreviate` in ex_docmd.c / insert.c).
//!
//! Abbreviations are stored per mode and expanded when the user types a
//! non-keyword character after a matching word. M2 provides the store plus the
//! word-expansion query; the insert-mode loop (M4) calls [`AbbrevStore::expand`].

use crate::maps::MapOptions;
use rvim_core::{VimError, VimResult};

/// Modes an abbreviation applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbbrevMode {
    /// `:iabbrev` — insert mode.
    Insert,
    /// `:cabbrev` — command line.
    Cmdline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AbbrevEntry {
    pub lhs: String,
    pub rhs: String,
    pub modes: Vec<AbbrevMode>,
    pub opts: MapOptions,
}

#[derive(Debug, Default)]
pub struct AbbrevStore {
    entries: Vec<AbbrevEntry>,
}

impl AbbrevStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[AbbrevEntry] {
        &self.entries
    }

    pub fn entries_for(&self, mode: AbbrevMode) -> Vec<&AbbrevEntry> {
        self.entries
            .iter()
            .filter(|e| e.modes.contains(&mode))
            .collect()
    }

    /// Define an abbreviation (`:abbreviate`).
    pub fn abbreviate(
        &mut self,
        lhs: &str,
        rhs: &str,
        modes: &[AbbrevMode],
        opts: MapOptions,
    ) -> VimResult<()> {
        if lhs.is_empty() {
            return Err(VimError::new(
                "E474",
                "Invalid argument: empty lhs".to_string(),
            ));
        }
        let entry = AbbrevEntry {
            lhs: lhs.to_string(),
            rhs: rhs.to_string(),
            modes: modes.to_vec(),
            opts,
        };
        if opts.unique
            && self
                .entries
                .iter()
                .any(|e| e.lhs == entry.lhs && modes.iter().any(|m| e.modes.contains(m)))
        {
            return Err(VimError::new(
                "E228",
                format!("Abbreviation already exists: {lhs}"),
            ));
        }
        self.entries
            .retain(|e| !(e.lhs == entry.lhs && modes.iter().any(|m| e.modes.contains(m))));
        self.entries.push(entry);
        Ok(())
    }

    /// Remove an abbreviation (`:unabbreviate`).
    pub fn unabbreviate(&mut self, lhs: &str, modes: &[AbbrevMode]) -> VimResult<()> {
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.lhs == lhs && modes.iter().any(|m| e.modes.contains(m))));
        if self.entries.len() == before {
            return Err(VimError::new(
                "E24",
                format!("No abbreviation found: {lhs}"),
            ));
        }
        Ok(())
    }

    pub fn clear(&mut self, modes: &[AbbrevMode]) {
        if modes.is_empty() {
            self.entries.clear();
        } else {
            let affected = modes.to_vec();
            self.entries
                .retain(|e| !e.modes.iter().any(|m| affected.contains(m)));
        }
    }

    /// Try to expand the word that ends `typed_before_cursor`.
    ///
    /// Returns `(word_len, replacement)` when the trailing word matches an
    /// abbreviation: the caller replaces the last `word_len` characters with
    /// `replacement`.
    pub fn expand(&self, mode: AbbrevMode, typed_before_cursor: &str) -> Option<(usize, String)> {
        // Find the trailing keyword run (letters, digits and '_').
        let word_start = typed_before_cursor
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let word = &typed_before_cursor[word_start..];
        if word.is_empty() {
            return None;
        }
        let entry = self
            .entries
            .iter()
            .filter(|e| e.modes.contains(&mode))
            .filter(|e| e.lhs == word)
            .max_by_key(|e| e.lhs.len())?;
        Some((word.len(), entry.rhs.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MapOptions {
        MapOptions::default()
    }

    #[test]
    fn insert_abbreviation_expands_trailing_word() {
        let mut store = AbbrevStore::new();
        store
            .abbreviate("teh", "the", &[AbbrevMode::Insert], opts())
            .unwrap();
        assert_eq!(
            store.expand(AbbrevMode::Insert, "I typed teh"),
            Some((3, "the".to_string()))
        );
        // No expansion when the word is incomplete.
        assert_eq!(store.expand(AbbrevMode::Insert, "te"), None);
        // Command-line abbreviations are separate.
        assert_eq!(store.expand(AbbrevMode::Cmdline, "teh"), None);
    }

    #[test]
    fn abbreviation_redefinition_and_removal() {
        let mut store = AbbrevStore::new();
        store
            .abbreviate("x", "one", &[AbbrevMode::Insert], opts())
            .unwrap();
        store
            .abbreviate("x", "two", &[AbbrevMode::Insert], opts())
            .unwrap();
        assert_eq!(store.entries_for(AbbrevMode::Insert).len(), 1);
        assert_eq!(
            store.expand(AbbrevMode::Insert, "x"),
            Some((1, "two".to_string()))
        );
        store.unabbreviate("x", &[AbbrevMode::Insert]).unwrap();
        assert!(store.expand(AbbrevMode::Insert, "x").is_none());
        assert_eq!(
            store
                .unabbreviate("x", &[AbbrevMode::Insert])
                .unwrap_err()
                .code,
            "E24"
        );
    }

    #[test]
    fn clear_is_mode_aware() {
        let mut store = AbbrevStore::new();
        store
            .abbreviate("a", "b", &[AbbrevMode::Insert], opts())
            .unwrap();
        store
            .abbreviate("c", "d", &[AbbrevMode::Cmdline], opts())
            .unwrap();
        store.clear(&[AbbrevMode::Insert]);
        assert!(store.entries_for(AbbrevMode::Insert).is_empty());
        assert_eq!(store.entries_for(AbbrevMode::Cmdline).len(), 1);
    }
}
