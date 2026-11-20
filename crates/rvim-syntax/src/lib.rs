//! rvim-syntax: the syntax highlighting engine (reference: syntax.c).
//!
//! The engine is a pure function over lines: given the state left by the
//! previous line it returns the highlighted byte ranges of the current line and
//! the state to carry forward. That is what vim does with `syn_state`, minus
//! the screen-side bookkeeping.
//!
//! Supported item kinds: `syntax keyword`, `syntax match` and `syntax region`
//! with `contains=`, plus `syntax cluster`, `syntax case` and highlight-group
//! links. Patterns are evaluated with the MVP regex engine
//! ([rvim_viml::regex_mvp]) and are matched left to right; later definitions win
//! over earlier ones, and nested items override their container.

pub mod highlight;
pub mod lang;
pub mod script;

use std::collections::HashMap;

use rvim_viml::regex_mvp;

/// What a syntax item matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// A list of whole words (`syntax keyword`).
    Keyword { words: Vec<String> },
    /// A regular expression (`syntax match`).
    Match {
        pattern: String,
        contains: Vec<String>,
    },
    /// A start/end pair (`syntax region`).
    Region {
        start: String,
        end: String,
        contains: Vec<String>,
    },
}

/// One `:syntax` definition. `id` is what `synID()` would report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: i64,
    pub group: String,
    pub kind: ItemKind,
}

/// Per-item flags from the trailing `:syntax` arguments (`contained`,
/// `transparent`, `skip=`, `nextgroup=`).  They live outside [ItemKind] so
/// existing definitions keep their shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemOptions {
    /// `contained`: only matches inside a region that lists the group.
    pub contained: bool,
    /// `transparent`: the region itself is not coloured, its contents are.
    pub transparent: bool,
    /// `skip=/pat/` (also the skipwhite/skipnl/skipempty shorthands).
    pub skip: Option<String>,
    /// `nextgroup=Group,...`: groups to try right after this item.
    pub nextgroup: Vec<String>,
    /// `display`: accepted, no rendering effect.
    pub display: bool,
    /// `conceal`: accepted, no rendering effect.
    pub conceal: bool,
    /// `fold`: the region creates a fold under 'foldmethod=syntax'.
    pub fold: bool,
}

impl ItemOptions {
    /// True when any non-default option is set.
    pub fn is_default(&self) -> bool {
        *self == ItemOptions::default()
    }
}

/// `syntax sync` configuration (`fromstart`, `minlines`, `maxlines`,
/// `ccomment`, `linebreaks`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncSpec {
    pub from_start: bool,
    pub minlines: Option<usize>,
    pub maxlines: Option<usize>,
    pub ccomment: bool,
    pub linebreaks: Option<usize>,
}

/// A complete syntax for one language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyntaxDef {
    pub name: String,
    /// `syntax case ignore`.
    pub ignore_case: bool,
    pub items: Vec<Item>,
    /// `@cluster` -> group names (`syntax cluster`).
    pub clusters: HashMap<String, Vec<String>>,
    /// Flags of one item, keyed by [Item::id].
    pub item_options: HashMap<i64, ItemOptions>,
    /// `syntax sync` state used when the per-buffer cache has no state.
    pub sync: SyncSpec,
}

/// A region that is still open at the end of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRegion {
    pub group: String,
    pub id: i64,
    pub end: String,
    pub contains: Vec<String>,
}

/// Highlighting state carried from one line to the next.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub stack: Vec<OpenRegion>,
}

impl State {
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}

/// One highlighted range: byte offsets (end exclusive) plus its group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub group: String,
    pub id: i64,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte length of the character at `pos` (1 for an invalid position).
fn char_len(text: &str, pos: usize) -> usize {
    text[pos..].chars().next().map(char::len_utf8).unwrap_or(1)
}

/// A pattern hit at the current position.  `span_*` are what gets drawn
/// (`\zs`/`\ze` adjust them), `advance` is how far the scanner moves so a
/// line does not re-match the same text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hit {
    span_start: usize,
    span_end: usize,
    advance: usize,
}

