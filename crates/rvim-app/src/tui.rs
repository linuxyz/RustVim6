//! Interactive TUI driver (M4): key loop, mode dispatch, command line, mapping
//! expansion and screen composition.
//!
//! The engine is deliberately independent of the terminal: it consumes
//! [`Key`]s and produces a [`Grid`], so the same code runs against a real
//! terminal (via `rvim-termio`) and against a scripted key list in tests.

use std::collections::VecDeque;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::maps::{Key, MapMode};
use rvim_editor::{Mode, Rect, WindowId};
use rvim_normal::{NormalEngine, insert};
use rvim_term::pane::{
    CmdlineText, PaneView, RowView, ScreenLayout, SelectionView, WildmenuItems, render_layout,
};
use rvim_term::{Grid, Input, KeySource, Theme, display_col, gutter_width};

use crate::Session;

/// A two-key normal command waiting for its second key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// `d`, `y` or `c`: waiting for a motion (`dw`, `dj`, `d2w`, ...).
    Operator(char),
    /// An operator whose motion is `gg`: waiting for the second `g`.
    OperatorGg(char),
    /// `g`: waiting for `g` (`gg`, `3gg`).
    G,
    /// `<C-w>`: waiting for the window command key.
    WindowCmd,
    /// An operator waiting for a text object character (`diw`, `ca("`).
    TextObject(char, bool),
    /// `g` in visual mode (only `gg` is supported).
    VisualG,
    /// `z`: waiting for the fold command key.
    Fold,
    /// `]` (true) or `[` (false): waiting for the following key.
    Bracket(bool),
}

/// Command line being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmdline {
    /// ':' for Ex commands, '/' for a forward search.
    pub prompt: char,
    pub text: String,
    /// Cursor as a char index into `text`.
    pub cursor: usize,
}

impl Cmdline {
    fn new(prompt: char) -> Self {
        Self {
            prompt,
            text: String::new(),
            cursor: 0,
        }
    }

    fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    fn insert(&mut self, ch: char) {
        let byte = self.byte_at(self.cursor);
        self.text.insert(byte, ch);
        self.cursor += 1;
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_at(self.cursor);
        let start = self.byte_at(self.cursor - 1);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_at(self.cursor);
        let end = self.byte_at(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }
}

/// The interactive editor front-end.
pub struct Tui {
    pub session: Session,
    pub theme: Theme,
    /// Message shown on the bottom row (vim's message line).
    pub message: String,
    /// Queued messages for the simple more-prompt (`-- More --`).
    pub message_queue: VecDeque<String>,
    pub quit: bool,
    pub exit_code: i32,
    pub cmdline: Option<Cmdline>,
    /// Keys buffered while deciding whether a mapping applies.
    pending_keys: Vec<Key>,
    /// Second key of a two-key normal command (`dd`, `yy`, `gg`, `dw`).
    pending_op: Option<Pending>,
    /// Direction of the last `/` or `?` search (`n` repeats it).
    search_forward: bool,
    /// Count typed in visual mode (`v3j`, `3i` in a block).
    visual_count: Option<u64>,
    /// When the pending key buffer started waiting, for 'timeoutlen'.
    pending_since: Option<std::time::Instant>,
    /// Wildmenu candidates while completing.
    pub wildmenu: Option<(Vec<String>, usize, String)>,
    /// Number of messages already consumed from the editor log.
    seen_messages: usize,
    /// Geometry of the last rendered frame (updated when the terminal resizes).
    pub screen_size: (usize, usize),
    last_frame: Option<Grid>,
}

impl Tui {
    pub fn new(session: Session) -> Self {
        let mut tui = Self {
            session,
            theme: Theme::default(),
            message: String::new(),
            message_queue: VecDeque::new(),
            quit: false,
            exit_code: 0,
            cmdline: None,
            pending_keys: Vec::new(),
            pending_op: None,
            search_forward: true,
            visual_count: None,
            pending_since: None,
            wildmenu: None,
            seen_messages: 0,
            screen_size: (80, 24),
            last_frame: None,
        };
        tui.seen_messages = tui.session.message_history().len();
        tui
    }

    fn editor(&mut self) -> &mut rvim_editor::Editor {
        &mut self.session.editor
    }

    // ------------------------------------------------------------ key input

    /// Process one decoded key.
    pub fn feed_key(&mut self, key: Key) -> VimResult<()> {
        if self.quit {
            return Ok(());
        }
        // Vim's message prompts (message.c): while output is still queued a key
        // shows the next message and is not a command ("-- More --"); a single
        // message is replaced by the next command ("Press ENTER or type command
        // to continue").
        if !self.message.is_empty() {
            if !self.message_queue.is_empty() {
                self.next_message();
                return Ok(());
            }
            self.message.clear();
        }
        // Command-line mode owns every key.
        if self.cmdline.is_some() {
            return self.feed_cmdline(key);
        }
        // Esc always cancels a pending multi-key state.
        if key == Key::Special("ESC".into()) {
            self.pending_keys.clear();
            self.pending_op = None;
            // A count typed before <Esc> is dropped, not carried to the next
            // command (normal.c nv_esc).
            self.session.normal.cancel_pending();
            self.visual_count = None;
            if self.session.editor.mode.is_insert_like() {
                self.finish_insert_mode();
            } else if self.session.editor.mode.is_visual() {
                rvim_normal::visual::stop(self.editor());
            } else if self.session.editor.mode != Mode::Normal {
                // Cancel a pending operator/motion (normal.c nv_esc) without
                // touching the cursor or the undo state.
                self.session.editor.mode = Mode::Normal;
            }
            return Ok(());
        }
        // Mapping expansion: buffer keys until we know whether a map applies.
        self.pending_keys.push(key);
        match self.expand_pending() {
            Expand::Waiting => {
                // Vim waits 'timeoutlen' for the rest of an ambiguous mapping and
                // then treats the buffered key literally (timeout handling).
                self.pending_since
                    .get_or_insert_with(std::time::Instant::now);
                Ok(())
            }
            Expand::Replaced(keys) => {
                self.pending_since = None;
                for key in keys {
                    self.dispatch_key(key)?;
                    if self.quit {
                        break;
                    }
                }
                Ok(())
            }
            Expand::Literal(mut keys) => {
                self.pending_since = None;
                let first = keys.remove(0);
                self.pending_keys = keys;
                self.dispatch_key(first)?;
                // Feed the rest of the buffered keys through expansion again.
                let rest = std::mem::take(&mut self.pending_keys);
                for key in rest {
                    if self.quit {
                        break;
                    }
                    self.feed_key(key)?;
                }
                Ok(())
            }
        }
    }

    /// Decide what to do with the buffered keys.
    fn expand_pending(&mut self) -> Expand {
        let mode = map_mode_for(self.session.editor.mode);
        let Some(mode) = mode else {
            return Expand::Literal(std::mem::take(&mut self.pending_keys));
        };
        let entries: Vec<(Vec<Key>, bool)> = self
            .session
            .editor
            .maps
            .entries_for(mode)
            .into_iter()
            .map(|e| (e.lhs.clone(), true))
            .collect();
        let pending = self.pending_keys.clone();
        if entries.iter().any(|(lhs, _)| lhs == &pending) {
            // Exact match: expand the whole buffer (handles recursion + <Nop>).
            let keys = self
                .session
                .editor
                .maps
                .expand(mode, &pending)
                .unwrap_or_else(|_| pending.clone());
            self.pending_keys.clear();
            return Expand::Replaced(keys);
        }
        if entries.iter().any(|(lhs, _)| lhs.starts_with(&pending)) {
            return Expand::Waiting;
        }
        Expand::Literal(std::mem::take(&mut self.pending_keys))
    }

