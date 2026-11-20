//! Per-buffer syntax highlighting state (reference: syntax.c syn_state).
//!
//! The engine in [rvim_syntax] is a pure line function; this module keeps the
//! per-line state and the highlight table, and caches both per
//! (buffer, changedtick) so a redraw does not re-scan unchanged text.

use std::collections::HashMap;

use rvim_syntax::highlight::{HighlightTable, HlAttr};
pub use rvim_syntax::lang;
use rvim_syntax::{Engine, Span, State as LineState, SyntaxDef};

/// How many lines before the requested one the highlighter restarts from when
/// it has no cached state (vim's 'syn sync minlines' approximation).
pub const SYNC_LINES: usize = 100;

pub struct SyntaxState {
    /// `:syntax on` was executed.
    pub enabled: bool,
    /// Name of the syntax in use (empty when none is loaded).
    pub name: String,
    engine: Option<Engine>,
    /// `:highlight` groups, with vim's default links.
    pub highlights: HighlightTable,
    /// What the engine was detected for (buffer + filetype), so a buffer or
    /// filetype change reloads the definition.
    pub(crate) resolved: String,
    cache_key: Option<(i32, u64)>,
    states: HashMap<usize, LineState>,
    spans: HashMap<usize, Vec<Span>>,
}

impl Default for SyntaxState {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxState {
    pub fn new() -> Self {
        Self {
            enabled: false,
            name: String::new(),
            engine: None,
            highlights: HighlightTable::with_defaults(),
            resolved: String::new(),
            cache_key: None,
            states: HashMap::new(),
            spans: HashMap::new(),
        }
    }

    /// True when a definition is loaded and highlighting is on.
    pub fn is_active(&self) -> bool {
        self.enabled && self.engine.is_some()
    }

    /// Load a built-in definition. Returns false for an unknown name.
    pub fn enable(&mut self, name: &str) -> bool {
        match lang::builtin(name) {
            Some(def) => {
                self.enabled = true;
                self.load(def);
                true
            }
            None => false,
        }
    }

    /// Install a definition (built-in or assembled by `:syntax`).
    pub fn load(&mut self, def: SyntaxDef) {
        self.name = def.name.clone();
        self.engine = Some(Engine::new(def));
        self.enabled = true;
        self.invalidate();
    }

    pub fn engine(&self) -> Option<&Engine> {
        self.engine.as_ref()
    }

    /// The definition being edited, creating an empty one when none is loaded.
    pub fn def_mut(&mut self) -> &mut SyntaxDef {
        if self.engine.is_none() {
            let name = self.name.clone();
            self.engine = Some(Engine::new(SyntaxDef {
                name,
                ..SyntaxDef::default()
            }));
        }
        self.engine.as_mut().expect("engine just created").def_mut()
    }

    /// Append one `:syntax` item, returning its id (`synID()`).
    pub fn add_item(&mut self, group: &str, kind: rvim_syntax::ItemKind) -> i64 {
        let id = self
            .def_mut()
            .items
            .iter()
            .map(|item| item.id)
            .max()
            .unwrap_or(0)
            + 1;
        self.def_mut().items.push(rvim_syntax::Item {
            id,
            group: group.to_string(),
            kind,
        });
        self.invalidate();
        id
    }

    /// `:syntax cluster Name add=Group,...`
    pub fn cluster_add(&mut self, name: &str, groups: &[String]) {
        let def = self.def_mut();
        let entry = def.clusters.entry(name.to_string()).or_default();
        for group in groups {
            if !entry.contains(group) {
                entry.push(group.clone());
            }
        }
        self.invalidate();
    }

    /// `:syntax case match|ignore`
    pub fn set_ignore_case(&mut self, ignore: bool) {
        self.def_mut().ignore_case = ignore;
        self.invalidate();
    }

    pub fn clear_engine(&mut self) {
        self.engine = None;
        self.name.clear();
        self.invalidate();
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        self.clear_engine();
    }

    pub fn invalidate(&mut self) {
        self.states.clear();
        self.spans.clear();
        self.cache_key = None;
    }

    /// The attributes of a group after following `:hi link`.
    pub fn highlight(&self, group: &str) -> Option<HlAttr> {
        self.highlights.resolve(group)
    }

    fn prepare(&mut self, bufnr: i32, tick: u64) {
        if self.cache_key != Some((bufnr, tick)) {
            self.invalidate();
            self.cache_key = Some((bufnr, tick));
        }
    }

    /// Spans of `line`, using `window` (contiguous lines ending at `line`) to
    /// rebuild the state when it is not cached.
    pub fn line_spans(
        &mut self,
        bufnr: i32,
        tick: u64,
        line: usize,
        window: &[(usize, String)],
    ) -> &[Span] {
        self.prepare(bufnr, tick);
        if !self.spans.contains_key(&line) {
            if let Some(engine) = self.engine.clone() {
                for (lnum, text) in window {
                    if self.spans.contains_key(lnum) {
                        continue;
                    }
                    let state = self.states.get(lnum).cloned().unwrap_or_default();
                    let (spans, next) = engine.line(&state, text);
                    self.states.insert(*lnum, next);
                    self.spans.insert(*lnum, spans);
                }
            }
        }
        self.spans.get(&line).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_of(state: &mut SyntaxState, lines: &[&str]) -> Vec<Vec<(String, String)>> {
        let tick = 1;
        let mut out = Vec::new();
        for index in 0..lines.len() {
            let window: Vec<(usize, String)> = lines
                .iter()
                .enumerate()
                .take(index + 1)
                .map(|(i, l)| (i, l.to_string()))
                .collect();
            let spans = state.line_spans(1, tick, index, &window);
            out.push(
                spans
                    .iter()
                    .map(|s| (lines[index][s.start..s.end].to_string(), s.group.clone()))
                    .collect(),
            );
        }
        out
    }

    #[test]
    fn cache_survives_repeated_queries_and_expires_on_a_new_tick() {
        let mut state = SyntaxState::new();
        assert!(state.enable("rust"));
        let first = spans_of(&mut state, &["let x = 1;"]);
        assert!(first[0].iter().any(|(t, g)| t == "let" && g == "Keyword"));
        // Same tick: cached, same answer.
        let again = spans_of(&mut state, &["let x = 1;"]);
        assert_eq!(first, again);
        // A new changedtick recomputes.
        let window = vec![(0usize, "let y = 2;".to_string())];
        let spans = state.line_spans(1, 2, 0, &window);
        assert!(spans.iter().any(|s| &"let y = 2;"[s.start..s.end] == "let"));
    }

    #[test]
    fn unknown_languages_are_reported() {
        let mut state = SyntaxState::new();
        assert!(!state.enable("no-such-language"));
        assert!(!state.is_active());
        assert!(state.enable("python"));
        assert!(state.is_active());
        assert_eq!(state.name, "python");
    }
}
