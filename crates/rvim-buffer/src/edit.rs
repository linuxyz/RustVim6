//! The universal line-range edit. Every buffer mutation (character insertion,
//! line deletion, paste, ":s", ...) is expressed as replacing the half-open
//! range "[start, end)" with "after". This makes undo/redo a matter of swapping
//! "before" and "after" — see the rvim-undo crate.

use crate::line::Line;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeEdit {
    /// Zero-based start line index.
    pub start: usize,
    /// Zero-based exclusive end line index.
    pub end: usize,
    /// Snapshot of lines removed by the edit.
    pub before: Vec<String>,
    /// Lines inserted by the edit.
    pub after: Vec<String>,
}

impl RangeEdit {
    /// Apply this edit directly to a line vector.
    pub fn apply(&self, lines: &mut Vec<Line>) {
        lines.splice(
            self.start..self.end,
            self.after.iter().map(|t| Line::new(t.clone())),
        );
    }

    /// The inverse edit: replacing what we inserted with what was there.
    pub fn inverse(&self) -> RangeEdit {
        RangeEdit {
            start: self.start,
            end: self.start + self.after.len(),
            before: self.after.clone(),
            after: self.before.clone(),
        }
    }
}