impl Hit {
    fn plain(len: usize) -> Self {
        Self {
            span_start: 0,
            span_end: len,
            advance: len,
        }
    }
}

/// Drop `\zs`/`\ze` so the raw extent of a pattern can be measured.
fn strip_zs_ze(pattern: &str) -> String {
    pattern.replace("\\zs", "").replace("\\ze", "")
}

/// Match `pattern` exactly at the start of `slice`.
fn matches_at(pattern: &str, slice: &str, ignore_case: bool) -> Option<Hit> {
    if pattern.is_empty() {
        return None;
    }
    let adjusted = regex_mvp::find_captures(pattern, slice, ignore_case)?;
    let stripped = strip_zs_ze(pattern);
    let raw_end = if stripped != pattern {
        match regex_mvp::find_captures(&stripped, slice, ignore_case) {
            Some(raw) if raw.start == 0 => raw.end,
            _ => adjusted.end,
        }
    } else {
        adjusted.end
    };
    if adjusted.start == 0 {
        Some(Hit {
            span_start: 0,
            span_end: adjusted.end,
            advance: raw_end.max(adjusted.end),
        })
    } else {
        // The raw match must still start here; \zs only moves the drawn start.
        let raw = regex_mvp::find_captures(&stripped, slice, ignore_case)?;
        if raw.start != 0 {
            return None;
        }
        Some(Hit {
            span_start: adjusted.start,
            span_end: adjusted.end,
            advance: raw.end.max(adjusted.end),
        })
    }
}

/// The engine for one [SyntaxDef].
#[derive(Clone, Debug)]
pub struct Engine {
    def: SyntaxDef,
}

impl Engine {
    pub fn new(def: SyntaxDef) -> Self {
        Self { def }
    }

    pub fn def(&self) -> &SyntaxDef {
        &self.def
    }

    /// Mutable access for `:syntax ...` definitions.
    pub fn def_mut(&mut self) -> &mut SyntaxDef {
        &mut self.def
    }

