//! rvim-undo: undo history.
//!
//! M1 provides a linear undo/redo chain built from the universal
//! `RangeEdit` records. The tree-shaped undo branch structure (`undo.c`'s
//! u_header / uhp tree, supporting `g-`/`g+` across branches) is the M5
//! extension; the API here already separates "apply inverse" from
//! "navigation" so the tree can replace the vector without touching callers.

use rvim_buffer::{Buffer, RangeEdit};
use rvim_core::{VimError, VimResult};

#[derive(Debug, Clone)]
struct UndoNode {
    /// Forward edit (re-applied on redo).
    edit: RangeEdit,
    /// Cursor position before the edit, restored on undo.
    cursor_before: rvim_core::Pos,
    /// Cursor position after the edit, restored on redo.
    cursor_after: rvim_core::Pos,
}

#[derive(Debug, Default)]
pub struct UndoHistory {
    /// Applied nodes in order.
    nodes: Vec<UndoNode>,
    /// Number of nodes reachable by undo (the redo frontier).
    head: usize,
    /// Monotonic counter surfaced as a test/debug hook (Vim's changenumber()).
    changes: u64,
}

impl UndoHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an edit that has already been applied to the buffer.
    /// Any redoable future beyond the current head is discarded, matching
    /// Vim's "new edit truncates the redo branch" rule.
    pub fn record(
        &mut self,
        edit: RangeEdit,
        cursor_before: rvim_core::Pos,
        cursor_after: rvim_core::Pos,
    ) {
        self.nodes.truncate(self.head);
        self.nodes.push(UndoNode {
            edit,
            cursor_before,
            cursor_after,
        });
        self.head = self.nodes.len();
        self.changes += 1;
    }

    pub fn can_undo(&self) -> bool {
        self.head > 0
    }

    pub fn can_redo(&self) -> bool {
        self.head < self.nodes.len()
    }

    pub fn change_count(&self) -> u64 {
        self.changes
    }

    /// Undo one change: apply the inverse edit to `buf` and return the
    /// cursor position to restore.
    pub fn undo(&mut self, buf: &mut Buffer) -> VimResult<rvim_core::Pos> {
        let idx = self.head.checked_sub(1).ok_or_else(VimError::cannot_undo)?;
        let node = self.nodes.get(idx).ok_or_else(VimError::cannot_undo)?;
        let inverse = node.edit.inverse();
        buf.apply_edit(&inverse)?;
        self.head -= 1;
        Ok(node.cursor_before)
    }

    /// Redo one change: re-apply the forward edit.
    pub fn redo(&mut self, buf: &mut Buffer) -> VimResult<rvim_core::Pos> {
        let node = self
            .nodes
            .get(self.head)
            .ok_or_else(VimError::cannot_redo)?;
        buf.apply_edit(&node.edit)?;
        self.head += 1;
        Ok(node.cursor_after)
    }

    /// Number of recorded edits (a mark for [`Self::truncate_to`]).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Drop every edit recorded since `mark`; an undo block replaces them with
    /// one synthesized step.
    pub fn truncate_to(&mut self, mark: usize) {
        if mark < self.nodes.len() {
            self.nodes.truncate(mark);
            self.head = self.nodes.len();
        }
    }
    /// Forget all history (e.g. after `:e!` reload or `u` reset).
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.head = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_core::Pos;

    #[test]
    fn undo_redo_restores_text_and_cursor() {
        let mut buf = Buffer::from_text(
            1,
            "one
two
three",
        );
        let mut hist = UndoHistory::new();
        let before = Pos::new(0, 0);
        let edit = buf.set_line_text(0, "ONE".to_string()).unwrap();
        hist.record(edit, before, Pos::new(0, 3));
        assert_eq!(
            buf.full_text(),
            "ONE
two
three"
        );

        let restored = hist.undo(&mut buf).unwrap();
        assert_eq!(
            buf.full_text(),
            "one
two
three"
        );
        assert_eq!(restored, before);

        let after = hist.redo(&mut buf).unwrap();
        assert_eq!(
            buf.full_text(),
            "ONE
two
three"
        );
        assert_eq!(after, Pos::new(0, 3));
    }

    #[test]
    fn new_edit_discards_redo_future() {
        let mut buf = Buffer::from_text(1, "abc");
        let mut hist = UndoHistory::new();
        let e1 = buf.set_line_text(0, "abcX".to_string()).unwrap();
        hist.record(e1, Pos::new(0, 0), Pos::new(0, 4));
        hist.undo(&mut buf).unwrap();
        assert!(hist.can_redo());
        let e2 = buf.set_line_text(0, "abcY".to_string()).unwrap();
        hist.record(e2, Pos::new(0, 0), Pos::new(0, 4));
        assert!(!hist.can_redo(), "redo frontier must be truncated");
        assert_eq!(buf.full_text(), "abcY");
        hist.undo(&mut buf).unwrap();
        assert_eq!(buf.full_text(), "abc");
    }

    #[test]
    fn boundaries_report_vim_errors() {
        let mut buf = Buffer::new(1);
        let mut hist = UndoHistory::new();
        assert!(hist.undo(&mut buf).is_err_and(|e| e.code == "E682"));
        let e = buf.set_line_text(0, "x".to_string()).unwrap();
        hist.record(e, Pos::new(0, 0), Pos::new(0, 1));
        hist.undo(&mut buf).unwrap();
        assert!(hist.redo(&mut buf).is_ok());
        assert!(hist.redo(&mut buf).is_err_and(|e| e.code == "E683"));
    }
}
