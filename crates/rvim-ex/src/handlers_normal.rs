//! `:normal` / `:normal!` (ex_docmd.c ex_normal): run normal-mode keys.
//!
//! Vim feeds the argument through the same state machine as typed keys, so this
//! module drives [NormalEngine] (normal commands), [rvim_normal::insert] (insert
//! mode) and [rvim_normal::visual] (visual mode). Mappings are applied unless
//! the bang form is used (ex_docmd.c REMAP_YES / REMAP_NONE).
//!
//! Like vim, an argument that stops in the middle of a command is aborted as if
//! `<Esc>` was typed, and the whole invocation is a single undo step.

use rvim_core::{Pos, VimResult};
use rvim_editor::maps::{self, Key, MapMode};
use rvim_editor::{Editor, Mode};
use rvim_normal::NormalEngine;
use rvim_normal::insert;
use rvim_normal::visual::{self, VisualAction, VisualPending};

use crate::exec::{ExOutcome, resolve_range_public};
use crate::parser::ParsedEx;

/// Two-key normal-mode command inside `:normal` (normal.c do_pending_operator).
#[derive(Clone, Copy, Debug)]
enum Pending {
    /// `d`/`y`/`c` waiting for its motion.
    Operator(char),
    /// An operator whose motion is `gg` (`dgg`).
    OperatorGg(char),
    /// `g` waiting for the second `g`.
    G,
    /// `i`/`a` after an operator: the next key names the text object.
    TextObject(char, bool),
    /// `z`: fold command.
    Fold,
    /// `]` (true) / `[` (false): diff jump.
    Bracket(bool),
    /// `<C-w>`: window command.
    Window,
}

/// `:normal[!] {keys}`.
pub fn cmd_normal(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let mut keys = maps::parse_keys(&p.args);
    if !p.bang {
        // ":normal" honours mappings; ":normal!" does not (ex_docmd.c).
        if let Ok(expanded) = ed.maps.expand(MapMode::Normal, &keys) {
            keys = expanded;
        }
    }
    // A range repeats the command for every line with the cursor in column 1;
    // without it the keys run once wherever the cursor is.
    let lines: Vec<usize> = if p.addr_count == 0 {
        Vec::new()
    } else {
        let (start, end) = resolve_range_public(ed, &p.range, false)?;
        (start..end).collect()
    };
    let mut engine = NormalEngine::new();
    ed.begin_undo_block();
    let result = (|| -> VimResult<()> {
        if lines.is_empty() {
            run_keys(ed, &mut engine, &keys)?;
        } else {
            for line in lines {
                let line = line.min(ed.line_count().saturating_sub(1));
                ed.set_cursor(Pos::new(line, 0));
                engine.cancel_pending();
                run_keys(ed, &mut engine, &keys)?;
            }
        }
        Ok(())
    })();
    ed.end_undo_block();
    result.map(|_| ExOutcome::Continue)
}

/// Feed every key, aborting as if `<Esc>` was typed at the end.
fn run_keys(ed: &mut Editor, engine: &mut NormalEngine, keys: &[Key]) -> VimResult<()> {
    let mut pending = None;
    let mut vpending = None;
    for key in keys {
        if !run_key(ed, engine, &mut pending, &mut vpending, key.clone())? {
            break;
        }
    }
    // "If {commands} does not finish a command, the last one will be aborted as
    // if <Esc> was typed" (ex_docmd.c).
    engine.cancel_pending();
    engine.clear_visual_count();
    if ed.mode.is_insert_like() {
        engine.finish_insert_esc(ed);
    } else if ed.mode.is_visual() {
        visual::stop(ed);
    } else if ed.mode != Mode::Normal {
        ed.mode = Mode::Normal;
    }
    Ok(())
}

