//! rvim-editor: aggregate editor state.
//!
//! Owns buffers, the (M1: single) window cursor, modes, options, registers,
//! marks, messages and per-buffer undo histories. Higher crates (rvim-ex,
//! rvim-normal, rvim-app) manipulate the document exclusively through this
//! crate so that undo recording, changedtick, marks and messages stay
//! consistent in one place.

pub mod abbrev;
pub mod autocmd;
pub mod diff;
pub mod editor;
pub mod fold;
pub mod keynames;
pub mod maps;
pub mod mark;
pub mod message;
pub mod mode;
pub mod options;
pub mod quickfix;
pub mod register;
pub mod usercmd;
pub mod viminfo;
pub mod window;

pub use abbrev::{AbbrevEntry, AbbrevMode, AbbrevStore};
pub use autocmd::{AutoCmd, AutocmdStore, EVENTS};
pub use editor::{
    BlockInsert, ChangePos, Editor, JumpPos, LastSubstitute, RepeatChange, SetScope, VisualState,
};
pub use maps::{Key, MapEntry, MapMode, MapOptions, MapStore};
pub use mark::MarkStore;
pub use message::MessageLog;
pub use mode::Mode;
pub use options::{OptValue, Options};
pub use register::{RegType, Register, RegisterStore};
pub use usercmd::{Nargs, UserCommand, UserCommandStore};
pub use window::{Dir, Layout, Rect, TabPage, TabpageId, Window, WindowId, Windows};