    /// Apply vim's mapping timeout: if a buffered prefix has been waiting longer
    /// than 'timeoutlen' (and 'timeout' is on), the first key is used literally.
    ///
    /// Returns true when something was flushed (the caller repaints).
    pub fn flush_pending_timeout(&mut self) -> VimResult<bool> {
        if self.pending_keys.is_empty() {
            self.pending_since = None;
            return Ok(false);
        }
        if !self.session.editor.options.bool_or("timeout", true) {
            return Ok(false);
        }
        let limit = self
            .session
            .editor
            .options
            .num_or("timeoutlen", 1000)
            .max(0) as u128;
        let Some(start) = self.pending_since else {
            self.pending_since = Some(std::time::Instant::now());
            return Ok(false);
        };
        if start.elapsed().as_millis() < limit {
            return Ok(false);
        }
        let mut keys = std::mem::take(&mut self.pending_keys);
        self.pending_since = None;
        if keys.is_empty() {
            return Ok(false);
        }
        let first = keys.remove(0);
        self.dispatch_key(first)?;
        for key in keys {
            if self.quit {
                break;
            }
            self.feed_key(key)?;
        }
        Ok(true)
    }
    /// Execute a literal (already expanded) key according to the mode.
    fn dispatch_key(&mut self, key: Key) -> VimResult<()> {
        match self.session.editor.mode {
            Mode::Insert | Mode::Replace => self.handle_insert_key(key),
            // The command line is handled before dispatch (feed_cmdline).
            Mode::CommandLine => Ok(()),
            mode if mode.is_visual() => self.handle_visual_key(key),
            _ => self.handle_normal_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: Key) -> VimResult<()> {
        // Finish a pending operator (`d`, `y`, `c`, `g`) with its second key.
        if let Some(pending) = self.pending_op.take() {
            return self.finish_pending_command(pending, key);
        }

        match &key {
            Key::Char(':') => {
                self.start_cmdline(':');
                return Ok(());
            }
            // ]c / [c jump between diff changes.
            Key::Char(']') => {
                self.pending_op = Some(Pending::Bracket(true));
                return Ok(());
            }
            Key::Char('[') => {
                self.pending_op = Some(Pending::Bracket(false));
                return Ok(());
            }
            // z starts a fold command (zo, zc, zf, zR, ...).
            Key::Char('z') => {
                self.pending_op = Some(Pending::Fold);
                return Ok(());
            }
            // v / V start a character-wise or line-wise selection.
            Key::Char('v') | Key::Char('V') => {
                let mode = if matches!(&key, Key::Char('V')) {
                    Mode::VisualLine
                } else {
                    Mode::Visual
                };
                rvim_normal::visual::start(self.editor(), mode)?;
                return Ok(());
            }
            Key::Char('/') => {
                self.session.editor.push_jump();
                self.start_cmdline('/');
                return Ok(());
            }
            // `?` searches backwards, `n`/`N` repeat the last search.
            Key::Char('?') => {
                self.session.editor.push_jump();
                self.start_cmdline('?');
                return Ok(());
            }
            Key::Char('n') | Key::Char('N') => {
                self.session.editor.push_jump();
                let same_direction = matches!(&key, Key::Char('n'));
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let count = engine.take_pending_count();
                self.session.normal = engine;
                return self.repeat_search(count, same_direction);
            }
            Key::Char(ch) => {
                let ch = *ch;
                if ch == 'G' {
                    self.session.editor.push_jump();
                }
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let result = engine.feed(self.editor(), ch);
                self.session.normal = engine;
                let consumed = result?;
                if !consumed {
                    return Ok(());
                }
                // `d`/`y`/`c` wait in MotionPending, `g` waits in OperatorPending:
                // both take a second key that the engine completes via feed_pair.
                if matches!(ch, 'd' | 'y' | 'c' | 'g')
                    && matches!(
                        self.session.editor.mode,
                        Mode::MotionPending { .. } | Mode::OperatorPending
                    )
                {
                    self.pending_op = Some(if ch == 'g' {
                        Pending::G
                    } else {
                        Pending::Operator(ch)
                    });
                }
                return Ok(());
            }
            _ => {}
        }

        // Special keys map onto normal-mode commands.
        let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
        let result = match key {
            Key::Special(name) => match name.as_str() {
                "CR" => engine.feed(self.editor(), 'j'),
                "DOWN" => engine.feed(self.editor(), 'j'),
                "UP" => engine.feed(self.editor(), 'k'),
                "LEFT" => engine.feed(self.editor(), 'h'),
                "RIGHT" => engine.feed(self.editor(), 'l'),
                "HOME" => engine.feed(self.editor(), '0'),
                "END" => engine.feed(self.editor(), '$'),
                "BS" => engine.feed(self.editor(), 'h'),
                "DELETE" => engine.feed(self.editor(), 'x'),
                "C-R" => engine.ctrl_r(self.editor()).map(|_| true),
                // CTRL-O / CTRL-I walk the jump list (mark.c).
                "C-O" => self.session.editor.jump_back().map(|_| true),
                "C-I" => self.session.editor.jump_forward().map(|_| true),
                // <C-w> starts a window command; the next key selects it.
                "C-W" => {
                    self.pending_op = Some(Pending::WindowCmd);
                    Ok(true)
                }
                // <C-v> starts a blockwise selection.
                "C-V" => rvim_normal::visual::start(self.editor(), Mode::VisualBlock).map(|_| true),
                // <C-a>/<C-x> increment/decrement the number under the cursor.
                "C-A" => engine.feed(self.editor(), '\u{1}'),
                "C-X" => engine.feed(self.editor(), '\u{18}'),
                // <PageDown>/<C-f> and <PageUp>/<C-b> scroll one screen and keep
                // the cursor in the same screen row (normal.c nv_page).
                "PAGEDOWN" | "C-F" => {
                    let page = self.text_rows();
                    self.scroll_line(page as i64, true);
                    Ok(true)
                }
                "PAGEUP" | "C-B" => {
                    let page = self.text_rows();
                    self.scroll_line(-(page as i64), false);
                    Ok(true)
                }
                _ => Ok(true),
            },
            _ => Ok(true),
        };
        self.session.normal = engine;
        result?;
        Ok(())
    }

    /// Complete a two-key command whose first key is buffered.
    fn finish_pending_command(&mut self, pending: Pending, key: Key) -> VimResult<()> {
        match pending {
            Pending::WindowCmd => {
                let Some(ch) = motion_key(&key) else {
                    return Ok(());
                };
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let count = engine.take_pending_count();
                self.session.normal = engine;
                self.session.editor.wincmd(ch, count)?;
                Ok(())
            }
            Pending::TextObject(op, around) => {
                let Some(object) = motion_key(&key) else {
                    self.session.editor.mode = Mode::Normal;
                    return Ok(());
                };
                self.apply_text_object(op, object, around)?;
                Ok(())
            }
            Pending::Bracket(forward) => {
                if matches!(&key, Key::Char('c')) {
                    self.diff_jump(forward)?;
                }
                Ok(())
            }
            Pending::Fold => {
                let Some(ch) = motion_key(&key) else {
                    return Ok(());
                };
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let count = engine.take_pending_count();
                self.session.normal = engine;
                // zf over a visual selection creates a fold for that range.
                if ch == 'f' && self.session.editor.visual.is_some() {
                    let Some(range) = rvim_normal::visual::range(&self.session.editor) else {
                        return Ok(());
                    };
                    let last = if range.linewise {
                        range.end.line.saturating_sub(1)
                    } else {
                        range.end.line
                    };
                    rvim_normal::visual::stop(self.editor());
                    rvim_normal::foldcmd::create_fold(self.editor(), range.start.line, last)?;
                    return Ok(());
                }
                if !rvim_normal::foldcmd::feed_z(self.editor(), ch, count)? {
                    rvim_normal::foldcmd::fold_motion(self.editor(), ch, count)?;
                }
                Ok(())
            }
            Pending::VisualG => {
                if matches!(&key, Key::Char('g')) {
                    let mut engine =
                        std::mem::replace(&mut self.session.normal, NormalEngine::new());
                    let count = engine.take_pending_count();
                    self.session.normal = engine;
                    let pos = rvim_normal::motions::goto_line(self.editor(), count, false)?;
                    self.session.editor.set_cursor(pos);
                }
                Ok(())
            }
            Pending::OperatorGg(op) => {
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let result = if matches!(&key, Key::Char('g')) {
                    engine.feed_operator_gg(self.editor(), op, 1)
                } else {
                    Ok(false)
                };
                self.session.normal = engine;
                if !result? {
                    // Not a valid completion: cancel like <Esc>.
                    self.session.editor.mode = Mode::Normal;
                }
                Ok(())
            }
            Pending::G => {
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let result = if matches!(&key, Key::Char('g')) {
                    self.session.editor.push_jump();
                    engine.feed_pair(self.editor(), 'g', 'g')
                } else {
                    Ok(())
                };
                self.session.normal = engine;
                if matches!(&key, Key::Char('v')) {
                    // gv: restore the last visual selection.
                    if rvim_normal::visual::reselect(self.editor()) {
                        return Ok(());
                    }
                }
                if !matches!(&key, Key::Char('g')) {
                    self.session.editor.mode = Mode::Normal;
                }
                result
            }
            Pending::Operator(op) => {
                // do / dp: copy the current diff hunk from or to the other window.
                if op == 'd' && matches!(&key, Key::Char('o') | Key::Char('p')) {
                    let win = self.session.editor.windows.current_id();
                    let target = self.session.editor.diff_counterparts(win).first().copied();
                    self.session.editor.mode = Mode::Normal;
                    if let Some(target) = target {
                        if matches!(&key, Key::Char('o')) {
                            self.session.editor.diff_get(target, None)?;
                        } else {
                            self.session.editor.diff_put(target, None)?;
                        }
                    }
                    return Ok(());
                }
                // i/a after an operator introduce a text object (diw, ca(, yap).
                if matches!(&key, Key::Char('i') | Key::Char('a')) {
                    let around = matches!(&key, Key::Char('a'));
                    self.pending_op = Some(Pending::TextObject(op, around));
                    self.session.editor.mode = Mode::OperatorPending;
                    return Ok(());
                }
                let Some(second) = motion_key(&key) else {
                    // Unsupported operator+motion combination: cancel like <Esc>.
                    self.session.editor.mode = Mode::Normal;
                    return Ok(());
                };
                if second == 'g' {
                    // dgg/ygg/cgg wait for the second g.
                    self.pending_op = Some(Pending::OperatorGg(op));
                    return Ok(());
                }
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let result = engine.feed_operator_key(self.editor(), op, second);
                self.session.normal = engine;
                if result? {
                    // A count was typed between operator and motion (d2w), so the
                    // engine is still waiting for the actual motion key.
                    if matches!(self.session.editor.mode, Mode::MotionPending { .. }) {
                        self.pending_op = Some(Pending::Operator(op));
                    }
                } else {
                    self.session.editor.mode = Mode::Normal;
                }
                Ok(())
            }
        }
    }

    fn handle_insert_key(&mut self, key: Key) -> VimResult<()> {
        match key {
            Key::Char(ch) => {
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let result = insert::type_char(self.editor(), &mut engine, ch);
                self.session.normal = engine;
                result
            }
            Key::Special(name) => {
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let result = match name.as_str() {
                    // <CR> inserts a newline exactly like typing one: the insert
                    // session must record it, otherwise a counted insert (3i%<CR>)
                    // or "." repeats the text without the line break (normal.c
                    // ins_eol / edit.c ins_char).
                    "CR" => insert::type_char(self.editor(), &mut engine, '\n'),
                    "TAB" => insert::type_char(self.editor(), &mut engine, '\t'),
                    "BS" => insert::backspace(self.editor(), &mut engine),
                    "DELETE" => {
                        let pos = self.editor().cursor();
                        self.editor().delete_chars(pos, 1).map(|_| ())
                    }
                    "LEFT" | "RIGHT" | "UP" | "DOWN" | "HOME" | "END" => {
                        let pos = self.editor().cursor();
                        let line = self.editor().current_line();
                        let col = match name.as_str() {
                            "LEFT" => rvim_core::text::move_chars(&line, pos.col, -1),
                            "RIGHT" => rvim_core::text::move_chars(&line, pos.col, 1),
                            "HOME" => 0,
                            "END" => line.len(),
                            _ => pos.col,
                        };
                        let line = match name.as_str() {
                            "UP" => pos.line.saturating_sub(1),
                            "DOWN" => (pos.line + 1).min(self.editor().line_count() - 1),
                            _ => pos.line,
                        };
                        let col = col.min(self.editor().current_buffer().line(line)?.len_bytes());
                        self.editor().set_cursor(Pos::new(line, col));
                        Ok(())
                    }
                    _ => Ok(()),
                };
                self.session.normal = engine;
                result
            }
            _ => Ok(()),
        }
    }

    fn finish_insert_mode(&mut self) {
        // A block insert already parked the cursor on the block's first column
        // (ops.c op_insert), so the usual "one char to the left" step is skipped.
        let block_insert = self.session.editor.block_insert.is_some();
        let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
        engine.finish_insert(self.editor());
        self.session.normal = engine;
        if block_insert {
            return;
        }
        // vim leaves the cursor on the last inserted character (ins_esc()),
        // while the buffer parks it just past the inserted text.
        let pos = self.editor().cursor();
        if pos.col > 0 {
            let line = self.editor().current_line();
            self.editor().set_cursor(Pos::new(
                pos.line,
                rvim_core::text::move_chars(&line, pos.col, -1),
            ));
        }
    }

    // --------------------------------------------------------- command line

    fn start_cmdline(&mut self, prompt: char) {
        // The prompt always starts empty; the previous pattern comes back from
        // the command-line history, not by pre-filling (cmdline.c). An empty
        // pattern still repeats the last search (search.c).
        self.cmdline = Some(Cmdline::new(prompt));
        self.session.editor.mode = Mode::CommandLine;
        self.wildmenu = None;
    }

    fn cancel_cmdline(&mut self) {
        self.cmdline = None;
        self.wildmenu = None;
        self.session.editor.mode = Mode::Normal;
    }

    fn feed_cmdline(&mut self, key: Key) -> VimResult<()> {
        match key {
            Key::Char(ch) => {
                if let Some(cmdline) = self.cmdline.as_mut() {
                    cmdline.insert(ch);
                }
                self.wildmenu = None;
            }
            Key::Special(name) => match name.as_str() {
                "BS" => {
                    if let Some(cmdline) = self.cmdline.as_mut() {
                        cmdline.backspace();
                    }
                    self.wildmenu = None;
                }
                "DELETE" => {
                    if let Some(cmdline) = self.cmdline.as_mut() {
                        cmdline.delete();
                    }
                    self.wildmenu = None;
                }
                "LEFT" => {
                    if let Some(cmdline) = self.cmdline.as_mut() {
                        cmdline.cursor = cmdline.cursor.saturating_sub(1);
                    }
                }
                "RIGHT" => {
                    if let Some(cmdline) = self.cmdline.as_mut() {
                        cmdline.cursor = (cmdline.cursor + 1).min(cmdline.char_len());
                    }
                }
                "HOME" => {
                    if let Some(cmdline) = self.cmdline.as_mut() {
                        cmdline.cursor = 0;
                    }
                }
                "END" => {
                    if let Some(cmdline) = self.cmdline.as_mut() {
                        cmdline.cursor = cmdline.char_len();
                    }
                }
                "TAB" | "C-N" => self.complete_cmdline(true)?,
                "C-P" => self.complete_cmdline(false)?,
                "CR" => return self.execute_cmdline(),
                // <Esc> / <C-C> abandon the command line (vim keeps the buffer).
                "ESC" | "C-C" => self.cancel_cmdline(),
                _ => {}
            },
            Key::Nop => {}
        }
        Ok(())
    }

    fn complete_cmdline(&mut self, forward: bool) -> VimResult<()> {
        let Some(cmdline) = self.cmdline.as_mut() else {
            return Ok(());
        };
        if self.wildmenu.is_none() {
            let user: Vec<String> = Vec::new();
            let matches = if cmdline.prompt == ':' {
                rvim_cmdline::complete_command(&cmdline.text, &user, 50)
            } else {
                Vec::new()
            };
            if matches.is_empty() {
                return Ok(());
            }
            let original = cmdline.text.clone();
            let first = matches[0].clone();
            self.wildmenu = Some((matches, 0, original));
            cmdline.text = first;
            cmdline.cursor = cmdline.char_len();
            return Ok(());
        }
        let Some((matches, selected, original)) = self.wildmenu.as_mut() else {
            return Ok(());
        };
        let len = matches.len();
        if forward {
            *selected = (*selected + 1) % len;
        } else {
            *selected = (*selected + len - 1) % len;
        }
        let chosen = matches[*selected].clone();
        // Undo any previously inserted completion before applying the next one.
        if let Some(cmdline) = self.cmdline.as_mut() {
            cmdline.text = format!("{original}{chosen}");
            cmdline.cursor = cmdline.char_len();
        }
        Ok(())
    }

    fn execute_cmdline(&mut self) -> VimResult<()> {
        let Some(cmdline) = self.cmdline.take() else {
            return Ok(());
        };
        self.wildmenu = None;
        self.session.editor.mode = Mode::Normal;
        match cmdline.prompt {
            '/' | '?' => {
                let forward = cmdline.prompt == '/';
                // An empty pattern repeats the previous one (search.c
                // last_search / E35 when there is none).
                let pattern = if cmdline.text.is_empty() {
                    self.session.editor.search_pattern.clone()
                } else {
                    self.session.editor.search_pattern = cmdline.text.clone();
                    cmdline.text.clone()
                };
                if pattern.is_empty() {
                    return Err(VimError::new("E35", "No previous regular expression"));
                }
                self.search_forward = forward;
                self.search(&pattern, forward)?;
            }
            _ => {
                let line = cmdline.text.trim().to_string();
                if line.is_empty() {
                    return Ok(());
                }
                self.session
                    .viml
                    .env
                    .set_var("v:cmdline", rvim_viml::value::Value::String(line.clone()))
                    .ok();
                // An error already went through Editor::emsg (history), so
                // collect_messages below displays it exactly once.
                if let Ok(
                    rvim_ex::ExOutcome::Quit { exit_code, .. }
                    | rvim_ex::ExOutcome::QuitAll { exit_code, .. },
                ) = self.session.ex(&line)
                {
                    self.quit = true;
                    self.exit_code = exit_code;
                }
            }
        }
        self.collect_messages();
        Ok(())
    }

    /// Repeat the last search (`n` in the same direction, `N` reversed).
    fn repeat_search(&mut self, count: u64, same_direction: bool) -> VimResult<()> {
        let pattern = self.session.editor.search_pattern.clone();
        if pattern.is_empty() {
            return Err(VimError::new("E35", "No previous regular expression"));
        }
        let forward = if same_direction {
            self.search_forward
        } else {
            !self.search_forward
        };
        for _ in 0..count.max(1) {
            self.search(&pattern, forward)?;
        }
        Ok(())
    }

    /// Literal search (regex is M6), honouring 'ignorecase' + 'wrapscan'.
    ///
    /// Like vim, a `/` search starts one character *after* the cursor and a `?`
    /// search one character before it, so a match elsewhere on the current line
    /// is found before wrapping to the next line.
    fn search(&mut self, pattern: &str, forward: bool) -> VimResult<()> {
        let ignorecase = self.session.editor.options.bool_or("ignorecase", false);
        let needle = if ignorecase {
            pattern.to_ascii_lowercase()
        } else {
            pattern.to_string()
        };
        let total = self.session.editor.line_count();
        let cur = self.session.editor.cursor();
        let mut found = None;
        // total+1 steps: the last one rescans the starting line from its first
        // character, which is how a wrapped search can come back to a match the
        // cursor is already on (vim reports "search hit BOTTOM, continuing at
        // TOP" and stays there).
        for step in 0..=total {
            let line = if forward {
                (cur.line + step) % total
            } else {
                (cur.line + total - step % total) % total
            };
            let text = self.session.editor.current_buffer().line_str(line)?;
            let haystack = if ignorecase {
                text.to_ascii_lowercase()
            } else {
                text.to_string()
            };
            // Only the line with the cursor is searched from the cursor; after
            // wrapping the whole line counts (vim's searchit()).
            let (from, to) = if step == 0 {
                if forward {
                    (
                        rvim_core::text::move_chars(text, cur.col, 1),
                        haystack.len(),
                    )
                } else {
                    (0, text.len().min(cur.col))
                }
            } else {
                (0, haystack.len())
            };
            let window = &haystack[from..to];
            let hit = if forward {
                window.find(&needle)
            } else {
                window.rfind(&needle)
            };
            if let Some(offset) = hit {
                found = Some(Pos::new(line, from + offset));
                break;
            }
        }
        match found {
            Some(pos) => {
                self.session.editor.set_cursor(pos);
                Ok(())
            }
            None => Err(rvim_core::VimError::pattern_not_found(pattern)),
        }
    }

    // ------------------------------------------------------------ rendering

    /// Content rows of the current window (the last row of its frame is its
    /// status line).
    fn text_rows(&self) -> usize {
        let rows = self.session.editor.windows.current().rect.text_rows();
        if rows == 0 {
            self.screen_size.1.saturating_sub(2).max(1)
        } else {
            rows
        }
    }

    /// Scroll the current window by `delta` lines, keeping the cursor on the same
    /// screen row (normal.c nv_page).
    fn scroll_line(&mut self, delta: i64, _keep_row: bool) {
        let total = self.session.editor.line_count().max(1);
        let win = self.session.editor.windows.current();
        let row = win.cursor.line.saturating_sub(win.top_line);
        let top = (win.top_line as i64 + delta).clamp(0, (total - 1) as i64) as usize;
        let line = (top + row).min(total - 1);
        self.session.editor.windows.current_mut().top_line = top;
        let col = self.session.editor.cursor().col;
        let pos = self
            .session
            .editor
            .current_buffer()
            .clamp_pos(Pos::new(line, col));
        self.session.editor.set_cursor(pos);
    }

    /// Keep every window's viewport on its own cursor.
    fn scroll_windows(&mut self, cols: usize) {
        let _ = cols;
        let tabstop = self.tabstop();
        for id in self.session.editor.windows.ids() {
            let Some(win) = self.session.editor.windows.get(id) else {
                continue;
            };
            let rect = win.rect;
            let text_rows = rect.text_rows().max(1);
            let number = self.session.editor.wopt_for_bool(id, "number", false);
            let line_count = self
                .session
                .editor
                .buffer(win.bufnr)
                .map(|b| b.line_count())
                .unwrap_or(1);
            let gutter = if number {
                gutter_width(true, line_count)
            } else {
                0
            };
            let text_cols = rect.cols.saturating_sub(gutter).max(1);
            let cursor = win.cursor;
            let bufnr = win.bufnr;
            let mut top = win.top_line;
            let mut left = win.left;
            if cursor.line < top {
                top = cursor.line;
            } else if cursor.line >= top + text_rows {
                top = cursor.line + 1 - text_rows;
            }
            let line = self
                .session
                .editor
                .buffer(bufnr)
                .ok()
                .and_then(|b| b.line_str(cursor.line).ok())
                .unwrap_or("")
                .to_string();
            let cursor_cell = display_col(&line, cursor.col, tabstop);
            if cursor_cell < left {
                left = cursor_cell;
            } else if cursor_cell >= left + text_cols {
                left = cursor_cell + 1 - text_cols;
            }
            if let Some(window) = self.session.editor.windows.get_mut(id) {
                window.top_line = top;
                window.left = left;
            }
        }
    }

    /// 'showcmd' text: the (partial) command typed so far.
    ///
    /// vim shows the raw keys (normal.c add_to_showcmd): the count, the operator
    /// or prefix key, and in Visual mode the size of the selection.
    fn showcmd_text(&self) -> String {
        if !self.session.editor.options.bool_or("showcmd", false) {
            return String::new();
        }
        // Visual mode shows the selection size instead (normal.c clear_showcmd).
        if let Some(state) = self.session.editor.visual {
            let cursor = self.session.editor.cursor();
            let (top, bot) = if state.anchor.line <= cursor.line {
                (state.anchor.line, cursor.line)
            } else {
                (cursor.line, state.anchor.line)
            };
            let lines = bot - top + 1;
            return match state.mode {
                Mode::VisualBlock => {
                    let left = state.anchor.col.min(cursor.col);
                    let right = state.anchor.col.max(cursor.col);
                    format!("{lines}x{}", right - left + 1)
                }
                Mode::VisualLine => format!("{lines}"),
                _ if lines > 1 => format!("{lines}"),
                _ => {
                    // Inclusive selection: the cursor character counts too.
                    let lo = state.anchor.col.min(cursor.col);
                    let hi = state.anchor.col.max(cursor.col);
                    let chars = self
                        .session
                        .editor
                        .current_buffer()
                        .line_str(top)
                        .map(|line| {
                            let start = lo.min(line.len());
                            let end = if hi >= line.len() {
                                line.len()
                            } else {
                                let snapped = rvim_core::text::snap_byte(line, hi);
                                snapped
                                    + line[snapped..]
                                        .chars()
                                        .next()
                                        .map(char::len_utf8)
                                        .unwrap_or(0)
                            };
                            line[start..end.max(start)].chars().count()
                        })
                        .unwrap_or(0);
                    format!("{chars}")
                }
            };
        }
        // vim displays the typed keys: the operator count before the operator
        // ("2d"), the motion count after it ("d2"), and no count at all for 1.
        let operator_count = match self.session.editor.mode {
            Mode::MotionPending { count, .. } => count,
            _ => 1,
        };
        let pending_count = self.session.normal.pending_count().unwrap_or(0);
        let leading = if operator_count > 1 {
            operator_count
        } else if self.pending_op.is_none() && pending_count > 1 {
            pending_count
        } else {
            0
        };
        let mut out = String::new();
        // The register name typed before the operator ("ad) stays visible: the
        // engine parks it in the pending operator state.
        if let Mode::MotionPending {
            reg: Some(name), ..
        } = self.session.editor.mode
        {
            out.push('"');
            out.push(name);
        }
        if leading > 0 {
            out.push_str(&leading.to_string());
        }
        match &self.pending_op {
            Some(Pending::Operator(op)) => out.push(*op),
            Some(Pending::OperatorGg(op)) => {
                out.push(*op);
                out.push('g');
            }
            Some(Pending::TextObject(op, around)) => {
                out.push(*op);
                out.push(if *around { 'a' } else { 'i' });
            }
            Some(Pending::G) | Some(Pending::VisualG) => out.push('g'),
            Some(Pending::WindowCmd) => out.push_str("^W"),
            Some(Pending::Fold) => out.push('z'),
            Some(Pending::Bracket(forward)) => out.push(if *forward { ']' } else { '[' }),
            None => {}
        }
        // The register prefix is displayed while it is being typed (" or "a).
        if self.session.editor.mode == Mode::RegisterPending
            || self.session.normal.pending_register().is_some()
        {
            out.push('"');
            if let Some(name) = self.session.normal.pending_register() {
                out.push(name);
            }
        }
        // A count typed after the operator ("d2") follows it.
        if self.pending_op.is_some() && pending_count > 0 {
            out.push_str(&pending_count.to_string());
        }
        out
    }

    /// Tabline: one entry per tabpage, like vim`s `1:file 2:other`.
    fn tabline(&self, cols: usize) -> String {
        let ed = &self.session.editor;
        let current = ed.windows.current_tab_index();
        let mut out = String::new();
        for (index, tab) in ed.windows.tabs.iter().enumerate() {
            let name = ed
                .windows
                .get(tab.current)
                .and_then(|w| ed.buffer(w.bufnr).ok())
                .map(|b| b.display_name().to_string())
                .unwrap_or_default();
            let modified = ed
                .windows
                .get(tab.current)
                .and_then(|w| ed.buffer(w.bufnr).ok())
                .is_some_and(|b| b.is_changed());
            let marker = if modified { " +" } else { "" };
            if index == current {
                out.push_str(&format!(" {}:{} ", index + 1, name));
            } else {
                out.push_str(&format!(" {}:{}{} ", index + 1, name, marker));
            }
            if out.chars().count() >= cols {
                break;
            }
        }
        out.chars().take(cols).collect()
    }

    /// Build one window pane.
    fn pane_for(&self, id: WindowId, rect: Rect, tabstop: usize, active: bool) -> PaneView {
        let ed = &self.session.editor;
        let Some(win) = ed.windows.get(id) else {
            return PaneView::default();
        };
        let bufnr = win.bufnr;
        let line_count = ed.buffer(bufnr).map(|b| b.line_count()).unwrap_or(1);
        let number = ed.wopt_for_bool(id, "number", false);
        let number_width = if number {
            gutter_width(true, line_count)
        } else {
            0
        };
        let foldcolumn = self.foldcolumn_width(id);
        let (signcolumn, sign_text) = self.sign_column(id, bufnr, line_count);
        let mut signs: Vec<String> = Vec::new();
        let mut foldcol: Vec<String> = Vec::new();
        let text_rows = rect.text_rows().max(1);
        let mut content = Vec::with_capacity(text_rows);
        let mut cursor_row = None;
        let mut row = 0usize;
        let mut line = win.top_line;
        while row < text_rows {
            if line >= line_count {
                content.push(RowView::Eob);
                foldcol.push(self.fold_column_text(id, line, foldcolumn));
                signs.push(String::new());
                row += 1;
                continue;
            }
            // Diff filler rows keep both sides aligned (diff.c diff_check_fill).
            let (fillers, _changed) = ed.diff_filler(id, line);
            for _ in 0..fillers {
                if row >= text_rows {
                    break;
                }
                foldcol.push(self.fold_column_text(id, line, foldcolumn));
                signs.push(String::new());
                content.push(RowView::Filler("^".to_string()));
                row += 1;
            }
            if row >= text_rows {
                break;
            }
            if let Some((end, label)) = self.fold_row(id, line) {
                if line == win.cursor.line {
                    cursor_row = Some(row);
                }
                foldcol.push(self.fold_column_text(id, line, foldcolumn));
                signs.push(sign_text.get(line).cloned().unwrap_or_default());
                content.push(RowView::Fold {
                    start: line,
                    end,
                    label,
                });
                row += 1;
                line = end + 1;
                continue;
            }
            let text = ed
                .buffer(bufnr)
                .ok()
                .and_then(|b| b.line_str(line).ok())
                .unwrap_or("")
                .to_string();
            if line == win.cursor.line {
                cursor_row = Some(row);
            }
            foldcol.push(self.fold_column_text(id, line, foldcolumn));
            signs.push(sign_text.get(line).cloned().unwrap_or_default());
            content.push(RowView::Text { line, text });
            row += 1;
            line += 1;
        }

        let cursor = if active {
            let line = ed
                .buffer(bufnr)
                .ok()
                .and_then(|b| b.line_str(win.cursor.line).ok())
                .unwrap_or("")
                .to_string();
            cursor_row.map(|row| (row, display_col(&line, win.cursor.col, tabstop)))
        } else {
            None
        };

        PaneView {
            row: rect.row,
            col: rect.col,
            rows: rect.rows.max(2),
            cols: rect.cols,
            content,
            cursor,
            left: win.left,
            status: self.statusline_for(id, rect.cols, text_rows),
            number_width,
            foldcolumn,
            signcolumn,
            signs,
            foldcol,
            tabstop,
            selection: self.selection_for(
                id,
                rect,
                number_width + foldcolumn + signcolumn,
                tabstop,
            ),
            active,
        }
    }

    /// The closed fold starting at line in window id: (last line, label).
    fn fold_row(&self, id: WindowId, line: usize) -> Option<(usize, String)> {
        let ed = &self.session.editor;
        if !ed.wopt_for_bool(id, "foldenable", true) {
            return None;
        }
        let fold = ed
            .folds_for(id)
            .iter()
            .find(|f| f.closed && f.start == line)?;
        let bufnr = ed.windows.get(id).map(|w| w.bufnr)?;
        let text = ed
            .buffer(bufnr)
            .ok()
            .and_then(|b| b.line_str(fold.start).ok())
            .unwrap_or("");
        Some((fold.end, ed.fold_text(text, fold.end - fold.start + 1)))
    }

    /// Render the 'foldcolumn' cells for one displayed line.
    fn fold_column_text(&self, id: WindowId, line: usize, width: usize) -> String {
        if width == 0 {
            return String::new();
        }
        let ed = &self.session.editor;
        let folds = ed.folds_for(id);
        let mut marker = " ";
        if let Some(fold) = folds.iter().find(|f| f.start == line) {
            marker = if fold.closed { "+" } else { "-" };
        } else if folds
            .iter()
            .any(|f| !f.closed && f.start < line && line <= f.end)
        {
            marker = "|";
        }
        format!("{:>width$}", marker, width = width)
    }

    /// The sign column of one window: (width, text per buffer line).
    ///
    /// 'signcolumn' "auto" only reserves cells when the buffer has signs, which
    /// is what vim does by default (optionstr.c).
    fn sign_column(&self, id: WindowId, bufnr: i32, line_count: usize) -> (usize, Vec<String>) {
        let meta = &self.session.viml.state.meta;
        let mut per_line = vec![String::new(); line_count];
        let mut any = false;
        for place in &meta.sign_places {
            if place.bufnr != bufnr as i64 {
                continue;
            }
            let line = (place.lnum.max(1) - 1) as usize;
            if line >= line_count {
                continue;
            }
            let text = meta
                .sign_defs
                .iter()
                .find(|def| def.name == place.name)
                .and_then(|def| def.text.clone())
                .or_else(|| Some(">".to_string()))
                .unwrap_or_default();
            per_line[line] = text.chars().take(2).collect();
            any = true;
        }
        let mode = self
            .session
            .editor
            .wopt_for(id, "signcolumn")
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "auto".to_string());
        let width = match mode.as_str() {
            "no" => 0,
            "auto" => usize::from(any) * 2,
            "number" => 2,
            other => other
                .strip_prefix("auto:")
                .and_then(|n| n.parse::<usize>().ok())
                .map(|n| n * 2)
                .unwrap_or(2),
        };
        (width, per_line)
    }

