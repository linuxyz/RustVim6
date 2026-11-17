//! Error type carrying Vim-style E-codes (see src/errors.h in the reference).

use std::fmt;

/// A Vim-compatible failure. The `code` uses the canonical `Ennn` numbering
/// whenever the reference implementation defines one; otherwise an `ERV...`
/// pseudo-code is used and must be registered in docs/compat_deviations.md.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimError {
    pub code: &'static str,
    pub message: String,
    /// When true the error aborts the whole script/command sequence (Vim's
    /// `gotofail` / interrupt behaviour); ordinary command errors are recoverable.
    pub abort: bool,
}

impl VimError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            abort: false,
        }
    }

    pub fn aborting(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            abort: true,
        }
    }

    // --- frequently used constructors, codes aligned with Vim errors.h ---

    pub fn trailing_char(detail: &str) -> Self {
        Self::new("E488", format!("Trailing characters: {detail}"))
    }
    pub fn not_editor_command(name: &str) -> Self {
        Self::new("E492", format!("Not an editor command: {name}"))
    }
    pub fn unknown_option(name: &str) -> Self {
        Self::new("E518", format!("Unknown option: {name}"))
    }
    pub fn invalid_argument(detail: &str) -> Self {
        Self::new("E475", format!("Invalid argument: {detail}"))
    }
    /// E474 (errors.h): `:set` rejected a value, e.g. `fileformat=bogus`.
    pub fn invalid_option_argument(detail: &str) -> Self {
        Self::new("E474", format!("Invalid argument: {detail}"))
    }
    /// E487 (errors.h): `:set` rejected a non-positive number option.
    pub fn argument_must_be_positive(option: &str, value: i64) -> Self {
        Self::new(
            "E487",
            format!("Argument must be positive: {option}={value}"),
        )
    }
    pub fn no_range_allowed() -> Self {
        Self::new("E481", "No range allowed".to_string())
    }
    pub fn range_required() -> Self {
        Self::new("E471", "Argument required".to_string())
    }
    pub fn mark_not_set(name: char) -> Self {
        Self::new("E20", format!("Mark not set: {name}"))
    }
    pub fn no_replacement() -> Self {
        Self::new("E477", "No replacement string".to_string())
    }
    pub fn pattern_not_found(pat: &str) -> Self {
        Self::new("E486", format!("Pattern not found: {pat}"))
    }
    pub fn cannot_undo() -> Self {
        Self::new("E682", "Already at oldest change".to_string())
    }
    pub fn cannot_redo() -> Self {
        Self::new("E683", "Already at newest change".to_string())
    }
    pub fn no_write_since_change() -> Self {
        Self::new(
            "E37",
            "No write since last change (add ! to override)".to_string(),
        )
    }
    pub fn no_name() -> Self {
        Self::new("E32", "No file name".to_string())
    }
    pub fn read_only() -> Self {
        Self::new(
            "E45",
            "'readonly' option is set (add ! to override)".to_string(),
        )
    }
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::aborting(
            "ERV001",
            format!("RustVim internal error: {}", detail.into()),
        )
    }
}

impl fmt::Display for VimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for VimError {}

pub type VimResult<T> = Result<T, VimError>;
