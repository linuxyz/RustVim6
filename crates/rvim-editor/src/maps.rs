//! Key mappings (reference: map.c).
//!
//! M2 models mappings as declared data plus a deterministic expansion engine:
//! `:nmap`/`:inoremap`/... store an entry, and [`MapStore::expand`] turns an
//! input key sequence into the expanded one. The TUI (M4) feeds real key
//! events into the same engine, and tests drive it headlessly.
//!
//! `<expr>` mappings need the VimL evaluator (M3): they are stored (so `:map`
//! listing and re-definition behave) but expansion reports `E15`.

use rvim_core::{VimError, VimResult};

/// Mapped modes, aligned with the single-letter mode prefixes of `:map`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MapMode {
    /// `n`
    Normal,
    /// `v` (visual + select)
    Visual,
    /// `x` (visual only)
    VisualOnly,
    /// `s` (select only)
    Select,
    /// `o`
    Operator,
    /// `i`
    Insert,
    /// `c`
    Cmdline,
    /// `l` (insert, cmdline and langmap)
    Language,
}

impl MapMode {
    pub fn from_char(c: char) -> Option<Self> {
        Some(match c {
            'n' => Self::Normal,
            'v' => Self::Visual,
            'x' => Self::VisualOnly,
            's' => Self::Select,
            'o' => Self::Operator,
            'i' => Self::Insert,
            'c' => Self::Cmdline,
            'l' => Self::Language,
            _ => return None,
        })
    }

    pub fn to_char(self) -> char {
        match self {
            Self::Normal => 'n',
            Self::Visual => 'v',
            Self::VisualOnly => 'x',
            Self::Select => 's',
            Self::Operator => 'o',
            Self::Insert => 'i',
            Self::Cmdline => 'c',
            Self::Language => 'l',
        }
    }
}

/// One key in a mapping: a printable character or a special key (`<CR>`, ...).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    /// Canonical special-key name without the angle brackets, e.g. `CR`, `C-R`.
    Special(String),
    /// `<Nop>`: consumes the lhs and produces nothing.
    Nop,
}

impl Key {
    /// Render back into `<...>`-aware text (the form `:map` displays).
    pub fn to_notation(&self) -> String {
        crate::keynames::to_notation(self)
    }
}

/// Parse `<...>` notation into keys (`:help notation`).
///
/// The name table, modifier canonicalisation and `<Char-N>` handling live in
/// [`crate::keynames`]; unknown `<foo>` is kept literally as text, matching vim.
pub fn parse_keys(text: &str) -> Vec<Key> {
    crate::keynames::parse_notation(text)
}

/// Render keys back to notation.
pub fn keys_to_notation(keys: &[Key]) -> String {
    crate::keynames::keys_to_notation(keys)
}

/// `:map` option flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapOptions {
    pub expr: bool,
    pub silent: bool,
    pub nowait: bool,
    pub buffer: bool,
    /// `remap` (recursive) vs `noremap`.
    pub recursive: bool,
    pub special: bool,
    pub unique: bool,
    pub script: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapEntry {
    pub lhs: Vec<Key>,
    pub rhs: Vec<Key>,
    /// Raw text as typed, kept for `:map` listing in the user's spelling.
    pub lhs_raw: String,
    pub rhs_raw: String,
    pub modes: Vec<MapMode>,
    pub opts: MapOptions,
}

impl MapEntry {
    pub fn lhs_notation(&self) -> String {
        keys_to_notation(&self.lhs)
    }

    pub fn rhs_notation(&self) -> String {
        keys_to_notation(&self.rhs)
    }
}

/// All mappings, kept per mode in declaration order.
#[derive(Debug, Default)]
pub struct MapStore {
    entries: Vec<MapEntry>,
}

/// Maximum recursive expansion depth (Vim reports E223 on recursion).
const MAX_EXPAND_DEPTH: usize = 100;

