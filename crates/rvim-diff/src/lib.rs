//! rvim-diff: line diff engine (alignment of Vim's `src/xdiff`).
//!
//! Vim's `:diffthis` and `vimdiff` need a line level diff between two buffers.
//! The C implementation lives in `src/xdiff/xdiffi.c`; this crate provides the
//! same observable results - a set of changed blocks forming a shortest edit
//! script - with std only.
//!
//! The editor consumes [`change_blocks`]: for each changed block it knows which
//! lines of buffer A were deleted/changed and how many filler lines buffer A
//! must show to keep both views aligned. [`hunks`] and [`unified`] build the
//! familiar `@@ ... @@` view on top of the same blocks.
//!
//! # Algorithm
//!
//! Lines are interned to `usize` ids, the common prefix and suffix are trimmed,
//! and the remaining window is diffed with Myers' O(ND) greedy algorithm
//! ("An O(ND) Difference Algorithm and its Variations", 1986). The forward
//! pass keeps one furthest-reaching x value per diagonal and snapshots the
//! diagonals of each depth for backtracking. With `N = a.len()`,
//! `M = b.len()` and edit distance `D`, that is O((N + M) * D) time and
//! O(D^2) memory; since the search depth is capped at `max(256, isqrt(N + M))`,
//! memory remains O(N + M) in the worst case and no N*M table is ever
//! allocated.
//!
//! If the edit distance exceeds that cap the window falls back to a single
//! substitution block (the whole trimmed middle of A replaced by the whole
//! trimmed middle of B). The result is still a valid script and still applies
//! to produce B, but it is no longer guaranteed to be minimal. This keeps
//! completely unrelated inputs (where D is close to N + M) from taking
//! quadratic time.
//!
//! # Sliding
//!
//! After the shortest script is known, change groups are slid as far down as
//! possible, like xdiff's `xdl_change_compact`. A group is the pair of ranges
//! `[a_start, a_start + a_len)` in A and `[b_start, b_start + b_len)` in B.
//! One slide moves both starts one line forward and is allowed while
//!
//! * both groups still have a line after them:
//!   `a_start + a_len < a.len()` and `b_start + b_len < b.len()`, and
//! * `a[a_start] == b[b_start]`.
//!
//! The equality is the joint form of xdiff's `group_slide_down()`: for a pure
//! insertion it says the first inserted line equals the line the insertion is
//! moving past; for a pure deletion it says the first deleted line equals the
//! line after the group (which is `b[b_start]` because B has no changed
//! lines there); for a replacement it is exactly the condition that lets the
//! first deleted and first inserted lines become a matching context pair.
//! Sliding continues until no further slide is possible. If a group catches up
//! with the following group, the two are merged (so the group grows) and
//! sliding resumes, which is why adjacent changed blocks never survive.
//!
//! # Deviations from xdiff
//!
//! * The multi-match record cleanup (`xdl_cleanup_records`) and the indentation
//!   heuristic (`XDF_INDENT_HEURISTIC`) are not implemented. They only choose among
//!   equally short scripts, so they cannot change the edit distance; they can
//!   pick a different placement for ambiguous runs of duplicate lines.
//! * xdiff slides every group up first and then down, pulling it back up when
//!   that aligns it with a group of changes in the other file. We only slide
//!   down. For a pure insertion or deletion the other file's group is empty,
//!   so xdiff also settles at the lowest position; a replacement produced by
//!   the minimal search never satisfies the slide condition (matching its two
//!   first lines would shorten the script), so both implementations agree
//!   there too.
//! * The capped fallback above trades minimality for a hard time/memory bound.
//!   xdiff instead splits the search box around a heuristic "good enough"
//!   point, so it can still return a mostly minimal script for huge inputs.

use std::collections::HashMap;

/// One changed block: `a_len` lines starting at `a_start` in A replaced by
/// `b_len` lines starting at `b_start` in B (both zero-based, half-open).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeBlock {
    pub a_start: usize,
    pub a_len: usize,
    pub b_start: usize,
    pub b_len: usize,
}

