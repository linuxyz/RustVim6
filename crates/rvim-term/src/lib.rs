//! rvim-term: the M4 screen layer.
//!
//! * [`grid`] — logical screen of styled cells, ANSI painting and dirty-region
//!   diffs (vim's `screen.c`);
//! * [`render`] — buffer viewport + statusline + command line + wildmenu layout;
//! * [`key`] — terminal byte decoding into mapping keys (CSI/SS3/modifyOtherKeys).
//!
//! Nothing here owns editor state: the caller fills a [`render::ScreenModel`] and
//! receives a [`grid::Grid`], which keeps the whole layer unit-testable without a
//! terminal.

pub mod grid;
pub mod key;
pub mod pane;
pub mod render;
pub mod style;
pub mod width;

pub use grid::{Cell, Grid};
pub use key::{Decoded, decode_keys, encode_keys};
pub use render::{
    CmdlineView, ScreenModel, Theme, WildmenuView, display_col, expand_cells, gutter_width, render,
};
pub use style::{Color, Style};
pub use width::char_width;

use std::io;
use std::time::Duration;

use rvim_editor::maps::Key;

/// One step of the input loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Key(Key),
    /// No input within the timeout: lets the loop notice a window resize.
    Timeout,
    /// Input closed.
    Eof,
}

/// Anything that can deliver decoded keys: a terminal, a byte script, or a
/// pre-built key list.
pub trait KeySource {
    /// `Ok(None)` means "no more input" (EOF); the command loop exits.
    fn next_key(&mut self) -> io::Result<Option<Key>>;

    /// Wait up to `timeout` for the next step. The default keeps fully scripted
    /// sources synchronous (they never report a timeout).
    fn next_input(&mut self, _timeout: Duration) -> io::Result<Input> {
        Ok(match self.next_key()? {
            Some(key) => Input::Key(key),
            None => Input::Eof,
        })
    }

    /// Current window size when the terminal can report one. The loop polls this
    /// so a resize re-renders even without a key press (vim does this on SIGWINCH).
    fn size(&mut self) -> Option<(usize, usize)> {
        None
    }
}

/// Decodes raw bytes from a [`rvim_termio::ByteSource`] into keys.
pub struct ByteKeySource<S> {
    source: S,
    pending: Vec<u8>,
}

impl<S: rvim_termio::ByteSource> ByteKeySource<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            pending: Vec::new(),
        }
    }
}

impl<S: rvim_termio::ByteSource> KeySource for ByteKeySource<S> {
    fn next_key(&mut self) -> io::Result<Option<Key>> {
        let mut buf = [0u8; 64];
        loop {
            // Decode exactly one key so the rest of the buffer survives.
            if let Some((key, used)) = key::decode_one_key(&self.pending) {
                self.pending.drain(..used);
                return Ok(Some(key));
            }
            // A lone ESC with nothing after it: report <Esc> instead of blocking.
            if self.pending == [0x1b] {
                self.pending.clear();
                return Ok(Some(Key::Special("ESC".into())));
            }
            let read = self.source.read(&mut buf)?;
            if read == 0 {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                // Partial sequence at EOF: flush it as characters, one per
                // call so no buffered byte is dropped.
                let text = String::from_utf8_lossy(&self.pending).to_string();
                let first = text.chars().next();
                if let Some(ch) = first {
                    self.pending.drain(..ch.len_utf8());
                }
                return Ok(first.map(Key::Char));
            }
            self.pending.extend_from_slice(&buf[..read]);
        }
    }

    fn size(&mut self) -> Option<(usize, usize)> {
        self.source.size().map(|s| (s.cols, s.rows))
    }
}

/// Terminal key source: a reader thread plus the raw-mode terminal handle.
///
/// Polling the console size on every timeout is how a resize is noticed; the
/// terminal thread keeps reading so the loop never blocks past `timeout`.
pub struct ChannelKeySource {
    reader: rvim_termio::InputChannel,
    terminal: rvim_termio::Terminal,
    pending: Vec<u8>,
}

