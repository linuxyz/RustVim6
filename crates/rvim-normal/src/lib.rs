//! rvim-normal: headless normal-mode command engine.
//!
//! Drives an Editor the way normal.c does: counts, the quote register
//! prefix, motions, operators and insert-mode entry. The TUI (M4) will feed raw
//! key codes into the same engine; everything here is UI-free and deterministic.

pub mod addsub;
pub mod foldcmd;
pub mod insert;
pub mod motions;
pub mod operators;
pub mod textobjects;
pub mod visual;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::maps::Key;
use rvim_editor::{Editor, Mode, RegType, RepeatChange};

/// Sentinel stored while waiting for the mark name after the "m" key.
const MARK_PENDING: char = '\u{0000}';

/// True when the character under the cursor is not a blank (the condition vim
/// uses to decide whether "cw" behaves like "ce").
fn cursor_on_nonblank(ed: &Editor) -> bool {
    let pos = ed.cursor();
    match ed.current_buffer().line_str(pos.line) {
        Ok(line) => line
            .get(pos.col.min(line.len())..)
            .and_then(|rest| rest.chars().next())
            .map(|ch| !rvim_core::charclass::is_blank(ch))
            .unwrap_or(false),
        Err(_) => false,
    }
}

#[derive(Debug, Default)]
pub struct NormalEngine {
    count: Option<u64>,
    pending_register: Option<char>,
    /// Text typed during the current insert session, for the "." register.
    insert_text: String,
    /// Keys typed during the current insert session (vim's redo buffer). The
    /// session is replayed key by key, so <BS>/<C-w>/<C-u> repeat and counts
    /// behave like the original keystrokes (edit.c start_redo_ins()).
    pub(crate) insert_keys: Vec<Key>,
    /// `<C-r>` was typed: the next key names the register to insert.
    pub(crate) insert_pending_register: bool,
    /// Position where the current insert session started (edit.c Insstart).
    pub(crate) insert_start: Pos,
    /// Count given to the insert command (`3i`, `3o`, ...); vim repeats the
    /// typed text that many times when <Esc> is pressed (edit.c ins_esc()).
    insert_repeat: u64,
    /// Which insert entry was used (i/a/A/I/o/O), needed to repeat correctly.
    insert_entry: char,
    /// Count typed in visual mode (\`v3j\`, \`2I\`).
    visual_count: Option<u64>,
}