/// One line of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Delete,
    Insert,
}

/// One hunk of a unified diff (`@@ -a_start,a_len +b_start,b_len @@`).
///
/// `a_start`/`b_start` are zero-based indexes; `a_len`/`b_len` are the
/// half-open lengths covered by the hunk including context. The first hunk
/// line is in `lines`, in file order (deletions before insertions inside
/// one changed block).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub a_start: usize,
    pub a_len: usize,
    pub b_start: usize,
    pub b_len: usize,
    pub lines: Vec<DiffLine>,
}

/// Shortest-edit-script changed blocks between `a` and `b`.
///
/// Blocks are returned in ascending file order, never overlap and are always
/// separated by at least one unchanged line. Identical inputs return an empty
/// vector.
pub fn change_blocks(a: &[String], b: &[String]) -> Vec<ChangeBlock> {
    let (a_ids, b_ids) = intern(a, b);
    let mut blocks = diff_ids(&a_ids, &b_ids);
    slide_blocks(&mut blocks, &a_ids, &b_ids);
    blocks
}

/// Hunks with `context` lines of unchanged context around every change.
///
/// Changes whose unchanged gap is at most `2 * context` lines are grouped into
/// one hunk; a larger gap starts a new hunk. Context is clipped at the buffer
/// edges. Identical inputs return an empty vector.
pub fn hunks(a: &[String], b: &[String], context: usize) -> Vec<Hunk> {
    let blocks = change_blocks(a, b);
    if blocks.is_empty() {
        return Vec::new();
    }

    let mut hunks = Vec::new();
    let mut first = 0;
    while first < blocks.len() {
        let mut last = first;
        while last + 1 < blocks.len() {
            let previous = blocks[last];
            let next = blocks[last + 1];
            let gap = next.a_start - (previous.a_start + previous.a_len);
            if gap > context.saturating_mul(2) {
                break;
            }
            last += 1;
        }

        let before_limit = if first == 0 {
            blocks[first].a_start
        } else {
            blocks[first].a_start - (blocks[first - 1].a_start + blocks[first - 1].a_len)
        };
        let after_limit = if last + 1 == blocks.len() {
            a.len() - (blocks[last].a_start + blocks[last].a_len)
        } else {
            blocks[last + 1].a_start - (blocks[last].a_start + blocks[last].a_len)
        };
        let before = context.min(before_limit);
        let after = context.min(after_limit);
        hunks.push(build_hunk(&blocks[first..=last], a, b, before, after));

        first = last + 1;
    }
    hunks
}

/// Unified diff text (as `diff -u` / `xdiff` produce it).
///
/// The result starts with the `--- a_name` / `+++ b_name` header lines and
/// contains one `@@` hunk per [`Hunk`]. Every line ends in a newline. An
/// empty diff returns the empty string (not the two header lines), matching
/// [`hunks`] and Vim's "no differences" case.
pub fn unified(a_name: &str, b_name: &str, a: &[String], b: &[String], context: usize) -> String {
    let hunks = hunks(a, b, context);
    if hunks.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("--- ");
    out.push_str(a_name);
    out.push('\n');
    out.push_str("+++ ");
    out.push_str(b_name);
    out.push('\n');

    for hunk in hunks {
        out.push_str("@@ -");
        out.push_str(&fmt_range(hunk.a_start, hunk.a_len));
        out.push_str(" +");
        out.push_str(&fmt_range(hunk.b_start, hunk.b_len));
        out.push_str(" @@\n");
        for line in &hunk.lines {
            out.push(match line.kind {
                LineKind::Context => ' ',
                LineKind::Delete => '-',
                LineKind::Insert => '+',
            });
            out.push_str(&line.text);
            out.push('\n');
        }
    }
    out
}

/// GNU unified header range: 1-based start, with `count == 1` omitting the count
/// and `count == 0` printing `start - 1` (i.e. our zero-based insertion
/// index).
fn fmt_range(start: usize, len: usize) -> String {
    match len {
        0 => format!("{start},0"),
        1 => (start + 1).to_string(),
        _ => format!("{},{len}", start + 1),
    }
}

