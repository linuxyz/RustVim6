//! rvim-ex: Ex command parsing and execution.
//!
//! The command table mirrors the baseline `ex_cmds.h` EXCMD entries (589 in
//! total) and is generated into `cmd_table.rs`; `implemented.rs` lists the
//! subset that has real handlers. The gap between the two is exactly what the
//! coverage tool reports.

pub mod addr;
pub mod autodispatch;
pub mod cmd_table;
pub mod errorformat;
pub mod exec;
pub mod handlers_diff;
pub mod handlers_edit;
pub mod handlers_fold;
pub mod handlers_map;
pub mod handlers_normal;
pub mod handlers_qf;
pub mod handlers_script;
pub mod handlers_session;
pub mod handlers_viml;
pub mod handlers_win;
pub mod host_meta;
pub mod host_more;
pub mod host_win;
pub mod implemented;
pub mod parser;
pub mod script;
pub mod table;
pub mod version;
pub mod wildcard;

pub use exec::{ExOutcome, run_command, run_command_ctx, run_line, run_line_ctx, run_line_with};
pub use parser::{ParsedEx, Range, parse_line};
pub use table::{AddrKind, Cmd, ExFlags};
pub use version::{RVIM_VERSION, VIM_BASELINE_PATCHES, VIM_BASELINE_TAG, version_banner};
