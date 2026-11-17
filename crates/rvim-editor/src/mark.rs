//! Marks (reference: mark.c).
//!
//! Local marks `a-z` are per-buffer; global marks `A-Z` and `0-9` carry a
//! buffer number so they can jump across files. Line-number adjustments when
//! text is inserted/deleted follow Vim's `mark_adjust()` (mark.c:998).

use rvim_core::Pos;
use std::collections::HashMap;

/// Vim's `MAXLNUM`: in `mark_adjust()` it means "the marked line was deleted".
/// Local marks become invalid, global ("no delete") marks collapse to the
/// first line of the changed range (mark.c:1090-1101).
pub const MAXLNUM: i64 = i64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlobalMark {
    pub bufnr: i32,
    pub pos: Pos,
}

/// A snapshot of the marks that belong to one buffer (its local marks plus
/// the global marks pointing at it). Used to restore marks on undo/redo the
/// way Vim saves `uh_namedm` in an undo header (undo.c:2886-2897).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkSnapshot {
    local: Vec<(char, Pos)>,
    global: Vec<(char, Pos)>,
}

#[derive(Debug, Default)]
pub struct MarkStore {
    local: HashMap<(i32, char), Pos>,
    global: HashMap<char, GlobalMark>,
}

impl MarkStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_local(&mut self, bufnr: i32, name: char, pos: Pos) {
        self.local.insert((bufnr, name), pos);
    }

    pub fn get_local(&self, bufnr: i32, name: char) -> Option<Pos> {
        self.local.get(&(bufnr, name)).copied()
    }

    pub fn set_global(&mut self, name: char, bufnr: i32, pos: Pos) {
        self.global.insert(name, GlobalMark { bufnr, pos });
    }

    pub fn get_global(&self, name: char) -> Option<GlobalMark> {
        self.global.get(&name).copied()
    }

    /// Resolve any mark name: local marks (a-z and the special '<', '>', '.',
    /// '^', '[' ', ']', '"' marks vim keeps per buffer) first, then global ones.
    pub fn resolve(&self, bufnr: i32, name: char) -> Option<Pos> {
        if name.is_ascii_lowercase() || matches!(name, '<' | '>' | '.' | '^' | '[' | ']' | '"') {
            self.get_local(bufnr, name)
        } else {
            self.get_global(name).map(|g| g.pos)
        }
    }

    pub fn clear_buffer(&mut self, bufnr: i32) {
        self.local.retain(|(b, _), _| *b != bufnr);
    }

    /// Vim's `mark_adjust()` (mark.c:998): adjust marks for a line-range
    /// change described with *1-based* line numbers.
    ///
    /// * `line1..=line2` is the changed range (`line2 < line1` means nothing
    ///   was removed, only inserted).
    /// * `amount` is `MAXLNUM` to delete the marked line; a local mark is
    ///   removed, a global mark collapses to `line1` (mark.c "don't delete the
    ///   line, just put at first deleted line").
    /// * `amount_after` shifts every mark below `line2`.
    pub fn adjust(
        &mut self,
        bufnr: i32,
        line1: usize,
        line2: usize,
        amount: i64,
        amount_after: i64,
    ) {
        if line2 < line1 && amount_after == 0 {
            return;
        }
        let adjust = |lnum: usize| -> Option<usize> {
            let l = lnum as i64;
            if l >= line1 as i64 && l <= line2 as i64 {
                if amount == MAXLNUM {
                    None
                } else {
                    Some((l + amount).max(1) as usize)
                }
            } else if amount_after != 0 && l > line2 as i64 {
                Some((l + amount_after).max(1) as usize)
            } else {
                Some(lnum)
            }
        };
        // nodel variant: a deleted line collapses to line1 instead of being
        // invalidated.
        let adjust_nodel = |lnum: usize| -> usize {
            let l = lnum as i64;
            if l >= line1 as i64 && l <= line2 as i64 {
                if amount == MAXLNUM {
                    line1.max(1)
                } else {
                    (l + amount).max(1) as usize
                }
            } else if amount_after != 0 && l > line2 as i64 {
                (l + amount_after).max(1) as usize
            } else {
                lnum
            }
        };
        let mut drop = Vec::new();
        for (&(b, name), pos) in self.local.iter_mut() {
            if b != bufnr {
                continue;
            }
            match adjust(pos.line + 1) {
                Some(vim_line) => pos.line = vim_line - 1,
                None => drop.push((b, name)),
            }
        }
        for key in drop {
            self.local.remove(&key);
        }
        for mark in self.global.values_mut() {
            if mark.bufnr == bufnr {
                mark.pos.line = adjust_nodel(mark.pos.line + 1) - 1;
            }
        }
    }

    /// Marks belonging to `bufnr` (local marks and globals pointing to it).
    pub fn snapshot(&self, bufnr: i32) -> MarkSnapshot {
        let mut snap = MarkSnapshot::default();
        for (&(b, name), &pos) in &self.local {
            if b == bufnr {
                snap.local.push((name, pos));
            }
        }
        for (&name, g) in &self.global {
            if g.bufnr == bufnr {
                snap.global.push((name, g.pos));
            }
        }
        snap
    }

    /// Put a snapshot back, leaving marks of other buffers untouched.
    pub fn restore(&mut self, bufnr: i32, snap: &MarkSnapshot) {
        self.local.retain(|(b, _), _| *b != bufnr);
        for &(name, pos) in &snap.local {
            self.local.insert((bufnr, name), pos);
        }
        // Globals that currently point at this buffer are restored; a global
        // that was moved to a different buffer after the snapshot is kept.
        let current: Vec<char> = self
            .global
            .iter()
            .filter(|(_, g)| g.bufnr == bufnr)
            .map(|(&n, _)| n)
            .collect();
        for name in current {
            if !snap.global.iter().any(|(n, _)| *n == name) {
                self.global.remove(&name);
            }
        }
        for &(name, pos) in &snap.global {
            self.global.insert(name, GlobalMark { bufnr, pos });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjust_moves_marks_below_a_deletion_up() {
        let mut marks = MarkStore::new();
        marks.set_local(1, 'a', Pos::new(2, 0)); // 1-based line 3
        marks.set_local(1, 'b', Pos::new(4, 0)); // 1-based line 5
        // Delete 1-based lines 1-2: mark_adjust(1, 2, MAXLNUM, -2).
        marks.adjust(1, 1, 2, MAXLNUM, -2);
        assert_eq!(marks.get_local(1, 'a'), Some(Pos::new(0, 0)));
        assert_eq!(marks.get_local(1, 'b'), Some(Pos::new(2, 0)));
    }

    #[test]
    fn adjust_invalidates_local_marks_inside_range_but_moves_globals_to_line1() {
        let mut marks = MarkStore::new();
        marks.set_local(1, 'a', Pos::new(1, 0)); // in the deleted range
        marks.set_global('A', 1, Pos::new(1, 0)); // in the deleted range
        marks.set_local(2, 'x', Pos::new(1, 0)); // other buffer untouched
        marks.adjust(1, 1, 2, MAXLNUM, -2);
        assert_eq!(marks.get_local(1, 'a'), None, "deleted local mark is gone");
        assert_eq!(marks.get_global('A').unwrap().pos, Pos::new(0, 0));
        assert_eq!(marks.get_local(2, 'x'), Some(Pos::new(1, 0)));
    }

    #[test]
    fn adjust_shifts_marks_for_an_insertion() {
        let mut marks = MarkStore::new();
        marks.set_local(1, 'a', Pos::new(1, 0)); // 1-based line 2
        // Insert two lines before 1-based line 2: mark_adjust(2, 1, MAXLNUM, 2).
        marks.adjust(1, 2, 1, MAXLNUM, 2);
        assert_eq!(marks.get_local(1, 'a'), Some(Pos::new(3, 0)));
    }

    #[test]
    fn snapshot_restore_roundtrips_only_this_buffer() {
        let mut marks = MarkStore::new();
        marks.set_local(1, 'a', Pos::new(2, 0));
        marks.set_global('B', 1, Pos::new(1, 0));
        marks.set_global('C', 2, Pos::new(0, 0));
        let snap = marks.snapshot(1);
        marks.set_local(1, 'a', Pos::new(0, 0));
        marks.adjust(1, 1, 2, MAXLNUM, -2);
        marks.restore(1, &snap);
        assert_eq!(marks.get_local(1, 'a'), Some(Pos::new(2, 0)));
        assert_eq!(marks.get_global('B').unwrap().pos, Pos::new(1, 0));
        assert_eq!(marks.get_global('C').unwrap().bufnr, 2);
    }
}
