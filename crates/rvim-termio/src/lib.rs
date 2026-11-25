//! Terminal I/O backend for the M4 TUI: raw mode, window size and byte input.
//!
//! This crate is the only place in RustVim that talks to the operating system
//! directly. Vim's equivalent is `os_win32.c` (console mode + screen buffer
//! info) and `os_unix.c`/termios. `std` exposes neither, so the Windows backend
//! declares three `kernel32` functions itself; the workspace denies
//! `unsafe_code`, which this module opts out of under `#[allow]` — the exception
//! is documented in `docs/ARCH.md` and `docs/compat_deviations.md`.
//!
//! Everything above this crate sees only [`Terminal`] and [`ByteSource`], so the
//! TUI can be driven from a byte script in tests (see [`ScriptedBytes`]).

use std::io::{self, Read, Write};

/// Console size in character cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Size {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows }
    }
}

/// Source of raw input bytes: a terminal, a file, or a test script.
pub trait ByteSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Current window size, when the source is a real terminal.
    fn size(&self) -> Option<Size> {
        None
    }
}

/// Reads from stdin (works for a console in raw/VT input mode and for pipes).
#[derive(Debug, Default)]
pub struct StdinSource;

impl ByteSource for StdinSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::stdin().read(buf)
    }
}

/// Feeds a fixed byte script; used by the headless `--script` driver and tests.
#[derive(Clone, Debug, Default)]
pub struct ScriptedBytes {
    data: Vec<u8>,
    pos: usize,
}

impl ScriptedBytes {
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self {
            data: data.into(),
            pos: 0,
        }
    }

    pub fn from_text(text: &str) -> Self {
        Self::new(text.as_bytes().to_vec())
    }

    pub fn is_exhausted(&self) -> bool {
        self.pos >= self.data.len()
    }
}

impl ByteSource for ScriptedBytes {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = &self.data[self.pos..];
        let n = remaining.len().min(buf.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        self.pos += n;
        Ok(n)
    }
}

/// A terminal in raw (character-at-a-time, no echo) mode.
///
/// Dropping the value restores the previous console modes / terminal settings.
pub struct Terminal {
    raw: platform::RawMode,
}

impl Terminal {
    /// Put the controlling terminal into raw mode.
    ///
    /// Returns `io::ErrorKind::Unsupported` when there is no interactive console
    /// (pipe, redirect, CI), which is how `rvim` decides between the TUI and the
    /// batch `-es` path.
    pub fn open() -> io::Result<Self> {
        Ok(Self {
            raw: platform::RawMode::enable()?,
        })
    }

    /// True when a terminal is available for interactive use.
    pub fn is_available() -> bool {
        platform::is_terminal()
    }

    pub fn size(&self) -> io::Result<Size> {
        self.raw.size()
    }

    /// Write already-encoded output (ANSI escape sequences) to the terminal.
    pub fn write_out(&mut self, text: &str) -> io::Result<()> {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        lock.write_all(text.as_bytes())?;
        lock.flush()
    }
}

impl ByteSource for Terminal {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::stdin().read(buf)
    }

    fn size(&self) -> Option<Size> {
        Terminal::size(self).ok()
    }
}

/// Result of [`InputChannel::next_event`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Bytes(Vec<u8>),
    /// Nothing arrived within the timeout (the loop uses this to detect a
    /// window resize and repaint, like vim's SIGWINCH handling).
    Timeout,
    /// stdin closed (EOF).
    Closed,
}

/// A background thread that reads stdin and forwards byte chunks.
///
/// The terminal loop needs to wake up periodically to notice a resize; a
/// blocking `read` would only wake on a key press. `std` has no `select`, so a
/// reader thread plus a channel is the portable solution.
pub struct InputChannel {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    _handle: std::thread::JoinHandle<()>,
}

