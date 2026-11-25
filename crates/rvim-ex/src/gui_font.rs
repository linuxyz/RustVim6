//! Optional GUI font-selection hook.
//!
//! The Ex layer is platform-neutral; the Windows GUI registers a callback that
//! opens the native ChooseFont dialog. In the TUI (or on Unix) the hook is
//! absent and :set guifont simply stores its string value.

use std::sync::{Mutex, OnceLock};

/// A callback that returns a Vim guifont string such as Consolas:h12.
pub type FontDialog = fn(current: &str, wide: bool) -> Option<String>;

fn hook() -> &'static Mutex<Option<FontDialog>> {
    static HOOK: OnceLock<Mutex<Option<FontDialog>>> = OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

/// Register (or clear) the GUI font dialog.
pub fn set_font_dialog_hook(dialog: Option<FontDialog>) {
    if let Ok(mut slot) = hook().lock() {
        *slot = dialog;
    }
}

/// Ask the GUI to choose a font. Returns None when no GUI hook is installed or
/// the user cancelled the dialog.
pub fn has_hook() -> bool {
    hook().lock().map(|slot| slot.is_some()).unwrap_or(false)
}

pub fn choose_font(current: &str, wide: bool) -> Option<String> {
    hook()
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .and_then(|dialog| dialog(current, wide))
}
