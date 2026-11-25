//! rvim-tools: repository maintenance utilities (std-only, no dependencies).
//!
//! Subcommands:
//!   gen-cmds   regenerate `crates/rvim-ex/src/cmd_table.rs` from the vendored
//!              reference `ex_cmds.h` (589 EXCMD entries)
//!   coverage   write `docs/COVERAGE.md` from the generated table + the
//!              implemented-command list
//!   freq       derive usage weights from the vendored Vim runtime scripts and
//!              report the frequency-weighted implementation rate
//!
//! Run from anywhere; paths are resolved against the workspace root
//! (CARGO_MANIFEST_DIR/../..).

mod coverage;
mod freq;
mod gencmds;
mod genfuncs;

use std::path::PathBuf;
use std::process::ExitCode;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root();
    let result = match args.first().map(String::as_str) {
        Some("gen-cmds") => gencmds::run(&root),
        Some("gen-funcs") => genfuncs::run(&root),
        Some("coverage") => coverage::run(&root, args.iter().any(|a| a == "--check")),
        Some("missing-funcs") => coverage::report_missing(),
        Some("freq") => freq::run(&root),
        Some("freq-top") => {
            let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(40);
            freq::run_top(&root, n)
        }
        Some("help") | Some("--help") | Some("-h") | None => {
            println!(
                "rvim-tools <command>\n\n  gen-cmds   regenerate the 589-entry Ex command table\n  gen-funcs  regenerate the 543-entry builtin function table\n  \
                 coverage   write docs/COVERAGE.md (--check: fail if out of date)\n  missing-funcs  list builtins from the 543-entry denominator without an implementation\n  \
                 freq       frequency-weighted implementation report from runtime scripts"
            );
            Ok(())
        }
        Some(other) => Err(format!("unknown command: {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rvim-tools: {e}");
            ExitCode::FAILURE
        }
    }
}