    /// The group name of an item id (`synIDattr(id, "name")`).
    pub fn group_of(&self, id: i64) -> Option<&str> {
        self.def
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.group.as_str())
    }

    /// Item ids, in definition order (`:syntax list`).
    pub fn items(&self) -> &[Item] {
        &self.def.items
    }

    /// The flags of an item (empty when it has none).
    pub fn options_of(&self, id: i64) -> ItemOptions {
        self.def.item_options.get(&id).cloned().unwrap_or_default()
    }

    /// Indices of the items allowed inside `container` (`None` = top level).
    fn allowed(&self, container: Option<&OpenRegion>) -> Vec<usize> {
        let Some(region) = container else {
            // `contained` items never match at the top level.
            return self
                .def
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| !self.options_of(item.id).contained)
                .map(|(index, _)| index)
                .collect();
        };
        if region.contains.is_empty() {
            return Vec::new();
        }
        let mut names: Vec<String> = Vec::new();
        for entry in &region.contains {
            if let Some(cluster) = entry.strip_prefix('@') {
                if let Some(members) = self.def.clusters.get(cluster) {
                    names.extend(members.iter().cloned());
                }
            } else if entry == "ALL" {
                return (0..self.def.items.len()).collect();
            } else if entry == "TOP" {
                return self
                    .def
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| !self.options_of(item.id).contained)
                    .map(|(index, _)| index)
                    .collect();
            } else {
                names.push(entry.clone());
            }
        }
        self.def
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| names.contains(&item.group))
            .map(|(index, _)| index)
            .collect()
    }

    /// The item starting at `pos`, with its drawn span and advance length.
    fn item_at(&self, allowed: &[usize], text: &str, pos: usize) -> Option<(&Item, Hit)> {
        let slice = &text[pos..];
        // Later definitions have priority (syntax.c: the last match wins).
        for index in allowed.iter().rev() {
            let item = &self.def.items[*index];
            let hit = match &item.kind {
                ItemKind::Keyword { words } => {
                    keyword_at(words, text, pos, self.def.ignore_case).map(Hit::plain)
                }
                ItemKind::Match { pattern, .. } => matches_at(pattern, slice, self.def.ignore_case),
                ItemKind::Region { start, .. } => matches_at(start, slice, self.def.ignore_case),
            };
            if let Some(hit) = hit {
                if hit.advance > 0 {
                    return Some((item, hit));
                }
            }
        }
        None
    }

    /// Highlight one line, returning its spans and the state for the next line.
    ///
    /// Spans are ordered outer-first: when ranges overlap the later span wins,
    /// which is how a contained item overrides its region.
    pub fn line(&self, state: &State, text: &str) -> (Vec<Span>, State) {
        let len = text.len();
        let mut stack: Vec<OpenRegion> = state.stack.clone();
        let mut starts: Vec<usize> = vec![0; stack.len()];
        let mut allowed = self.allowed(stack.last());
        let mut spans: Vec<Span> = Vec::new();
        let mut pos = 0usize;
        // Every step either consumes at least one byte or changes the stack.
        let mut guard = len * 8 + 128;
        while pos < len && guard > 0 {
            guard -= 1;
            // 1. Does the innermost open region end here (or skip this part)?
            if let Some(top) = stack.last().cloned() {
                let opts = self.options_of(top.id);
                if let Some(skip) = opts.skip.as_deref() {
                    if let Some(hit) = matches_at(skip, &text[pos..], self.def.ignore_case) {
                        pos += hit.advance.max(char_len(text, pos));
                        continue;
                    }
                }
                if let Some(hit) = matches_at(&top.end, &text[pos..], self.def.ignore_case) {
                    let start = starts.pop().unwrap_or(0);
                    stack.pop();
                    // A transparent region is a container only: it is not drawn.
                    if !opts.transparent {
                        spans.push(Span {
                            start,
                            end: pos + hit.span_end,
                            group: top.group.clone(),
                            id: top.id,
                        });
                    }
                    pos += hit.advance;
                    allowed = self.allowed(stack.last());
                    continue;
                }
            }
            // 2. Does an allowed item start here?
            if let Some((item, hit)) = self.item_at(&allowed, text, pos) {
                let opts = self.options_of(item.id);
                match &item.kind {
                    ItemKind::Region { end, contains, .. } => {
                        // `skip=` on a region start: ignore this start and move on.
                        if let Some(skip) = opts.skip.as_deref() {
                            if matches_at(skip, &text[pos..], self.def.ignore_case)
                                .is_some_and(|hit| hit.advance > 0)
                            {
                                pos += char_len(text, pos);
                                continue;
                            }
                        }
                        stack.push(OpenRegion {
                            group: item.group.clone(),
                            id: item.id,
                            end: end.clone(),
                            contains: contains.clone(),
                        });
                        starts.push(pos + hit.span_start);
                        allowed = self.allowed(stack.last());
                    }
                    _ => spans.push(Span {
                        start: pos + hit.span_start,
                        end: pos + hit.span_end,
                        group: item.group.clone(),
                        id: item.id,
                    }),
                }
                pos += hit.advance;
                // `nextgroup=`: try the listed groups immediately after this item.
                // `skipwhite`/`skipnl` move over the skipped text before trying;
                // when nothing matches the cursor stays where the item ended.
                if !opts.nextgroup.is_empty() && pos < len {
                    let mut groups = opts.nextgroup.clone();
                    let mut depth = 0;
                    let mut opened = false;
                    let mut try_pos = pos;
                    if let Some(skip) = opts.skip.as_deref() {
                        if let Some(hit) = matches_at(skip, &text[try_pos..], self.def.ignore_case)
                            .filter(|hit| hit.advance > 0)
                        {
                            try_pos += hit.advance;
                        }
                    }
                    let mut cursor = try_pos;
                    while depth < 8 && cursor < len && !groups.is_empty() {
                        let Some((next, hit)) = self.item_by_groups(&groups, text, cursor) else {
                            break;
                        };
                        let next_opts = self.options_of(next.id);
                        match &next.kind {
                            ItemKind::Region { end, contains, .. } => {
                                stack.push(OpenRegion {
                                    group: next.group.clone(),
                                    id: next.id,
                                    end: end.clone(),
                                    contains: contains.clone(),
                                });
                                starts.push(cursor + hit.span_start);
                                allowed = self.allowed(stack.last());
                                opened = true;
                            }
                            _ => spans.push(Span {
                                start: cursor + hit.span_start,
                                end: cursor + hit.span_end,
                                group: next.group.clone(),
                                id: next.id,
                            }),
                        }
                        cursor += hit.advance;
                        groups = next_opts.nextgroup;
                        depth += 1;
                        if opened {
                            break;
                        }
                    }
                    if cursor > pos {
                        pos = cursor;
                    }
                }
                continue;
            }
            pos += char_len(text, pos);
        }
        // Regions still open cover the rest of the line.
        for (index, region) in stack.iter().enumerate() {
            let start = starts.get(index).copied().unwrap_or(0);
            if self.options_of(region.id).transparent {
                continue;
            }
            spans.push(Span {
                start,
                end: len,
                group: region.group.clone(),
                id: region.id,
            });
        }
        // Outer ranges first; equal starts put the longer range first.
        spans.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));
        (spans, State { stack })
    }

    /// Find the highest-priority item whose group is listed in `groups`.
    fn item_by_groups(&self, groups: &[String], text: &str, pos: usize) -> Option<(&Item, Hit)> {
        let mut names: Vec<String> = Vec::new();
        for entry in groups {
            if let Some(cluster) = entry.strip_prefix('@') {
                if let Some(members) = self.def.clusters.get(cluster) {
                    names.extend(members.iter().cloned());
                }
            } else if entry == "ALL" || entry == "TOP" {
                names.extend(self.def.items.iter().map(|item| item.group.clone()));
            } else {
                names.push(entry.clone());
            }
        }
        let slice = &text[pos..];
        for item in self.def.items.iter().rev() {
            if !names.contains(&item.group) {
                continue;
            }
            let hit = match &item.kind {
                ItemKind::Keyword { words } => {
                    keyword_at(words, text, pos, self.def.ignore_case).map(Hit::plain)
                }
                ItemKind::Match { pattern, .. } => matches_at(pattern, slice, self.def.ignore_case),
                ItemKind::Region { start, .. } => matches_at(start, slice, self.def.ignore_case),
            };
            if let Some(hit) = hit.filter(|hit| hit.advance > 0) {
                return Some((item, hit));
            }
        }
        None
    }
}