impl ChannelKeySource {
    pub fn open() -> io::Result<Self> {
        let terminal = rvim_termio::Terminal::open()?;
        Ok(Self {
            reader: rvim_termio::InputChannel::spawn(),
            terminal,
            pending: Vec::new(),
        })
    }
}

impl KeySource for ChannelKeySource {
    fn next_key(&mut self) -> io::Result<Option<Key>> {
        match self.next_input(Duration::from_secs(3600))? {
            Input::Key(key) => Ok(Some(key)),
            _ => Ok(None),
        }
    }

    fn next_input(&mut self, timeout: Duration) -> io::Result<Input> {
        loop {
            if let Some((key, used)) = key::decode_one_key(&self.pending) {
                self.pending.drain(..used);
                return Ok(Input::Key(key));
            }
            // A lone ESC: report it instead of waiting for a sequence that will
            // never come (vim uses 'timeoutlen' for this).
            if self.pending == [0x1b] {
                self.pending.clear();
                return Ok(Input::Key(Key::Special("ESC".into())));
            }
            match self.reader.next_event(timeout) {
                rvim_termio::InputEvent::Bytes(bytes) => {
                    self.pending.extend_from_slice(&bytes);
                }
                rvim_termio::InputEvent::Timeout => {
                    if self.pending == [0x1b] {
                        self.pending.clear();
                        return Ok(Input::Key(Key::Special("ESC".into())));
                    }
                    return Ok(Input::Timeout);
                }
                rvim_termio::InputEvent::Closed => return Ok(Input::Eof),
            }
        }
    }

    fn size(&mut self) -> Option<(usize, usize)> {
        self.terminal.size().ok().map(|s| (s.cols, s.rows))
    }
}
/// Keys supplied up front: the `--script` driver and the unit tests use this.
#[derive(Clone, Debug, Default)]
pub struct ScriptedKeys {
    keys: std::collections::VecDeque<Key>,
}

impl ScriptedKeys {
    pub fn new(keys: Vec<Key>) -> Self {
        Self { keys: keys.into() }
    }

    /// Build from vim key notation (`ihello<Esc>:wq<CR>`).
    pub fn from_notation(text: &str) -> Self {
        Self::new(rvim_editor::maps::parse_keys(text))
    }

    pub fn remaining(&self) -> usize {
        self.keys.len()
    }
}

impl KeySource for ScriptedKeys {
    fn next_key(&mut self) -> io::Result<Option<Key>> {
        Ok(self.keys.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_source_decodes_across_reads() {
        let mut src = ByteKeySource::new(rvim_termio::ScriptedBytes::from_text("ab\u{1b}[A"));
        assert_eq!(src.next_key().unwrap(), Some(Key::Char('a')));
        assert_eq!(src.next_key().unwrap(), Some(Key::Char('b')));
        assert_eq!(src.next_key().unwrap(), Some(Key::Special("UP".into())));
        assert_eq!(src.next_key().unwrap(), None, "EOF");
    }

    #[test]
    fn lone_escape_is_reported_as_esc() {
        let mut src = ByteKeySource::new(rvim_termio::ScriptedBytes::new(vec![0x1b]));
        assert_eq!(src.next_key().unwrap(), Some(Key::Special("ESC".into())));
        assert_eq!(src.next_key().unwrap(), None);
    }

    #[test]
    fn notation_driver_feeds_scripted_keys() {
        let mut src = ScriptedKeys::from_notation("ihey<Esc>:wq<CR>");
        let mut keys = Vec::new();
        while let Some(k) = src.next_key().unwrap() {
            keys.push(k);
        }
        assert_eq!(keys.len(), 9);
        assert_eq!(keys[0], Key::Char('i'));
        assert_eq!(keys[3], Key::Char('y'));
        assert_eq!(keys[4], Key::Special("ESC".into()));
        assert_eq!(keys[5], Key::Char(':'));
        assert_eq!(keys[7], Key::Char('q'));
        assert_eq!(keys[8], Key::Special("CR".into()));
        assert_eq!(src.remaining(), 0);
    }
}