/// Myers-style edit operation over the trimmed window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Give every distinct line an id so the diff only compares `usize` values.
fn intern(a: &[String], b: &[String]) -> (Vec<usize>, Vec<usize>) {
    let mut ids: HashMap<&str, usize> = HashMap::with_capacity(a.len() + b.len());
    let mut a_ids = Vec::with_capacity(a.len());
    for line in a {
        let next = ids.len();
        a_ids.push(*ids.entry(line.as_str()).or_insert(next));
    }
    let mut b_ids = Vec::with_capacity(b.len());
    for line in b {
        let next = ids.len();
        b_ids.push(*ids.entry(line.as_str()).or_insert(next));
    }
    (a_ids, b_ids)
}

/// Diff two interned line buffers and return the changed blocks.
fn diff_ids(a: &[usize], b: &[usize]) -> Vec<ChangeBlock> {
    let n = a.len();
    let m = b.len();

    let mut prefix = 0;
    while prefix < n && prefix < m && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let max_suffix = n.min(m) - prefix;
    let mut suffix = 0;
    while suffix < max_suffix && a[n - 1 - suffix] == b[m - 1 - suffix] {
        suffix += 1;
    }

    let a_middle = &a[prefix..n - suffix];
    let b_middle = &b[prefix..m - suffix];

    if a_middle.is_empty() && b_middle.is_empty() {
        return Vec::new();
    }
    if a_middle.is_empty() {
        return vec![ChangeBlock {
            a_start: prefix,
            a_len: 0,
            b_start: prefix,
            b_len: b_middle.len(),
        }];
    }
    if b_middle.is_empty() {
        return vec![ChangeBlock {
            a_start: prefix,
            a_len: a_middle.len(),
            b_start: prefix,
            b_len: 0,
        }];
    }

    let max_depth = isqrt(a_middle.len().saturating_add(b_middle.len())).max(256);
    match myers(a_middle, b_middle, max_depth) {
        Some(ops) => blocks_from_ops(&ops, prefix),
        // Too expensive: replace the whole trimmed middle. Still a valid script,
        // just not guaranteed to be minimal.
        None => vec![ChangeBlock {
            a_start: prefix,
            a_len: a_middle.len(),
            b_start: prefix,
            b_len: b_middle.len(),
        }],
    }
}

/// Turn an edit operation stream into ascending changed blocks.
fn blocks_from_ops(ops: &[Op], offset: usize) -> Vec<ChangeBlock> {
    let mut blocks = Vec::new();
    let mut a_pos = offset;
    let mut b_pos = offset;
    let mut index = 0;

    while index < ops.len() {
        if ops[index] == Op::Equal {
            a_pos += 1;
            b_pos += 1;
            index += 1;
            continue;
        }

        let a_start = a_pos;
        let b_start = b_pos;
        while index < ops.len() && ops[index] != Op::Equal {
            match ops[index] {
                Op::Delete => a_pos += 1,
                Op::Insert => b_pos += 1,
                Op::Equal => unreachable!("loop condition excludes Equal"),
            }
            index += 1;
        }
        blocks.push(ChangeBlock {
            a_start,
            a_len: a_pos - a_start,
            b_start,
            b_len: b_pos - b_start,
        });
    }
    blocks
}

