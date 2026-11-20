//! rvim-buffer: in-memory document model for RustVim.
//!
//! Mirrors Vim's `memline.c` / `buffer.c` at the semantic level: logical lines
//! with copy-on-write sharing, a monotonic `changedtick`, line-range edits that
//! are general enough to express every mutation, and file (de)serialisation for
//! the unix/dos/mac file formats.

pub mod blowfish_tables;
pub mod buffer;
pub mod crypt;
pub mod edit;
pub mod fileio;
pub mod line;

pub use buffer::{Buffer, BufferId, FileFormat};
pub use edit::RangeEdit;
pub use line::Line;
