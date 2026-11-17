//! Cursor / text positions, matching Vim's 1-based line convention at the
//! public boundary while keeping zero-based storage internally.

/// Byte column offset inside a line. Stored zero-based; Vim's 1-based
/// `col('.')` value is derived at the boundary.
pub type Col = usize;

/// Internal zero-based buffer line index.
pub type Lineno = usize;

/// A position in a buffer. `off` is reserved for virtual column offset
/// (virtualedit / double-width handling) and is zero in M1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, PartialOrd, Ord)]
pub struct Pos {
    pub line: Lineno,
    pub col: Col,
    pub off: u32,
}

impl Pos {
    pub const fn new(line: Lineno, col: Col) -> Self {
        Self { line, col, off: 0 }
    }

    /// Vim's 1-based line number.
    pub fn vim_line(self) -> usize {
        self.line + 1
    }

    /// Vim's 1-based column number.
    pub fn vim_col(self) -> usize {
        self.col + 1
    }
}