    /// Visible popups as overlay rectangles (host_meta computes the geometry).
    fn popup_overlays(&self) -> Vec<(Vec<String>, rvim_term::pane::PopupRect)> {
        let mut out = Vec::new();
        for popup in &self.session.viml.state.meta.popups {
            if !popup.visible() {
                continue;
            }
            let rect = rvim_term::pane::PopupRect {
                row: (popup.line.max(1) - 1) as usize,
                col: (popup.col.max(1) - 1) as usize,
                rows: popup.height.max(1) as usize,
                cols: popup.width.max(1) as usize,
            };
            out.push((popup.text.clone(), rect));
        }
        out
    }

    /// 'foldcolumn' width in cells ("auto", "0", "3", ...).
    fn foldcolumn_width(&self, id: WindowId) -> usize {
        let value = self
            .session
            .editor
            .wopt_for(id, "foldcolumn")
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "0".to_string());
        if value == "auto" {
            return 1;
        }
        value.parse::<usize>().unwrap_or(0).min(12)
    }

    /// Feed one key while a visual selection is active.
    fn handle_visual_key(&mut self, key: Key) -> VimResult<()> {
        if let Some(pending) = self.pending_op.take() {
            return self.finish_pending_command(pending, key);
        }
        match &key {
            Key::Char(':') => {
                // vim writes the visual range into the command line.
                let prefix = rvim_normal::visual::ex_range_prefix(self.editor());
                self.cmdline = Some(Cmdline {
                    prompt: ':',
                    text: prefix.clone(),
                    cursor: prefix.chars().count(),
                });
                self.session.editor.mode = Mode::CommandLine;
                self.wildmenu = None;
                Ok(())
            }
            Key::Char(ch) => {
                let ch = *ch;
                // Digits accumulate a count for the next motion or block insert.
                if ch.is_ascii_digit() && !(ch == '0' && self.visual_count.is_none()) {
                    let digit = ch.to_digit(10).unwrap_or(0) as u64;
                    self.visual_count = Some(
                        self.visual_count
                            .unwrap_or(0)
                            .saturating_mul(10)
                            .saturating_add(digit),
                    );
                    return Ok(());
                }
                let count = self.visual_count.take().unwrap_or(1);
                match ch {
                    'd' | 'x' => {
                        let _ = rvim_normal::visual::apply_operator(self.editor(), 'd', None)?;
                    }
                    'y' => {
                        let _ = rvim_normal::visual::apply_operator(self.editor(), 'y', None)?;
                    }
                    'c' | 's' => {
                        let _ = rvim_normal::visual::apply_operator(self.editor(), 'c', None)?;
                    }
                    'p' | 'P' => self.paste_over_selection(ch)?,
                    // Block insert: I at the block's left edge, A after its right edge
                    // (vim only accepts these in blockwise visual mode).
                    'I' | 'A' => {
                        let blockwise = self.session.editor.mode == Mode::VisualBlock;
                        if blockwise {
                            let inserted =
                                rvim_normal::visual::apply_operator(self.editor(), ch, None)?;
                            if inserted {
                                // 3Itext<Esc> repeats the text three times per line.
                                self.session.normal.set_insert_repeat(count);
                            }
                        }
                    }
                    '>' | '<' | '~' | 'u' | 'U' | 'J' => {
                        let _ = rvim_normal::visual::apply_operator(self.editor(), ch, None)?;
                    }
                    'o' | 'O' => rvim_normal::visual::swap_ends(self.editor())?,
                    'i' | 'a' => {
                        self.pending_op = Some(Pending::TextObject(' ', ch == 'a'));
                    }
                    'g' => self.pending_op = Some(Pending::VisualG),
                    'z' => self.pending_op = Some(Pending::Fold),
                    _ => {
                        if let Some(motion) = visual_motion(ch) {
                            let pos = visual_move(self.editor(), motion, count)?;
                            self.session.editor.set_cursor(pos);
                        }
                    }
                }
                Ok(())
            }
            Key::Special(name) => {
                let motion = match name.as_str() {
                    "LEFT" => 'h',
                    "RIGHT" => 'l',
                    "UP" => 'k',
                    "DOWN" => 'j',
                    "HOME" => '0',
                    "END" => '$',
                    _ => return Ok(()),
                };
                let pos = visual_move(self.editor(), motion, 1)?;
                self.session.editor.set_cursor(pos);
                Ok(())
            }
            Key::Nop => Ok(()),
        }
    }

