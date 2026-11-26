//! rvim-app: headless session orchestration plus the M4 interactive TUI driver.
//!
//! Owns the Editor plus a normal-mode engine and runs Ex command sequences for
//! the silent batch mode ("-es"), which is the primary channel for tests and
//! for Vim compatibility diffing. [`tui::Tui`] drives the same session from
//! decoded keys for interactive use.

pub mod tui;

use rvim_core::VimResult;
use rvim_editor::{Editor, OptValue};
use rvim_ex::script::Ctx;
use rvim_ex::{ExOutcome, run_line_ctx};
use rvim_normal::NormalEngine;

/// Result of a batch session, mapped to a process exit code by the binary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionReport {
    pub quit: bool,
    pub exit_code: i32,
    pub error_count: usize,
}

pub struct Session {
    pub editor: Editor,
    pub normal: NormalEngine,
    pub report: SessionReport,
    /// VimL variables and user functions (M3).
    pub viml: Ctx,
}

/// Locate a relocatable runtime beside an executable, including help-only trees.
pub fn runtime_near_executable(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = exe.parent()?;
    for candidate in [
        dir.join("runtime"),
        dir.join("../runtime"),
        dir.join("../share/vim/runtime"),
    ] {
        if candidate.join("filetype.vim").is_file()
            || candidate.join("syntax").is_dir()
            || candidate.join("doc/help.txt").is_file()
        {
            if let Ok(path) = candidate.canonicalize() {
                return Some(path);
            }
        }
    }
    None
}

/// Explicit runtime overrides, then the installed runtime, then the development
/// checkout. Installed packages do not depend on the working directory.
pub fn default_runtimepath() -> String {
    for name in ["RVIM_RUNTIME", "VIMRUNTIME"] {
        if let Ok(path) = std::env::var(name) {
            if !path.trim().is_empty() {
                return path;
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(path) = runtime_near_executable(&exe) {
            return path.display().to_string();
        }
    }
    let vendored =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/vim/runtime");
    vendored
        .canonicalize()
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

impl Session {
    pub fn new() -> Self {
        let mut editor = Editor::new();
        let rtp = default_runtimepath();
        if !rtp.is_empty() {
            let _ = editor.options.set("runtimepath", OptValue::Str(rtp));
        }
        Self {
            editor,
            normal: NormalEngine::new(),
            report: SessionReport::default(),
            viml: Ctx::new(),
        }
    }

    /// Execute one Ex line (may contain bars). Errors are recorded, surfaced via
    /// the message log, and turn into a non-zero exit code at session end —
    /// matching "vim -es" behaviour for unattended scripts.
    pub fn ex(&mut self, line: &str) -> VimResult<ExOutcome> {
        let result = match run_line_ctx(&mut self.editor, &mut self.viml, line) {
            Ok(outcome) => {
                if let ExOutcome::Quit { exit_code, .. } | ExOutcome::QuitAll { exit_code, .. } =
                    outcome
                {
                    self.report.quit = true;
                    self.report.exit_code = exit_code;
                }
                Ok(outcome)
            }
            Err(e) => {
                self.editor.emsg(e.to_string());
                Err(e)
            }
        };
        self.sync_errors();
        result
    }

    /// Copy the editor-wide error count into the session report.
    ///
    /// Errors raised inside sourced scripts never surface as `Err` from
    /// [`Session::ex`] (legacy VimL keeps running), so the counter lives on the
    /// `Editor` — where `emsg()` bumps it — and is mirrored here before the exit
    /// code is computed.
    pub fn sync_errors(&mut self) {
        self.report.error_count = self.editor.error_count;
    }

    /// Record a session-level error that has no Ex command behind it (bad CLI
    /// argument, unreadable file), keeping the exit code in sync with `emsg()`.
    pub fn record_error(&mut self, message: impl Into<String>) {
        self.editor.emsg(message);
        self.sync_errors();
    }

    /// Count an error whose message was already reported elsewhere (stderr).
    pub fn count_error(&mut self) {
        self.editor.error_count += 1;
        self.sync_errors();
    }

    /// Run a sequence of Ex commands, stopping at a quit.
    pub fn run_script(&mut self, lines: &[String]) -> SessionReport {
        for line in lines {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('"') {
                continue;
            }
            match self.ex(trimmed) {
                Ok(ExOutcome::Continue) | Ok(ExOutcome::Finish) => {}
                Ok(_) => break,
                Err(_) => continue,
            }
        }
        self.report.clone()
    }

    /// Source a script file the way `:source` does (`:finish` stops that file).
    pub fn source(&mut self, path: &std::path::Path) -> VimResult<ExOutcome> {
        let result = match rvim_ex::handlers_script::source_file(
            &mut self.editor,
            &mut self.viml.env,
            &mut self.viml.state,
            path,
        ) {
            Ok(outcome) => {
                // ":cq"/":q" inside a sourced script must still terminate the
                // process with that code (getout(), main.c:1563-1575).
                if let ExOutcome::Quit { exit_code, .. } | ExOutcome::QuitAll { exit_code, .. } =
                    outcome
                {
                    self.report.quit = true;
                    self.report.exit_code = exit_code;
                }
                Ok(outcome)
            }
            Err(e) => {
                self.editor.emsg(e.to_string());
                Err(e)
            }
        };
        self.sync_errors();
        result
    }

    /// Open a file into the current session (CLI positional argument).
    ///
    /// Goes through the `:edit` event chain so `BufReadPre`/`BufRead`/
    /// `BufNewFile`/`BufEnter` (and therefore filetype detection) fire for
    /// start-up files too, as vim does after processing `-c` commands.
    pub fn open_file(&mut self, path: &std::path::Path) -> VimResult<()> {
        rvim_ex::autodispatch::edit_file(
            &mut self.editor,
            &mut self.viml.env,
            &mut self.viml.state,
            path,
        )?;
        self.sync_errors();
        Ok(())
    }

    /// Lines printed via ":p" / ":echo" style message history (batch output).
    pub fn message_history(&self) -> &[String] {
        self.editor.messages.history()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_edit_and_save() {
        let path = std::env::temp_dir().join("rvim_app_batch.txt");
        let _ = std::fs::remove_file(&path);
        let mut s = Session::new();
        s.run_script(&[
            "call_ignored".to_string(), // unknown command -> recorded error, no panic
            format!(
                "r {}",
                std::env::temp_dir().join("nonexistent_rvim.txt").display()
            ),
        ]);
        assert!(s.report.error_count >= 1);

        let mut ok = Session::new();
        ok.run_script(&["set et tabstop=4".to_string(), "normal_ignored".to_string()]);
        assert!(ok.editor.options.bool_or("expandtab", false));
        let _ = path;
    }

    #[test]
    fn quit_sets_report() {
        let mut s = Session::new();
        let out = s.ex("q").unwrap();
        assert!(matches!(out, ExOutcome::Quit { .. }));
        assert!(s.report.quit);
    }
}