/// One key. Returns false when the sequence has to stop (`:` would need a
/// command line, which `:normal` does not provide here).
fn run_key(
    ed: &mut Editor,
    engine: &mut NormalEngine,
    pending: &mut Option<Pending>,
    vpending: &mut Option<VisualPending>,
    key: Key,
) -> VimResult<bool> {
    // <Esc> ends insert/visual mode and cancels a pending command, wherever it
    // appears in the argument (normal.c nv_esc / edit.c ins_esc).
    if key == Key::Special("ESC".into()) {
        *pending = None;
        *vpending = None;
        engine.cancel_pending();
        engine.clear_visual_count();
        if ed.mode.is_insert_like() {
            engine.finish_insert_esc(ed);
        } else if ed.mode.is_visual() {
            visual::stop(ed);
        } else {
            ed.mode = Mode::Normal;
        }
        return Ok(true);
    }
    if ed.mode.is_visual() {
        let action = visual::key(ed, engine, vpending, key)?;
        return Ok(action != VisualAction::CommandLine);
    }
    if ed.mode.is_insert_like() {
        return insert::key(ed, engine, key);
    }
    if let Some(state) = pending.take() {
        return finish_pending(ed, engine, pending, state, key);
    }
    match key {
        Key::Char(ch) => match ch {
            ':' => Ok(false),
            'v' | 'V' => {
                let mode = if ch == 'V' {
                    Mode::VisualLine
                } else {
                    Mode::Visual
                };
                visual::start(ed, mode)?;
                Ok(true)
            }
            'z' => {
                *pending = Some(Pending::Fold);
                Ok(true)
            }
            ']' => {
                *pending = Some(Pending::Bracket(true));
                Ok(true)
            }
            '[' => {
                *pending = Some(Pending::Bracket(false));
                Ok(true)
            }
            _ => {
                if !engine.feed(ed, ch)? {
                    // Not a command this engine knows: vim would beep and keep
                    // going, so skip the key.
                    return Ok(true);
                }
                match ed.mode {
                    Mode::MotionPending { op, .. } => *pending = Some(Pending::Operator(op)),
                    Mode::OperatorPending => *pending = Some(Pending::G),
                    _ => {}
                }
                Ok(true)
            }
        },
        Key::Special(name) => match name.as_str() {
            // <Esc> cancels the pending command without moving the cursor.
            "ESC" => {
                engine.cancel_pending();
                if ed.mode.is_visual() {
                    visual::stop(ed);
                } else {
                    ed.mode = Mode::Normal;
                }
                Ok(true)
            }
            "CR" | "DOWN" => engine.feed(ed, 'j').map(|_| true),
            "UP" => engine.feed(ed, 'k').map(|_| true),
            "LEFT" => engine.feed(ed, 'h').map(|_| true),
            "RIGHT" => engine.feed(ed, 'l').map(|_| true),
            "HOME" => engine.feed(ed, '0').map(|_| true),
            "END" => engine.feed(ed, '$').map(|_| true),
            "BS" => engine.feed(ed, 'h').map(|_| true),
            "DELETE" => engine.feed(ed, 'x').map(|_| true),
            "C-A" => engine.feed(ed, '\u{1}').map(|_| true),
            "C-X" => engine.feed(ed, '\u{18}').map(|_| true),
            "C-R" => engine.ctrl_r(ed).map(|_| true),
            "C-O" => ed.jump_back().map(|_| true),
            "C-I" => ed.jump_forward().map(|_| true),
            "C-W" => {
                *pending = Some(Pending::Window);
                Ok(true)
            }
            _ => Ok(true),
        },
        Key::Nop => Ok(true),
    }
}

/// Complete the two-key command whose first key is buffered.
fn finish_pending(
    ed: &mut Editor,
    engine: &mut NormalEngine,
    pending: &mut Option<Pending>,
    state: Pending,
    key: Key,
) -> VimResult<bool> {
    let ch = match key {
        Key::Char(ch) => ch,
        // Special keys cancel a pending command (normal.c nv_esc).
        _ => {
            engine.cancel_pending();
            ed.mode = Mode::Normal;
            return Ok(true);
        }
    };
    match state {
        Pending::Operator(op) => {
            // `dgg`/`ygg`/`cgg` wait for the second g.
            if ch == 'g' {
                *pending = Some(Pending::OperatorGg(op));
                return Ok(true);
            }
            // Text objects: `diw`, `ca"`, ...
            if ch == 'i' || ch == 'a' {
                *pending = Some(Pending::TextObject(op, ch == 'a'));
                return Ok(true);
            }
            if engine.feed_operator_key(ed, op, ch)? {
                // A count between operator and motion (d2w) keeps it waiting.
                if matches!(ed.mode, Mode::MotionPending { .. }) {
                    *pending = Some(Pending::Operator(op));
                }
            } else {
                ed.mode = Mode::Normal;
            }
            Ok(true)
        }
        Pending::OperatorGg(op) => {
            if ch == 'g' {
                engine.feed_operator_gg(ed, op, 1)?;
            } else {
                ed.mode = Mode::Normal;
            }
            Ok(true)
        }
        Pending::G => {
            if ch == 'g' {
                engine.feed_pair(ed, 'g', 'g')?;
            } else {
                ed.mode = Mode::Normal;
            }
            Ok(true)
        }
        Pending::TextObject(op, around) => {
            let which = if around {
                rvim_normal::textobjects::Which::Around
            } else {
                rvim_normal::textobjects::Which::Inner
            };
            let _ = rvim_normal::textobjects::apply(ed, op, ch, which)?;
            Ok(true)
        }
        Pending::Fold => {
            if ch == 'f' && ed.visual.is_some() {
                if let Some(range) = visual::range(ed) {
                    let last = if range.linewise {
                        range.end.line.saturating_sub(1)
                    } else {
                        range.end.line
                    };
                    visual::stop(ed);
                    rvim_normal::foldcmd::create_fold(ed, range.start.line, last)?;
                }
                return Ok(true);
            }
            if !rvim_normal::foldcmd::feed_z(ed, ch, 1)? {
                rvim_normal::foldcmd::fold_motion(ed, ch, 1)?;
            }
            Ok(true)
        }
        Pending::Bracket(forward) => {
            if ch == 'c' {
                let win = ed.windows.current_id();
                let line = ed.cursor().line;
                if let Some(target) = ed.diff_next_change(win, line, forward) {
                    let pos = ed.current_buffer().clamp_pos(Pos::new(target, 0));
                    ed.set_cursor(pos);
                }
            }
            Ok(true)
        }
        Pending::Window => {
            let count = engine.take_pending_count();
            ed.wincmd(ch, count)?;
            Ok(true)
        }
    }
}
