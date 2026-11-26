//! The aggregate editor: buffers, single-window cursor, undo histories,
//! registers, marks, options and messages.

use std::path::{Path, PathBuf};

use rvim_buffer::{Buffer, FileFormat, RangeEdit};
use rvim_core::text::{last_char_offset, snap_byte};
use rvim_core::{Pos, VimError, VimResult};
use rvim_undo::UndoHistory;

use crate::autocmd::AutocmdStore;
use crate::fold::FoldStore;
use crate::maps::MapStore;
use crate::mark::{MAXLNUM, MarkSnapshot, MarkStore};
use crate::message::MessageLog;
use crate::mode::Mode;
use crate::options::{OptValue, Options};
use crate::register::{RegType, Register, RegisterStore};
use crate::usercmd::UserCommandStore;
use crate::window::{Dir, Rect, TabpageId, WindowId, Windows};

/// Visual mode selection (normal.c VIsual): the anchor stays put while the
/// cursor moves, so the selection is always anchor..cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualState {
    /// Visual, VisualLine or VisualBlock.
    pub mode: Mode,
    /// The fixed end of the selection.
    pub anchor: Pos,
}

/// A pending visual-block insert (`<C-v>` + `I`/`A`).
///
/// vim records the block in `oap` and, when insert mode ends, repeats the typed
/// text on every other line of the block (ops.c `op_insert()`/`block_insert()`).
/// The first line is inserted normally, so only `first + 1 ..= last` is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockInsert {
    /// First line of the block (the line the user typed on).
    pub first: usize,
    /// Last line of the block.
    pub last: usize,
    /// Byte column to insert at on the remaining lines.
    pub col: usize,
    /// True for `A` (append after the block): short lines are padded to `col`.
    pub append: bool,
}

/// One jump-list entry (mark.c setpcmark / getpcmark).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JumpPos {
    pub bufnr: i32,
    pub pos: Pos,
}

/// One change-list entry (:changes): the buffer and 1-based line of a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangePos {
    pub bufnr: i32,
    pub lnum: usize,
}

/// One repeatable change, replayed by "." (normal.c nv_dot / u_save_cursor).
///
/// Vim records the *command* rather than the text, so a repeat re-executes the
/// same operator with the same count at the new cursor position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepeatChange {
    /// An insert session: the entry key (i/a/A/o/O/I/c/s/C) and the keys typed
    /// in it (vim's redo buffer, so `<BS>`/`<C-w>` replay too).
    Insert {
        entry: char,
        keys: Vec<crate::maps::Key>,
    },
    /// An operator with its motion key and total count (dd, dw, d2w, ...).
    Operator { op: char, motion: char, count: u64 },
    /// x / X / D / C with their count.
    Simple { key: char, count: u64 },
    /// p / P with the register.
    Paste { key: char, reg: Option<char> },
}