/// Greedy Myers O(ND) with a trace for backtracking.
///
/// Returns `None` when the edit distance exceeds `max_depth`.
fn myers(a: &[usize], b: &[usize], max_depth: usize) -> Option<Vec<Op>> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let max_depth_i = max_depth as isize;
    let offset = max_depth_i + 1;
    let mut v = vec![-1i32; max_depth.saturating_mul(2).saturating_add(3)];
    v[(offset + 1) as usize] = 0;

    let mut trace: Vec<Vec<i32>> = Vec::new();
    let mut found = None;

    for depth in 0..=max_depth {
        let depth_i = depth as isize;
        let mut snapshot = Vec::with_capacity(depth + 1);
        let mut k = -depth_i;
        while k <= depth_i {
            let index = (k + offset) as usize;
            let left = v[(k - 1 + offset) as usize];
            let right = v[(k + 1 + offset) as usize];
            let start = if k == -depth_i || (k != depth_i && left < right) {
                right
            } else {
                left + 1
            };
            let mut x = start as isize;
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[index] = x as i32;
            snapshot.push(x as i32);
            if x >= n && y >= m {
                found = Some(depth);
                break;
            }
            k += 2;
        }
        trace.push(snapshot);
        if found.is_some() {
            break;
        }
    }

    let shortest = found?;
    let mut ops = Vec::with_capacity((n + m) as usize);
    let mut x = n;
    let mut y = m;
    let mut depth = shortest;

    while depth > 0 {
        let previous_depth = depth - 1;
        let k = x - y;
        let depth_i = depth as isize;
        let previous_k = if k == -depth_i
            || (k != depth_i
                && trace_x(&trace, previous_depth, k - 1) < trace_x(&trace, previous_depth, k + 1))
        {
            k + 1
        } else {
            k - 1
        };
        let previous_x = trace_x(&trace, previous_depth, previous_k);
        debug_assert!(previous_x >= 0, "backtracked diagonal was never reached");
        let previous_y = previous_x - previous_k;

        while x > previous_x && y > previous_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if x == previous_x {
            ops.push(Op::Insert);
            y -= 1;
        } else {
            ops.push(Op::Delete);
            x -= 1;
        }
        depth = previous_depth;
    }

    while x > 0 && y > 0 {
        ops.push(Op::Equal);
        x -= 1;
        y -= 1;
    }
    while x > 0 {
        ops.push(Op::Delete);
        x -= 1;
    }
    while y > 0 {
        ops.push(Op::Insert);
        y -= 1;
    }
    ops.reverse();
    Some(ops)
}

/// Furthest-reaching x value for diagonal `k` at `depth`, or -1 if the
/// diagonal is outside the stored diamond.
fn trace_x(trace: &[Vec<i32>], depth: usize, k: isize) -> isize {
    let depth_i = depth as isize;
    if k < -depth_i || k > depth_i {
        return -1;
    }
    debug_assert_eq!((k + depth_i) % 2, 0, "diagonal has the wrong parity");
    trace[depth][((k + depth_i) / 2) as usize] as isize
}

/// Slide every changed block as far down as possible, merging blocks that touch.
fn slide_blocks(blocks: &mut Vec<ChangeBlock>, a: &[usize], b: &[usize]) {
    let mut index = 0;
    while index < blocks.len() {
        loop {
            if !can_slide_down(blocks[index], a, b) {
                break;
            }
            blocks[index].a_start += 1;
            blocks[index].b_start += 1;

            if index + 1 < blocks.len() {
                let current = blocks[index];
                let next = blocks[index + 1];
                if current.a_start + current.a_len == next.a_start
                    && current.b_start + current.b_len == next.b_start
                {
                    blocks[index].a_len = next.a_start + next.a_len - current.a_start;
                    blocks[index].b_len = next.b_start + next.b_len - current.b_start;
                    blocks.remove(index + 1);
                }
            }
        }
        index += 1;
    }
}

/// Whether one block can move down one line while keeping its size and the
/// alignment between the unchanged context in A and B.
fn can_slide_down(block: ChangeBlock, a: &[usize], b: &[usize]) -> bool {
    block.a_start + block.a_len < a.len()
        && block.b_start + block.b_len < b.len()
        && a[block.a_start] == b[block.b_start]
}

