//! Windows GUI front-end for RustVim.
//!
//! The window, input loop and Direct2D/DirectWrite rendering live in the win
//! module. COM/Win32 calls are necessarily unsafe; this crate is the GUI
//! platform layer, like rvim-termio is the terminal platform layer. Non-Windows
//! builds keep a stub so the workspace still builds everywhere.

#![allow(unsafe_op_in_unsafe_fn)]

use rvim_app::Session;

#[cfg(windows)]
mod win;

/// Run the GUI until the window closes. Returns the session (for viminfo) and
/// an exit code. On non-Windows this reports that the GUI is unavailable.
#[cfg(windows)]
pub fn run(session: Session) -> (Session, i32) {
    win::run(session)
}

#[cfg(not(windows))]
pub fn run(session: Session) -> (Session, i32) {
    eprintln!("rvim: the GUI front-end is only available on Windows");
    (session, 1)
}

/// Detach the console so the --gui option behaves like a windowed application.
#[cfg(windows)]
pub fn detach_console() {
    // SAFETY: FreeConsole has no preconditions; failure simply leaves the
    // console attached (for example when started from a debugger).
    unsafe {
        winapi::um::wincon::FreeConsole();
    }
}

#[cfg(not(windows))]
pub fn detach_console() {}

/// True when this process owns a freshly created console: the usual result of
/// double-clicking a console-subsystem executable in Explorer. A shell launch
/// has at least the shell process in the console list.
#[cfg(windows)]
pub fn console_is_private() -> bool {
    let mut processes = [0u32; 2];
    // SAFETY: the buffer is valid for two DWORDs and the API writes at most
    // that many entries.
    unsafe { winapi::um::wincon::GetConsoleProcessList(processes.as_mut_ptr(), 2) == 1 }
}

#[cfg(not(windows))]
pub fn console_is_private() -> bool {
    false
}