/// State of the most recent `:substitute`, for `:`&, `:`~ and the `'`~`'`
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
    /// True while a native GUI owns this session.
    pub gui_running: bool,
    buffers: Vec<Buffer>,
    /// Windows, tabpages and the split layout (see [`crate::window`]).
    pub windows: Windows,
    /// Per-window fold state (see [`crate::fold`]).
    pub fold_store: FoldStore,
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
    /// Quickfix and location-list state (M5).
    pub(crate) quickfix: crate::quickfix::QuickfixState,
    pub search_pattern: String,
    /// Most recent successful `:substitute`, for `:&` / `:~`.
    pub last_sub: Option<LastSubstitute>,
    /// Active visual selection, if any.
    pub visual: Option<VisualState>,
    /// Last visual selection (state plus the cursor end), restored by `gv`.
    pub last_visual: Option<(VisualState, Pos)>,
    /// The last change, replayed by ".".
    pub last_change: Option<RepeatChange>,
    /// Pending visual-block insert, applied when the insert session ends.
    pub block_insert: Option<BlockInsert>,
    /// Jump list (:jumps, CTRL-O/CTRL-I), oldest first.
    pub jumplist: Vec<JumpPos>,
    /// Index of the current position inside jumplist.
    pub jumplist_idx: usize,
    /// Change list (:changes, g;/g,), oldest first.
    pub changelist: Vec<ChangePos>,
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
    /// `:syntax on` state, the loaded definition and the `:highlight` table.
    pub syntax: crate::syntax::SyntaxState,
    /// Loaded spell word lists and the user's good/bad/rare words (M6).
    pub spell: crate::spell::SpellState,
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
    /// `:vertical` seen earlier on the same Ex line: the next split or resize
    /// uses it up (ex_docmd.c `EX_VERTSPLIT`).
    pub ex_vertical: bool,
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
        let mut undo = std::collections::HashMap::new();
        undo.insert(1, UndoHistory::new());
        Self {
            gui_running: false,
            buffers,
            windows: Windows::new(),
            fold_store: FoldStore::new(),
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
            quickfix: crate::quickfix::QuickfixState::default(),
            search_pattern: String::new(),
            last_sub: None,
            visual: None,
            last_visual: None,
            last_change: None,
            block_insert: None,
            jumplist: Vec::new(),
            jumplist_idx: 0,
            changelist: Vec::new(),
            script_depth: 0,
            filetype_detection: false,
            filetype_plugin: false,
            filetype_indent: false,
            filetype_script_loaded: false,
            syntax: crate::syntax::SyntaxState::new(),
            spell: crate::spell::SpellState::default(),
            arglist: Vec::new(),
            arg_index: 0,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            pending_count: 0,
            error_count: 0,
            exmode: false,
            ex_vertical: false,
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
        self.windows.current().bufnr
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
    /// Load a file into buffer `id`, decrypting it with the current 'key'
    /// option and recording the method found in the file header.
    pub fn load_buffer_file(&mut self, id: i32, path: &Path) -> VimResult<Buffer> {
        let key = self.options.str_or("key", "");
        let (buf, method) = Buffer::open_crypt(id, path, &key)?;
        if let Some(method) = method {
            let _ = self.options.set(
                "cryptmethod",
                crate::OptValue::Str(method.name().to_string()),
            );
        }
        Ok(buf)
    }

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
        let reusable = self.windows.all_ids().len() == 1
            && self.buffers.len() == 1
            && self.buffers[0].name.is_none()
            && !self.buffers[0].is_changed()
            && self.buffers[0].line_count() == 1
            && self.buffers[0]
                .line_str(0)
                .map(str::is_empty)
                .unwrap_or(false);
        let id = if reusable {
            let id = self.buffers[0].id;
            let fresh = self.load_buffer_file(id, path)?;
            self.buffers[0] = fresh;
            id
        } else {
            let id = self.next_bufnr;
            self.next_bufnr += 1;
            let buf = self.load_buffer_file(id, path)?;
            self.buffers.push(buf);
            self.undo.insert(id, UndoHistory::new());
            id
        };
        self.switch_buffer(id)?;
        Ok(id)
    }

    pub fn switch_buffer(&mut self, bufnr: i32) -> VimResult<()> {
        self.index_of(bufnr)?;
        let remembered = self.buffer(bufnr)?.last_cursor;
        let clamped = self.buffer(bufnr)?.clamp_pos(remembered);
        let desired = crate::display::display_col(
            self.buffer(bufnr)?.line_str(clamped.line)?,
            clamped.col,
            self.options.num_or("tabstop", 8).max(1) as usize,
        );
        let win = self.windows.current_mut();
        win.bufnr = bufnr;
        win.cursor = clamped;
        win.curswant = desired;
        // The viewport follows the cursor of the buffer being shown (M5 keeps no
        // per-buffer window position yet; see compat_deviations §9).
        win.top_line = clamped.line;
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

    // --- windows / tabpages (M5) ------------------------------------------

    /// Number of windows in the current tabpage.
    pub fn window_count(&self) -> usize {
        self.windows.wincount()
    }

    /// Number of tabpages.
    pub fn tabpage_count(&self) -> usize {
        self.windows.tab_count()
    }

    pub fn current_win_id(&self) -> WindowId {
        self.windows.current_id()
    }

    /// Compute the screen layout: `rows` excludes the command/message rows.
    pub fn layout_windows(&mut self, rows: usize, cols: usize) -> Rect {
        let id = self.windows.current_id();
        let rects = self.windows.layout(rows, cols);
        *rects.get(&id).unwrap_or(&Rect::default())
    }

    /// `:split` / `:vsplit` on the current buffer.
    ///
    /// The new window inherits the cursor and window-local options of the one it
    /// was split from, like vim`s `win_split()`.
    pub fn split_window(&mut self, vertical: bool) -> VimResult<WindowId> {
        // Batch/Ex mode has no real screen; test scripts split many scratch
        // windows (and close them again), so give the layout room instead of
        // failing with E36 after a few splits.
        if self.exmode {
            let win = self.windows.current_mut();
            if vertical {
                win.width = win.width.max(200);
            } else {
                win.height = win.height.max(200);
            }
        }
        let bufnr = self.current_bufnr();
        let id = self.windows.split(vertical, bufnr, true)?;
        self.sync_fileformat_option(bufnr);
        Ok(id)
    }

    /// `:split {file}` / `:vsplit {file}`: split first, then edit in the new window.
    pub fn split_and_open(&mut self, vertical: bool, path: &Path) -> VimResult<WindowId> {
        let id = self.split_window(vertical)?;
        self.open_file(path)?;
        Ok(id)
    }

    /// `:close` on the current window. A modified buffer refuses unless `force` or
    /// `hidden` is set (E37, ex_docmd.c ex_close).
    pub fn close_current_window(&mut self, force: bool) -> VimResult<()> {
        let id = self.windows.current_id();
        let modified = self.current_buffer().is_changed();
        let hidden = self.options.bool_or("hidden", false);
        let shown_elsewhere = self.windows.all_ids().into_iter().any(|other| {
            other != id
                && self
                    .windows
                    .get(other)
                    .is_some_and(|win| win.bufnr == self.current_bufnr())
        });
        if modified && !force && !hidden && !shown_elsewhere {
            return Err(VimError::no_write_since_change());
        }
        self.windows.close(id)
    }

    /// `:only` / <C-w>o`: close every other window of the current tabpage.
    pub fn only_current_window(&mut self) -> VimResult<()> {
        let id = self.windows.current_id();
        self.windows.only(id)
    }

    /// Make `id` the current window, syncing the buffer state it shows.
    pub fn goto_window(&mut self, id: WindowId) -> VimResult<()> {
        self.windows.goto(id)?;
        let bufnr = self.current_bufnr();
        let remembered = self.buffer(bufnr)?.last_cursor;
        let clamped = self.buffer(bufnr)?.clamp_pos(remembered);
        self.windows.current_mut().cursor = clamped;
        self.sync_fileformat_option(bufnr);
        Ok(())
    }

    pub fn goto_window_dir(&mut self, dir: Dir) -> VimResult<()> {
        self.windows.goto_dir(dir)
    }

    pub fn goto_window_cycle(&mut self, forward: bool, count: usize) -> VimResult<()> {
        self.windows.cycle(forward, count)
    }

    /// `:wincmd {key}` / <C-w>{key}`.
    ///
    /// Supported keys: h j k l w W t b p c q o n s S v x r = + - _ &#124; and the
    /// move commands H J K L. Unknown keys are ignored the way vim ignores them.
    pub fn wincmd(&mut self, key: char, count: u64) -> VimResult<()> {
        self.wincmd_counted(key, (count > 1).then_some(count))
    }

    /// Preserve an explicit count of one, as distinct from the default action.
    pub fn wincmd_counted(&mut self, key: char, explicit: Option<u64>) -> VimResult<()> {
        let count = explicit.unwrap_or(1).clamp(1, 999_999_999);
        let result = match key {
            'h' | 'j' | 'k' | 'l' => {
                let dir = Dir::from_key(key).expect("direction key");
                for _ in 0..count {
                    self.goto_window_dir(dir)?;
                }
                Ok(())
            }
            'w' | 'W' if explicit.is_some() => {
                let ids = self.windows.ids();
                self.windows
                    .goto(ids[(count as usize - 1).min(ids.len() - 1)])
            }
            'w' => self.goto_window_cycle(true, 1),
            'W' => self.goto_window_cycle(false, 1),
            't' => self.windows.goto_edge(false),
            'b' => self.windows.goto_edge(true),
            'p' => self.windows.goto_prev(),
            'c' | 'q' => self.close_current_window(false),
            'o' => self.only_current_window(),
            'n' | 's' | 'S' | 'v' => {
                let vertical = key == 'v';
                self.split_window(vertical)?;
                if let Some(size) = explicit {
                    self.windows.resize_frame(
                        size.min(999_999_999) as usize + usize::from(!vertical),
                        vertical,
                    )?;
                }
                if key == 'n' {
                    let bufnr = self.new_buffer();
                    self.switch_buffer(bufnr)?;
                }
                Ok(())
            }
            'x' => self.exchange_windows(),
            'r' => self.rotate_windows(),
            'H' | 'J' | 'K' | 'L' => self.move_window(key),
            '=' => self.equalize_windows(),
            '+' => self.windows.resize_frame_by(count as i64, false),
            '-' => self.windows.resize_frame_by(-(count as i64), false),
            '>' => self.windows.resize_frame_by(count as i64, true),
            '<' => self.windows.resize_frame_by(-(count as i64), true),
            '_' => self.windows.resize_frame(
                explicit
                    .map(|size| size.min(999_999_999) as usize + 1)
                    .unwrap_or(usize::MAX),
                false,
            ),
            '|' => self.windows.resize_frame(
                explicit
                    .map(|size| size.min(999_999_999) as usize)
                    .unwrap_or(usize::MAX),
                true,
            ),
            _ => Ok(()),
        };
        if result.is_ok() {
            self.sync_fileformat_option(self.current_bufnr());
        }
        result
    }

    /// Swap the contents of the current window with the next one
    /// (<C-w>x). The frames stay where they are, so the effect is the same
    /// two-window exchange vim performs.
    fn exchange_windows(&mut self) -> VimResult<()> {
        let ids = self.windows.ids();
        if ids.len() < 2 {
            return Err(VimError::new("E11", "Only one window"));
        }
        let cur = self.windows.current_id();
        let pos = ids.iter().position(|w| *w == cur).unwrap_or(0);
        let other = ids[(pos + 1) % ids.len()];
        self.swap_window_state(cur, other);
        Ok(())
    }

    /// Rotate the window contents by one (<C-w>r).
    ///
    /// Vim rotates the *frames*; rotating everything a window shows has the same
    /// observable effect for the two-window case and keeps the layout stable for
    /// larger ones (documented in compat_deviations §9).
    fn rotate_windows(&mut self) -> VimResult<()> {
        let ids = self.windows.ids();
        if ids.len() < 2 {
            return Err(VimError::new("E11", "Only one window"));
        }
        let states: Vec<crate::window::Window> = ids
            .iter()
            .filter_map(|id| self.windows.get(*id).cloned())
            .collect();
        for (index, id) in ids.iter().enumerate() {
            let src = &states[(index + states.len() - 1) % states.len()];
            if let Some(win) = self.windows.get_mut(*id) {
                win.bufnr = src.bufnr;
                win.cursor = src.cursor;
                win.top_line = src.top_line;
                win.left = src.left;
                win.curswant = src.curswant;
                win.local = src.local.clone();
                win.quickfix = src.quickfix;
            }
        }
        Ok(())
    }

    /// <C-w>H/J/K/L: move the current window to an edge by exchanging it with the
    /// window that is already there.
    fn move_window(&mut self, key: char) -> VimResult<()> {
        let ids = self.windows.ids();
        if ids.len() < 2 {
            return Err(VimError::new("E11", "Only one window"));
        }
        let cur = self.windows.current_id();
        let target = match key {
            'H' | 'K' => ids[0],
            _ => *ids.last().expect("non-empty"),
        };
        if target == cur {
            return Ok(());
        }
        self.swap_window_state(cur, target);
        self.goto_window(target)?;
        Ok(())
    }

    /// Swap everything a window shows (buffer, cursor, viewport, local options).
    fn swap_window_state(&mut self, a: WindowId, b: WindowId) {
        let (Some(wa), Some(wb)) = (self.windows.get(a).cloned(), self.windows.get(b).cloned())
        else {
            return;
        };
        if let Some(win) = self.windows.get_mut(a) {
            win.bufnr = wb.bufnr;
            win.cursor = wb.cursor;
            win.top_line = wb.top_line;
            win.left = wb.left;
            win.curswant = wb.curswant;
            win.local = wb.local;
            win.quickfix = wb.quickfix;
        }
        if let Some(win) = self.windows.get_mut(b) {
            win.bufnr = wa.bufnr;
            win.cursor = wa.cursor;
            win.top_line = wa.top_line;
            win.left = wa.left;
            win.curswant = wa.curswant;
            win.local = wa.local;
            win.quickfix = wa.quickfix;
        }
    }

    /// <C-w>=: give every window of the current tabpage the same size.
    fn equalize_windows(&mut self) -> VimResult<()> {
        let ids = self.windows.ids();
        let count = ids.len();
        if count < 2 {
            return Ok(());
        }
        let rows: usize = ids
            .iter()
            .filter_map(|id| self.windows.get(*id).map(|w| w.height + 1))
            .sum();
        let cols: usize = ids
            .iter()
            .filter_map(|id| self.windows.get(*id).map(|w| w.width))
            .sum();
        let each_h = (rows / count).saturating_sub(1).max(1);
        let each_w = (cols / count).max(10);
        for id in ids {
            if let Some(win) = self.windows.get_mut(id) {
                win.height = each_h;
                win.width = each_w;
            }
        }
        Ok(())
    }

    /// `:tabnew` / `:tabedit`: a new tabpage showing `bufnr` (or a fresh buffer).
    pub fn new_tabpage(&mut self, bufnr: Option<i32>) -> VimResult<TabpageId> {
        let bufnr = match bufnr {
            Some(id) => {
                self.index_of(id)?;
                id
            }
            None => self.new_buffer(),
        };
        let id = self.windows.new_tabpage(bufnr, 20, 80);
        self.sync_fileformat_option(bufnr);
        Ok(id)
    }

    /// `:tabclose` the current tabpage.
    pub fn close_current_tabpage(&mut self, force: bool) -> VimResult<()> {
        let modified = self.current_buffer().is_changed();
        let hidden = self.options.bool_or("hidden", false);
        if modified && !force && !hidden {
            return Err(VimError::no_write_since_change());
        }
        let n = self.windows.current_tab_index() + 1;
        self.windows.close_tabpage(n)
    }

    /// `:tabnext`/`:tabprevious`/`:tabfirst`/`:tablast`.
    pub fn goto_tabpage(&mut self, n: usize) -> VimResult<()> {
        self.windows.goto_tabpage(n)
    }

    pub fn cycle_tabpage(&mut self, forward: bool, count: usize) -> VimResult<()> {
        let total = self.windows.tab_count();
        if total < 2 {
            return Ok(());
        }
        let cur = self.windows.current_tab_index() as i64;
        let step = count.max(1) as i64;
        let next = if forward {
            (cur + step).rem_euclid(total as i64)
        } else {
            (cur - step).rem_euclid(total as i64)
        };
        self.windows.goto_tabpage(next as usize + 1)
    }

    pub fn tabonly(&mut self) -> VimResult<()> {
        let n = self.windows.current_tab_index() + 1;
        self.windows.tabonly(n)
    }

    pub fn move_tabpage(&mut self, to: usize) -> VimResult<()> {
        self.windows.move_tabpage(to)
    }

    // --- window-local options -------------------------------------------

    /// Read an option, preferring the window-local value for a P_WIN option.
    pub fn wopt(&self, name: &str) -> VimResult<OptValue> {
        self.wopt_for(self.windows.current_id(), name)
    }

    /// Read a boolean option, preferring the window-local value.
    pub fn bool_option(&self, name: &str, default: bool) -> bool {
        self.wopt_bool(name, default)
    }

    // --- jump list / change list (M5) ------------------------------------

    /// Record the current position before a jump (mark.c setpcmark()).
    ///
    /// Repeating the jump from the same line collapses into one entry, like
    /// vim's "the last entry already is the current position" check.
    pub fn push_jump(&mut self) {
        let entry = JumpPos {
            bufnr: self.current_bufnr(),
            pos: self.cursor(),
        };
        if self.jumplist.last() == Some(&entry) {
            return;
        }
        self.jumplist.truncate(self.jumplist_idx.saturating_add(1));
        self.jumplist.push(entry);
        if self.jumplist.len() > 100 {
            self.jumplist.remove(0);
        }
        self.jumplist_idx = self.jumplist.len().saturating_sub(1);
    }

    /// CTRL-O: jump to the previous position in the jump list.
    pub fn jump_back(&mut self) -> VimResult<()> {
        if self.jumplist.is_empty() || self.jumplist_idx == 0 {
            return Err(VimError::new("E662", "No previous position"));
        }
        let current = JumpPos {
            bufnr: self.current_bufnr(),
            pos: self.cursor(),
        };
        if self.jumplist_idx + 1 >= self.jumplist.len() {
            self.jumplist.push(current);
        }
        self.jumplist_idx -= 1;
        let target = self.jumplist[self.jumplist_idx];
        self.goto_jump(target)
    }

    /// CTRL-I: jump forward again.
    pub fn jump_forward(&mut self) -> VimResult<()> {
        if self.jumplist.is_empty() || self.jumplist_idx + 1 >= self.jumplist.len() {
            return Err(VimError::new("E663", "No next position"));
        }
        self.jumplist_idx += 1;
        let target = self.jumplist[self.jumplist_idx];
        self.goto_jump(target)
    }

    fn goto_jump(&mut self, target: JumpPos) -> VimResult<()> {
        if target.bufnr != self.current_bufnr() {
            self.switch_buffer(target.bufnr)?;
        }
        let pos = self.buffer(target.bufnr)?.clamp_pos(target.pos);
        self.set_cursor(pos);
        Ok(())
    }

    /// True when CTRL-O (backward) or CTRL-I (forward) can move.
    pub fn has_jump(&self, forward: bool) -> bool {
        if forward {
            self.jumplist_idx + 1 < self.jumplist.len()
        } else {
            !self.jumplist.is_empty() && self.jumplist_idx > 0
        }
    }

    /// :jumps output.
    pub fn jumps_report(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (index, entry) in self.jumplist.iter().enumerate() {
            let marker = if index == self.jumplist_idx { ">" } else { " " };
            let name = self
                .buffer(entry.bufnr)
                .map(|b| b.display_name().to_string())
                .unwrap_or_default();
            out.push(format!(
                "{marker} {:>3} {:>4} {:>3}  {}",
                index + 1,
                entry.pos.line + 1,
                entry.pos.col + 1,
                name
            ));
        }
        if out.is_empty() {
            out.push("Jump list is empty".to_string());
        }
        out
    }

    /// :changes output, newest first like vim.
    pub fn changes_report(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (index, entry) in self.changelist.iter().enumerate().rev() {
            let marker = if index + 1 == self.changelist.len() {
                ">"
            } else {
                " "
            };
            let name = self
                .buffer(entry.bufnr)
                .map(|b| b.display_name().to_string())
                .unwrap_or_default();
            out.push(format!(
                "{marker} {:>3} {:>4}    1  {}",
                index + 1,
                entry.lnum,
                name
            ));
        }
        if out.is_empty() {
            out.push("No changes".to_string());
        }
        out
    }

    /// Read an option in the context of a specific window (used by the TUI and
    /// by the window builtins).
    pub fn wopt_for(&self, id: WindowId, name: &str) -> VimResult<OptValue> {
        let canon = Options::canonical(name)?;
        let window = self.windows.get(id);
        let help = window
            .and_then(|win| self.buffer(win.bufnr).ok())
            .is_some_and(|buf| buf.is_help());
        if help {
            match canon.as_str() {
                "buftype" | "filetype" => return Ok(OptValue::Str("help".to_string())),
                _ => {}
            }
        }
        if crate::options::is_window_local(&canon) {
            if let Some(value) = window.and_then(|win| win.get_local(&canon)) {
                return Ok(value.clone());
            }
        }
        if help {
            match canon.as_str() {
                "readonly" => return Ok(OptValue::Bool(true)),
                "modifiable" => return Ok(OptValue::Bool(false)),
                _ => {}
            }
        }
        Ok(self.options.get(&canon)?.clone())
    }

    pub fn wopt_for_bool(&self, id: WindowId, name: &str, default: bool) -> bool {
        self.wopt_for(id, name)
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    pub fn wopt_for_num(&self, id: WindowId, name: &str, default: i64) -> i64 {
        self.wopt_for(id, name)
            .ok()
            .and_then(|v| v.as_num())
            .unwrap_or(default)
    }

    pub fn wopt_bool(&self, name: &str, default: bool) -> bool {
        self.wopt(name)
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    pub fn wopt_num(&self, name: &str, default: i64) -> i64 {
        self.wopt(name)
            .ok()
            .and_then(|v| v.as_num())
            .unwrap_or(default)
    }

    pub fn wopt_str(&self, name: &str, default: &str) -> String {
        self.wopt(name)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| default.to_string())
    }

    /// `:set` / `:setlocal` / `:setglobal` routing.
    ///
    /// `All` (plain `:set`) writes the global value and, for a window option, the
    /// current window`s value too — exactly what option.c does, which is why
    /// `:set nu` affects new splits while `:setlocal nu` does not.
    pub fn set_option(&mut self, name: &str, value: OptValue, scope: SetScope) -> VimResult<()> {
        let canon = Options::canonical(name)?;
        let checked = Options::validate_value(&canon, value)?;
        let window_local = crate::options::is_window_local(&canon);
        match scope {
            // `:set`: the global value is the default for new windows and the
            // current window gets the value too (option.c do_set).
            SetScope::All => {
                self.options.set(&canon, checked.clone())?;
                if window_local {
                    self.windows.current_mut().set_local(&canon, checked);
                }
            }
            // `:setlocal` must not touch the global value.
            SetScope::Local => {
                if window_local {
                    self.windows.current_mut().set_local(&canon, checked);
                } else {
                    self.options.set(&canon, checked)?;
                }
            }
            SetScope::Global => {
                self.options.set(&canon, checked)?;
            }
        }
        Ok(())
    }

    /// `:setlocal {opt}&`: drop the window-local override.
    pub fn reset_local_option(&mut self, name: &str) -> VimResult<()> {
        let canon = Options::canonical(name)?;
        self.windows.current_mut().local.remove(&canon);
        Ok(())
    }

    /// Set a window-local option on a specific window (`win_execute()` support).
    pub fn set_window_option(
        &mut self,
        id: WindowId,
        name: &str,
        value: OptValue,
    ) -> VimResult<()> {
        let canon = Options::canonical(name)?;
        let checked = Options::validate_value(&canon, value)?;
        if let Some(win) = self.windows.get_mut(id) {
            win.set_local(&canon, checked);
        }
        Ok(())
    }

    // --- cursor / window -------------------------------------------------

    pub fn cursor(&self) -> Pos {
        self.windows.current().cursor
    }

    pub fn set_cursor(&mut self, pos: Pos) {
        let bufnr = self.current_bufnr();
        let clamped = self.current_buffer().clamp_pos(pos);
        let desired = crate::display::display_col(
            self.current_buffer().line_str(clamped.line).unwrap_or(""),
            clamped.col,
            self.options.num_or("tabstop", 8).max(1) as usize,
        );
        let win = self.windows.current_mut();
        win.cursor = clamped;
        win.curswant = desired;
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
        // Vim keeps the last 100 changes for :changes and g;/g, (mark.c).
        self.changelist.push(ChangePos {
            bufnr,
            lnum: cursor_before.line + 1,
        });
        if self.changelist.len() > 100 {
            self.changelist.remove(0);
        }
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

    fn check_help_modifiable(&self) -> VimResult<()> {
        if self.current_buffer().is_help() && !self.wopt_bool("modifiable", true) {
            return Err(VimError::new(
                "E21",
                "Cannot make changes, 'modifiable' is off",
            ));
        }
        Ok(())
    }

    /// Apply a range edit to the current buffer and record it for undo.
    pub fn apply_edit(&mut self, edit: RangeEdit) -> VimResult<()> {
        self.check_help_modifiable()?;
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
        self.check_help_modifiable()?;
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
        self.check_help_modifiable()?;
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
        self.check_help_modifiable()?;
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
        self.check_help_modifiable()?;
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
        self.check_help_modifiable()?;
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
        self.check_help_modifiable()?;
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
        let readonly = self.wopt_bool("readonly", false);
        self.buffer_mut(bufnr)?.set_readonly(readonly);
        let own_file = match path {
            None => true,
            Some(p) => self.buffer(bufnr)?.name.as_deref() == Some(p),
        };
        let source = self.options.str_or("key", "");
        if source.is_empty() {
            match path {
                Some(p) => self.buffer_mut(bufnr)?.save(p, bang)?,
                None => self.buffer_mut(bufnr)?.save_named(bang)?,
            }
        } else {
            let method = rvim_buffer::crypt::CryptMethod::from_name(
                &self.options.str_or("cryptmethod", "blowfish2"),
            )?;
            let target = match path {
                Some(p) => Some(p.to_path_buf()),
                None => self.buffer(bufnr)?.name.clone(),
            };
            let Some(target) = target else {
                return Err(VimError::no_name());
            };
            self.buffer_mut(bufnr)?
                .save_crypt(&target, bang, &source, method)?;
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

    // --- syntax highlighting (M6) ----------------------------------------

    /// `:syntax on|off`: toggling on detects the language for the buffer.
    pub fn set_syntax_enabled(&mut self, on: bool) {
        if on {
            self.syntax.enabled = true;
            self.ensure_syntax();
        } else {
            self.syntax.disable();
        }
    }

    /// Load a named syntax (`:syntax on rust`, `:set syntax=rust`).
    ///
    /// Returns false when the name has no built-in definition; highlighting
    /// stays enabled so a later `:syntax keyword ...` can build one.
    pub fn set_syntax_language(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case("off") {
            self.syntax.disable();
            return true;
        }
        let known = self.syntax.enable(name);
        if !known {
            self.syntax.enabled = true;
            if self.load_runtime_syntax(name, None) {
                return true;
            }
        }
        known
    }

    /// The first `syntax/<name>.vim` found on 'runtimepath', if any.
    pub fn runtime_syntax_file(&self, name: &str) -> Option<PathBuf> {
        if name.is_empty() {
            return None;
        }
        let rtp = self.options.str_or("runtimepath", "");
        for dir in rtp.split(',').filter(|d| !d.is_empty()) {
            let path = Path::new(dir).join("syntax").join(format!("{name}.vim"));
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    /// Load a syntax definition from `syntax/<name>.vim` on 'runtimepath'.
    pub fn load_runtime_syntax(&mut self, name: &str, cluster: Option<&str>) -> bool {
        let Some(path) = self.runtime_syntax_file(name) else {
            return false;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return false;
        };
        let text = String::from_utf8_lossy(&bytes);
        if cluster.is_some() {
            self.syntax.include_script(name, &text, cluster) > 0
        } else {
            self.syntax.load_script(name, &text)
        }
    }

    /// Pick the syntax from 'filetype' (or the file name) when it changed.
    pub fn ensure_syntax(&mut self) {
        if !self.syntax.enabled {
            return;
        }
        let explicit = self.options.str_or("syntax", "");
        let filetype = self.options.str_or("filetype", "");
        let name = if !explicit.is_empty() {
            Some(explicit)
        } else if filetype.is_empty() {
            let display = self.current_buffer().display_name();
            crate::syntax::lang::from_filename(&display).map(str::to_string)
        } else {
            Some(filetype)
        };
        let key = format!(
            "{}:{}",
            self.current_bufnr(),
            name.clone().unwrap_or_default()
        );
        if self.syntax.resolved == key && self.syntax.engine().is_some() {
            return;
        }
        self.syntax.resolved = key;
        match name {
            Some(name) if !name.is_empty() => {
                if !self.syntax.enable(&name) {
                    self.syntax.enabled = true;
                    let _ = self.load_runtime_syntax(&name, None);
                }
            }
            _ => self.syntax.clear_engine(),
        }
    }

    /// Highlighted byte ranges of one buffer line: (start, end, group).
    ///
    /// Spans are ordered outer-first: later spans win when they overlap.
    pub fn syntax_spans(&mut self, bufnr: i32, line: usize) -> Vec<(usize, usize, String)> {
        if !self.syntax.enabled {
            return Vec::new();
        }
        self.ensure_syntax();
        if self.syntax.engine().is_none() {
            return Vec::new();
        }
        let Ok(buffer) = self.buffer(bufnr) else {
            return Vec::new();
        };
        let tick = buffer.changedtick();
        let last = buffer.line_count().saturating_sub(1);
        let end = line.min(last);
        let sync = self.syntax.sync_start();
        let start = if sync == usize::MAX {
            0
        } else {
            end.saturating_sub(sync)
        };
        let window: Vec<(usize, String)> = (start..=end)
            .map(|l| (l, buffer.line_str(l).unwrap_or("").to_string()))
            .collect();
        let line_text = window
            .iter()
            .find(|(lnum, _)| *lnum == end)
            .map(|(_, text)| text.clone())
            .unwrap_or_default();
        let mut out: Vec<(usize, usize, String)> = self
            .syntax
            .line_spans(bufnr, tick, end, &window)
            .iter()
            .map(|s| (s.start, s.end, s.group.clone()))
            .collect();
        // Spell checking is drawn on top of the syntax colours (spell.c puts
        // the SpellBad/SpellCap group on the word).
        if self.options.bool_or("spell", false) && !line_text.is_empty() {
            out.extend(crate::spell::bad_word_spans(self, &line_text));
        }
        out
    }

    /// The syntax item ids covering a 1-based (lnum, col), outermost first.
    /// `col` is a 1-based byte column, like Vim's `synstack(lnum, col)`.
    pub fn syntax_stack(&mut self, bufnr: i32, lnum: i64, col: i64) -> Vec<i64> {
        if lnum < 1 || col < 1 {
            return Vec::new();
        }
        if !self.syntax.enabled {
            return Vec::new();
        }
        self.ensure_syntax();
        if self.syntax.engine().is_none() {
            return Vec::new();
        }
        let Ok(buffer) = self.buffer(bufnr) else {
            return Vec::new();
        };
        let total = buffer.line_count();
        let line = (lnum as usize - 1).min(total.saturating_sub(1));
        let byte_col = col as usize - 1;
        let tick = buffer.changedtick();
        let sync = self.syntax.sync_start();
        let start = if sync == usize::MAX {
            0
        } else {
            line.saturating_sub(sync)
        };
        let window: Vec<(usize, String)> = (start..=line)
            .map(|l| (l, buffer.line_str(l).unwrap_or("").to_string()))
            .collect();
        let spans = self.syntax.line_items(bufnr, tick, line, &window);
        spans
            .iter()
            .filter(|span| span.start <= byte_col && byte_col < span.end)
            .map(|span| span.id)
            .collect()
    }

    /// The innermost syntax item id at (lnum, col), 0 when none.
    pub fn syntax_id_at(&mut self, bufnr: i32, lnum: i64, col: i64) -> i64 {
        self.syntax_stack(bufnr, lnum, col)
            .last()
            .copied()
            .unwrap_or(0)
    }

    /// The group name of a syntax item id (`synIDattr(id, "name")`).
    pub fn syntax_group_of(&self, id: i64) -> Option<&str> {
        self.syntax.engine().and_then(|engine| engine.group_of(id))
    }

    /// The flags of a syntax item id.
    pub fn syntax_options_of(&self, id: i64) -> rvim_syntax::ItemOptions {
        self.syntax
            .engine()
            .map(|engine| engine.options_of(id))
            .unwrap_or_default()
    }

    /// `synIDtrans()`: the id of the group an item resolves to after
    /// `:hi link` resolution.  RustVim links groups (not items), so a defined
    /// group keeps its own id and an unknown one becomes 0.
    pub fn syntax_trans(&self, id: i64) -> i64 {
        if id == 0 {
            return 0;
        }
        let Some(group) = self.syntax_group_of(id) else {
            return 0;
        };
        if self.syntax.highlights.exists(group) {
            id
        } else {
            0
        }
    }

    /// Attributes of a highlight group (`:highlight` + vim's default links).
    pub fn syntax_highlight(&self, group: &str) -> Option<rvim_syntax::highlight::HlAttr> {
        self.syntax.highlights.resolve_for(
            group,
            self.gui_running || self.options.bool_or("termguicolors", false),
        )
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

/// Which set of option values `:set` writes (option.c P_GLOBAL/P_LOCAL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetScope {
    /// Plain `:set`: global value plus the current window`s local value.
    All,
    /// `:setlocal`: only the current window`s value.
    Local,
    /// `:setglobal`: only the global value.
    Global,
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