/// Build one hunk from a run of blocks, with `before`/`after` context lines.
fn build_hunk(
    blocks: &[ChangeBlock],
    a: &[String],
    b: &[String],
    before: usize,
    after: usize,
) -> Hunk {
    debug_assert!(
        !blocks.is_empty(),
        "a hunk needs at least one changed block"
    );
    let first = blocks[0];
    let last = blocks[blocks.len() - 1];
    let a_start = first.a_start - before;
    let b_start = first.b_start - before;
    let a_end = last.a_start + last.a_len + after;
    let b_end = last.b_start + last.b_len + after;

    let mut lines = Vec::new();
    let mut a_pos = a_start;
    let mut b_pos = b_start;

    for block in blocks {
        while a_pos < block.a_start {
            debug_assert_eq!(a[a_pos], b[b_pos], "context lines must match");
            lines.push(DiffLine {
                kind: LineKind::Context,
                text: a[a_pos].clone(),
            });
            a_pos += 1;
            b_pos += 1;
        }
        for text in &a[block.a_start..block.a_start + block.a_len] {
            lines.push(DiffLine {
                kind: LineKind::Delete,
                text: text.clone(),
            });
        }
        for text in &b[block.b_start..block.b_start + block.b_len] {
            lines.push(DiffLine {
                kind: LineKind::Insert,
                text: text.clone(),
            });
        }
        a_pos = block.a_start + block.a_len;
        b_pos = block.b_start + block.b_len;
    }

    while a_pos < a_end {
        debug_assert_eq!(a[a_pos], b[b_pos], "trailing context lines must match");
        lines.push(DiffLine {
            kind: LineKind::Context,
            text: a[a_pos].clone(),
        });
        a_pos += 1;
        b_pos += 1;
    }
    debug_assert_eq!(a_pos, a_end);
    debug_assert_eq!(b_pos, b_end);

    Hunk {
        a_start,
        a_len: a_end - a_start,
        b_start,
        b_len: b_end - b_start,
        lines,
    }
}

