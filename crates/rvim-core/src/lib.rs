//! rvim-core: L1 foundation crate for RustVim.
//!
//! Zero-dependency primitives shared by every higher crate: error codes aligned
//! with Vim's `errors.h`, cursor positions, UTF-8 character helpers and character
//! classification mirroring `charset.c` / `mbyte.c` semantics.

pub mod charclass;
pub mod error;
pub mod position;
pub mod text;

pub use error::{VimError, VimResult};
pub use position::{Col, Lineno, Pos};
