//! Version banner aligned with Vim's `:version` layout.

pub const RVIM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Baseline this implementation is measured against.
pub const VIM_BASELINE_TAG: &str = "v8.2.5172";
pub const VIM_BASELINE_PATCHES: &str = "1-5172";

pub fn version_banner() -> String {
    [
        format!("RustVim {RVIM_VERSION} (Vim-compatible, std-only)"),
        format!("Reference baseline: vim/vim {VIM_BASELINE_TAG}, included patches: {VIM_BASELINE_PATCHES}"),
        "vi-compatible mode: not supported ('compatible' permanently off)".to_string(),
        "Features: +cryptv +crypt-blowfish +crypt-blowfish2 +crypt-zip +spell +syntax \
+quickfix +folding +diff +windows +tabpages +job +channel +terminal +timers \
+multi_byte +cmdline_compl +cmdline_hist"
            .to_string(),
    ]
    .join("\n")
}