impl NormalEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn take_count(&mut self) -> u64 {
        self.count.take().unwrap_or(1)
    }

    fn take_register(&mut self) -> Option<char> {
        self.pending_register.take()
    }

    /// Take a count typed ahead of a command the front-end handles itself
    /// (`3n` repeats a search three times).
    pub fn take_pending_count(&mut self) -> u64 {
        self.take_count()
    }

    /// The count typed so far, without consuming it (`showcmd` displays it).
    pub fn pending_count(&self) -> Option<u64> {
        self.count
    }

    /// The register name typed after `"`, without consuming it.
    pub fn pending_register(&self) -> Option<char> {
        self.pending_register
    }

    /// Set the repeat count for the next insert session (used by visual-block
    /// `I`/`A`, where the front-end owns the count, and by `.` replay).
    pub fn set_insert_repeat(&mut self, count: u64) {
        self.insert_repeat = count.max(1);
    }

    /// Visual-mode count typed so far (digits accumulate like normal mode).
    pub fn visual_count(&self) -> Option<u64> {
        self.visual_count
    }

    /// Accumulate one visual-mode count digit.
    pub fn push_visual_digit(&mut self, ch: char) {
        let digit = ch.to_digit(10).unwrap_or(0) as u64;
        let cur = self.visual_count.unwrap_or(0);
        self.visual_count = Some(cur.saturating_mul(10).saturating_add(digit));
    }

    /// Consume the visual-mode count (defaults to 1, like `take_count`).
    pub fn take_visual_count(&mut self) -> u64 {
        self.visual_count.take().unwrap_or(1)
    }

    /// Drop a visual-mode count (`<Esc>`).
    pub fn clear_visual_count(&mut self) {
        self.visual_count = None;
    }

    /// Drop a count or register prefix typed before <Esc> (normal.c nv_esc).
    pub fn cancel_pending(&mut self) {
        self.count = None;
        self.pending_register = None;
    }

    /// Feed one normal-mode key (printable ASCII). Returns Ok(true) when the key
    /// was consumed, Ok(false) when it is not bound yet.
    pub fn feed(&mut self, ed: &mut Editor, key: char) -> VimResult<bool> {
        if !matches!(
            ed.mode,
            Mode::Normal | Mode::OperatorPending | Mode::RegisterPending
        ) {
            return Err(VimError::internal("feed() called outside normal mode"));
        }
        // register prefix
        if key == '"' {
            ed.mode = Mode::RegisterPending;
            return Ok(true);
        }
        if ed.mode == Mode::RegisterPending {
            if self.pending_register == Some(MARK_PENDING) {
                self.complete_mark(ed, key)?;
                return Ok(true);
            }
            self.pending_register = Some(key);
            ed.mode = Mode::Normal;
            return Ok(true);
        }
        // count digits — except a leading `0`, which is the "column 1" motion
        // (vim only treats `0` as a count digit once a count has started, so
        // `10j` works while `0` still moves to the first column).
        if key.is_ascii_digit() && !(key == '0' && self.count.is_none()) {
            let digit = key.to_digit(10).unwrap() as u64;
            let cur = self.count.unwrap_or(0);
            self.count = Some(cur.saturating_mul(10).saturating_add(digit));
            return Ok(true);
        }

        let count = self.take_count();
        let reg = self.take_register();
        match key {
            'h' | 'j' | 'k' | 'l' | ' ' => {
                let pos = motions::move_cursor(ed, key, count)?;
                ed.set_cursor(pos);
            }
            '0' => ed.set_cursor(Pos::new(ed.cursor().line, 0)),
            '$' => {
                let pos = motions::line_end(ed, count)?;
                ed.set_cursor(pos);
            }
            '^' | '_' => {
                let line = ed.cursor().line;
                let col = ed.current_buffer().first_nonblank_byte(line)?;
                ed.set_cursor(Pos::new(line, col));
            }
            'G' => {
                let pos = motions::goto_line(ed, count, true)?;
                ed.set_cursor(pos);
            }
            'g' => {
                // two-key "gg": feed_pair handles the second key
                self.count = Some(count);
                ed.pending_count = count;
                ed.mode = Mode::OperatorPending;
                return Ok(true);
            }
            'w' | 'W' | 'b' | 'B' | 'e' | 'E' => {
                let pos = motions::word_motion(ed, key, count)?;
                ed.set_cursor(pos);
            }
            'x' => {
                operators::delete_char(ed, count, reg)?;
                ed.last_change = Some(RepeatChange::Simple { key: 'x', count });
            }
            // CTRL-A / CTRL-X: add to or subtract from the number under the
            // cursor (normal.c nv_addsub -> ops.c do_addsub).
            '\u{1}' | '\u{18}' => {
                let add = key == '\u{1}';
                if addsub::addsub(ed, add, count)? {
                    ed.last_change = Some(RepeatChange::Simple { key, count });
                }
            }
            // "." repeats the last change (normal.c nv_dot).
            '.' => self.repeat_last_change(ed, count)?,
            'd' | 'y' | 'c' => return self.operator_prefix(ed, key, count, reg),
            // D/C/Y are the one-key forms of d$/c$/yy (normal.c nv_D/nv_C/nv_Y).
            'D' | 'C' => {
                // A count moves down that many lines, like d<count>$ (nv_dollar).
                let start = ed.cursor();
                let last = (start.line + count as usize - 1).min(ed.line_count() - 1);
                let end = ed.current_buffer().line_str(last)?.len();
                let range = operators::OpRange {
                    start,
                    end: Pos::new(last, end),
                    linewise: false,
                };
                let op = if key == 'D' { 'd' } else { 'c' };
                self.apply_operator(ed, op, range, reg)?;
                ed.last_change = Some(RepeatChange::Simple { key, count });
                return Ok(true);
            }
            'Y' => {
                let line = ed.cursor().line;
                let range = operators::line_range(line, line + count as usize - 1, ed.line_count());
                operators::yank_range(ed, range, reg)?;
                ed.mode = Mode::Normal;
                return Ok(true);
            }
            'p' | 'P' => {
                operators::paste(ed, key, reg)?;
                ed.last_change = Some(RepeatChange::Paste { key, reg });
            }
            'u' => ed.undo()?,
            'm' => {
                ed.mode = Mode::RegisterPending;
                self.pending_register = Some(MARK_PENDING);
                return Ok(true);
            }
            'i' | 'a' | 'A' | 'o' | 'O' | 'I' => {
                // The block must start *before* the entry edit (o/O insert a line)
                // so the whole session becomes one undo step.
                ed.begin_undo_block();
                insert::enter_insert(ed, key)?;
                let start = ed.cursor();
                self.start_insert_session(start, count, key);
                // enter_insert switches to Mode::Insert; do not fall through to
                // the unconditional Normal reset below.
                return Ok(true);
            }
            _ => return Ok(false),
        }
        ed.mode = Mode::Normal;
        Ok(true)
    }

    fn operator_prefix(
        &mut self,
        ed: &mut Editor,
        op: char,
        count: u64,
        reg: Option<char>,
    ) -> VimResult<bool> {
        ed.mode = Mode::MotionPending { op, count, reg };
        Ok(true)
    }

    /// Handle the second key of a two-key command (gg, dd, yy, dw, ...).
    pub fn feed_pair(&mut self, ed: &mut Editor, first: char, second: char) -> VimResult<()> {
        if first == 'g' {
            // The count was already consumed when the g key was pressed and is
            // carried by Mode::OperatorPending/self.count (e.g. 3gg).
            let count = self.take_count();
            ed.set_cursor(motions::goto_line(ed, count, false)?);
            ed.mode = Mode::Normal;
            return Ok(());
        }
        if !self.feed_operator_key(ed, first, second)? {
            ed.mode = Mode::Normal;
        }
        Ok(())
    }

    /// Feed one key to a pending operator (normal.c do_pending_operator()).
    ///
    /// Digits extend the motion count (d2w), the operator's own key makes a
    /// line-wise command (dd/yy/cc) and every other key is a motion. Returns
    /// Ok(false) when the key cannot be used, so the caller can cancel like
    /// <Esc>.
    pub fn feed_operator_key(&mut self, ed: &mut Editor, op: char, key: char) -> VimResult<bool> {
        if !matches!(op, 'd' | 'y' | 'c') {
            return Ok(false);
        }
        // A count typed after the operator (d2w) multiplies the operator count.
        if key.is_ascii_digit() && !(key == '0' && self.count.is_none()) {
            let digit = key.to_digit(10).unwrap() as u64;
            self.count = Some(
                self.count
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(digit),
            );
            return Ok(true);
        }
        let motion_count = self.count.take().unwrap_or(1);
        let (op_count, reg) = match ed.mode {
            Mode::MotionPending { count, reg, .. } => (count.max(1), reg),
            _ => (1, self.take_register()),
        };
        let count = op_count.saturating_mul(motion_count);
        // vim turns cw/cW into ce/cE when the cursor is on a non-blank and no
        // count was given (normal.c, the "cw" special case), so "cw" does not
        // eat the space after the word while "2cw" still changes two words.
        let motion =
            if op == 'c' && matches!(key, 'w' | 'W') && count == 1 && cursor_on_nonblank(ed) {
                if key == 'w' { 'e' } else { 'E' }
            } else {
                key
            };
        let range = if motion == op {
            operators::line_range(
                ed.cursor().line,
                ed.cursor().line + count as usize - 1,
                ed.line_count(),
            )
        } else {
            match operators::motion_range(ed, motion, count)? {
                Some(range) => range,
                None => return Ok(false),
            }
        };
        self.apply_operator(ed, op, range, reg)?;
        ed.last_change = Some(RepeatChange::Operator {
            op,
            motion: if motion == op { op } else { motion },
            count,
        });
        Ok(true)
    }

    /// Feed the second key of an operator whose motion is "gg" (dgg/ygg/cgg).
    pub fn feed_operator_gg(&mut self, ed: &mut Editor, op: char, count: u64) -> VimResult<bool> {
        if !matches!(op, 'd' | 'y' | 'c') {
            return Ok(false);
        }
        let (op_count, reg) = match ed.mode {
            Mode::MotionPending { count, reg, .. } => (count.max(1), reg),
            _ => (1, self.take_register()),
        };
        let total = op_count.saturating_mul(count.max(1));
        let range = operators::gg_range(ed, total)?;
        self.apply_operator(ed, op, range, reg)?;
        ed.last_change = Some(RepeatChange::Operator {
            op,
            motion: 'g',
            count: total,
        });
        Ok(true)
    }

    /// Run a resolved operator, entering insert mode for c (normal.c).
    fn apply_operator(
        &mut self,
        ed: &mut Editor,
        op: char,
        range: operators::OpRange,
        reg: Option<char>,
    ) -> VimResult<()> {
        match op {
            'd' => {
                operators::delete_range(ed, range, reg)?;
                ed.mode = Mode::Normal;
            }
            'y' => {
                operators::yank_range(ed, range, reg)?;
                ed.mode = Mode::Normal;
            }
            'c' => {
                // The deletion and the text typed afterwards are one undo step,
                // so the block opens before the range is removed.
                ed.begin_undo_block();
                // 'autoindent' keeps the indent of the first changed line
                // (ops.c op_change() -> ins_char eol handling).
                let indent = if range.linewise {
                    let text = ed.current_buffer().line_str(range.start.line)?;
                    if ed.bool_option("autoindent", false) {
                        insert::indent_of_line(text)
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                operators::change_range(ed, range, reg)?;
                if !indent.is_empty() {
                    ed.insert_text(Pos::new(range.start.line, 0), &indent)?;
                    ed.set_cursor(Pos::new(range.start.line, indent.len()));
                }
                let start = ed.cursor();
                self.start_insert_session(start, 1, 'i');
                ed.mode = Mode::Insert;
            }
            _ => ed.mode = Mode::Normal,
        }
        Ok(())
    }

    /// Repeat a counted insert `times` more times (edit.c `ins_esc()`).
    ///
    /// vim does not copy the text: it re-executes the whole insertion from the
    /// redo buffer, so the entry key runs again at the current cursor. That is
    /// what puts `3o` below the previous copy, `3O` above it (the cursor ends on
    /// the last inserted line either way), and replaying the *keys* means
    /// `3ifoo<C-w>bar<Esc>` repeats the correction as well.
    fn repeat_insert_text(
        &mut self,
        ed: &mut Editor,
        keys: &[Key],
        times: u64,
        entry: char,
    ) -> VimResult<()> {
        for _ in 0..times {
            insert::enter_insert(ed, entry)?;
            for key in keys {
                insert::key(ed, self, key.clone())?;
            }
        }
        // The replay recorded the keys again; this session already stored them.
        self.insert_text.clear();
        self.insert_keys.clear();
        Ok(())
    }

    /// Start an insert session: clear the redo buffer and remember the entry
    /// point (edit.c start_redo_ins() / Insstart).
    fn start_insert_session(&mut self, pos: Pos, count: u64, entry: char) {
        self.insert_text.clear();
        self.insert_keys.clear();
        self.insert_pending_register = false;
        self.insert_start = pos;
        self.insert_repeat = count.max(1);
        self.insert_entry = entry;
    }
    /// While in RegisterPending after "m", bind the mark name.
    pub fn complete_mark(&mut self, ed: &mut Editor, name: char) -> VimResult<()> {
        if self.pending_register == Some(MARK_PENDING) {
            let bufnr = ed.current_bufnr();
            let pos = ed.cursor();
            if name.is_ascii_uppercase() {
                ed.marks.set_global(name, bufnr, pos);
            } else {
                ed.marks.set_local(bufnr, name, pos);
            }
            self.pending_register = None;
            ed.mode = Mode::Normal;
        }
        Ok(())
    }

    pub fn ctrl_r(&mut self, ed: &mut Editor) -> VimResult<()> {
        ed.redo()
    }

    /// Record typed insert text so the "." register can be set on exit.
    pub fn note_insert(&mut self, text: &str) {
        self.insert_text.push_str(text);
    }

    /// Record one key of the current insert session (vim's redo buffer).
    pub fn note_key(&mut self, key: Key) {
        self.insert_keys.push(key);
    }

    pub fn finish_insert(&mut self, ed: &mut Editor) {
        let text = std::mem::take(&mut self.insert_text);
        let keys = std::mem::take(&mut self.insert_keys);
        let repeat = std::mem::take(&mut self.insert_repeat).max(1);
        let entry = std::mem::replace(&mut self.insert_entry, 'i');
        self.insert_pending_register = false;
        if !text.is_empty() {
            ed.registers.set_last_insert(text.clone());
        }
        // A count on the insert command repeats the typed keys (vim:
        // "3iabc<Esc>" inserts "abcabcabc"). The first copy is already in the
        // buffer.
        if repeat > 1 && !keys.is_empty() {
            let result = self.repeat_insert_text(ed, &keys, repeat - 1, entry);
            if let Err(e) = result {
                ed.emsg(e.to_string());
            }
        }
        // A visual-block insert (C-v I / A) repeats the typed text on the other
        // lines of the block (ops.c block_insert()); it also places the cursor
        // back on the block's first column, so it runs after the inline repeat.
        if let Some(block) = ed.block_insert.take() {
            if let Err(e) = Self::repeat_block_insert(ed, &block, &text, repeat) {
                ed.emsg(e.to_string());
            }
        }
        if !text.is_empty() {
            ed.last_change = Some(RepeatChange::Insert { entry, keys });
        }
        ed.end_undo_block();
        ed.mode = Mode::Normal;
    }

    /// Replay the last change `count` times (".").
    ///
    /// Vim stores the command, not the resulting text, so the repeat runs the
    /// same operator at the new cursor position (normal.c nv_dot).
    pub fn repeat_last_change(&mut self, ed: &mut Editor, count: u64) -> VimResult<()> {
        let Some(change) = ed.last_change.clone() else {
            // Nothing to repeat is not an error in vim; the key is just ignored.
            return Ok(());
        };
        for _ in 0..count.max(1) {
            match change.clone() {
                RepeatChange::Insert { entry, keys } => {
                    ed.begin_undo_block();
                    insert::enter_insert(ed, entry)?;
                    let start = ed.cursor();
                    self.start_insert_session(start, 1, entry);
                    for key in &keys {
                        insert::key(ed, self, key.clone())?;
                    }
                    self.finish_insert(ed);
                    ed.last_change = Some(RepeatChange::Insert { entry, keys });
                }
                RepeatChange::Operator { op, motion, count } => {
                    if motion == 'g' {
                        ed.mode = Mode::MotionPending {
                            op,
                            count,
                            reg: None,
                        };
                        self.feed_operator_gg(ed, op, 1)?;
                    } else {
                        ed.mode = Mode::MotionPending {
                            op,
                            count,
                            reg: None,
                        };
                        self.feed_operator_key(ed, op, motion)?;
                    }
                }
                RepeatChange::Simple { key, count } => match key {
                    '\u{1}' | '\u{18}' => {
                        if addsub::addsub(ed, key == '\u{1}', count)? {
                            ed.last_change = Some(RepeatChange::Simple { key, count });
                        }
                    }
                    'x' => {
                        operators::delete_char(ed, count, None)?;
                        ed.last_change = Some(RepeatChange::Simple { key, count });
                    }
                    'D' | 'C' => {
                        let start = ed.cursor();
                        let last = (start.line + count as usize - 1).min(ed.line_count() - 1);
                        let end = ed.current_buffer().line_str(last)?.len();
                        let range = operators::OpRange {
                            start,
                            end: Pos::new(last, end),
                            linewise: false,
                        };
                        let op = if key == 'D' { 'd' } else { 'c' };
                        self.apply_operator(ed, op, range, None)?;
                        ed.last_change = Some(RepeatChange::Simple { key, count });
                    }
                    _ => {}
                },
                RepeatChange::Paste { key, reg } => {
                    operators::paste(ed, key, reg)?;
                    ed.last_change = Some(RepeatChange::Paste { key, reg });
                }
            }
        }
        Ok(())
    }

    /// Repeat the text typed during a visual-block insert on the block's other
    /// lines, then put the cursor back on the block's first column.
    ///
    /// Mirrors ops.c `block_insert()`: `I` skips lines that end before the block
    /// starts, `A` pads those lines with spaces up to the block's right edge.
    fn repeat_block_insert(
        ed: &mut Editor,
        block: &rvim_editor::BlockInsert,
        text: &str,
        repeat: u64,
    ) -> VimResult<()> {
        if block.last <= block.first {
            return Ok(());
        }
        let insert = text.repeat(repeat.max(1) as usize);
        let total = ed.line_count();
        for line in (block.first + 1)..=block.last {
            if line >= total {
                break;
            }
            let current = ed.current_buffer().line_str(line)?.to_string();
            if insert.is_empty() {
                continue;
            }
            let next = if block.col > current.len() {
                if !block.append {
                    // "I" on a line that ends before the block starts: untouched.
                    continue;
                }
                // "A" pads a short line out to the block edge, like vim.
                format!(
                    "{}{}{}",
                    current,
                    " ".repeat(block.col - current.len()),
                    insert
                )
            } else {
                let at = rvim_core::text::snap_byte(&current, block.col);
                format!("{}{}{}", &current[..at], insert, &current[at..])
            };
            ed.set_line_text(line, next)?;
        }
        let first_len = ed.current_buffer().line_str(block.first)?.len();
        ed.set_cursor(Pos::new(block.first, block.col.min(first_len)));
        Ok(())
    }

    pub fn jump_mark(&mut self, ed: &mut Editor, name: char) -> VimResult<bool> {
        let bufnr = ed.current_bufnr();
        if let Some(pos) = ed.marks.resolve(bufnr, name) {
            ed.set_cursor(pos);
            Ok(true)
        } else {
            Err(VimError::mark_not_set(name))
        }
    }

    pub fn pending_register_type(&self) -> RegType {
        RegType::CharWise
    }
}