/// Match a keyword at `pos`, returning its byte length.
fn keyword_at(words: &[String], text: &str, pos: usize, ignore_case: bool) -> Option<usize> {
    if pos > 0 {
        if let Some(prev) = text[..pos].chars().next_back() {
            if is_word_char(prev) {
                return None;
            }
        }
    }
    let word: String = text[pos..]
        .chars()
        .take_while(|c| is_word_char(*c))
        .collect();
    if word.is_empty() {
        return None;
    }
    let hit = if ignore_case {
        words.iter().any(|w| w.eq_ignore_ascii_case(&word))
    } else {
        words.contains(&word)
    };
    hit.then_some(word.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang;

    fn spans_of(engine: &Engine, lines: &[&str]) -> Vec<Vec<(String, String)>> {
        let mut state = State::default();
        let mut out = Vec::new();
        for line in lines {
            let (spans, next) = engine.line(&state, line);
            state = next;
            out.push(
                spans
                    .iter()
                    .map(|s| (line[s.start..s.end].to_string(), s.group.clone()))
                    .collect(),
            );
        }
        out
    }

    #[test]
    fn keywords_match_whole_words_only() {
        let mut def = SyntaxDef {
            name: "t".into(),
            ..SyntaxDef::default()
        };
        def.items.push(Item {
            id: 1,
            group: "Keyword".into(),
            kind: ItemKind::Keyword {
                words: vec!["if".into(), "else".into()],
            },
        });
        let engine = Engine::new(def);
        assert_eq!(
            spans_of(&engine, &["if x else y", "iffy"]),
            vec![
                vec![
                    ("if".to_string(), "Keyword".to_string()),
                    ("else".to_string(), "Keyword".to_string())
                ],
                vec![]
            ]
        );
    }

    #[test]
    fn regions_continue_across_lines_and_contain_items() {
        let engine = Engine::new(lang::builtin("rust").expect("rust syntax"));
        let lines = spans_of(&engine, &["/* start", "still if here", "end */ let x = 1;"]);
        // The comment covers the first two lines and the closing line's prefix.
        assert!(
            lines[0]
                .iter()
                .any(|(t, g)| t == "/* start" && g == "Comment")
        );
        assert!(
            lines[1]
                .iter()
                .any(|(t, g)| t == "still if here" && g == "Comment")
        );
        assert!(
            !lines[1].iter().any(|(_, g)| g == "Keyword"),
            "keywords inside a comment are not highlighted: {:?}",
            lines[1]
        );
        assert!(
            lines[2]
                .iter()
                .any(|(t, g)| t == "end */" && g == "Comment")
        );
        assert!(lines[2].iter().any(|(t, g)| t == "let" && g == "Keyword"));
        assert!(lines[2].iter().any(|(t, g)| t == "1" && g == "Number"));
    }

    #[test]
    fn strings_hide_keywords() {
        let engine = Engine::new(lang::builtin("rust").expect("rust syntax"));
        let spans = spans_of(&engine, &["let s = \"if let\";"]);
        assert!(
            spans[0]
                .iter()
                .any(|(t, g)| t == "\"if let\"" && g == "String")
        );
        assert!(!spans[0].iter().any(|(t, _)| t == "if"));
    }

    #[test]
    fn clusters_resolve_inside_regions() {
        let mut def = SyntaxDef {
            name: "t".into(),
            ..SyntaxDef::default()
        };
        def.items.push(Item {
            id: 1,
            group: "Comment".into(),
            kind: ItemKind::Region {
                start: "<".into(),
                end: ">".into(),
                contains: vec!["@inside".into()],
            },
        });
        def.items.push(Item {
            id: 2,
            group: "Special".into(),
            kind: ItemKind::Match {
                pattern: "x\\+".into(),
                contains: Vec::new(),
            },
        });
        def.clusters.insert("inside".into(), vec!["Special".into()]);
        let engine = Engine::new(def);
        let spans = spans_of(&engine, &["<xxx> <y>"]);
        assert!(spans[0].iter().any(|(t, g)| t == "<xxx>" && g == "Comment"));
        assert!(spans[0].iter().any(|(t, g)| t == "xxx" && g == "Special"));
    }

    #[test]
    fn nested_regions_override_the_container() {
        let mut def = SyntaxDef {
            name: "t".into(),
            ..SyntaxDef::default()
        };
        def.items.push(Item {
            id: 1,
            group: "Comment".into(),
            kind: ItemKind::Region {
                start: "/\\*".into(),
                end: "\\*/".into(),
                contains: vec!["String".into()],
            },
        });
        def.items.push(Item {
            id: 2,
            group: "String".into(),
            kind: ItemKind::Region {
                start: "\"".into(),
                end: "\"".into(),
                contains: Vec::new(),
            },
        });
        let engine = Engine::new(def);
        let (spans, _) = engine.line(&State::default(), "/* a \"b\" c */");
        // The string span comes after the comment span, so it wins when drawn.
        let string = spans.iter().position(|s| s.group == "String").unwrap();
        let comment = spans.iter().position(|s| s.group == "Comment").unwrap();
        assert!(comment < string, "outer region first: {spans:?}");
    }

    #[test]
    fn ignore_case_matches_uppercase_keywords() {
        let mut def = SyntaxDef {
            name: "t".into(),
            ignore_case: true,
            ..SyntaxDef::default()
        };
        def.items.push(Item {
            id: 1,
            group: "Keyword".into(),
            kind: ItemKind::Keyword {
                words: vec!["select".into()],
            },
        });
        let engine = Engine::new(def);
        let spans = spans_of(&engine, &["SELECT * FROM t"]);
        assert!(
            spans[0]
                .iter()
                .any(|(t, g)| t == "SELECT" && g == "Keyword")
        );
    }
}
