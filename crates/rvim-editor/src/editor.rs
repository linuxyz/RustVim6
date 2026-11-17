//! The aggregate editor: buffers, single-window cursor, undo histories,
//! registers, marks, options and messages.

use std::path::{Path, PathBuf};

use rvim_buffer::{Buffer, FileFormat, RangeEdit};
use rvim_core::text::{last_char_offset, snap_byte};
use rvim_core::{Pos, VimError, VimResult};
use rvim_undo::UndoHistory;

use crate::autocmd::AutocmdStore;
use crate::maps::MapStore;
use crate::mark::{MAXLNUM, MarkSnapshot, MarkStore};
use crate::message::MessageLog;
use crate::mode::Mode;
use crate::options::Options;
use crate::register::{RegType, Register, RegisterStore};
use crate::usercmd::UserCommandStore;

/// One viewport onto a buffer. M1 ships a single window; the struct already
/// carries the fields required by split windows in M5.
#[derive(Clone, Debug)]
pub struct Window {
    pub id: i32,
    pub bufnr: i32,
    pub cursor: Pos,
    /// Top line of the viewport (zero-based), used by the future TUI.
    pub top_line: usize,
}

/// State of the most recent `:substitute`, for `:&`, `:~` and the `'~'`
/// special register (ex_cmds.c last substitute state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastSubstitute {
    pub pattern: String,
    pub replacement: String,
    pub global: bool,
    pub count: u32,
}

/// Snapshot taken at the start of an undo block (an insert session).
#[derive(Debug)]
struct UndoBlock {
    bufnr: i32,
    mark: usize,
    lines: Vec<String>,
    cursor: Pos,
    /// Marks/'modified'/ML_EMPTY captured when the block opened.
    snapshot: EditSnapshot,
    /// Snapshot-history depth when the block opened.
    snap_pos: usize,
}

/// The state Vim keeps in an undo header besides the text: `uh_namedm` (marks)
/// plus the `UH_CHANGED` and `UH_EMPTYBUF` flags. Undo/redo restores these
/// (undo.c:2877-2897).
#[derive(Clone, Debug)]
struct EditSnapshot {
    marks: MarkSnapshot,
    changed: bool,
    empty_buffer: bool,
}

/// Per-buffer parallel to the undo history: one before/after snapshot pair per
/// undo step, so undo/redo can restore marks and 'modified' exactly the way
/// Vim's undo headers do.
#[derive(Debug, Default)]
struct SnapshotHistory {
    before: Vec<EditSnapshot>,
    after: Vec<EditSnapshot>,
    /// Number of applied edits, i.e. the undo head.
    pos: usize,
}

pub struct Editor {
    buffers: Vec<Buffer>,
    windows: Vec<Window>,
    current_win: usize,
    next_bufnr: i32,
    undo: std::collections::HashMap<i32, UndoHistory>,
    pub mode: Mode,
    pub options: Options,
    pub registers: RegisterStore,
    pub marks: MarkStore,
    pub messages: MessageLog,
    pub maps: MapStore,
    pub abbrevs: crate::abbrev::AbbrevStore,
    pub autocmds: AutocmdStore,
    pub usercmds: UserCommandStore,
    pub search_pattern: String,
    /// Most recent successful `:substitute`, for `:&` / `:~`.
    pub last_sub: Option<LastSubstitute>,
    /// `:source` nesting depth (guards runaway recursion).
    pub script_depth: usize,
    /// `:filetype on/detect` state (actual detection is M3/M6 work in progress).
    pub filetype_detection: bool,
    /// `:filetype plugin on`
    pub filetype_plugin: bool,
    /// `:filetype indent on`
    pub filetype_indent: bool,
    /// True once `:filetype on` has sourced the runtime `filetype.vim`
    /// (vim does this only on the first enable).
    pub filetype_script_loaded: bool,
    /// `:syntax on/enable`
    pub syntax_enabled: bool,
    /// Argument list (`:args`, `:argadd`, ...).
    pub arglist: Vec<PathBuf>,
    /// Index of the current argument.
    pub arg_index: usize,
    pub cwd: PathBuf,
    /// Pending normal-mode count prefix (e.g. the `3` in `3dd`).
    pub pending_count: u64,
    /// Number of `emsg()` calls, i.e. vim's `did_emsg`/`ex_exitval` signal.
    /// Every error path funnels through [`Editor::emsg`], so batch sessions can
    /// derive their exit status from this single counter (main.c:1575).
    pub error_count: usize,
    /// `-e`/`-es` Ex mode (vim's `exmode_active`), which changes what a range
    /// without a command does (`:1,3` prints in Ex mode, jumps otherwise).
    pub exmode: bool,
    /// True while an autocommand is running; non-`++nested` autocommands must
    /// not trigger further events (autocmd.c `autocmd_busy`).
    pub autocmd_busy: bool,
    /// The running autocommand asked for `++nested`, so its events may fire.
    pub autocmd_nested: bool,
    /// Nesting depth of autocommand execution (guards runaway recursion).
    pub autocmd_depth: usize,
    /// Open undo block: the buffer text and history mark captured when an insert
    /// session started, so the whole session becomes one undo step like in vim.
    undo_block: Option<UndoBlock>,
    /// Nesting depth of `begin_undo_block()` so a compound command (`:g`,
    /// `:s` over many lines) still forms one undo step.
    undo_block_depth: usize,
    /// Mark/'modified' snapshots matching each buffer's undo history.
    snapshots: std::collections::HashMap<i32, SnapshotHistory>,
}