    /// ]c / [c: jump to the next/previous changed line of the current window.
    fn diff_jump(&mut self, forward: bool) -> VimResult<()> {
        let ed = &mut self.session.editor;
        let win = ed.windows.current_id();
        let line = ed.cursor().line;
        let Some(target) = ed.diff_next_change(win, line, forward) else {
            ed.echo("No diff changes");
            return Ok(());
        };
        let col = ed.cursor().col;
        let pos = ed.current_buffer().clamp_pos(Pos::new(target, col));
        ed.set_cursor(pos);
        Ok(())
    }

    /// Apply an operator to a text object (diw, yap, ...).
    fn apply_text_object(&mut self, op: char, object: char, around: bool) -> VimResult<()> {
        let which = if around {
            rvim_normal::textobjects::Which::Around
        } else {
            rvim_normal::textobjects::Which::Inner
        };
        let lines: Vec<String> = (0..self.session.editor.line_count())
            .filter_map(|i| {
                self.session
                    .editor
                    .current_buffer()
                    .line_str(i)
                    .ok()
                    .map(str::to_string)
            })
            .collect();
        let cursor = self.session.editor.cursor();
        let Some(found) =
            rvim_normal::textobjects::text_object(&lines, (cursor.line, cursor.col), object, which)
        else {
            self.session.editor.mode = Mode::Normal;
            return Ok(());
        };
        // Text objects report the byte offset of their last character; the
        // operators take an exclusive end (a linewise object covers whole lines).
        let range = if found.linewise {
            rvim_normal::operators::line_range(
                found.start_line,
                found.end_line,
                self.session.editor.line_count(),
            )
        } else {
            let text = self
                .session
                .editor
                .current_buffer()
                .line_str(found.end_line)
                .unwrap_or("")
                .to_string();
            let end_col = if found.end_col >= text.len() {
                text.len()
            } else {
                rvim_core::text::snap_byte(&text, found.end_col)
                    + text[rvim_core::text::snap_byte(&text, found.end_col)..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(0)
            };
            rvim_normal::operators::OpRange {
                start: Pos::new(found.start_line, found.start_col),
                end: Pos::new(found.end_line, end_col),
                linewise: false,
            }
        };
        match op {
            // In visual mode the object extends the selection instead of being
            // consumed by an operator.
            ' ' => {
                rvim_normal::visual::select_object(self.editor(), object, which)?;
            }
            'd' => {
                rvim_normal::operators::delete_range(self.editor(), range, None)?;
                self.session.editor.mode = Mode::Normal;
            }
            'y' => {
                rvim_normal::operators::yank_range(self.editor(), range, None)?;
                self.session.editor.mode = Mode::Normal;
            }
            'c' => {
                self.session.editor.begin_undo_block();
                rvim_normal::operators::change_range(self.editor(), range, None)?;
                self.session.editor.mode = Mode::Insert;
            }
            _ => self.session.editor.mode = Mode::Normal,
        }
        // A visual-mode object keeps the selection; operators end it.
        if op != 'c' && op != ' ' {
            self.session.editor.visual = None;
        }
        Ok(())
    }

    /// p/P over a visual selection: replace the selection with the register.
    fn paste_over_selection(&mut self, key: char) -> VimResult<()> {
        let Some(range) = rvim_normal::visual::range(&self.session.editor) else {
            return Ok(());
        };
        let data = self
            .session
            .editor
            .registers
            .unnamed()
            .cloned()
            .or_else(|| self.session.editor.registers.get('"').cloned());
        let Some(data) = data else {
            return Ok(());
        };
        self.session.editor.begin_undo_block();
        rvim_normal::operators::delete_range(&mut self.session.editor, range, None).ok();
        self.session.editor.end_undo_block();
        self.session.editor.visual = None;
        self.session.editor.mode = Mode::Normal;
        self.session.editor.registers.yank(None, data);
        rvim_normal::operators::paste(self.editor(), key, None)?;
        Ok(())
    }

    /// Visual selection of window id, in pane cell coordinates.
    fn selection_for(
        &self,
        id: WindowId,
        _rect: Rect,
        _gutter: usize,
        tabstop: usize,
    ) -> Option<SelectionView> {
        let ed = &self.session.editor;
        let state = ed.visual?;
        let win = ed.windows.get(id)?;
        if win.id != ed.windows.current_id() {
            return None;
        }
        let cursor = ed.cursor();
        let (start, end) = if (state.anchor.line, state.anchor.col) <= (cursor.line, cursor.col) {
            (state.anchor, cursor)
        } else {
            (cursor, state.anchor)
        };
        let line_col = |line: usize, col: usize| -> usize {
            let text = ed
                .buffer(win.bufnr)
                .ok()
                .and_then(|b| b.line_str(line).ok())
                .unwrap_or("");
            display_col(text, col, tabstop)
        };
        Some(SelectionView {
            start_row: start.line.saturating_sub(win.top_line),
            start_col: line_col(start.line, start.col).saturating_sub(0),
            end_row: end.line.saturating_sub(win.top_line),
            end_col: line_col(end.line, end.col).saturating_sub(0),
            block: state.mode == Mode::VisualBlock,
        })
    }

    pub fn grid(&mut self, cols: usize, rows: usize) -> Grid {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let tabpages = self.session.editor.tabpage_count();
        let tabline_rows = usize::from(tabpages > 1);
        let avail = rows.saturating_sub(1 + tabline_rows).max(2);
        // Fold levels depend on the buffer and the window options, so they are
        // recomputed once per frame (O(lines) for indent/marker folds).
        let _ = self.session.editor.refresh_folds_for_current_window();
        // Changed lines of diff windows are cached per changedtick; refresh the
        // cache once per frame (vim's tp_diff_invalid path).
        let _ = self.session.editor.diff_update();
        self.session.editor.layout_windows(avail, cols);
        self.scroll_windows(cols);
        let tabstop = self.tabstop();
        let current = self.session.editor.windows.current_id();
        let ids = self.session.editor.windows.ids();
        let rects = self.session.editor.windows.rects().clone();
        let mut panes = Vec::with_capacity(ids.len());
        for id in ids {
            let rect = rects.get(&id).copied().unwrap_or_default();
            panes.push(self.pane_for(id, rect, tabstop, id == current));
        }
        let message = self.message.clone();
        let cmdline = self.cmdline.as_ref().map(|c| CmdlineText {
            prompt: c.prompt,
            text: c.text.clone(),
            cursor: c.cursor,
        });
        let wildmenu = self
            .wildmenu
            .as_ref()
            .map(|(items, selected, _)| WildmenuItems {
                items: items.clone(),
                selected: *selected,
            });
        let tabline = if tabline_rows > 0 {
            Some(self.tabline(cols))
        } else {
            None
        };
        let model = ScreenLayout {
            panes,
            tabline,
            message,
            cmdline,
            wildmenu,
            more: !self.message_queue.is_empty(),
            tabstop,
            popups: self.popup_overlays(),
            showcmd: self.showcmd_text(),
        };
        render_layout(&model, cols, rows, &self.theme)
    }

    /// Screen as plain text - used by the `--script` driver and the tests.
    pub fn screen_text(&mut self, cols: usize, rows: usize) -> String {
        self.grid(cols, rows).text()
    }

    /// `'tabstop'` as a cell stride (option validation keeps it >= 1).
    fn tabstop(&self) -> usize {
        self.session
            .editor
            .options
            .num_or("tabstop", 8)
            .clamp(1, 1000) as usize
    }

    /// Statusline of one window: file name and flags on the left, the ruler on
    /// the right (vim right-aligns it so it stays visible in narrow windows).
    pub fn statusline_for(&self, id: WindowId, cols: usize, text_rows: usize) -> String {
        let ed = &self.session.editor;
        let Some(win) = ed.windows.get(id) else {
            return String::new();
        };
        let buffer = ed.buffer(win.bufnr).ok();
        let name = buffer
            .map(|b| b.display_name().to_string())
            .unwrap_or_default();
        let modified = if buffer.is_some_and(|b| b.is_changed()) {
            " [+]"
        } else {
            ""
        };
        let left = format!("{name}{modified}");
        let line = win.cursor.line + 1;
        let col = win.cursor.col + 1;
        let mode = ed.mode.vim_name();
        let total = buffer.map(|b| b.line_count()).unwrap_or(1).max(1);
        let above = win.top_line as i64;
        let below = total as i64 - win.top_line as i64 - text_rows as i64;
        let percent = if below <= 0 {
            if above == 0 { "All" } else { "Bot" }.to_string()
        } else if above <= 0 {
            "Top".to_string()
        } else {
            format!("{}%", above * 100 / (above + below))
        };
        let right = format!("{line},{col}  {percent}  [{mode}]");
        let left_len = left.chars().count();
        let right_len = right.chars().count();
        if left_len + right_len + 1 > cols {
            return right;
        }
        let pad = cols - left_len - right_len;
        format!("{left}{}{right}", " ".repeat(pad))
    }

    /// Statusline of the current window (kept for the `--script` driver).
    pub fn statusline(&mut self, cols: usize, rows: usize) -> String {
        let id = self.session.editor.windows.current_id();
        let text_rows = self.text_rows().max(rows.saturating_sub(2)).min(rows);
        self.statusline_for(id, cols, text_rows)
    }

    /// Put a message on the message line, queueing it when one is showing.
    pub fn push_message(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self.message.is_empty() && self.message_queue.is_empty() {
            self.message = text;
        } else if !(self.message == text && self.message_queue.is_empty()) {
            self.message_queue.push_back(text);
        }
    }

    /// Pull anything the editor logged since the last frame.
    pub fn collect_messages(&mut self) {
        let history = self.session.message_history();
        // ':silent' truncates the history; never stay stuck past its end.
        if history.len() < self.seen_messages {
            self.seen_messages = history.len();
        }
        if history.len() > self.seen_messages {
            for line in history[self.seen_messages..].iter() {
                if !line.is_empty() {
                    self.message_queue.push_back(line.clone());
                }
            }
            self.seen_messages = history.len();
        }
        // `:echo` only sets the transient status text (message.c), which vim
        // shows on the message line just like history output. Errors and `:p`
        // are recorded in both places, so skip a duplicate here.
        if let Some(status) = self.session.editor.messages.take_status() {
            if self.session.message_history().last() != Some(&status) {
                self.push_message(status);
            }
        }
        if self.message.is_empty() {
            if let Some(next) = self.message_queue.pop_front() {
                self.message = next;
            }
        }
    }

    /// More-prompt: show the next queued message (bound to <CR> on the message
    /// line, like vim's hit-enter prompt).
    pub fn next_message(&mut self) -> bool {
        self.message = self.message_queue.pop_front().unwrap_or_default();
        !self.message.is_empty()
    }

    // ------------------------------------------------------------ main loop

    /// Read keys from `source`, paint to `out` and return the exit code.
    pub fn run<S: KeySource, W: std::io::Write>(
        &mut self,
        source: &mut S,
        out: &mut W,
        cols: usize,
        rows: usize,
    ) -> std::io::Result<i32> {
        let mut cols = cols.max(1);
        let mut rows = rows.max(1);
        self.screen_size = (cols, rows);
        out.write_all(b"\x1b[?1049h")?; // alternate screen
        out.write_all(b"\x1b[?25h")?;
        out.write_all(self.grid(cols, rows).to_ansi().as_bytes())?;
        out.flush()?;
        loop {
            // A timeout (rather than a blocking read) lets us notice a resize.
            let input = source.next_input(std::time::Duration::from_millis(120))?;
            match &input {
                Input::Key(key) => {
                    // feed_key() dismisses a pending message (hit-enter / more).
                    if let Err(e) = self.feed_key(key.clone()) {
                        self.session.record_error(e.to_string());
                    }
                    self.collect_messages();
                }
                Input::Timeout => {
                    if let Err(e) = self.flush_pending_timeout() {
                        self.session.record_error(e.to_string());
                    }
                }
                Input::Eof => {}
            }
            // Follow a terminal resize: new geometry, full repaint.
            if let Some((new_cols, new_rows)) = source.size() {
                if (new_cols, new_rows) != (cols, rows) && new_cols > 0 && new_rows > 0 {
                    cols = new_cols;
                    rows = new_rows;
                    self.screen_size = (cols, rows);
                    self.last_frame = None;
                    // Clear stale cells from the old, larger screen.
                    out.write_all(b"\x1b[2J")?;
                }
            }
            if self.quit || matches!(input, Input::Eof) {
                break;
            }
            let next = self.grid(cols, rows);
            let paint = match &self.last_frame {
                Some(prev) => next.diff_ansi(prev),
                None => next.to_ansi(),
            };
            if !paint.is_empty() {
                out.write_all(paint.as_bytes())?;
                out.flush()?;
            }
            self.last_frame = Some(next);
        }
        out.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l")?; // leave alternate screen
        out.flush()?;
        Ok(self.exit_code)
    }
}

enum Expand {
    Waiting,
    Replaced(Vec<Key>),
    Literal(Vec<Key>),
}

/// Map a decoded key onto the character the normal engine uses for that motion.
///
/// The special keys are the motions vim binds them to (normal.c nv_left etc.),
/// so `d<Right>` is `dl` and `d<End>` is `d$`.
fn motion_key(key: &Key) -> Option<char> {
    match key {
        Key::Char(ch) => Some(*ch),
        Key::Special(name) => match name.as_str() {
            "RIGHT" => Some('l'),
            "LEFT" => Some('h'),
            "DOWN" => Some('j'),
            "UP" => Some('k'),
            "HOME" => Some('0'),
            "END" => Some('$'),
            _ => None,
        },
        Key::Nop => None,
    }
}

/// Motions that extend a visual selection.
fn visual_motion(ch: char) -> Option<char> {
    match ch {
        'h' | 'j' | 'k' | 'l' | 'w' | 'W' | 'b' | 'B' | 'e' | 'E' | '0' | '$' | '^' | 'G' => {
            Some(ch)
        }
        _ => None,
    }
}

/// Move the cursor for a visual-mode motion.
fn visual_move(ed: &mut rvim_editor::Editor, motion: char, count: u64) -> VimResult<Pos> {
    match motion {
        'w' | 'W' | 'b' | 'B' | 'e' | 'E' => rvim_normal::motions::word_motion(ed, motion, count),
        '$' => rvim_normal::motions::line_end(ed, count),
        'G' => rvim_normal::motions::goto_line(ed, count, true),
        '0' => Ok(Pos::new(ed.cursor().line, 0)),
        '^' => {
            let line = ed.cursor().line;
            let col = ed.current_buffer().first_nonblank_byte(line)?;
            Ok(Pos::new(line, col))
        }
        other => rvim_normal::motions::move_cursor(ed, other, count),
    }
}

/// Map mode for the editor's current mode (vim's mode families).
fn map_mode_for(mode: Mode) -> Option<MapMode> {
    match mode {
        Mode::Normal
        | Mode::OperatorPending
        | Mode::MotionPending { .. }
        | Mode::RegisterPending => Some(MapMode::Normal),
        Mode::Insert | Mode::Replace => Some(MapMode::Insert),
        Mode::Visual | Mode::VisualLine | Mode::VisualBlock | Mode::Select => Some(MapMode::Visual),
        Mode::CommandLine => Some(MapMode::Cmdline),
        Mode::Terminal => None,
    }
}
