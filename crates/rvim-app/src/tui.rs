//! Interactive TUI driver (M4): key loop, mode dispatch, command line, mapping
//! expansion and screen composition.
//!
//! The engine is deliberately independent of the terminal: it consumes
//! [`Key`]s and produces a [`Grid`], so the same code runs against a real
//! terminal (via `rvim-termio`) and against a scripted key list in tests.

use std::collections::VecDeque;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::Mode;
use rvim_editor::maps::{Key, MapMode};
use rvim_normal::{NormalEngine, insert};
use rvim_term::{
    CmdlineView, Grid, Input, KeySource, ScreenModel, Theme, WildmenuView, display_col,
    gutter_width, render,
};

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
    /// First visible buffer line (vertical scroll).
    pub top: usize,
    /// Horizontal scroll in display cells.
    pub left: usize,
    pub cmdline: Option<Cmdline>,
    /// Keys buffered while deciding whether a mapping applies.
    pending_keys: Vec<Key>,
    /// Second key of a two-key normal command (`dd`, `yy`, `gg`, `dw`).
    pending_op: Option<Pending>,
    /// Direction of the last `/` or `?` search (`n` repeats it).
    search_forward: bool,
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
            top: 0,
            left: 0,
            cmdline: None,
            pending_keys: Vec::new(),
            pending_op: None,
            search_forward: true,
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
            if self.session.editor.mode.is_insert_like() {
                self.finish_insert_mode();
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
            Key::Char('/') => {
                self.start_cmdline('/');
                return Ok(());
            }
            // `?` searches backwards, `n`/`N` repeat the last search.
            Key::Char('?') => {
                self.start_cmdline('?');
                return Ok(());
            }
            Key::Char('n') | Key::Char('N') => {
                let same_direction = matches!(&key, Key::Char('n'));
                let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
                let count = engine.take_pending_count();
                self.session.normal = engine;
                return self.repeat_search(count, same_direction);
            }
            Key::Char(ch) => {
                let ch = *ch;
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
                // <PageDown>/<C-f> and <PageUp>/<C-b> scroll one screen and keep
                // the cursor in the same screen row (normal.c nv_page).
                "PAGEDOWN" | "C-F" => {
                    let page = self.screen_size.1.saturating_sub(2).max(1);
                    let total = self.session.editor.line_count();
                    let row = self.session.editor.cursor().line.saturating_sub(self.top);
                    self.top = (self.top + page).min(total.saturating_sub(1));
                    let line = (self.top + row).min(total.saturating_sub(1));
                    let pos = self.session.editor.cursor();
                    let pos = self
                        .session
                        .editor
                        .current_buffer()
                        .clamp_pos(Pos::new(line, pos.col));
                    self.session.editor.set_cursor(pos);
                    Ok(true)
                }
                "PAGEUP" | "C-B" => {
                    let page = self.screen_size.1.saturating_sub(2).max(1);
                    let row = self.session.editor.cursor().line.saturating_sub(self.top);
                    self.top = self.top.saturating_sub(page);
                    let line = self.top + row;
                    let pos = self.session.editor.cursor();
                    let pos = self
                        .session
                        .editor
                        .current_buffer()
                        .clamp_pos(Pos::new(line, pos.col));
                    self.session.editor.set_cursor(pos);
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
                    engine.feed_pair(self.editor(), 'g', 'g')
                } else {
                    Ok(())
                };
                self.session.normal = engine;
                if !matches!(&key, Key::Char('g')) {
                    self.session.editor.mode = Mode::Normal;
                }
                result
            }
            Pending::Operator(op) => {
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
                    // <CR> splits the line at the cursor; the text before stays
                    // on the current line and the cursor moves to the new one.
                    "CR" => {
                        let pos = self.editor().cursor();
                        let line = self.editor().current_line();
                        let before = line[..pos.col].to_string();
                        let after = line[pos.col..].to_string();
                        let result = self
                            .editor()
                            .replace_lines(pos.line, pos.line + 1, vec![before, after])
                            .map(|_| ());
                        if result.is_ok() {
                            self.editor().set_cursor(Pos::new(pos.line + 1, 0));
                        }
                        result
                    }
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
        let mut engine = std::mem::replace(&mut self.session.normal, NormalEngine::new());
        engine.finish_insert(self.editor());
        self.session.normal = engine;
        // vim leaves the cursor on the last inserted character.
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

    pub fn grid(&mut self, cols: usize, rows: usize) -> Grid {
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
        let text_rows = rows.saturating_sub(2).max(1);
        let tabstop = self.tabstop();

        // Vertical scroll follows the cursor.
        if cursor.line < self.top {
            self.top = cursor.line;
        } else if cursor.line >= self.top + text_rows {
            self.top = cursor.line + 1 - text_rows;
        }
        // Horizontal scroll follows the cursor. The 'number' gutter is not part
        // of the scrollable text area, so it is subtracted from the width.
        let number = self.session.editor.options.bool_or("number", false);
        let gutter = gutter_width(number, lines.len());
        let text_cols = cols.saturating_sub(gutter).max(1);
        let cursor_text = lines.get(cursor.line).map(String::as_str).unwrap_or("");
        let cursor_col = display_col(cursor_text, cursor.col, tabstop);
        if cursor_col < self.left {
            self.left = cursor_col;
        } else if cursor_col >= self.left + text_cols {
            self.left = cursor_col + 1 - text_cols;
        }

        let status = self.statusline(cols, rows);
        let message = self.message.clone();
        let cmdline = self.cmdline.as_ref().map(|c| CmdlineView {
            prompt: c.prompt,
            text: &c.text,
            cursor: c.cursor,
        });
        let wildmenu = self
            .wildmenu
            .as_ref()
            .map(|(items, selected, _)| WildmenuView {
                items,
                selected: *selected,
            });
        let model = ScreenModel {
            lines: &lines,
            cursor: (cursor.line, cursor.col),
            top: self.top,
            left: self.left,
            status: &status,
            message: &message,
            cmdline,
            wildmenu,
            show_eob: true,
            number,
            tabstop,
            more: !self.message_queue.is_empty(),
        };
        render(&model, cols, rows, &self.theme)
    }

    /// Screen as plain text — used by the `--script` driver and the tests.
    pub fn screen_text(&mut self, cols: usize, rows: usize) -> String {
        self.grid(cols, rows).text()
    }

    /// 'tabstop' as a cell stride (option validation keeps it >= 1).
    fn tabstop(&self) -> usize {
        self.session
            .editor
            .options
            .num_or("tabstop", 8)
            .clamp(1, 1000) as usize
    }

    /// Statusline: file name on the left, `line,col  percent  [MODE]` on the
    /// right (vim right-aligns the ruler so it stays visible in narrow windows).
    pub fn statusline(&mut self, cols: usize, rows: usize) -> String {
        let name = self
            .session
            .editor
            .current_buffer()
            .display_name()
            .to_string();
        let modified = if self.session.editor.current_buffer().is_changed() {
            " [+]"
        } else {
            ""
        };
        let left = format!("{name}{modified}");
        let line = self.session.editor.cursor().line + 1;
        let col = self.session.editor.cursor().col + 1;
        let mode = self.session.editor.mode.vim_name();
        let total = self.session.editor.line_count().max(1);
        // get_rel_pos() (buffer.c): "All" when the whole buffer fits, "Top"/"Bot"
        // at the ends, otherwise the percentage of lines above the window.
        let text_rows = rows.saturating_sub(2).max(1);
        let above = self.top as i64;
        let below = total as i64 - self.top as i64 - text_rows as i64;
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
            // Too narrow: keep the position/mode visible and clip the name.
            return right;
        }
        let pad = cols - left_len - right_len;
        format!("{left}{}{right}", " ".repeat(pad))
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