impl Editor {
    pub fn new() -> Self {
        let buffers = vec![Buffer::new(1)];
        let win = Window {
            id: 1,
            bufnr: 1,
            cursor: Pos::new(0, 0),
            top_line: 0,
        };
        let mut undo = std::collections::HashMap::new();
        undo.insert(1, UndoHistory::new());
        Self {
            buffers,
            windows: vec![win],
            current_win: 0,
            next_bufnr: 2,
            undo,
            mode: Mode::Normal,
            options: Options::new(),
            registers: RegisterStore::new(),
            marks: MarkStore::new(),
            messages: MessageLog::new(),
            maps: MapStore::new(),
            abbrevs: crate::abbrev::AbbrevStore::new(),
            autocmds: AutocmdStore::new(),
            usercmds: UserCommandStore::new(),
            search_pattern: String::new(),
            last_sub: None,
            script_depth: 0,
            filetype_detection: false,
            filetype_plugin: false,
            filetype_indent: false,
            filetype_script_loaded: false,
            syntax_enabled: false,
            arglist: Vec::new(),
            arg_index: 0,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            pending_count: 0,
            error_count: 0,
            exmode: false,
            autocmd_busy: false,
            autocmd_nested: false,
            autocmd_depth: 0,
            undo_block: None,
            undo_block_depth: 0,
            snapshots: std::collections::HashMap::new(),
        }
    }