impl InputChannel {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            loop {
                match io::stdin().read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            rx,
            _handle: handle,
        }
    }

    /// Wait up to `timeout` for the next chunk of input.
    pub fn next_event(&self, timeout: std::time::Duration) -> InputEvent {
        match self.rx.recv_timeout(timeout) {
            Ok(bytes) => InputEvent::Bytes(bytes),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => InputEvent::Timeout,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => InputEvent::Closed,
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    //! Win32 console backend (`os_win32.c` equivalents):
    //! `SetConsoleMode` for VT input/output, `GetConsoleScreenBufferInfo` for the
    //! window size.

    use std::io;

    #[repr(C)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }

    #[repr(C)]
    struct ScreenBufferInfo {
        size: Coord,
        cursor_position: Coord,
        attributes: u16,
        window: SmallRect,
        maximum_window_size: Coord,
    }

    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> isize;
        fn GetConsoleMode(handle: isize, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: isize, mode: u32) -> i32;
        fn GetConsoleScreenBufferInfo(handle: isize, info: *mut ScreenBufferInfo) -> i32;
    }

    const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
    const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;

    const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
    const ENABLE_WRAP_AT_EOL_OUTPUT: u32 = 0x0002;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

    fn console_mode(handle: isize) -> Option<u32> {
        let mut mode = 0u32;
        let ok = unsafe { GetConsoleMode(handle, &mut mode) };
        (ok != 0).then_some(mode)
    }

    pub fn is_terminal() -> bool {
        console_mode(std_handle(STD_OUTPUT_HANDLE)).is_some()
            && console_mode(std_handle(STD_INPUT_HANDLE)).is_some()
    }

    fn std_handle(which: u32) -> isize {
        unsafe { GetStdHandle(which) }
    }

    pub struct RawMode {
        out_handle: isize,
        in_handle: isize,
        saved_out: u32,
        saved_in: u32,
    }

    impl RawMode {
        pub fn enable() -> io::Result<Self> {
            let out_handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
            let in_handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
            let Some(saved_out) = console_mode(out_handle) else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "no console on stdout",
                ));
            };
            let Some(saved_in) = console_mode(in_handle) else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "no console on stdin",
                ));
            };
            let out = saved_out
                | ENABLE_PROCESSED_OUTPUT
                | ENABLE_WRAP_AT_EOL_OUTPUT
                | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
            let input = (saved_in
                & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
                | ENABLE_VIRTUAL_TERMINAL_INPUT;
            let ok_out = unsafe { SetConsoleMode(out_handle, out) };
            let ok_in = unsafe { SetConsoleMode(in_handle, input) };
            if ok_out == 0 || ok_in == 0 {
                return Err(io::Error::other("SetConsoleMode failed"));
            }
            Ok(Self {
                out_handle,
                in_handle,
                saved_out,
                saved_in,
            })
        }

        pub fn size(&self) -> io::Result<super::Size> {
            let mut info = ScreenBufferInfo {
                size: Coord { x: 0, y: 0 },
                cursor_position: Coord { x: 0, y: 0 },
                attributes: 0,
                window: SmallRect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                maximum_window_size: Coord { x: 0, y: 0 },
            };
            let ok = unsafe { GetConsoleScreenBufferInfo(self.out_handle, &mut info) };
            if ok == 0 {
                return Err(io::Error::other("GetConsoleScreenBufferInfo failed"));
            }
            let cols = (info.window.right - info.window.left + 1).max(1) as usize;
            let rows = (info.window.bottom - info.window.top + 1).max(1) as usize;
            Ok(super::Size { cols, rows })
        }
    }

    impl Drop for RawMode {
        fn drop(&mut self) {
            unsafe {
                SetConsoleMode(self.out_handle, self.saved_out);
                SetConsoleMode(self.in_handle, self.saved_in);
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    //! POSIX backend. `std` has no termios, so raw mode and the window size come
    //! from the `stty` utility (shipped with every POSIX system).

    use std::io;
    use std::process::Command;

    pub fn is_terminal() -> bool {
        Command::new("stty")
            .arg("size")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub struct RawMode;

    impl RawMode {
        pub fn enable() -> io::Result<Self> {
            let status = Command::new("stty").arg("raw").arg("-echo").status()?;
            if !status.success() {
                return Err(io::Error::other("stty raw failed"));
            }
            Ok(Self)
        }

        pub fn size(&self) -> io::Result<super::Size> {
            let out = Command::new("stty").arg("size").output()?;
            let text = String::from_utf8_lossy(&out.stdout);
            let mut parts = text.split_whitespace();
            let rows = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .ok_or_else(|| io::Error::other("stty size: missing rows"))?;
            let cols = parts
                .next()
                .and_then(|v| v.parse::<usize>().ok())
                .ok_or_else(|| io::Error::other("stty size: missing cols"))?;
            Ok(super::Size::new(cols, rows))
        }
    }

    impl Drop for RawMode {
        fn drop(&mut self) {
            // Best effort: restore the user's terminal settings.
            let _ = Command::new("stty").arg("sane").status();
        }
    }
}

/// Set an environment variable for this process.
///
/// The editor runs builtins on one thread; this is the platform-layer wrapper
/// for the edition-2024 unsafe 'std::env::set_var' call.
#[allow(unsafe_code)]
pub fn set_env(name: &str, value: &str) {
    // SAFETY: called from the single-threaded editor loop, never while another
    // thread reads the environment.
    unsafe {
        std::env::set_var(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_source_yields_bytes_in_chunks() {
        let mut src = ScriptedBytes::from_text("abc");
        let mut buf = [0u8; 2];
        assert_eq!(src.read(&mut buf).unwrap(), 2);
        assert_eq!(&buf, b"ab");
        assert!(!src.is_exhausted());
        let mut buf2 = [0u8; 4];
        assert_eq!(src.read(&mut buf2).unwrap(), 1);
        assert_eq!(&buf2[..1], b"c");
        assert!(src.is_exhausted());
        assert_eq!(src.read(&mut buf2).unwrap(), 0, "EOF keeps returning 0");
    }

    #[test]
    fn size_is_addressable() {
        let s = Size::new(80, 24);
        assert_eq!(s.cols, 80);
        assert_eq!(s.rows, 24);
    }
}