/// Integer square root (floor), used to size the Myers search bound.
fn isqrt(n: usize) -> usize {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn block(a_start: usize, a_len: usize, b_start: usize, b_len: usize) -> ChangeBlock {
        ChangeBlock {
            a_start,
            a_len,
            b_start,
            b_len,
        }
    }

    /// Rebuild B from A by replacing every block's A range with its B range.
    /// This is the inverse application of the change script and checks that
    /// the blocks agree about the unchanged context between them.
    fn apply_blocks(a: &[String], b: &[String], blocks: &[ChangeBlock]) -> Vec<String> {
        let mut out = Vec::with_capacity(b.len());
        let mut a_pos = 0;
        let mut b_pos = 0;
        for (index, block) in blocks.iter().enumerate() {
            assert!(block.a_start >= a_pos, "block {index} overlaps in A");
            assert!(block.b_start >= b_pos, "block {index} overlaps in B");
            assert_eq!(
                block.a_start - a_pos,
                block.b_start - b_pos,
                "block {index} context count mismatch"
            );
            for offset in 0..block.a_start - a_pos {
                assert_eq!(a[a_pos + offset], b[b_pos + offset], "context mismatch");
                out.push(a[a_pos + offset].clone());
            }
            for offset in 0..block.b_len {
                out.push(b[block.b_start + offset].clone());
            }
            a_pos = block.a_start + block.a_len;
            b_pos = block.b_start + block.b_len;
        }
        assert_eq!(
            a.len() - a_pos,
            b.len() - b_pos,
            "trailing context mismatch"
        );
        for offset in 0..a.len() - a_pos {
            assert_eq!(a[a_pos + offset], b[b_pos + offset], "trailing mismatch");
            out.push(a[a_pos + offset].clone());
        }
        out
    }

    /// Small deterministic linear congruential generator for the property test.
    struct Lcg(u64);

    impl Lcg {
        fn next_u32(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32
        }

        fn below(&mut self, limit: usize) -> usize {
            (self.next_u32() as usize) % limit
        }
    }

    /// Classic O(n * m) LCS length, independent of the Myers implementation.
    fn lcs_length(a: &[String], b: &[String]) -> usize {
        let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for (i, a_line) in a.iter().enumerate() {
            for (j, b_line) in b.iter().enumerate() {
                table[i + 1][j + 1] = if a_line == b_line {
                    table[i][j] + 1
                } else {
                    table[i + 1][j].max(table[i][j + 1])
                };
            }
        }
        table[a.len()][b.len()]
    }

    #[test]
    fn identical_is_empty() {
        let a = lines(&["a", "b", "c"]);
        assert!(change_blocks(&a, &a).is_empty());
        assert!(hunks(&a, &a, 3).is_empty());
        assert_eq!(unified("a", "b", &a, &a, 3), "");
    }

    #[test]
    fn both_empty_is_empty() {
        assert!(change_blocks(&[], &[]).is_empty());
        assert!(hunks(&[], &[], 3).is_empty());
    }

    #[test]
    fn empty_a_inserts_everything() {
        let b = lines(&["x", "y"]);
        assert_eq!(change_blocks(&[], &b), vec![block(0, 0, 0, 2)]);
    }

    #[test]
    fn empty_b_deletes_everything() {
        let a = lines(&["x", "y"]);
        assert_eq!(change_blocks(&a, &[]), vec![block(0, 2, 0, 0)]);
    }

    #[test]
    fn insert_at_start_middle_and_end() {
        let a = lines(&["a", "b"]);

        let start = lines(&["x", "a", "b"]);
        assert_eq!(change_blocks(&a, &start), vec![block(0, 0, 0, 1)]);

        let middle = lines(&["a", "x", "b"]);
        assert_eq!(change_blocks(&a, &middle), vec![block(1, 0, 1, 1)]);

        let end = lines(&["a", "b", "x"]);
        assert_eq!(change_blocks(&a, &end), vec![block(2, 0, 2, 1)]);
    }

    #[test]
    fn delete_at_start_middle_and_end() {
        let b = lines(&["a", "b"]);

        let start = lines(&["x", "a", "b"]);
        assert_eq!(change_blocks(&start, &b), vec![block(0, 1, 0, 0)]);

        let middle = lines(&["a", "x", "b"]);
        assert_eq!(change_blocks(&middle, &b), vec![block(1, 1, 1, 0)]);

        let end = lines(&["a", "b", "x"]);
        assert_eq!(change_blocks(&end, &b), vec![block(2, 1, 2, 0)]);
    }

    #[test]
    fn replacement_is_one_merged_block() {
        let a = lines(&["a", "b", "c"]);
        let b = lines(&["a", "x", "y", "c"]);
        assert_eq!(change_blocks(&a, &b), vec![block(1, 1, 1, 2)]);
    }

    #[test]
    fn insertion_slides_down() {
        // "b" is inserted before "a"; the second inserted line equals the "r"
        // that follows, so the insertion can slide past it to the end.
        let a = lines(&["p", "a", "r"]);
        let b = lines(&["p", "b", "a", "r", "r"]);
        assert_eq!(
            change_blocks(&a, &b),
            vec![block(1, 0, 1, 1), block(3, 0, 4, 1)]
        );
    }

    #[test]
    fn deletion_slides_down() {
        // Two identical "x" lines; the deleted one slides past the first "x"
        // and stops before "r".
        let a = lines(&["p", "a", "x", "x", "r"]);
        let b = lines(&["p", "b", "a", "x", "r"]);
        assert_eq!(
            change_blocks(&a, &b),
            vec![block(1, 0, 1, 1), block(3, 1, 4, 0)]
        );
    }

    #[test]
    fn hunks_are_separated_when_far_apart() {
        let a: Vec<String> = (0..10).map(|index| format!("line {index}")).collect();
        let mut b = a.clone();
        b[1] = "changed 1".to_string();
        b[8] = "changed 8".to_string();

        let hunks = hunks(&a, &b, 1);
        assert_eq!(hunks.len(), 2);
        assert_eq!((hunks[0].a_start, hunks[0].a_len), (0, 3));
        assert_eq!((hunks[1].a_start, hunks[1].a_len), (7, 3));
        assert_eq!((hunks[0].b_start, hunks[0].b_len), (0, 3));
        assert_eq!((hunks[1].b_start, hunks[1].b_len), (7, 3));
    }

    #[test]
    fn hunk_split_uses_two_context_boundary() {
        let a: Vec<String> = (0..10).map(|index| format!("line {index}")).collect();

        // Gap of 2 == 2 * context: one hunk.
        let mut close = a.clone();
        close[2] = "changed 2".to_string();
        close[5] = "changed 5".to_string();
        assert_eq!(hunks(&a, &close, 1).len(), 1);

        // Gap of 3 > 2 * context: two hunks.
        let mut far = a.clone();
        far[2] = "changed 2".to_string();
        far[6] = "changed 6".to_string();
        assert_eq!(hunks(&a, &far, 1).len(), 2);
    }

    #[test]
    fn hunk_context_is_clipped_at_both_edges() {
        let a = lines(&["0", "1", "2", "3", "4"]);

        let mut start = a.clone();
        start[0] = "zero".to_string();
        let start_hunks = hunks(&a, &start, 2);
        assert_eq!(start_hunks.len(), 1);
        assert_eq!((start_hunks[0].a_start, start_hunks[0].a_len), (0, 3));
        assert_eq!(start_hunks[0].lines.len(), 4);

        let mut end = a.clone();
        end[4] = "four".to_string();
        let end_hunks = hunks(&a, &end, 2);
        assert_eq!(end_hunks.len(), 1);
        assert_eq!((end_hunks[0].a_start, end_hunks[0].a_len), (2, 3));

        let mut both = a.clone();
        both[0] = "zero".to_string();
        both[4] = "four".to_string();
        let both_hunks = hunks(&a, &both, 2);
        assert_eq!(both_hunks.len(), 1);
        assert_eq!(both_hunks[0].a_start, 0);
        assert_eq!(both_hunks[0].a_len, 5);
        assert_eq!(both_hunks[0].b_start, 0);
        assert_eq!(both_hunks[0].b_len, 5);
    }

    #[test]
    fn unified_matches_literal_expected() {
        let a = lines(&["one", "two", "three"]);
        let b = lines(&["one", "TWO", "three"]);
        let expected = r"--- a.txt
+++ b.txt
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
        assert_eq!(unified("a.txt", "b.txt", &a, &b, 3), expected);
    }

    #[test]
    fn unified_header_zero_and_one_counts() {
        let b = lines(&["x"]);
        let expected_insert = r"--- a
+++ b
@@ -0,0 +1 @@
+x
";
        assert_eq!(unified("a", "b", &[], &b, 3), expected_insert);

        let a = lines(&["x"]);
        let expected_delete = r"--- a
+++ b
@@ -1 +0,0 @@
-x
";
        assert_eq!(unified("a", "b", &a, &[], 3), expected_delete);
    }

    #[test]
    fn randomized_blocks_round_trip() {
        const ALPHABET: [&str; 4] = ["a", "b", "c", "d"];
        let mut rng = Lcg(0x5eed_1234_abcd_9876);

        for case in 0..200 {
            let a_len = rng.below(31);
            let b_len = rng.below(31);
            let a: Vec<String> = (0..a_len)
                .map(|_| ALPHABET[rng.below(ALPHABET.len())].to_string())
                .collect();
            let b: Vec<String> = (0..b_len)
                .map(|_| ALPHABET[rng.below(ALPHABET.len())].to_string())
                .collect();

            let blocks = change_blocks(&a, &b);
            let mut previous_a_end = 0;
            let mut previous_b_end = 0;
            for (index, block) in blocks.iter().enumerate() {
                assert!(
                    block.a_len > 0 || block.b_len > 0,
                    "case {case}: empty block"
                );
                assert!(
                    block.a_start + block.a_len <= a.len(),
                    "case {case}: A range out of bounds"
                );
                assert!(
                    block.b_start + block.b_len <= b.len(),
                    "case {case}: B range out of bounds"
                );
                if index == 0 {
                    assert_eq!(block.a_start, block.b_start, "case {case}: first offset");
                } else {
                    assert!(
                        block.a_start > previous_a_end,
                        "case {case}: blocks are not separated"
                    );
                    assert_eq!(
                        block.a_start - previous_a_end,
                        block.b_start - previous_b_end,
                        "case {case}: context count mismatch"
                    );
                }
                previous_a_end = block.a_start + block.a_len;
                previous_b_end = block.b_start + block.b_len;
            }

            let rebuilt = apply_blocks(&a, &b, &blocks);
            assert_eq!(rebuilt, b, "case {case}: a={a:?} b={b:?} blocks={blocks:?}");

            let edit_distance: usize = blocks.iter().map(|block| block.a_len + block.b_len).sum();
            assert_eq!(
                edit_distance,
                a.len() + b.len() - 2 * lcs_length(&a, &b),
                "case {case}: not a shortest script"
            );

            let context = rng.below(5);
            for hunk in hunks(&a, &b, context) {
                assert!(
                    hunk.a_start + hunk.a_len <= a.len(),
                    "case {case}: hunk A range out of bounds"
                );
                assert!(
                    hunk.b_start + hunk.b_len <= b.len(),
                    "case {case}: hunk B range out of bounds"
                );
                let mut a_text = Vec::new();
                let mut b_text = Vec::new();
                for line in &hunk.lines {
                    match line.kind {
                        LineKind::Context => {
                            a_text.push(line.text.clone());
                            b_text.push(line.text.clone());
                        }
                        LineKind::Delete => a_text.push(line.text.clone()),
                        LineKind::Insert => b_text.push(line.text.clone()),
                    }
                }
                assert_eq!(
                    a_text.as_slice(),
                    &a[hunk.a_start..hunk.a_start + hunk.a_len],
                    "case {case}: hunk A text"
                );
                assert_eq!(
                    b_text.as_slice(),
                    &b[hunk.b_start..hunk.b_start + hunk.b_len],
                    "case {case}: hunk B text"
                );
            }
        }
    }

    #[test]
    fn large_mostly_equal_input_is_fast() {
        let a: Vec<String> = (0..2000).map(|index| format!("line {index}")).collect();
        let mut b = a.clone();
        b[500] = "changed 500".to_string();
        b[1000] = "changed 1000".to_string();
        b[1500] = "changed 1500".to_string();

        let started = std::time::Instant::now();
        let blocks = change_blocks(&a, &b);
        let elapsed = started.elapsed();

        assert_eq!(
            blocks,
            vec![
                block(500, 1, 500, 1),
                block(1000, 1, 1000, 1),
                block(1500, 1, 1500, 1),
            ]
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "diff took too long: {elapsed:?}"
        );
    }

    #[test]
    fn exact_myers_below_depth_cap_stays_minimal() {
        // 30 common lines, 240 edits: below the 256 depth cap, so the result
        // must keep every match instead of collapsing into one replacement.
        let a: Vec<String> = (0..150).map(|index| format!("a{index}")).collect();
        let mut b: Vec<String> = (0..150).map(|index| format!("b{index}")).collect();
        for index in (0..150).step_by(5) {
            b[index] = a[index].clone();
        }

        let blocks = change_blocks(&a, &b);
        assert_eq!(blocks.len(), 30);
        assert_eq!(blocks[0], block(1, 4, 1, 4));
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.a_len + block.b_len)
                .sum::<usize>(),
            240
        );
    }

    #[test]
    fn dissimilar_large_input_uses_bounded_fallback() {
        let a: Vec<String> = (0..1000).map(|index| format!("a{index}")).collect();
        let b: Vec<String> = (0..1000).map(|index| format!("b{index}")).collect();

        let started = std::time::Instant::now();
        let blocks = change_blocks(&a, &b);
        let elapsed = started.elapsed();

        assert_eq!(blocks, vec![block(0, 1000, 0, 1000)]);
        assert!(
            elapsed < Duration::from_secs(5),
            "fallback took too long: {elapsed:?}"
        );
    }
}