    // --- buffer management ----------------------------------------------

    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }

    pub fn current_bufnr(&self) -> i32 {
        self.windows[self.current_win].bufnr
    }

    fn index_of(&self, bufnr: i32) -> VimResult<usize> {
        self.buffers
            .iter()
            .position(|b| b.id == bufnr)
            .ok_or_else(|| VimError::internal(format!("no buffer {bufnr}")))
    }

    pub fn buffer(&self, bufnr: i32) -> VimResult<&Buffer> {
        Ok(&self.buffers[self.index_of(bufnr)?])
    }

    pub fn buffer_mut(&mut self, bufnr: i32) -> VimResult<&mut Buffer> {
        let idx = self.index_of(bufnr)?;
        Ok(&mut self.buffers[idx])
    }

    pub fn current_buffer(&self) -> &Buffer {
        let n = self.current_bufnr();
        self.buffers
            .iter()
            .find(|b| b.id == n)
            .expect("current buffer exists")
    }

    pub fn current_buffer_mut(&mut self) -> &mut Buffer {
        let n = self.current_bufnr();
        self.buffers
            .iter_mut()
            .find(|b| b.id == n)
            .expect("current buffer exists")
    }

    pub fn all_buffers(&self) -> &[Buffer] {
        &self.buffers
    }

    pub fn new_buffer(&mut self) -> i32 {
        let id = self.next_bufnr;
        self.next_bufnr += 1;
        let mut buf = Buffer::new(id);
        // A fresh buffer starts with the global 'fileformat' value
        // (optiondefs.h DFLT_FF), which the buffer model uses when writing.
        let default_ff = FileFormat::platform_default().vim_name();
        if let Ok(ff) = FileFormat::parse(&self.options.str_or("fileformat", default_ff)) {
            buf.set_fileformat(ff);
        }
        self.buffers.push(buf);
        self.undo.insert(id, UndoHistory::new());
        self.snapshots.remove(&id);
        id
    }

    /// Open a file into a fresh buffer and switch the window to it.
    pub fn open_file(&mut self, path: &Path) -> VimResult<i32> {
        // Reuse an existing buffer for the same canonical path when possible.
        if let Some(existing) = self
            .buffers
            .iter()
            .find(|b| b.name.as_deref() == Some(path))
        {
            let id = existing.id;
            self.switch_buffer(id)?;
            return Ok(id);
        }
        // The startup buffer is an empty "[No Name]" buffer. Vim reuses it for
        // the first file instead of creating a second buffer, so `:ls` and
        // `bufnr('%')` show one buffer numbered 1.
        let reusable = self.buffers.len() == 1
            && self.buffers[0].name.is_none()
            && !self.buffers[0].is_changed()
            && self.buffers[0].line_count() == 1
            && self.buffers[0]
                .line_str(0)
                .map(str::is_empty)
                .unwrap_or(false);
        let id = if reusable {
            let id = self.buffers[0].id;
            let fresh = Buffer::open(id, path)?;
            self.buffers[0] = fresh;
            id
        } else {
            let id = self.next_bufnr;
            self.next_bufnr += 1;
            let buf = Buffer::open(id, path)?;
            self.buffers.push(buf);
            self.undo.insert(id, UndoHistory::new());
            id
        };
        self.switch_buffer(id)?;
        Ok(id)
    }

    pub fn switch_buffer(&mut self, bufnr: i32) -> VimResult<()> {
        self.index_of(bufnr)?;
        // remember cursor on the old window (single window M1)
        self.windows[self.current_win].bufnr = bufnr;
        let remembered = self.buffer(bufnr)?.last_cursor;
        let clamped = self.buffer(bufnr)?.clamp_pos(remembered);
        self.windows[self.current_win].cursor = clamped;
        // 'fileformat' is a buffer-local option in vim; the buffer's own field
        // is authoritative, so mirror it into the option table on switch.
        self.sync_fileformat_option(bufnr);
        Ok(())
    }

    /// Push the buffer's 'fileformat' into the option table so `&fileformat` /
    /// `:set ff?` report what was read from the file (optiondefs.h:928 marks
    /// 'fileformat' P_RSTAT/buffer-local).
    pub fn sync_fileformat_option(&mut self, bufnr: i32) {
        let Ok(buf) = self.buffer(bufnr) else {
            return;
        };
        let name = buf.fileformat().vim_name();
        let _ = self
            .options
            .set("fileformat", crate::OptValue::Str(name.to_string()));
    }

    // --- cursor / window -------------------------------------------------

    pub fn cursor(&self) -> Pos {
        self.windows[self.current_win].cursor
    }

    pub fn set_cursor(&mut self, pos: Pos) {
        let bufnr = self.current_bufnr();
        let clamped = self.current_buffer().clamp_pos(pos);
        self.windows[self.current_win].cursor = clamped;
        if let Ok(buf) = self.buffer_mut(bufnr) {
            buf.last_cursor = clamped;
        }
    }

    pub fn line_count(&self) -> usize {
        self.current_buffer().line_count()
    }

    /// Startup positioning for `-e`/`-E`/`-es`: "Ex starts at last line of the
    /// file" (reference vim main.c:719-721).
    pub fn start_ex_mode_cursor(&mut self) {
        let last = self.line_count().saturating_sub(1);
        self.set_cursor(Pos::new(last, 0));
    }

    pub fn current_line(&self) -> String {
        let pos = self.cursor();
        self.current_buffer()
            .line_str(pos.line)
            .unwrap_or("")
            .to_string()
    }

    // --- mutation with undo bookkeeping ---------------------------------

    fn history_mut(&mut self, bufnr: i32) -> &mut UndoHistory {
        self.undo.entry(bufnr).or_default()
    }

    pub fn history_for(&self, bufnr: i32) -> Option<&UndoHistory> {
        self.undo.get(&bufnr)
    }

    // --- mark / 'modified' snapshots -------------------------------------

    /// Capture the undo-header state (marks, 'modified', ML_EMPTY) of `bufnr`.
    fn capture_snapshot(&self, bufnr: i32) -> EditSnapshot {
        let (changed, empty_buffer) = match self.buffer(bufnr) {
            Ok(buf) => (buf.is_changed(), buf.is_empty_buffer()),
            Err(_) => (false, false),
        };
        EditSnapshot {
            marks: self.marks.snapshot(bufnr),
            changed,
            empty_buffer,
        }
    }

    /// Restore the state saved in an undo header (undo.c:2877-2897).
    fn apply_snapshot(&mut self, bufnr: i32, snap: &EditSnapshot) {
        self.marks.restore(bufnr, &snap.marks);
        if let Ok(buf) = self.buffer_mut(bufnr) {
            buf.set_changed(snap.changed);
            buf.set_empty_buffer(snap.empty_buffer);
        }
    }

    /// Move the snapshot-history cursor by one step and restore that state.
    fn step_snapshot(&mut self, bufnr: i32, redo: bool) {
        let snap = match self.snapshots.get_mut(&bufnr) {
            Some(hist) => {
                if redo {
                    let snap = hist.after.get(hist.pos).cloned();
                    if snap.is_some() {
                        hist.pos += 1;
                    }
                    snap
                } else if hist.pos > 0 {
                    hist.pos -= 1;
                    hist.before.get(hist.pos).cloned()
                } else {
                    None
                }
            }
            None => None,
        };
        if let Some(snap) = snap {
            self.apply_snapshot(bufnr, &snap);
        }
    }

    /// Record a line-range edit in the undo history together with the matching
    /// mark/'modified' snapshot pair. Mark adjustment for the edit must already
    /// have happened.
    fn record_edit(
        &mut self,
        bufnr: i32,
        edit: RangeEdit,
        cursor_before: Pos,
        cursor_after: Pos,
        snapshot_before: EditSnapshot,
    ) {
        let snapshot_after = self.capture_snapshot(bufnr);
        let hist = self.snapshots.entry(bufnr).or_default();
        hist.before.truncate(hist.pos);
        hist.after.truncate(hist.pos);
        hist.before.push(snapshot_before);
        hist.after.push(snapshot_after);
        hist.pos += 1;
        self.history_mut(bufnr)
            .record(edit, cursor_before, cursor_after);
    }

    /// Apply Vim's `mark_adjust()` (mark.c:998) for an edit to the current
    /// buffer. Higher layers that mutate the buffer through lower-level APIs
    /// (e.g. `:global`) may call this to keep marks in sync.
    pub fn adjust_marks_for_edit(&mut self, edit: &RangeEdit) {
        let bufnr = self.current_bufnr();
        self.adjust_marks_for_buffer_edit(bufnr, edit);
    }

    /// Same as [`Self::adjust_marks_for_edit`] for an explicit buffer.
    pub fn adjust_marks_for_buffer_edit(&mut self, bufnr: i32, edit: &RangeEdit) {
        let old_count = edit.end.saturating_sub(edit.start);
        let new_count = edit.after.len();
        if old_count == new_count {
            // Line numbers did not change; nothing for mark_adjust() to do.
            return;
        }
        if new_count == 0 {
            // Pure deletion: mark_adjust(start+1, end, MAXLNUM, -old).
            self.marks.adjust(
                bufnr,
                edit.start + 1,
                edit.end,
                MAXLNUM,
                -(old_count as i64),
            );
        } else if old_count == 0 {
            // Pure insertion before 1-based line start+1:
            // mark_adjust(start+1, start, MAXLNUM, new).
            self.marks
                .adjust(bufnr, edit.start + 1, edit.start, MAXLNUM, new_count as i64);
        } else {
            // Replacement: lines in the range are removed (marks invalidated /
            // collapsed), lines below shift by the difference.
            self.marks.adjust(
                bufnr,
                edit.start + 1,
                edit.end,
                MAXLNUM,
                new_count as i64 - old_count as i64,
            );
        }
    }

    // --- edits -----------------------------------------------------------

    /// Apply a range edit to the current buffer and record it for undo.
    pub fn apply_edit(&mut self, edit: RangeEdit) -> VimResult<()> {
        let bufnr = self.current_bufnr();
        let before = self.cursor();
        let snap_before = self.capture_snapshot(bufnr);
        self.buffer_mut(bufnr)?.apply_edit(&edit)?;
        self.adjust_marks_for_buffer_edit(bufnr, &edit);
        let after = self.current_buffer().clamp_pos(before);
        self.record_edit(bufnr, edit, before, after, snap_before);
        self.set_cursor(after);
        Ok(())
    }

    pub fn replace_lines(&mut self, start: usize, end: usize, after: Vec<String>) -> VimResult<()> {
        let bufnr = self.current_bufnr();
        let before = self.cursor();
        let snap_before = self.capture_snapshot(bufnr);
        let edit = self
            .buffer_mut(bufnr)?
            .replace_line_range(start, end, after)?;
        self.adjust_marks_for_buffer_edit(bufnr, &edit);
        let after = self.current_buffer().clamp_pos(before);
        self.record_edit(bufnr, edit, before, after, snap_before);
        self.set_cursor(after);
        Ok(())
    }

    pub fn insert_text(&mut self, pos: Pos, text: &str) -> VimResult<()> {
        let bufnr = self.current_bufnr();
        let before = self.cursor();
        let snap_before = self.capture_snapshot(bufnr);
        // The buffer inserts at a *byte* offset; use the snapped start so the
        // resulting cursor lands on the last inserted character.
        let start = {
            let buf = self.current_buffer();
            let line = buf.line_str(pos.line).unwrap_or("");
            snap_byte(line, pos.col)
        };
        let edit = self.buffer_mut(bufnr)?.insert_text(pos, text)?;
        self.adjust_marks_for_buffer_edit(bufnr, &edit);
        let after = self
            .current_buffer()
            .clamp_pos(insert_end_position(Pos::new(pos.line, start), text));
        self.record_edit(bufnr, edit, before, after, snap_before);
        Ok(())
    }

    pub fn delete_chars(&mut self, pos: Pos, count: usize) -> VimResult<String> {
        let bufnr = self.current_bufnr();
        // Vim's `$` leaves the cursor *on* the last character, but the M1
        // motion layer parks a normal-mode cursor one byte past it (col ==
        // len). Treat that as the last character so `$x` deletes it like Vim
        // (normal.c end-of-line handling); insert-mode Delete still no-ops.
        let pos = if !self.mode.is_insert_like() && count > 0 {
            let line = self.current_buffer().line_str(pos.line).unwrap_or("");
            if !line.is_empty() && pos.col >= line.len() {
                Pos::new(pos.line, last_char_offset(line))
            } else {
                pos
            }
        } else {
            pos
        };
        let before = self.cursor();
        let snap_before = self.capture_snapshot(bufnr);
        let (removed, edit) = self.buffer_mut(bufnr)?.delete_chars(pos, count)?;
        self.adjust_marks_for_buffer_edit(bufnr, &edit);
        let after = self.current_buffer().clamp_pos(before);
        self.record_edit(bufnr, edit, before, after, snap_before);
        self.set_cursor(after);
        Ok(removed)
    }

    pub fn set_line_text(&mut self, idx: usize, text: String) -> VimResult<()> {
        let bufnr = self.current_bufnr();
        let before = self.cursor();
        let snap_before = self.capture_snapshot(bufnr);
        let edit = self.buffer_mut(bufnr)?.set_line_text(idx, text)?;
        self.adjust_marks_for_buffer_edit(bufnr, &edit);
        let after = self.current_buffer().clamp_pos(before);
        self.record_edit(bufnr, edit, before, after, snap_before);
        Ok(())
    }

    pub fn undo(&mut self) -> VimResult<()> {
        let bufnr = self.current_bufnr();
        let idx = self.index_of(bufnr)?;
        let restored = {
            let hist = self
                .undo
                .get_mut(&bufnr)
                .ok_or_else(VimError::cannot_undo)?;
            hist.undo(&mut self.buffers[idx])?
        };
        self.step_snapshot(bufnr, false);
        self.set_cursor(restored);
        Ok(())
    }

    pub fn redo(&mut self) -> VimResult<()> {
        let bufnr = self.current_bufnr();
        let idx = self.index_of(bufnr)?;
        let restored = {
            let hist = self
                .undo
                .get_mut(&bufnr)
                .ok_or_else(VimError::cannot_redo)?;
            hist.redo(&mut self.buffers[idx])?
        };
        self.step_snapshot(bufnr, true);
        self.set_cursor(restored);
        Ok(())
    }

    // --- registers convenience ------------------------------------------

    pub fn yank_lines(
        &mut self,
        start: usize,
        end_exclusive: usize,
        reg: Option<char>,
    ) -> VimResult<()> {
        let texts: Vec<String> = self.current_buffer().lines()[start..end_exclusive]
            .iter()
            .map(|l| l.as_str().to_string())
            .collect();
        self.registers.yank(
            reg,
            Register {
                text: texts.join("\n") + "\n",
                reg_type: RegType::LineWise,
            },
        );
        Ok(())
    }

    pub fn delete_lines(
        &mut self,
        start: usize,
        end_exclusive: usize,
        reg: Option<char>,
    ) -> VimResult<()> {
        let texts: Vec<String> = self.current_buffer().lines()[start..end_exclusive]
            .iter()
            .map(|l| l.as_str().to_string())
            .collect();
        let reg_data = Register {
            text: texts.join("\n") + "\n",
            reg_type: RegType::LineWise,
        };
        self.registers.delete(reg, reg_data);
        // Keep at least one line in the buffer (Vim deletes all text but keeps
        // an empty last line rather than a zero-line document).
        let total = self.line_count();
        let after = if start == 0 && end_exclusive >= total {
            vec![String::new()]
        } else {
            Vec::new()
        };
        self.replace_lines(start, end_exclusive, after)
    }

    // --- persistence -----------------------------------------------------

    pub fn save_current(&mut self, path: Option<&Path>, bang: bool) -> VimResult<()> {
        let bufnr = self.current_bufnr();
        // 'readonly' *is* the buffer flag (option.c:5504 PV_RO), so mirror the
        // option onto the buffer before writing: check_readonly() raises E45
        // unless the write is forced (ex_cmds.c:2311-2353).
        let readonly = self.options.bool_or("readonly", false);
        self.buffer_mut(bufnr)?.set_readonly(readonly);
        let own_file = match path {
            None => true,
            Some(p) => self.buffer(bufnr)?.name.as_deref() == Some(p),
        };
        match path {
            Some(p) => self.buffer_mut(bufnr)?.save(p, bang)?,
            None => self.buffer_mut(bufnr)?.save_named(bang)?,
        }
        // ":w!" to the buffer's own file resets 'readonly' (bufwrite.c:1624).
        if bang && own_file && readonly {
            let _ = self.options.set("readonly", crate::OptValue::Bool(false));
            self.buffer_mut(bufnr)?.set_readonly(false);
        }
        Ok(())
    }

    // --- changedtick / 'modified' ---------------------------------------

    /// The current buffer's `b:changedtick` (change.c `CHANGEDTICK`).
    pub fn changedtick(&self) -> u64 {
        self.current_buffer().changedtick()
    }

    /// `b:changedtick` of a specific buffer, `None` when it is not loaded.
    pub fn changedtick_of(&self, bufnr: i32) -> Option<u64> {
        self.buffer(bufnr).ok().map(Buffer::changedtick)
    }

    /// `:set [no]modified`: 'modified' is an alias for `b_changed`
    /// (option.c:3001), so this only flips the buffer flag and leaves
    /// `b:changedtick` alone.
    pub fn set_modified(&mut self, modified: bool) {
        let bufnr = self.current_bufnr();
        if let Ok(buf) = self.buffer_mut(bufnr) {
            buf.set_changed(modified);
        }
    }

    // --- messages --------------------------------------------------------

    pub fn echo(&mut self, text: impl Into<String>) {
        self.messages.echo(text);
    }

    /// Record an error. Vim's `emsg()` sets `ex_exitval = 1` (message.c:729), so
    /// every error — including ones raised deep inside a sourced script — is
    /// counted here and turned into the batch exit code at session end.
    pub fn emsg(&mut self, text: impl Into<String>) {
        self.error_count += 1;
        self.messages.emsg(text);
    }

    // --- undo blocks ------------------------------------------------------

    /// Start grouping edits into a single undo step (an insert session).
    pub fn begin_undo_block(&mut self) {
        if self.undo_block_depth == 0 && self.undo_block.is_none() {
            let bufnr = self.current_bufnr();
            let mark = self.undo.get(&bufnr).map(|h| h.len()).unwrap_or(0);
            let snapshot = self.capture_snapshot(bufnr);
            let snap_pos = self.snapshots.get(&bufnr).map(|h| h.pos).unwrap_or(0);
            self.undo_block = Some(UndoBlock {
                bufnr,
                mark,
                lines: self.buffer_lines(bufnr),
                cursor: self.cursor(),
                snapshot,
                snap_pos,
            });
        }
        self.undo_block_depth += 1;
    }

    /// Close the current undo block, replacing the per-keystroke history entries
    /// with one minimal edit (vim records one undo step per insert session).
    pub fn end_undo_block(&mut self) {
        if self.undo_block_depth > 0 {
            self.undo_block_depth -= 1;
        }
        if self.undo_block_depth > 0 {
            return;
        }
        let Some(block) = self.undo_block.take() else {
            return;
        };
        let after = self.buffer_lines(block.bufnr);
        let cursor_after = self.cursor();
        let Some(history) = self.undo.get_mut(&block.bufnr) else {
            return;
        };
        if history.len() <= block.mark + 1 {
            return; // a single edit is already one undo step
        }
        history.truncate_to(block.mark);
        if let Some(edit) = minimal_edit(&block.lines, &after) {
            history.record(edit, block.cursor, cursor_after);
            // Collapse the per-keystroke mark snapshots into the same single
            // undo step: the state before the session and after it.
            let snapshot_after = self.capture_snapshot(block.bufnr);
            let hist = self.snapshots.entry(block.bufnr).or_default();
            hist.before.truncate(block.snap_pos);
            hist.after.truncate(block.snap_pos);
            hist.before.push(block.snapshot);
            hist.after.push(snapshot_after);
            hist.pos = block.snap_pos + 1;
        } else {
            // Nothing actually changed; drop the collapsed snapshots too.
            let hist = self.snapshots.entry(block.bufnr).or_default();
            hist.before.truncate(block.snap_pos);
            hist.after.truncate(block.snap_pos);
            hist.pos = block.snap_pos;
        }
    }

    /// All lines of `bufnr` as owned strings (snapshot helper for undo blocks).
    fn buffer_lines(&self, bufnr: i32) -> Vec<String> {
        let Ok(buffer) = self.buffer(bufnr) else {
            return Vec::new();
        };
        (0..buffer.line_count())
            .map(|i| buffer.line_str(i).unwrap_or("").to_string())
            .collect()
    }
    // --- special registers -----------------------------------------------

    /// The read-only `%` register: current file name.
    pub fn percent_register(&self) -> String {
        self.current_buffer()
            .name
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    pub fn dot_register(&self) -> Option<&Register> {
        self.registers.get('.')
    }

    /// Replace the buffer object for an existing id (used by `:e` reload).
    pub fn replace_buffer(&mut self, id: i32, buf: Buffer) -> VimResult<()> {
        let idx = self.index_of(id)?;
        self.buffers[idx] = buf;
        self.undo.insert(id, UndoHistory::new());
        self.snapshots.remove(&id);
        // A reload takes 'fileformat' from the newly read buffer.
        self.sync_fileformat_option(id);
        Ok(())
    }

    /// Record an edit produced by lower-level buffer APIs directly (file I/O).
    ///
    /// The edit has already been applied and its marks have not been seen by
    /// this editor before, so the pre-edit mark snapshot is not recoverable;
    /// the current snapshot is stored as "before" (undo keeps the valid
    /// post-edit marks rather than inventing stale ones).
    pub fn record_external_edit(&mut self, bufnr: i32, edit: RangeEdit) -> VimResult<()> {
        let before = self.cursor();
        self.adjust_marks_for_buffer_edit(bufnr, &edit);
        let snapshot = self.capture_snapshot(bufnr);
        self.record_edit(
            bufnr,
            edit,
            before,
            self.buffer(bufnr)?.clamp_pos(before),
            snapshot,
        );
        Ok(())
    }

    /// Drop a buffer from the pool; callers must switch away first.
    pub fn drop_buffer(&mut self, bufnr: i32) {
        self.buffers.retain(|b| b.id != bufnr);
        self.undo.remove(&bufnr);
        self.snapshots.remove(&bufnr);
        self.marks.clear_buffer(bufnr);
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// Cursor position after inserting `text` at byte offset `start`: Vim leaves
/// the cursor on the last inserted character (or on the line after a newline
/// when the text ends with one).
fn insert_end_position(start: Pos, text: &str) -> Pos {
    if text.is_empty() {
        return start;
    }
    let newlines = text.matches('\n').count();
    if newlines == 0 {
        Pos::new(start.line, start.col + last_char_offset(text))
    } else {
        let tail = text.rsplit('\n').next().unwrap_or("");
        Pos::new(start.line + newlines, last_char_offset(tail))
    }
}

/// Smallest single edit turning `before` into `after` (common prefix/suffix
/// trimmed), as used by undo blocks.
fn minimal_edit(before: &[String], after: &[String]) -> Option<RangeEdit> {
    let mut start = 0usize;
    while start < before.len() && start < after.len() && before[start] == after[start] {
        start += 1;
    }
    let mut end_before = before.len();
    let mut end_after = after.len();
    while end_before > start && end_after > start && before[end_before - 1] == after[end_after - 1]
    {
        end_before -= 1;
        end_after -= 1;
    }
    if start == end_before && start == end_after {
        return None;
    }
    Some(RangeEdit {
        start,
        end: end_before,
        before: before[start..end_before].to_vec(),
        after: after[start..end_after].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_undo_redo_through_editor() {
        let mut ed = Editor::new();
        ed.set_line_text(0, "hello".to_string()).unwrap();
        assert_eq!(ed.current_buffer().full_text(), "hello");
        ed.undo().unwrap();
        assert_eq!(ed.current_buffer().full_text(), "");
        ed.redo().unwrap();
        assert_eq!(ed.current_buffer().full_text(), "hello");
    }

    #[test]
    fn line_delete_and_yank_registers() {
        let mut ed = Editor::new();
        ed.replace_lines(0, 1, vec!["a".into(), "b".into(), "c".into()])
            .unwrap();
        ed.yank_lines(0, 2, None).unwrap();
        assert_eq!(ed.registers.get('0').unwrap().text, "a\nb\n");
        ed.delete_lines(1, 2, None).unwrap();
        assert_eq!(ed.current_buffer().full_text(), "a\nc");
        assert_eq!(ed.registers.get('1').unwrap().text, "b\n");
    }

    #[test]
    fn deleting_all_lines_keeps_one_empty_line() {
        let mut ed = Editor::new();
        ed.set_line_text(0, "only".to_string()).unwrap();
        ed.delete_lines(0, 1, None).unwrap();
        assert_eq!(ed.line_count(), 1);
        assert_eq!(ed.current_buffer().full_text(), "");
    }

    #[test]
    fn open_and_switch_buffers() {
        let path = std::env::temp_dir().join("rvim_editor_open.txt");
        std::fs::write(&path, "x\ny").unwrap();
        let mut ed = Editor::new();
        let id = ed.open_file(&path).unwrap();
        assert_eq!(ed.current_bufnr(), id);
        assert_eq!(ed.line_count(), 2);
        let unnamed = ed.new_buffer();
        ed.switch_buffer(unnamed).unwrap();
        assert_eq!(ed.line_count(), 1);
        ed.switch_buffer(id).unwrap();
        assert_eq!(ed.line_count(), 2);
        let _ = std::fs::remove_file(&path);
    }

    fn seed_lines(ed: &mut Editor, lines: &[&str]) {
        let after: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        ed.replace_lines(0, 1, after).unwrap();
    }

    #[test]
    fn marks_adjust_on_delete_and_undo_restores_them() {
        let mut ed = Editor::new();
        seed_lines(&mut ed, &["one", "two", "three", "four", "five"]);
        let bufnr = ed.current_bufnr();
        ed.marks.set_local(bufnr, 'a', Pos::new(2, 0)); // line 3: "three"
        ed.marks.set_local(bufnr, 'b', Pos::new(1, 0)); // line 2: deleted
        ed.delete_lines(0, 2, None).unwrap();
        // 'a' was below the deleted range -> moved up two lines (mark.c:998).
        assert_eq!(ed.marks.get_local(bufnr, 'a'), Some(Pos::new(0, 0)));
        assert_eq!(ed.current_buffer().line_str(0).unwrap(), "three");
        // 'b' was inside the deleted range -> invalidated for a local mark.
        assert_eq!(ed.marks.get_local(bufnr, 'b'), None);
        // Undo restores the pre-change marks from the undo header
        // (undo.c:2886-2897).
        ed.undo().unwrap();
        assert_eq!(ed.marks.get_local(bufnr, 'a'), Some(Pos::new(2, 0)));
        assert_eq!(ed.marks.get_local(bufnr, 'b'), Some(Pos::new(1, 0)));
        ed.redo().unwrap();
        assert_eq!(ed.marks.get_local(bufnr, 'a'), Some(Pos::new(0, 0)));
        assert_eq!(ed.marks.get_local(bufnr, 'b'), None);
    }

    #[test]
    fn marks_adjust_on_inserted_lines() {
        let mut ed = Editor::new();
        seed_lines(&mut ed, &["one", "two", "three"]);
        let bufnr = ed.current_bufnr();
        ed.marks.set_local(bufnr, 'a', Pos::new(1, 0)); // 1-based line 2
        ed.replace_lines(1, 1, vec!["new1".into(), "new2".into()])
            .unwrap();
        assert_eq!(ed.marks.get_local(bufnr, 'a'), Some(Pos::new(3, 0)));
    }

    #[test]
    fn changedtick_is_monotonic_across_edit_undo_redo() {
        let mut ed = Editor::new();
        let t0 = ed.changedtick();
        ed.set_line_text(0, "a".to_string()).unwrap();
        let t1 = ed.changedtick();
        assert!(t1 > t0, "an edit bumps b:changedtick");
        ed.set_line_text(0, "b".to_string()).unwrap();
        let t2 = ed.changedtick();
        assert!(t2 > t1);
        ed.undo().unwrap();
        let t3 = ed.changedtick();
        assert!(t3 > t2, "undo changes the text so the tick keeps growing");
        ed.redo().unwrap();
        assert!(ed.changedtick() > t3);
    }

    #[test]
    fn set_modified_and_undo_restore_the_modified_flag() {
        let mut ed = Editor::new();
        assert!(!ed.current_buffer().is_changed());
        ed.set_line_text(0, "x".to_string()).unwrap();
        assert!(ed.current_buffer().is_changed());
        ed.set_modified(false);
        assert!(!ed.current_buffer().is_changed(), ":set nomodified");
        ed.set_line_text(0, "xy".to_string()).unwrap();
        ed.undo().unwrap();
        assert!(
            !ed.current_buffer().is_changed(),
            "undo restores the UH_CHANGED flag saved before the edit"
        );
        ed.undo().unwrap();
        assert!(!ed.current_buffer().is_changed());
        ed.redo().unwrap();
        assert!(ed.current_buffer().is_changed());
    }

    #[test]
    fn insert_text_and_delete_chars_use_byte_columns() {
        let mut ed = Editor::new();
        seed_lines(&mut ed, &["あb"]);
        // byte 3 is the start of 'b', not "character column 3".
        ed.insert_text(Pos::new(0, 3), "x").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "あxb");
        ed.undo().unwrap();
        assert_eq!(ed.current_buffer().full_text(), "あb");
        ed.delete_chars(Pos::new(0, 3), 1).unwrap();
        assert_eq!(ed.current_buffer().full_text(), "あ");
    }

    #[test]
    fn delete_chars_byte_column() {
        let mut ed = Editor::new();
        seed_lines(&mut ed, &["abcあ"]);
        // `$` parks a normal-mode cursor one byte past the last character.
        ed.set_cursor(Pos::new(0, 6));
        ed.delete_chars(ed.cursor(), 1).unwrap();
        assert_eq!(ed.current_buffer().full_text(), "abc");
    }

    #[test]
    fn readonly_option_blocks_write_unless_bang() {
        let path = std::env::temp_dir().join("rvim_editor_readonly_opt.txt");
        std::fs::write(&path, "orig\n").unwrap();
        let mut ed = Editor::new();
        ed.open_file(&path).unwrap();
        ed.set_line_text(0, "changed".to_string()).unwrap();
        ed.options
            .set("readonly", crate::OptValue::Bool(true))
            .unwrap();
        let err = ed.save_current(None, false).unwrap_err();
        assert_eq!(err.code, "E45");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "orig\n");
        ed.save_current(None, true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed\n");
        // ":w!" to the buffer's own file resets 'readonly' (bufwrite.c:1624).
        assert!(!ed.options.bool_or("readonly", true));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_missing_file_is_a_new_empty_buffer() {
        let path = std::env::temp_dir().join("rvim_editor_new_file.txt");
        let _ = std::fs::remove_file(&path);
        let mut ed = Editor::new();
        let id = ed.open_file(&path).unwrap();
        assert_eq!(ed.current_bufnr(), id);
        assert_eq!(ed.line_count(), 1);
        assert_eq!(ed.current_buffer().full_text(), "");
        assert!(!ed.current_buffer().is_changed());
        ed.save_current(None, false).unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), Vec::<u8>::new());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_file_syncs_fileformat_option() {
        let path = std::env::temp_dir().join("rvim_editor_ff_sync.txt");
        std::fs::write(&path, "one\r\ntwo\r\n").unwrap();
        let mut ed = Editor::new();
        ed.open_file(&path).unwrap();
        assert_eq!(ed.current_buffer().fileformat(), FileFormat::Dos);
        assert_eq!(ed.options.str_or("fileformat", ""), "dos");
        // A freshly created buffer uses the platform default (DFLT_FF).
        let id = ed.new_buffer();
        assert_eq!(
            ed.buffer(id).unwrap().fileformat(),
            FileFormat::platform_default()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn named_yank_becomes_the_unnamed_register() {
        let mut ed = Editor::new();
        seed_lines(&mut ed, &["one", "two"]);
        ed.yank_lines(0, 1, Some('a')).unwrap();
        assert_eq!(ed.registers.unnamed().unwrap().text, "one\n");
        ed.yank_lines(1, 2, Some('A')).unwrap();
        assert_eq!(ed.registers.get('a').unwrap().text, "one\ntwo\n");
        assert_eq!(ed.registers.unnamed().unwrap().text, "one\ntwo\n");
    }
}