impl MapStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[MapEntry] {
        &self.entries
    }

    /// Define a mapping. `modes` lists every mode it applies to.
    pub fn map(
        &mut self,
        lhs: &str,
        rhs: &str,
        modes: &[MapMode],
        opts: MapOptions,
    ) -> VimResult<()> {
        let lhs_keys = parse_keys(lhs);
        if lhs_keys.is_empty() {
            return Err(VimError::new(
                "E474",
                "Invalid argument: empty lhs".to_string(),
            ));
        }
        let rhs_keys = parse_keys(rhs);
        if opts.unique && self.find(&lhs_keys, modes).is_some() {
            return Err(VimError::new(
                "E227",
                format!("Mapping already exists: {lhs}"),
            ));
        }
        // One entry per mode: `:map` covers several modes, and unmapping one of
        // them (`:sunmap`) must not remove the mapping from the others.
        for mode in modes {
            self.entries
                .retain(|e| !(e.lhs == lhs_keys && e.modes.contains(mode)));
            self.entries.push(MapEntry {
                lhs: lhs_keys.clone(),
                rhs: rhs_keys.clone(),
                lhs_raw: lhs.to_string(),
                rhs_raw: rhs.to_string(),
                modes: vec![*mode],
                opts,
            });
        }
        Ok(())
    }

    fn find(&self, lhs: &[Key], modes: &[MapMode]) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.lhs == lhs && modes.iter().any(|m| e.modes.contains(m)))
    }

    /// Remove a mapping from the given modes. Reports E31 when absent.
    pub fn unmap(&mut self, lhs: &str, modes: &[MapMode]) -> VimResult<()> {
        let keys = parse_keys(lhs);
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.lhs == keys && modes.iter().any(|m| e.modes.contains(m))));
        if self.entries.len() == before {
            return Err(VimError::new("E31", format!("No mapping found: {lhs}")));
        }
        Ok(())
    }

    /// Remove every mapping of the given modes (no mode list = all).
    pub fn clear(&mut self, modes: &[MapMode]) {
        if modes.is_empty() {
            self.entries.clear();
        } else {
            let affected = modes.to_vec();
            self.entries
                .retain(|e| !e.modes.iter().any(|m| affected.contains(m)));
        }
    }

    /// Entries that apply to `mode`, in declaration order.
    pub fn entries_for(&self, mode: MapMode) -> Vec<&MapEntry> {
        self.entries
            .iter()
            .filter(|e| e.modes.contains(&mode))
            .collect()
    }

    /// Longest lhs fully present at the start of `keys`.
    fn longest_match(&self, mode: MapMode, keys: &[Key]) -> Option<&MapEntry> {
        self.entries
            .iter()
            .filter(|e| e.modes.contains(&mode) && !e.lhs.is_empty() && keys.starts_with(&e.lhs))
            .max_by_key(|e| e.lhs.len())
    }

    /// Expand `input` using the mappings of `mode`.
    ///
    /// `noremap` entries are inserted verbatim; `remap` entries are expanded
    /// recursively (depth-limited). `<expr>` entries need the M3 evaluator.
    pub fn expand(&self, mode: MapMode, input: &[Key]) -> VimResult<Vec<Key>> {
        self.expand_depth(mode, input, 0)
    }

    fn expand_depth(&self, mode: MapMode, input: &[Key], depth: usize) -> VimResult<Vec<Key>> {
        if depth > MAX_EXPAND_DEPTH {
            return Err(VimError::new(
                "E223",
                "Recursive mapping detected".to_string(),
            ));
        }
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            match self.longest_match(mode, &input[i..]) {
                Some(entry) => {
                    if entry.opts.expr {
                        return Err(VimError::new(
                            "E15",
                            format!(
                                "<expr> mapping needs the VimL evaluator (M3): {}",
                                entry.lhs_notation()
                            ),
                        ));
                    }
                    i += entry.lhs.len();
                    if entry.opts.recursive {
                        out.extend(self.expand_depth(mode, &entry.rhs, depth + 1)?);
                    } else {
                        out.extend(entry.rhs.iter().cloned());
                    }
                }
                None => {
                    out.push(input[i].clone());
                    i += 1;
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(recursive: bool) -> MapOptions {
        MapOptions {
            recursive,
            ..Default::default()
        }
    }

    fn keys(s: &str) -> Vec<Key> {
        parse_keys(s)
    }

    #[test]
    fn special_keys_are_canonicalised() {
        assert_eq!(keys("<CR>"), vec![Key::Special("CR".to_string())]);
        assert_eq!(keys("<c-r>"), vec![Key::Special("C-R".to_string())]);
        assert_eq!(keys("<Nop>"), vec![Key::Nop]);
        // Unknown names stay literal.
        assert_eq!(
            keys("<foo>"),
            vec![
                Key::Char('<'),
                Key::Char('f'),
                Key::Char('o'),
                Key::Char('o'),
                Key::Char('>')
            ]
        );
        assert_eq!(keys_to_notation(&keys("<C-r>x")), "<C-R>x");
    }

    #[test]
    fn non_recursive_and_recursive_expansion() {
        let mut store = MapStore::new();
        store.map("x", "y", &[MapMode::Normal], opts(true)).unwrap();
        store.map("y", "z", &[MapMode::Normal], opts(true)).unwrap();
        // Recursive: x -> y -> z.
        assert_eq!(
            keys_to_notation(&store.expand(MapMode::Normal, &keys("x")).unwrap()),
            "z"
        );
        // With a recursive lhs but a non-recursive target: x -> y, and y is
        // expanded once more because x's rhs is recursive, yielding z.
        store
            .map("y", "z", &[MapMode::Normal], opts(false))
            .unwrap();
        assert_eq!(
            keys_to_notation(&store.expand(MapMode::Normal, &keys("x")).unwrap()),
            "z"
        );
        // A non-recursive mapping stops expansion of its own rhs: typing y
        // gives the literal z only (no further mapping applies).
        assert_eq!(
            keys_to_notation(&store.expand(MapMode::Normal, &keys("y")).unwrap()),
            "z"
        );
        // Non-recursive x -> y with y mapped to z stays y.
        store
            .map("x", "y", &[MapMode::Normal], opts(false))
            .unwrap();
        assert_eq!(
            keys_to_notation(&store.expand(MapMode::Normal, &keys("x")).unwrap()),
            "y"
        );
    }

    #[test]
    fn longest_lhs_wins_and_modes_are_isolated() {
        let mut store = MapStore::new();
        store
            .map("ab", "X", &[MapMode::Normal], opts(false))
            .unwrap();
        store
            .map("a", "Y", &[MapMode::Normal], opts(false))
            .unwrap();
        store
            .map("a", "I", &[MapMode::Insert], opts(false))
            .unwrap();
        assert_eq!(
            keys_to_notation(&store.expand(MapMode::Normal, &keys("abc")).unwrap()),
            "Xc"
        );
        assert_eq!(
            keys_to_notation(&store.expand(MapMode::Insert, &keys("a")).unwrap()),
            "I"
        );
        // Unmapped mode leaves input untouched.
        assert_eq!(
            keys_to_notation(&store.expand(MapMode::Visual, &keys("ab")).unwrap()),
            "ab"
        );
    }

    #[test]
    fn unmap_and_clear() {
        let mut store = MapStore::new();
        // Unmapping something that was never mapped is E31.
        assert_eq!(
            store.unmap("a", &[MapMode::Normal]).unwrap_err().code,
            "E31"
        );
        store
            .map("a", "b", &[MapMode::Normal], opts(false))
            .unwrap();
        store
            .map("c", "d", &[MapMode::Insert], opts(false))
            .unwrap();
        store.unmap("a", &[MapMode::Normal]).unwrap();
        // Clearing normal mode leaves insert mappings alone.
        store.clear(&[MapMode::Normal]);
        assert!(store.entries_for(MapMode::Normal).is_empty());
        assert_eq!(store.entries_for(MapMode::Insert).len(), 1);
        store.clear(&[]);
        assert!(store.entries().is_empty());
    }

    #[test]
    fn recursive_mapping_is_detected() {
        let mut store = MapStore::new();
        store.map("a", "b", &[MapMode::Normal], opts(true)).unwrap();
        store.map("b", "a", &[MapMode::Normal], opts(true)).unwrap();
        assert_eq!(
            store.expand(MapMode::Normal, &keys("a")).unwrap_err().code,
            "E223"
        );
    }

    #[test]
    fn expr_mapping_reports_unsupported() {
        let mut store = MapStore::new();
        let mut o = opts(false);
        o.expr = true;
        store.map("a", "1+1", &[MapMode::Normal], o).unwrap();
        assert_eq!(
            store.expand(MapMode::Normal, &keys("a")).unwrap_err().code,
            "E15"
        );
    }

    #[test]
    fn nop_consumes_the_lhs() {
        let mut store = MapStore::new();
        store
            .map("<Nop>", "", &[MapMode::Normal], opts(false))
            .unwrap();
        // A Nop lhs swallows the input and produces nothing.
        assert_eq!(
            keys_to_notation(&store.expand(MapMode::Normal, &keys("<Nop>")).unwrap()),
            ""
        );
    }
}
