//! Editor mode state machine (reference: normal.c mode handling).

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Replace,
    Visual,
    VisualLine,
    VisualBlock,
    Select,
    CommandLine,
    OperatorPending,
    /// Waiting for a register name (the `"` prefix in normal mode).
    RegisterPending,
    /// Waiting for a motion after an operator (d/y/c/...).
    MotionPending {
        op: char,
        count: u64,
        reg: Option<char>,
    },
    Terminal,
}

impl Mode {
    pub fn vim_name(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Replace => "REPLACE",
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock => "VISUAL",
            Mode::Select => "SELECT",
            Mode::CommandLine => "COMMAND",
            Mode::OperatorPending | Mode::MotionPending { .. } | Mode::RegisterPending => {
                "OP-PENDING"
            }
            Mode::Terminal => "TERMINAL",
        }
    }

    pub fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine | Mode::VisualBlock)
    }

    pub fn is_insert_like(self) -> bool {
        matches!(self, Mode::Insert | Mode::Replace)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.vim_name())
    }
}
