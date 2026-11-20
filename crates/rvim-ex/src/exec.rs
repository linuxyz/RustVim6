//! Ex command execution against an Editor.

use std::path::PathBuf;
use std::process::Command;

use rvim_core::{Pos, VimError, VimResult};
use rvim_editor::{Editor, SetScope};

use crate::parser::{ParsedEx, Range, parse_line};
use crate::script::{Ctx, ScriptState};
use crate::table::lookup;
use rvim_viml::Env;

/// Canonical names of the commands this milestone implements.
///
/// This list drives coverage reporting and the "declared but not implemented"
/// gate, and it must stay in step with the dispatch arms below.
/// `tests::implemented_list_matches_dispatch` enforces that in both directions.
macro_rules! impl_implemented {
    ($($name:literal),* $(,)?) => {
        pub const IMPLEMENTED: &[&str] = &[$($name),*];
    };
}

impl_implemented![
    "break",
    "catch",
    "continue",
    "else",
    "elseif",
    "endfor",
    "endtry",
    "endwhile",
    "finally",
    "for",
    "endif",
    "if",
    "return",
    "throw",
    "try",
    "while",
    "!",
    "#",
    "&",
    ">",
    "<",
    "~",
    "abbreviate",
    "abclear",
    "argadd",
    "argdelete",
    "args",
    "augroup",
    "autocmd",
    "badd",
    "bdelete",
    "bfirst",
    "blast",
    "bmodified",
    "bnext",
    "bprevious",
    "buffer",
    "buffers",
    "bunload",
    "bwipeout",
    "cabbrev",
    "cabclear",
    "cd",
    "center",
    "chdir",
    "cmap",
    "cmapclear",
    "cnoreabbrev",
    "cnoremap",
    "comclear",
    "command",
    "copy",
    "cquit",
    "cunabbrev",
    "cunmap",
    "delcommand",
    "delete",
    "display",
    "doautocmd",
    "call",
    "delfunction",
    "echo",
    "echohl",
    "echoerr",
    "eval",
    "execute",
    "filetype",
    "function",
    "let",
    "setfiletype",
    "syntax",
    "unlet",
    "echomsg",
    "echon",
    "edit",
    "enew",
    "exit",
    "file",
    "files",
    "cNext",
    "cabove",
    "caddexpr",
    "cbelow",
    "cc",
    "cclose",
    "cexpr",
    "cfile",
    "cfirst",
    "cgetexpr",
    "cgetfile",
    "clast",
    "clist",
    "cnewer",
    "cnext",
    "colder",
    "copen",
    "cprevious",
    "crewind",
    "cwindow",
    "lNext",
    "labove",
    "laddexpr",
    "lbelow",
    "lclose",
    "lexpr",
    "lfile",
    "lfirst",
    "lgetexpr",
    "lgetfile",
    "ll",
    "llast",
    "llist",
    "lnewer",
    "lnext",
    "lolder",
    "lopen",
    "lprevious",
    "lrewind",
    "lwindow",
    "changes",
    "diffget",
    "diffoff",
    "diffput",
    "diffsplit",
    "diffthis",
    "diffupdate",
    "fold",
    "foldclose",
    "folddoopen",
    "folddoclosed",
    "foldopen",
    "finish",
    "global",
    "help",
    "iabbrev",
    "iabclear",
    "imap",
    "imapclear",
    "inoreabbrev",
    "inoremap",
    "iunabbrev",
    "iunmap",
    "join",
    "k",
    "left",
    "list",
    "lmap",
    "lmapclear",
    "lnoremap",
    "ls",
    "lunmap",
    "map",
    "mapclear",
    "mark",
    "marks",
    "messages",
    "move",
    "nmap",
    "nmapclear",
    "nnoremap",
    "nohlsearch",
    "noreabbrev",
    "noremap",
    "number",
    "nunmap",
    "omap",
    "omapclear",
    "onoremap",
    "ounmap",
    "print",
    "put",
    "pwd",
    "qall",
    "quit",
    "quitall",
    "read",
    "redo",
    "normal",
    "highlight",
    "rviminfo",
    "redraw",
    "registers",
    "retab",
    "right",
    "runtime",
    "saveas",
    "set",
    "setglobal",
    "setlocal",
    "silent",
    "smap",
    "smapclear",
    "snoremap",
    "sort",
    "source",
    "stop",
    "substitute",
    "sunmap",
    "suspend",
    "t",
    "unabbreviate",
    "undo",
    "unmap",
    "update",
    "version",
    "vglobal",
    "vmap",
    "vmapclear",
    "vnoremap",
    "vunmap",
    "argdo",
    "bufdo",
    "close",
    "hide",
    "new",
    "only",
    "resize",
    "split",
    "sview",
    "tabclose",
    "tabdo",
    "tabedit",
    "tabfirst",
    "tablast",
    "tabmove",
    "tabnew",
    "tabnext",
    "tabonly",
    "tabprevious",
    "tabs",
    "vertical",
    "vnew",
    "vsplit",
    "wincmd",
    "windo",
    "wall",
    "wq",
    "wqall",
    "write",
    "xit",
    "xmap",
    "xmapclear",
    "xnoremap",
    "xunmap",
    "yank",
    "jumps",
    "wviminfo",
];

/// Outcome of running an Ex line, letting the front-end decide process exit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExOutcome {
    Continue,
    Quit {
        bang: bool,
        exit_code: i32,
    },
    QuitAll {
        bang: bool,
        exit_code: i32,
    },
    /// `:finish`: stop executing the current sourced script (not the session).
    Finish,
}

/// Execute one possibly multi-command line. Stops at the first quit request.
///
/// A trailing `|` only starts a new command when the current command has
/// EX_TRLBAR (the parser records it in `ParsedEx::trailing`); otherwise the bar
/// is part of the arguments, e.g. `:!echo a|b`.
pub fn run_line(ed: &mut Editor, line: &str) -> VimResult<ExOutcome> {
    let mut ctx = Ctx::new();
    run_line_ctx(ed, &mut ctx, line)
}

/// Run a line with a persistent script context.
pub fn run_line_ctx(ed: &mut Editor, ctx: &mut Ctx, line: &str) -> VimResult<ExOutcome> {
    let Ctx { env, state } = ctx;
    run_line_with(ed, env, state, line)
}

/// Run a line with separate environment/script-state borrows.
pub fn run_line_with(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    line: &str,
) -> VimResult<ExOutcome> {
    run_line_impl(ed, env, state, line, true)
}

/// Like [`run_line_with`] but without control-flow routing; used by the script
/// runner for ordinary statement lines (prevents `:finish` from recursing).
pub fn run_line_plain(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    line: &str,
) -> VimResult<ExOutcome> {
    run_line_impl(ed, env, state, line, false)
}

fn run_line_impl(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    line: &str,
    allow_structural: bool,
) -> VimResult<ExOutcome> {
    let mut remaining = line.trim().to_string();
    let mut outcome = ExOutcome::Continue;
    while !remaining.is_empty() {
        let mut parsed = parse_line(&remaining, ed.line_count())?;
        // A user-defined command only splits trailing bars/comments when it was
        // defined with `-bar`; the builtin table does not know about it, so the
        // parser cannot decide (usercmd.c:816-817 sets EX_TRLBAR).
        if parsed.trailing.is_none()
            && !parsed.command.is_empty()
            && lookup(&parsed.command).is_none()
        {
            if let Some(cmd) = ed.usercmds.get(&parsed.command) {
                let (args, trailing) =
                    crate::parser::split_user_command_tail(&parsed.args, cmd.bar);
                parsed.args = args;
                parsed.trailing = trailing;
            }
        }
        // Record the last used /pat/ or ?pat? address the way vim's
        // search_regcomp(RE_SEARCH) does (search.c:159-183).
        if let Range::Deferred(text) = &parsed.range {
            if let Some(pat) = crate::addr::last_search_atom(text) {
                ed.search_pattern = pat;
            }
        }
        if parsed.command.is_empty() {
            // vim: ex_docmd.c:1958-1971 — a range with no command is either a
            // jump (`:3`), a print (`:1,3` in Ex mode, `:|`, `:3|cmd`), or
            // nothing at all (blank/comment line).
            let text = parsed.raw.trim();
            if text.is_empty() {
                break;
            }
            let (after, print_first) = match parsed.args.strip_prefix('|') {
                Some(rest) => (rest.trim().to_string(), true),
                None => (parsed.args.trim().to_string(), false),
            };
            if !after.is_empty() && !print_first {
                return Err(VimError::not_editor_command(text));
            }
            outcome = range_without_command(ed, &parsed.range, print_first)?;
            if after.is_empty() {
                break;
            }
            remaining = after;
            continue;
        }
        // Control-flow keywords are statements: run the whole line as a small
        // script so one-line `:if cond | ... | endif` works like Vim.
        if allow_structural && is_structural_command(&parsed.command) {
            let lines = crate::parser::split_bars(&remaining);
            let stmts = crate::script::parse_script_lines(&lines, env.current_sid)?;
            let flow = crate::script::exec_stmts(ed, env, state, &stmts)?;
            return match flow {
                crate::script::Flow::Quit(outcome) => Ok(outcome),
                crate::script::Flow::Throw(value) => Err(VimError::new(
                    "E605",
                    format!("Exception not caught: {}", value.to_string_value()),
                )),
                _ => Ok(ExOutcome::Continue),
            };
        }
        outcome = run_command_ctx(ed, env, state, &parsed)?;
        if !matches!(outcome, ExOutcome::Continue) {
            break;
        }
        match parsed.trailing {
            Some(next) if !next.is_empty() => remaining = next,
            _ => break,
        }
    }
    Ok(outcome)
}

/// A backwards range is an error (not an interactive "swap?" question) because
/// RustVim has no interactive confirmation: vim reports E493 when sourcing a
/// script or in Ex mode and asks only with a terminal (ex_docmd.c:2146-2162).
fn err_backwards_range() -> VimError {
    VimError::new("E493", "Backwards range given".to_string())
}

fn resolve_range(ed: &Editor, range: &Range, default_whole: bool) -> VimResult<(usize, usize)> {
    let total = ed.line_count();
    let order = |first: usize, last: usize| -> VimResult<(usize, usize)> {
        if first > last {
            return Err(err_backwards_range());
        }
        Ok((first, last + 1))
    };
    match range {
        Range::None => {
            if default_whole {
                Ok((0, total))
            } else {
                let l = ed.cursor().line;
                Ok((l, l + 1))
            }
        }
        Range::Whole => Ok((0, total)),
        Range::Current => {
            let l = ed.cursor().line;
            Ok((l, l + 1))
        }
        Range::Deferred(text) => {
            let (first, last) = crate::addr::resolve_deferred_pair(ed, text)?;
            let (s, e) = order(first, last)?;
            if e > total || s >= total {
                return Err(VimError::new("E16", format!("Invalid range: {s},{e}")));
            }
            Ok((s, e))
        }
        Range::Lines(s, e) => {
            let (s, e) = order(*s, e.saturating_sub(1))?;
            if e > total || s >= total {
                return Err(VimError::new("E16", format!("Invalid range: {s},{e}")));
            }
            Ok((s, e))
        }
    }
}

/// Handle a range with no command (vim `ex_range_without_command()`,
/// ex_docmd.c:2669-2711).
///
/// * `:3` jumps to line 3;
/// * `:1,3` prints the lines when there is no command *and* Ex mode is active
///   (`exmode_active && line1 != line2`), otherwise it just jumps;
/// * `:|` and `:3|cmd` print first, then the next command runs.
///
/// A line number past the end of the buffer is clamped to the last line, which
/// is vim's behaviour unless the `-` flag is in 'cpoptions'.
fn range_without_command(
    ed: &mut Editor,
    range: &Range,
    print_first: bool,
) -> VimResult<ExOutcome> {
    let total = ed.line_count();
    let (mut s, mut e) = match range {
        Range::None | Range::Current => {
            let l = ed.cursor().line;
            (l, l + 1)
        }
        Range::Whole => (0, total),
        Range::Lines(a, b) => (*a, *b),
        Range::Deferred(text) => crate::addr::resolve_deferred(ed, text)?,
    };
    if s >= total {
        s = total.saturating_sub(1);
    }
    if e > total {
        e = total;
    }
    if e <= s {
        e = (s + 1).min(total);
    }
    if print_first || (ed.exmode && e != s + 1) {
        let texts: Vec<String> = {
            let buf = ed.current_buffer();
            let mut out = Vec::with_capacity(e.saturating_sub(s));
            for i in s..e {
                out.push(buf.line_str(i)?.to_string());
            }
            out
        };
        for text in texts {
            ed.messages.info(text);
        }
    }
    let line = e.saturating_sub(1).min(total.saturating_sub(1));
    ed.set_cursor(Pos::new(line, 0));
    Ok(ExOutcome::Continue)
}

/// Run a single parsed command without script context (tests, simple callers).
pub fn run_command(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let mut env = Env::new();
    let mut state = ScriptState::default();
    run_command_ctx(ed, &mut env, &mut state, p)
}

/// Run a single parsed command with script context.
pub fn run_command_ctx(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    // :silent [cmd] — execute the wrapped command(s), suppressing messages.
    // :silent! also swallows the error itself: vim's emsg_silent returns before
    // `ex_exitval = 1` is reached (message.c:700-729), so a suppressed error
    // must not make a batch run exit non-zero.
    // Any abbreviation of :silent (`:sil`, `:sile`, ...) takes this path, which
    // is why the table lookup — not a string compare — decides.
    if lookup(&p.command).map(|d| d.name) == Some("silent") {
        let before = ed.messages.history().len();
        let before_errors = ed.error_count;
        let out = if p.args.is_empty() {
            Err(VimError::range_required())
        } else {
            run_line_with(ed, env, state, &p.args)
        };
        ed.messages.truncate_history(before);
        if p.bang {
            ed.error_count = before_errors;
            return Ok(ExOutcome::Continue);
        }
        return out;
    }

    // Builtin commands first; a missing builtin may still be a user-defined
    // command (`:command MyCmd ...`), which Vim resolves as a last resort.
    let lookup_result = lookup(&p.command);
    let def = match lookup_result {
        Some(def) => def,
        None => return run_user_command(ed, env, state, p),
    };
    if !IMPLEMENTED.contains(&def.name) {
        return Err(VimError::new(
            "ERV337",
            format!("Command not implemented yet: {}", def.name),
        ));
    }

    // What the user typed, before the table lookup rewrites it to the
    // canonical name (`:diff` resolves to `diffupdate`).
    let typed_command = p.command.clone();

    // Handlers distinguish families by looking at `p.command` (`:v`/`:vglobal`
    // inversions, `:le`/`:left` alignment, `:ab`/`:abbreviate`, `:bf`/`:bfirst`),
    // so they must see the table's canonical name, not the typed abbreviation.
    let mut canonical = p.clone();
    canonical.command = def.name.to_string();
    let p = &canonical;

    match def.name {
        "quit" | "qall" => {
            if ed.current_buffer().is_changed() && !p.bang {
                return Err(VimError::no_write_since_change());
            }
            Ok(ExOutcome::Quit {
                bang: p.bang,
                exit_code: 0,
            })
        }
        "cquit" => Ok(ExOutcome::QuitAll {
            bang: p.bang,
            exit_code: parse_exit_code(&p.args).unwrap_or(1),
        }),
        "write" | "wq" | "xit" | "wall" => write_current(ed, env, state, p),
        "edit" => cmd_edit(ed, env, state, p),
        "enew" => {
            let id = ed.new_buffer();
            crate::autodispatch::switch_buffer(ed, env, state, id)?;
            Ok(ExOutcome::Continue)
        }
        "buffer" => cmd_buffer(ed, env, state, p),
        "bnext" | "bprevious" => {
            let delta: i32 = if def.name == "bnext" { 1 } else { -1 };
            let n = ed.buffer_count() as i32;
            let cur = ed.current_bufnr();
            let target = ((cur - 1 + delta).rem_euclid(n)) + 1;
            crate::autodispatch::switch_buffer(ed, env, state, target)?;
            Ok(ExOutcome::Continue)
        }
        "bdelete" => {
            let id = if p.args.is_empty() {
                ed.current_bufnr()
            } else {
                p.args
                    .parse::<i32>()
                    .map_err(|_| VimError::invalid_argument("buffer number"))?
            };
            if ed.buffer_count() <= 1 {
                return Err(VimError::new("E516", "No buffers were deleted".to_string()));
            }
            cmd_bdelete(ed, id, p.bang)?;
            crate::autodispatch::fire_for_buffer(ed, env, state, "BufDelete");
            Ok(ExOutcome::Continue)
        }
        "buffers" | "ls" | "files" => {
            let rows: Vec<String> = ed
                .all_buffers()
                .iter()
                .map(|b| {
                    let cur = if b.id == ed.current_bufnr() { "%" } else { " " };
                    let ch = if b.is_changed() { "+" } else { " " };
                    format!("{cur}{ch} {}\t{}", b.id, b.display_name())
                })
                .collect();
            for row in rows {
                ed.messages.info(row);
            }
            Ok(ExOutcome::Continue)
        }
        "read" => cmd_read(ed, p),
        "delete" => cmd_delete(ed, p),
        "yank" => cmd_yank(ed, p),
        "copy" | "t" | "move" => cmd_copy_move(ed, p, def.name == "move"),
        "print" | "list" | "number" | "p" => cmd_print(ed, p, def.name),
        "substitute" => cmd_substitute(ed, p),
        "undo" => {
            ed.undo()?;
            Ok(ExOutcome::Continue)
        }
        "redo" => {
            ed.redo()?;
            Ok(ExOutcome::Continue)
        }
        "set" | "setlocal" | "setglobal" => {
            let scope = match def.name {
                "setlocal" => SetScope::Local,
                "setglobal" => SetScope::Global,
                _ => SetScope::All,
            };
            cmd_set(ed, &p.args, scope)?;
            Ok(ExOutcome::Continue)
        }
        // --- M5: windows and tabpages -----------------------------------
        "split" | "vsplit" | "sview" | "new" | "vnew" => {
            crate::handlers_win::cmd_splitview(ed, env, state, p, def.name)
        }
        "close" | "hide" => crate::handlers_win::cmd_close(ed, env, state, p, def.name),
        "only" => crate::handlers_win::cmd_only(ed, env, state),
        "resize" => crate::handlers_win::cmd_resize(ed, p),
        "vertical" => crate::handlers_win::cmd_vertical(ed, env, state, p),
        "wincmd" => crate::handlers_win::cmd_wincmd(ed, p),
        "tabnew" | "tabedit" => crate::handlers_win::cmd_tabnew(ed, env, state, p, def.name),
        "tabclose" | "tabonly" | "tabnext" | "tabprevious" | "tabfirst" | "tablast" | "tabmove" => {
            crate::handlers_win::cmd_tab(ed, env, state, p, def.name)
        }
        "tabs" => crate::handlers_win::cmd_tabs(ed),
        "windo" | "tabdo" | "bufdo" | "argdo" => {
            crate::handlers_win::cmd_doall(ed, env, state, p, def.name)
        }
        // --- M5: quickfix / location lists ----------------------------------
        "copen" | "cclose" | "cnext" | "cprevious" | "cNext" | "cfirst" | "clast" | "cc"
        | "crewind" | "cwindow" | "cbelow" | "cabove" | "cexpr" | "caddexpr" | "cgetexpr"
        | "cfile" | "cgetfile" | "clist" | "colder" | "cnewer" | "lopen" | "lclose" | "lnext"
        | "lprevious" | "lNext" | "lfirst" | "llast" | "ll" | "lrewind" | "lwindow" | "lexpr"
        | "laddexpr" | "lgetexpr" | "lfile" | "lgetfile" | "llist" | "lolder" | "lnewer"
        | "lbelow" | "labove" => crate::handlers_qf::cmd_quickfix(ed, env, state, p, def.name),
        // --- M5: session (jumps, changes, viminfo) --------------------------
        "jumps" | "changes" | "rviminfo" | "wviminfo" => {
            crate::handlers_session::cmd_session(ed, env, state, p, def.name)
        }
        // --- M5: diff mode -------------------------------------------------
        "diffthis" | "diffoff" | "diffsplit" | "diffupdate" | "diffget" | "diffput" => {
            let name = if typed_command == "diff" {
                "diff"
            } else {
                def.name
            };
            crate::handlers_diff::cmd_diff(ed, env, state, p, name)
        }
        // --- M5: folds ----------------------------------------------------
        "fold" | "foldopen" | "foldclose" | "folddoopen" | "folddoclosed" => {
            crate::handlers_fold::cmd_fold(ed, env, state, p, def.name)
        }
        "nohlsearch" => {
            ed.search_pattern.clear();
            Ok(ExOutcome::Continue)
        }
        "put" => cmd_put(ed, env, state, p),
        "k" | "mark" => {
            // ":k{a-zA-Z}" / ":mark {name}": set a mark on the addressed line.
            let (line, _) = resolve_range(ed, &p.range, false)?;
            let name = p.args.trim().chars().next();
            match name {
                Some(c) if c.is_ascii_alphabetic() => {
                    let bufnr = ed.current_bufnr();
                    ed.marks.set_local(bufnr, c, Pos::new(line, 0));
                    Ok(ExOutcome::Continue)
                }
                Some(c) => Err(VimError::new("E191", format!("Invalid register name: {c}"))),
                None => Err(VimError::new("E471", "Argument required".to_string())),
            }
        }
        "echo" | "echomsg" | "echon" | "echoerr" => {
            crate::handlers_viml::cmd_echo(ed, env, state, p, def.name)
        }
        "let" => crate::handlers_viml::cmd_let(ed, env, state, p),
        "unlet" => crate::handlers_viml::cmd_unlet(ed, env, state, p),
        "execute" => crate::handlers_viml::cmd_execute(ed, env, state, p),
        "eval" => crate::handlers_viml::cmd_eval(ed, env, state, p),
        "call" => crate::handlers_viml::cmd_call(ed, env, state, p),
        "function" => crate::handlers_viml::cmd_function(ed, env, state, p),
        "delfunction" => crate::handlers_viml::cmd_delfunction(ed, env, state, p),
        "setf" | "setfiletype" => crate::handlers_viml::cmd_setfiletype(ed, env, state, p),
        "echohl" => Ok(ExOutcome::Continue),
        "syntax" => crate::handlers_syntax::cmd_syntax(ed, p),
        "highlight" => crate::handlers_syntax::cmd_highlight(ed, p),
        "filetype" => crate::handlers_viml::cmd_filetype_state(ed, env, state, p),
        "if" | "elseif" | "else" | "endif" | "while" | "endwhile" | "for" | "endfor" | "try"
        | "catch" | "finally" | "endtry" | "break" | "continue" | "throw" | "return" => {
            Err(VimError::new(
                "EVL001",
                format!(":{} only works inside a sourced script", def.name),
            ))
        }
        "version" => {
            ed.messages.info(crate::version_banner());
            Ok(ExOutcome::Continue)
        }
        "help" => {
            ed.messages.info(
                "RustVim help: see https://vim-jp.org/vimdoc-en/version8.html (bundled docs arrive in M5)"
                    .to_string(),
            );
            Ok(ExOutcome::Continue)
        }
        "pwd" => {
            ed.messages.info(ed.cwd.display().to_string());
            Ok(ExOutcome::Continue)
        }
        "cd" | "chdir" => {
            let dir = PathBuf::from(&p.args);
            std::env::set_current_dir(&dir).map_err(|e| {
                VimError::new("E344", format!("Can't cd to {}: {e}", dir.display()))
            })?;
            ed.cwd = std::env::current_dir().unwrap_or(dir);
            Ok(ExOutcome::Continue)
        }
        "file" => {
            let b = ed.current_buffer();
            ed.messages.info(format!(
                "\"{}\" {} line{} -- {}% -- (col {})",
                b.display_name(),
                b.line_count(),
                if b.line_count() == 1 { "" } else { "s" },
                ((ed.cursor().line + 1) * 100) / b.line_count().max(1),
                ed.cursor().vim_col()
            ));
            Ok(ExOutcome::Continue)
        }
        "marks" => {
            ed.messages.info(
                "marks: local a-z / global A-Z storage ready (normal-mode m{a-z} supported)"
                    .to_string(),
            );
            Ok(ExOutcome::Continue)
        }
        "registers" | "reg" | "display" => {
            let names = ['"', '0', '1', '2', '-', '.', ':', '/', '_'];
            for n in names {
                if let Some(r) = ed.registers.get(n) {
                    ed.messages
                        .info(format!("{n} {:?} {:?}", r.reg_type, r.text));
                }
            }
            Ok(ExOutcome::Continue)
        }
        // --- M2 command families -------------------------------------
        "autocmd" => crate::handlers_script::cmd_autocmd(ed, env, state, p),
        "augroup" => crate::handlers_script::cmd_augroup(ed, p),
        "doautocmd" => crate::handlers_script::cmd_doautocmd(ed, env, state, p),
        "command" => crate::handlers_script::cmd_command(ed, p),
        "delcommand" => crate::handlers_script::cmd_delcommand(ed, p),
        "comclear" => crate::handlers_script::cmd_comclear(ed, p),
        "source" => crate::handlers_script::cmd_source(ed, env, state, p),
        "runtime" => crate::handlers_script::cmd_runtime(ed, env, state, p),
        "finish" => Ok(ExOutcome::Finish),
        "global" | "vglobal" => crate::handlers_edit::cmd_global(ed, env, state, p),
        "sort" => crate::handlers_edit::cmd_sort(ed, p),
        "join" => crate::handlers_edit::cmd_join(ed, p),
        "left" | "right" | "center" => crate::handlers_edit::cmd_align(ed, p),
        ">" | "<" => crate::handlers_edit::cmd_shift(ed, p),
        "retab" => crate::handlers_edit::cmd_retab(ed, p),
        "#" => crate::handlers_edit::cmd_hash_print(ed, p),
        "&" | "~" => crate::handlers_edit::cmd_repeat_substitute(ed, p),
        "args" => crate::handlers_edit::cmd_args(ed, p),
        "argadd" => crate::handlers_edit::cmd_argadd(ed, p),
        "argdelete" => crate::handlers_edit::cmd_argdelete(ed, p),
        "badd" => crate::handlers_edit::cmd_badd(ed, p),
        "bfirst" | "blast" => crate::handlers_edit::cmd_buffer_edge(ed, p),
        "bunload" | "bwipeout" => crate::handlers_edit::cmd_bunload(ed, p),
        "bmodified" => crate::handlers_edit::cmd_bmodified(ed, p),
        "update" => crate::handlers_edit::cmd_update(ed, env, state, p),
        "saveas" => crate::handlers_edit::cmd_saveas(ed, env, state, p),
        "wqall" => crate::handlers_edit::cmd_wqall(ed, env, state, p),
        "quitall" => {
            if ed.all_buffers().iter().any(|b| b.is_changed()) && !p.bang {
                return Err(VimError::no_write_since_change());
            }
            Ok(ExOutcome::QuitAll {
                bang: p.bang,
                exit_code: 0,
            })
        }
        "exit" => {
            if ed.current_buffer().is_changed() {
                write_current(ed, env, state, p)?;
            }
            Ok(ExOutcome::QuitAll {
                bang: p.bang,
                exit_code: 0,
            })
        }
        "stop" | "suspend" => crate::handlers_edit::cmd_stop(ed, p),
        "messages" => {
            let history = ed.messages.history().to_vec();
            for line in history {
                ed.messages.info(line);
            }
            Ok(ExOutcome::Continue)
        }
        "redraw" => Ok(ExOutcome::Continue),
        "normal" => crate::handlers_normal::cmd_normal(ed, p),
        "!" => cmd_shell(ed, p),
        other if crate::handlers_map::is_map_command(other) => {
            crate::handlers_map::map_command(ed, p)
        }
        other if crate::handlers_map::is_abbrev_command(other) => {
            crate::handlers_map::abbrev_command(ed, p)
        }
        other => Err(VimError::internal(format!(
            "implemented command {other} has no dispatch arm"
        ))),
    }
}

/// Apply `++opt=val` file arguments to the current buffer (`:e ++ff=dos`,
/// `:w ++bin`...). Only the options that make sense for the M2 UTF-8 model are
/// accepted; anything else is an explicit E474 rather than a silent no-op.
fn apply_fileopts(ed: &mut Editor, p: &ParsedEx) -> VimResult<()> {
    for opt in &p.fileopts {
        match opt.name.as_str() {
            "ff" => {
                let value = opt
                    .value
                    .as_deref()
                    .ok_or_else(|| VimError::invalid_argument("++ff requires a value"))?;
                let ff = rvim_buffer::FileFormat::parse(value)?;
                let bufnr = ed.current_bufnr();
                ed.buffer_mut(bufnr)?.set_fileformat(ff);
                ed.options.set(
                    "fileformat",
                    rvim_editor::OptValue::Str(ff.vim_name().into()),
                )?;
            }
            "bin" => {
                // Binary mode: no EOL translation, so use 'unix' terminators.
                let bufnr = ed.current_bufnr();
                ed.buffer_mut(bufnr)?
                    .set_fileformat(rvim_buffer::FileFormat::Unix);
            }
            "enc" => {
                let value = opt.value.as_deref().unwrap_or("utf-8");
                if !value.eq_ignore_ascii_case("utf-8") && !value.eq_ignore_ascii_case("utf8") {
                    return Err(VimError::new(
                        "E474",
                        format!("Only UTF-8 is supported in this milestone: ++enc={value}"),
                    ));
                }
            }
            other => {
                return Err(VimError::new("E474", format!("Unsupported ++opt: {other}")));
            }
        }
    }
    Ok(())
}

/// Run a `+cmd` / `+N` argument after `:edit`.
fn apply_plus_cmd(ed: &mut Editor, p: &ParsedEx) -> VimResult<()> {
    let Some(plus) = p.plus.as_deref() else {
        return Ok(());
    };
    if let Ok(line) = plus.parse::<usize>() {
        let target = line
            .saturating_sub(1)
            .min(ed.line_count().saturating_sub(1));
        let col = ed.current_buffer().first_nonblank_byte(target).unwrap_or(0);
        ed.set_cursor(Pos::new(target, col));
        return Ok(());
    }
    run_line(ed, plus)?;
    Ok(())
}
/// Execute a user-defined command (`:command`): resolve placeholders, validate
/// `-nargs`, then run the replacement text as Ex commands.
fn run_user_command(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let Some(cmd) = ed.usercmds.get(&p.command).cloned() else {
        return Err(VimError::not_editor_command(&p.command));
    };
    if !cmd.bang && p.bang {
        return Err(VimError::new(
            "E477",
            format!("No ! allowed for user command: {}", cmd.name),
        ));
    }
    // `-count` / `-range=N` accept the count as an initial argument, e.g.
    // "Cnt 3" (usercmd.c:891-912, ex_docmd.c:2354-2384).
    let mut args = p.args.clone();
    let mut typed_count: Option<u32> = None;
    if cmd.count.is_some() {
        let trimmed = args.trim_start();
        let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            typed_count = digits.parse().ok();
            args = trimmed[digits.len()..].trim_start().to_string();
        }
    }
    // `-count` and `-range=N` imply a range (usercmd.c:891-912 sets EX_RANGE).
    let has_range = cmd.range || cmd.count.is_some();
    cmd.nargs.check(&args)?;
    // -range=% (EX_DFLALL) defaults to the whole file; a plain -range defaults
    // to the current line (usercmd.c:862-889, ex_docmd.c:2312-2313).
    let range = if has_range {
        if matches!(p.range, Range::None) {
            if cmd.count == Some(u32::MAX) {
                Some((0, ed.line_count()))
            } else {
                let l = ed.cursor().line;
                Some((l, (l + 1).min(ed.line_count())))
            }
        } else {
            Some(resolve_range(ed, &p.range, false)?)
        }
    } else {
        None
    };
    // <count> is the typed count, else the last addressed line, else the
    // attribute default (usercmd.c:1660-1669).
    let mut expand_cmd = cmd.clone();
    if cmd.count.is_some() {
        let n = if let Some(tc) = typed_count {
            tc
        } else if p.addr_count > 0 {
            range.map(|(_, e)| e as u32).unwrap_or(0)
        } else {
            cmd.count.filter(|n| *n != u32::MAX).unwrap_or(0)
        };
        expand_cmd.count = Some(n);
    }
    // <range> expands to the *number of addresses* (usercmd.c:1668 ct_RANGE),
    // which usercmd::expand_placeholders does not model.
    expand_cmd.command = expand_cmd
        .command
        .replace("<range>", &p.addr_count.to_string());
    let expanded = rvim_editor::usercmd::expand_placeholders(&expand_cmd, &args, p.bang, range)?;
    // Note: vim does not move the cursor for user commands; the replacement
    // text runs at the current position (like any other Ex command).
    // `-expr`: the replacement text is evaluated and its result is executed as
    // an Ex command (vim's -expr attribute).
    if cmd.expr {
        let value = crate::script::eval_expr(ed, env, state, &expanded)?;
        let text = value.to_string_value();
        return run_line_with(ed, env, state, &text);
    }
    run_line_with(ed, env, state, &expanded)
}

/// The `:write`-family body, shared with `:update` and `:saveas`.
pub fn write_current(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let name = match lookup(&p.command) {
        Some(d) => d.name,
        None => "write",
    };
    // ":update" and ":xit" write only when the buffer changed.
    if (name == "xit" || name == "update") && !ed.current_buffer().is_changed() {
        return if name == "xit" {
            Ok(ExOutcome::Quit {
                bang: p.bang,
                exit_code: 0,
            })
        } else {
            Ok(ExOutcome::Continue)
        };
    }
    let path = if p.args.is_empty() {
        None
    } else {
        Some(PathBuf::from(&p.args))
    };
    apply_fileopts(ed, p)?;
    // BufWritePre / write / BufWritePost (autocmd.c).
    crate::autodispatch::write_file(ed, env, state, path.as_deref(), p.bang)?;
    let written = ed
        .current_buffer()
        .name
        .as_ref()
        .map(|x| x.display().to_string())
        .unwrap_or_default();
    ed.messages
        .info(format!("\"{written}\" {}L written", ed.line_count()));
    if name == "wq" || name == "xit" {
        Ok(ExOutcome::Quit {
            bang: p.bang,
            exit_code: 0,
        })
    } else {
        Ok(ExOutcome::Continue)
    }
}

fn parse_exit_code(s: &str) -> Option<i32> {
    s.trim().parse().ok()
}

fn cmd_edit(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    // ":e" refuses to abandon unsaved changes; ":e!" overrides
    // (ex_cmds.c:2593-2604 do_ecmd → check_changed(CCGD_EXCMD) → E37).
    if !p.bang && ed.current_buffer().is_changed() {
        return Err(VimError::new(
            "E37",
            "No write since last change (add ! to override)".to_string(),
        ));
    }
    if p.args.is_empty() {
        let name = ed.current_buffer().name.clone();
        if let Some(path) = name {
            let id = ed.new_buffer();
            let buf = rvim_buffer::Buffer::open(id, &path)?;
            ed.replace_buffer(id, buf)?;
            crate::autodispatch::switch_buffer(ed, env, state, id)?;
            apply_fileopts(ed, p)?;
            apply_plus_cmd(ed, p)?;
            // Ex starts on the last line of a freshly edited file (main.c:719).
            if ed.exmode {
                ed.start_ex_mode_cursor();
            }
        }
        return Ok(ExOutcome::Continue);
    }
    // ":e {file}" fires BufReadPre/BufRead|BufNewFile/BufEnter.
    crate::autodispatch::edit_file(ed, env, state, &PathBuf::from(&p.args))?;
    apply_fileopts(ed, p)?;
    apply_plus_cmd(ed, p)?;
    // Ex starts on the last line of a freshly edited file (main.c:719).
    if ed.exmode {
        ed.start_ex_mode_cursor();
    }
    Ok(ExOutcome::Continue)
}

fn cmd_buffer(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    if p.args.is_empty() {
        return Ok(ExOutcome::Continue);
    }
    if let Ok(n) = p.args.parse::<i32>() {
        crate::autodispatch::switch_buffer(ed, env, state, n)?;
        return Ok(ExOutcome::Continue);
    }
    let target = ed
        .all_buffers()
        .iter()
        .find(|b| {
            b.display_name() == p.args || b.name.as_ref().is_some_and(|n| n.ends_with(&p.args))
        })
        .map(|b| b.id)
        .ok_or_else(|| VimError::new("E86", format!("Buffer not found: {}", p.args)))?;
    crate::autodispatch::switch_buffer(ed, env, state, target)?;
    Ok(ExOutcome::Continue)
}

fn cmd_bdelete(ed: &mut Editor, id: i32, bang: bool) -> VimResult<()> {
    let changed = ed.buffer(id)?.is_changed();
    if changed && !bang {
        return Err(VimError::no_write_since_change());
    }
    if id == ed.current_bufnr() {
        let fallback = ed
            .all_buffers()
            .iter()
            .find(|b| b.id != id)
            .map(|b| b.id)
            .unwrap();
        ed.switch_buffer(fallback)?;
    }
    ed.drop_buffer(id);
    Ok(())
}

fn cmd_read(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    if p.args.is_empty() {
        return Err(VimError::range_required());
    }
    let text = std::fs::read_to_string(&p.args)
        .map_err(|e| VimError::new("E484", format!("Can't open file {}: {e}", p.args)))?;
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (_, e) = resolve_range(ed, &p.range, false)?;
    // ":read" inserts *after* the addressed line; with a range the last address
    // is used (ex_docmd.c:7369/7401 readfile(..., eap->line2, ...)).
    // ":0read" is the special case that inserts before the first line.
    let addressed = e.saturating_sub(1);
    let zero_address = matches!(&p.range, Range::Deferred(t) if t.trim() == "0");
    let at = if zero_address {
        0
    } else {
        (addressed + 1).min(ed.line_count())
    };
    let count = lines.len();
    let first = at;
    ed.replace_lines(at, at, lines)?;
    // Vim leaves the cursor on the last line read (do_read in ex_cmds.c).
    if count > 0 {
        let last = (first + count - 1).min(ed.line_count().saturating_sub(1));
        ed.set_cursor(Pos::new(last, 0));
    }
    Ok(ExOutcome::Continue)
}

/// Parse the register / count tail of `:delete` and `:yank`.
///
/// Vim order (ex_docmd.c:2315-2398): an optional register name first, then a
/// numeric count; both commands carry EX_REGSTR|EX_COUNT so a *leading digit* is
/// a count, never a register (ex_docmd.c:2315-2320). Any remaining text is
/// E488 (neither command has EX_EXTRA or EX_FLAGS: ":2d 2 l" is an error and
/// only the ":dl"/":dp" command-word quirk sets the list/print flag).
fn split_count_register(args: &str) -> VimResult<(Option<usize>, Option<char>)> {
    let mut rest = args.trim();
    let mut reg = None;
    if let Some(c) = rest.chars().next() {
        let is_register = c.is_ascii_alphabetic()
            || matches!(c, '"' | '-' | '_' | '#' | '%' | '.' | ':' | '/' | '=' | '~');
        if is_register {
            reg = Some(c);
            rest = rest[c.len_utf8()..].trim_start();
        }
    }
    let mut count = None;
    if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        let len = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let n: usize = rest[..len]
            .parse()
            .map_err(|_| VimError::new("E499", "Positive count required".to_string()))?;
        rest = rest[len..].trim_start();
        if n == 0 {
            return Err(VimError::new("E499", "Positive count required".to_string()));
        }
        count = Some(n);
    }
    if !rest.is_empty() {
        return Err(VimError::trailing_char(rest));
    }
    Ok((count, reg))
}

fn cmd_delete(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let (s, e) = resolve_range(ed, &p.range, false)?;
    let mut q = p.clone();
    let list_flag = crate::parser::take_delete_flag(&mut q);
    // A trailing [count] extends the range from its last line: ":2d 2" deletes
    // lines 2-3 (ex_docmd.c:2354-2384 sets line1 = line2, line2 += n-1).
    let (count, reg) = split_count_register(&q.args)?;
    let (s, e) = match count {
        Some(n) => {
            let s = e.saturating_sub(1);
            (s, (s + n).min(ed.line_count()))
        }
        None => (s, e),
    };
    ed.delete_lines(s, e, reg)?;
    // ":dl" / ":dp" delete and then list/print the deleted lines
    // (ex_docmd.c delete_flag_quirk + ex_may_print()).
    if let Some(flag) = list_flag {
        if let Some(text) = ed.registers.get(reg.unwrap_or('"')).map(|r| r.text.clone()) {
            for line in text.trim_end_matches('\n').lines() {
                let rendered = match flag {
                    'l' => format!("{line}$"),
                    '#' => format!("{:>6}\t{line}", s + 1),
                    _ => line.to_string(),
                };
                ed.messages.info(rendered);
            }
        }
    }
    ed.set_cursor(Pos::new(s.min(ed.line_count().saturating_sub(1)), 0));
    Ok(ExOutcome::Continue)
}

fn cmd_yank(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let (s, e) = resolve_range(ed, &p.range, false)?;
    // Same count/register parsing as :delete; ":2y 2" yanks two lines.
    let (count, reg) = split_count_register(&p.args)?;
    let (s, e) = match count {
        Some(n) => {
            let s = e.saturating_sub(1);
            (s, (s + n).min(ed.line_count()))
        }
        None => (s, e),
    };
    ed.yank_lines(s, e, reg)?;
    // Vim's ":yank" does not move the cursor (ex_cmds.c ex_yank → do_yank only).
    Ok(ExOutcome::Continue)
}

fn cmd_copy_move(ed: &mut Editor, p: &ParsedEx, is_move: bool) -> VimResult<ExOutcome> {
    let (s, e) = resolve_range(ed, &p.range, false)?;
    // The destination is a full Ex address, not just a number: ":co$", ":m.",
    // ":t+1", ":m'a" are all valid (ex_docmd.c:8014 get_address()).
    let at = resolve_destination(ed, &p.args)?;
    let texts: Vec<String> = ed.current_buffer().lines()[s..e]
        .iter()
        .map(|l| l.as_str().to_string())
        .collect();
    let count = texts.len();
    let insert_at = if is_move {
        // ":move" does not touch any register, but the delete step below does;
        // save and restore the register set around it (ex_cmds.c do_move()).
        let registers = ed.registers.clone();
        ed.delete_lines(s, e, None)?;
        ed.registers = registers;
        if at > s { at.saturating_sub(count) } else { at }
    } else {
        at
    };
    let insert_at = insert_at.min(ed.line_count());
    ed.replace_lines(insert_at, insert_at, texts)?;
    // Vim leaves the cursor on the last moved/copied line, at its first
    // non-blank (do_move/ex_copy + beginline(BL_SOL|BL_FIX)).
    let last = (insert_at + count).saturating_sub(1);
    ed.set_cursor(Pos::new(last.min(ed.line_count().saturating_sub(1)), 0));
    Ok(ExOutcome::Continue)
}

/// Resolve the destination address of `:copy`/`:move` to a 0-based insert index
/// ("after line N" means inserting before 0-based index N).
fn resolve_destination(ed: &Editor, args: &str) -> VimResult<usize> {
    let text = args.trim();
    if text.is_empty() {
        return Err(VimError::invalid_argument("target line"));
    }
    // ":co0" / ":m0" mean "before the first line".
    if text == "0" {
        return Ok(0);
    }
    let (line, _) = crate::addr::resolve_deferred(ed, text)?;
    Ok((line + 1).min(ed.line_count()))
}

/// Public wrapper over [`resolve_destination`] for the `:global` line tracking.
pub fn resolve_destination_public(ed: &Editor, args: &str) -> VimResult<usize> {
    resolve_destination(ed, args)
}

fn cmd_print(ed: &mut Editor, p: &ParsedEx, name: &str) -> VimResult<ExOutcome> {
    let (s, e) = resolve_range(ed, &p.range, false)?;
    for i in s..e {
        let text = ed.current_buffer().line_str(i)?;
        let line = match name {
            "number" => format!("{:>6}\t{}", i + 1, text),
            "list" => format!("{text}$"),
            _ => text.to_string(),
        };
        ed.messages.info(line);
    }
    // ":p"/":l"/":nu" leave the cursor on the last line printed.
    let last = e.saturating_sub(1).min(ed.line_count().saturating_sub(1));
    ed.set_cursor(Pos::new(last, 0));
    Ok(ExOutcome::Continue)
}

/// Minimal ":s/pat/repl/[flags]" over the addressed lines. No regex engine
/// until M6 — the pattern is literal; `ignorecase` applies. Flags: `g` (all
/// matches per line), `i` (ignore case), `n` (count matches, do not substitute).
/// Flags of `:substitute` (ex_cmds.c:3883-3941).
#[derive(Clone, Copy, Debug, Default)]
struct SubFlags {
    global: bool,
    /// `i` forces ignore-case on, `I` forces it off, `None` keeps 'ignorecase'.
    ignorecase: Option<bool>,
    count_only: bool,
    /// `e`: do not report E486 when there is no match.
    no_error: bool,
}

/// Split an `:s` argument tail into (pattern, replacement, flags) honouring
/// backslash-escaped delimiters.
fn split_substitute(body: &str) -> Option<(String, String, String)> {
    let delim = body.chars().next()?;
    let mut parts: Vec<String> = vec![String::new()];
    let mut escaped = false;
    let mut closed = 0usize;
    for c in body.chars().skip(1) {
        if escaped {
            parts.last_mut().unwrap().push('\\');
            parts.last_mut().unwrap().push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == delim && closed < 2 {
            closed += 1;
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    if escaped {
        parts.last_mut().unwrap().push('\\');
    }
    if closed < 1 {
        return None;
    }
    let pattern = parts.first().cloned().unwrap_or_default();
    let replacement = parts.get(1).cloned().unwrap_or_default();
    let rest = parts.get(2).cloned().unwrap_or_default();
    Some((pattern, replacement, rest))
}

/// Parse the flag string that follows the replacement, then a trailing count.
/// Vim: 'g' toggles do_all, 'e' toggles do_error, 'i'/'I' override case
/// (ex_cmds.c:3883-3918); a trailing number means "substitute on N lines"
/// (ex_cmds.c:3928-3941).
fn parse_sub_flags(flags: &str) -> VimResult<(SubFlags, Option<usize>)> {
    let mut out = SubFlags::default();
    let mut chars = flags.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            'g' => out.global = !out.global,
            'c' => {}
            'e' => out.no_error = !out.no_error,
            'n' => out.count_only = true,
            'i' => out.ignorecase = Some(true),
            'I' => out.ignorecase = Some(false),
            'p' | '#' | 'l' | 'r' => {}
            '0'..='9' => {
                let mut n = c.to_digit(10).unwrap() as usize;
                while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                    n = n * 10 + d as usize;
                    chars.next();
                }
                return Ok((out, Some(n)));
            }
            other => {
                return Err(VimError::new(
                    "E488",
                    format!("Trailing characters: {other}"),
                ));
            }
        }
    }
    Ok((out, None))
}

/// Replace literal `pattern` occurrences in `[start, end)`; returns the number
/// of changed lines. Never fails on "no match" (the caller decides about E486).
fn substitute_lines(
    ed: &mut Editor,
    start: usize,
    end: usize,
    pattern: &str,
    replacement: &str,
    global: bool,
    ignorecase: Option<bool>,
) -> VimResult<usize> {
    let ignorecase = ignorecase.unwrap_or_else(|| ed.options.bool_or("ignorecase", false));
    // One ":s" command is one undo step even across many lines (undo.c u_sync()
    // runs once per Ex command, not per edited line).
    ed.begin_undo_block();
    let mut matched = 0usize;
    let mut last_changed = None;
    let mut failure = None;
    for i in start..end {
        match ed.current_buffer().line_str(i) {
            Ok(text) => {
                let original = text.to_string();
                let replaced = literal_replace(&original, pattern, replacement, global, ignorecase);
                if replaced != original {
                    matched += 1;
                    last_changed = Some(i);
                    if let Err(e) = ed.set_line_text(i, replaced) {
                        failure = Some(e);
                        break;
                    }
                }
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    ed.end_undo_block();
    if failure.is_none() {
        // Vim leaves the cursor on the last line where a substitution happened
        // (ex_cmds.c:3895-3902 do_sub()).
        if let Some(line) = last_changed {
            ed.set_cursor(Pos::new(line, 0));
        }
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(matched),
    }
}

fn cmd_substitute(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let body = p.args.trim();
    let (subst, flags, trailing_count) = if body.is_empty() {
        // ":s" repeats the previous substitute with the default flags
        // (ex_cmds.c:3794-3807).
        let Some(last) = ed.last_sub.clone() else {
            return Err(VimError::new(
                "E33",
                "No previous substitute regular expression".to_string(),
            ));
        };
        let subst = rvim_editor::LastSubstitute {
            pattern: last.pattern,
            replacement: last.replacement,
            global: false,
            count: 0,
        };
        (subst, SubFlags::default(), None)
    } else {
        let Some((pat, repl, rest)) = split_substitute(body) else {
            return Err(VimError::new(
                "E146",
                format!("Invalid argument: {}", p.args),
            ));
        };
        let (flags, count) = parse_sub_flags(&rest)?;
        // An empty pattern reuses the last used search pattern
        // (search_regcomp with RE_LAST, search.c:140-158).
        let pattern = if pat.is_empty() {
            let last = ed.search_pattern.clone();
            if last.is_empty() {
                return Err(VimError::new(
                    "E35",
                    "No previous regular expression".to_string(),
                ));
            }
            last
        } else {
            pat
        };
        // Both the search and the substitute pattern are saved
        // (save_re_pat / RE_SUBST): ":s/pat/repl/" also becomes the last used
        // search pattern.
        ed.search_pattern = pattern.clone();
        let subst = rvim_editor::LastSubstitute {
            pattern,
            replacement: repl,
            global: flags.global,
            count: 0,
        };
        (subst, flags, count)
    };
    // ":s/pat/repl/" records the pattern/replacement even when nothing matches
    // (ex_cmds.c saves them before searching).
    ed.last_sub = Some(subst.clone());
    let (mut s, mut e) = resolve_range(ed, &p.range, false)?;
    // A trailing count means "substitute on N lines starting at the last
    // addressed line" (ex_cmds.c:3928-3941), not the Nth match.
    if let Some(n) = trailing_count {
        s = e.saturating_sub(1);
        e = (s + n).min(ed.line_count());
    }
    if flags.count_only {
        let ignorecase = flags
            .ignorecase
            .unwrap_or_else(|| ed.options.bool_or("ignorecase", false));
        let mut hits = 0usize;
        for i in s..e {
            let original = ed.current_buffer().line_str(i)?.to_string();
            if literal_replace(&original, &subst.pattern, "", true, ignorecase) != original {
                hits += 1;
            }
        }
        ed.messages
            .info(format!("{hits} match{}", if hits == 1 { "" } else { "es" }));
        return Ok(ExOutcome::Continue);
    }
    let matched = substitute_lines(
        ed,
        s,
        e,
        &subst.pattern,
        &subst.replacement,
        subst.global,
        flags.ignorecase,
    )?;
    if matched == 0 && !flags.no_error {
        return Err(VimError::pattern_not_found(&subst.pattern));
    }
    Ok(ExOutcome::Continue)
}

/// Apply a substitute state to `[start, end)` (used by `:&`).
pub fn apply_substitute(
    ed: &mut Editor,
    start: usize,
    end: usize,
    subst: &rvim_editor::LastSubstitute,
) -> VimResult<ExOutcome> {
    ed.search_pattern = subst.pattern.clone();
    ed.last_sub = Some(subst.clone());
    let matched = substitute_lines(
        ed,
        start,
        end,
        &subst.pattern,
        &subst.replacement,
        subst.global,
        None,
    )?;
    if matched == 0 {
        return Err(VimError::pattern_not_found(&subst.pattern));
    }
    Ok(ExOutcome::Continue)
}

/// Public wrapper over [`resolve_range`] for the handler modules.
pub fn resolve_range_public(
    ed: &Editor,
    range: &Range,
    default_whole: bool,
) -> VimResult<(usize, usize)> {
    resolve_range(ed, range, default_whole)
}

/// Public wrapper over [`substitute_lines`] for `:~` (replacement only from the
/// previous `:s`, pattern from the last search).
pub fn substitute_lines_public(
    ed: &mut Editor,
    start: usize,
    end: usize,
    pattern: &str,
    replacement: &str,
    global: bool,
    ignorecase: Option<bool>,
) -> VimResult<usize> {
    substitute_lines(ed, start, end, pattern, replacement, global, ignorecase)
}

/// Expand the `:s` replacement string: `&` stands for the matched text and a
/// backslash quotes the next character (ex_cmds.c regtilde/do_sub handling of
/// '&').
fn expand_replacement(repl: &str, matched: &str) -> String {
    let mut out = String::new();
    let mut chars = repl.chars();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.push_str(matched),
            '\\' => match chars.next() {
                Some('&') => out.push('&'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            },
            _ => out.push(c),
        }
    }
    out
}

/// Replace `:s` matches in one line.
///
/// The pattern uses vim's magic syntax through the shared MVP regexp engine
/// (`rvim_viml::regex_mvp`, the same subset the runtime scripts use), and the
/// replacement expands `&` to the matched text.
fn literal_replace(
    haystack: &str,
    pat: &str,
    repl: &str,
    global: bool,
    ignorecase: bool,
) -> String {
    if pat.is_empty() {
        return haystack.to_string();
    }
    let mut out = String::new();
    let mut pos = 0usize;
    // End of the previous match; an empty match directly after it is not
    // replaced again (ex_cmds.c do_sub "prev_matchcol" rule).
    let mut prev_end: Option<usize> = None;
    while let Some((s, e)) = rvim_viml::regex_mvp::find(pat, &haystack[pos..], ignorecase) {
        let abs_start = pos + s;
        if e == s && prev_end == Some(abs_start) {
            match haystack[abs_start..].chars().next() {
                Some(ch) => {
                    let next = abs_start + ch.len_utf8();
                    out.push_str(&haystack[pos..next]);
                    pos = next;
                    prev_end = None;
                    continue;
                }
                None => break,
            }
        }
        out.push_str(&haystack[pos..abs_start]);
        out.push_str(&expand_replacement(repl, &haystack[abs_start..pos + e]));
        pos += e;
        prev_end = Some(pos);
        if !global {
            break;
        }
        if e == s {
            // Empty match: copy one character so the loop makes progress.
            let Some(ch) = haystack[pos..].chars().next() else {
                break;
            };
            out.push(ch);
            pos += ch.len_utf8();
            prev_end = None;
        }
    }
    out.push_str(&haystack[pos..]);
    out
}

fn cmd_put(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let args = p.args.trim_start();
    // ":put =expr" inserts the value of the expression (the "=" register;
    // ex_docmd.c:2335-2344 stores the rest of the line as the expression).
    let lines: Vec<String> = if let Some(expr) = args.strip_prefix('=') {
        let value = crate::script::eval_expr(ed, env, state, expr.trim())?;
        match value {
            rvim_viml::Value::List(items) => items
                .borrow()
                .iter()
                .map(rvim_viml::Value::to_string_value)
                .collect(),
            other => other
                .to_string_value()
                .split('\n')
                .map(str::to_string)
                .collect(),
        }
    } else {
        let reg_name = args.chars().next().unwrap_or('"');
        let data = ed
            .registers
            .get(reg_name)
            .cloned()
            .ok_or_else(|| VimError::new("E353", "Nothing to put".to_string()))?;
        data.text
            .trim_end_matches('\n')
            .lines()
            .map(str::to_string)
            .collect()
    };
    // ":put" inserts after the addressed line; "!" (or an address of zero, which
    // behaves like ":1put!") inserts above it (ex_docmd.c:7986-7998: a zero
    // line2 becomes line2=1 + forceit, and forceit means BACKWARD).
    let addressed_1based = match &p.range {
        Range::None => ed.cursor().line + 1,
        Range::Current => ed.cursor().line + 1,
        Range::Whole => ed.line_count(),
        Range::Lines(_, e) => *e,
        Range::Deferred(text) => {
            if text.trim() == "0" {
                0
            } else {
                let (_, last) = crate::addr::resolve_deferred_pair(ed, text)?;
                last + 1
            }
        }
    };
    let (mut line2, mut backward) = (addressed_1based, p.bang);
    if line2 == 0 {
        // ":0put" works like ":1put!".
        line2 = 1;
        backward = true;
    }
    let at = if backward {
        line2.saturating_sub(1).min(ed.line_count())
    } else {
        line2.min(ed.line_count())
    };
    let count = lines.len();
    ed.replace_lines(at, at, lines)?;
    if count > 0 {
        let last = (at + count - 1).min(ed.line_count().saturating_sub(1));
        ed.set_cursor(Pos::new(last, 0));
    }
    Ok(ExOutcome::Continue)
}

fn cmd_set(ed: &mut Editor, args: &str, scope: SetScope) -> VimResult<()> {
    if args.is_empty() {
        for line in ed.options.render_all() {
            ed.messages.info(line);
        }
        return Ok(());
    }
    for token in args.split_whitespace() {
        // 'modified' is a window/buffer flag, not a real option: read the
        // buffer's changed flag and write it through set_modified()
        // (option.c displays it from curbuf->b_changed).
        if token == "modified" || token == "nomodified" {
            ed.set_modified(token == "modified");
            continue;
        }
        if token == "modified?" {
            let changed = ed.current_buffer().is_changed();
            ed.messages
                .info(if changed { "modified" } else { "nomodified" });
            continue;
        }
        if let Some(name) = token.strip_suffix('?') {
            // ":set opt?" reports the value in effect, i.e. the window-local
            // one for a P_WIN option (option.c showoneopt/do_set).
            ed.messages.info(render_option(ed, name)?);
        } else if let Some(name) = token
            .strip_suffix("&vim")
            .or_else(|| token.strip_suffix("&vi"))
        {
            // ":set opt&vim" resets to the Vim default.
            ed.options.reset_default(name)?;
            ed.reset_local_option(name)?;
            if rvim_editor::Options::canonical(name)? == "fileformat" {
                sync_buffer_fileformat(ed);
            }
        } else if let Some(name) = token.strip_suffix('&') {
            ed.options.reset_default(name)?;
            ed.reset_local_option(name)?;
            if rvim_editor::Options::canonical(name)? == "fileformat" {
                sync_buffer_fileformat(ed);
            }
        } else if let Some((name, op, value)) = split_set_compound(token) {
            apply_set_compound(ed, name, op, value, scope)?;
        } else if let Some((name, value)) = token.split_once('=') {
            let resolved = if let Ok(n) = value.parse::<i64>() {
                rvim_editor::OptValue::Num(n)
            } else {
                rvim_editor::OptValue::Str(value.to_string())
            };
            ed.set_option(name, resolved, scope)?;
            if rvim_editor::Options::canonical(name)? == "fileformat" {
                sync_buffer_fileformat(ed);
            }
        } else if let Some(name) = token.strip_prefix("inv") {
            // ":set inv{option}" toggles a boolean option (option.c).
            let canon = rvim_editor::Options::canonical(name)?;
            let current = ed.bool_option(&canon, false);
            ed.set_option(&canon, rvim_editor::OptValue::Bool(!current), scope)?;
        } else if let Some(name) = token.strip_prefix("no") {
            ed.set_option(name, rvim_editor::OptValue::Bool(false), scope)?;
        } else {
            ed.set_option(token, rvim_editor::OptValue::Bool(true), scope)?;
        }
    }
    Ok(())
}

/// Render one option as `:set opt?` prints it, using the value in effect.
fn render_option(ed: &Editor, name: &str) -> VimResult<String> {
    let canon = rvim_editor::Options::canonical(name)?;
    let value = ed.wopt(&canon)?;
    Ok(match value {
        rvim_editor::OptValue::Bool(true) => canon,
        rvim_editor::OptValue::Bool(false) => format!("no{canon}"),
        rvim_editor::OptValue::Num(n) => format!("{canon}={n}"),
        rvim_editor::OptValue::Str(s) => format!("{canon}={s:?}"),
    })
}

/// Push the global `'fileformat'` option onto the current buffer, whose own
/// field is what `:write` uses to choose line terminators (see buffer::save).
fn sync_buffer_fileformat(ed: &mut Editor) {
    let ff = ed.options.str_or("fileformat", "unix");
    if let Ok(parsed) = rvim_buffer::FileFormat::parse(&ff) {
        let bufnr = ed.current_bufnr();
        // Changing 'fileformat' is a buffer-local option change, which marks the
        // buffer modified in Vim (option.c: changing a buffer option sets
        // b_changed; vim reports &modified=1 after ":set ff=dos").
        let changed = match ed.buffer_mut(bufnr) {
            Ok(buf) if buf.fileformat() != parsed => {
                buf.set_fileformat(parsed);
                true
            }
            _ => false,
        };
        if changed {
            ed.set_modified(true);
        }
    }
}

fn cmd_shell(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let shell = std::env::var("COMSPEC")
        .unwrap_or_else(|_| std::env::var("SHELL").unwrap_or_else(|_| "cmd".to_string()));
    let (program, flag) = if shell.contains("cmd") || shell.ends_with("cmd.exe") {
        (shell.as_str(), "/C")
    } else {
        (shell.as_str(), "-c")
    };
    let output = Command::new(program)
        .arg(flag)
        .arg(&p.args)
        .output()
        .map_err(|e| VimError::new("E379", format!("Cannot run shell: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    print!("{text}");
    if !output.status.success() {
        // vim reports a shell return status with smsg(), not emsg()
        // (os_win32.c:5091), so it must not affect the batch exit code.
        ed.messages.info(format!(
            "shell returned {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    Ok(ExOutcome::Continue)
}

/// Split `:set` compound forms (`nrformats-=octal`, `wildignore+=*.o`,
/// `langmap^=a:b`) into (name, op, value).
fn split_set_compound(token: &str) -> Option<(&str, &str, &str)> {
    for op in ["-=", "+=", "^="] {
        if let Some(idx) = token.find(op) {
            let name = &token[..idx];
            let value = &token[idx + op.len()..];
            if !name.is_empty() {
                return Some((name, op, value));
            }
        }
    }
    None
}

/// Apply a compound `:set` operation to a string or number option.
fn apply_set_compound(
    ed: &mut Editor,
    name: &str,
    op: &str,
    value: &str,
    scope: SetScope,
) -> VimResult<()> {
    let canon = rvim_editor::Options::canonical(name)?;
    match ed.wopt(&canon)? {
        rvim_editor::OptValue::Str(current) => {
            let mut items: Vec<String> = current
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            match op {
                "-=" => items.retain(|item| item != value),
                "+=" => {
                    for part in value.split(',').filter(|s| !s.is_empty()) {
                        if !items.iter().any(|i| i == part) {
                            items.push(part.to_string());
                        }
                    }
                }
                "^=" => {
                    for part in value.split(',').filter(|s| !s.is_empty()) {
                        if let Some(pos) = items.iter().position(|i| i == part) {
                            items.remove(pos);
                        } else {
                            items.push(part.to_string());
                        }
                    }
                }
                _ => {}
            }
            ed.set_option(&canon, rvim_editor::OptValue::Str(items.join(",")), scope)?;
            Ok(())
        }
        rvim_editor::OptValue::Num(current) => {
            let delta: i64 = value
                .parse()
                .map_err(|_| VimError::invalid_argument(&format!("number required: {value}")))?;
            let next = match op {
                "+=" => current + delta,
                "-=" => current - delta,
                "^=" => current * delta,
                _ => current,
            };
            ed.set_option(&canon, rvim_editor::OptValue::Num(next), scope)?;
            Ok(())
        }
        rvim_editor::OptValue::Bool(_) => Err(VimError::invalid_argument(&format!(
            "compound assignment not supported for boolean option {canon}"
        ))),
    }
}
/// Control-flow commands implemented by the script runner.
fn is_structural_command(word: &str) -> bool {
    matches!(
        lookup(word).map(|c| c.name),
        Some(
            "if" | "elseif"
                | "else"
                | "endif"
                | "while"
                | "endwhile"
                | "for"
                | "endfor"
                | "try"
                | "catch"
                | "finally"
                | "endtry"
                | "break"
                | "continue"
                | "throw"
                | "return"
                | "finish"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_print_flow() {
        let mut ed = Editor::new();
        run_line(&mut ed, "set number tabstop=4").unwrap();
        assert!(ed.options.bool_or("number", false));
        assert_eq!(ed.options.num_or("tabstop", 0), 4);
        ed.set_line_text(0, "alpha".into()).unwrap();
        run_line(&mut ed, "1p").unwrap();
        assert!(ed.messages.history().iter().any(|m| m == "alpha"));
    }

    #[test]
    fn write_quit_outcome() {
        let path = std::env::temp_dir().join("rvim_ex_wq.txt");
        let _ = std::fs::remove_file(&path);
        let mut ed = Editor::new();
        // Keep the on-disk bytes platform-independent (Windows default 'ff' is
        // dos, so the file would end in CRLF like vim's DFLT_FF).
        run_line(&mut ed, "set fileformat=unix").unwrap();
        ed.set_line_text(0, "data".into()).unwrap();
        let out = run_line(&mut ed, &format!("wq {}", path.display())).unwrap();
        assert!(matches!(out, ExOutcome::Quit { exit_code: 0, .. }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "data\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn substitute_literal() {
        let mut ed = Editor::new();
        ed.set_line_text(0, "cat bat cat".into()).unwrap();
        run_line(&mut ed, "s/cat/dog/g").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "dog bat dog");
        run_line(&mut ed, "1s/dog/X/").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "X bat dog");
    }

    #[test]
    fn delete_and_put_register() {
        let mut ed = Editor::new();
        ed.replace_lines(0, 1, vec!["a".into(), "b".into(), "c".into()])
            .unwrap();
        run_line(&mut ed, "1d").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "b\nc");
        // "$pu" addresses the last line and inserts the register after it.
        run_line(&mut ed, "$pu").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "b\nc\na");
        // Bare ":pu" inserts after the current line.
        let mut ed2 = Editor::new();
        ed2.replace_lines(0, 1, vec!["a".into(), "b".into()])
            .unwrap();
        ed2.registers
            .put('"', rvim_editor::Register::linewise("Z\n"));
        run_line(&mut ed2, "pu").unwrap();
        assert_eq!(ed2.current_buffer().full_text(), "a\nZ\nb");
    }

    fn editor(lines: &[&str]) -> Editor {
        let mut ed = Editor::new();
        let text: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        ed.replace_lines(0, 1, text).unwrap();
        ed
    }

    #[test]
    fn delete_and_yank_accept_a_trailing_count() {
        // ":2d 2" deletes lines 2-3; the count is not a register
        // (ex_docmd.c:2354-2384, EX_COUNT extends line2).
        let mut ed = editor(&["one", "two", "three", "four", "five"]);
        run_line(&mut ed, "2d 2").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "one\nfour\nfive");

        // ":2y 2" yanks two lines into the unnamed register.
        let mut ed = editor(&["one", "two", "three", "four"]);
        run_line(&mut ed, "2y 2").unwrap();
        run_line(&mut ed, "$pu").unwrap();
        assert_eq!(
            ed.current_buffer().full_text(),
            "one\ntwo\nthree\nfour\ntwo\nthree"
        );

        // A register and a count may both be given (":2d a 2").
        let mut ed = editor(&["one", "two", "three", "four"]);
        run_line(&mut ed, "2d a 2").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "one\nfour");
        assert_eq!(
            ed.registers.get('a').map(|r| r.text.clone()),
            Some("two\nthree\n".to_string())
        );

        // :delete has no EX_FLAGS, so a trailing list flag is E488; only the
        // :dl/:dp command-word quirk sets it (ex_cmds.h CMD_delete).
        let mut ed = editor(&["one", "two", "three"]);
        assert_eq!(run_line(&mut ed, "2d 2 l").unwrap_err().code, "E488");
        assert_eq!(ed.current_buffer().full_text(), "one\ntwo\nthree");
    }

    #[test]
    fn put_bang_inserts_above_the_addressed_line() {
        // ex_docmd.c:7986-7998: forceit means BACKWARD, and :0put behaves like
        // :1put!.
        let mut ed = editor(&["one", "two", "three"]);
        run_line(&mut ed, "2y").unwrap();
        run_line(&mut ed, "3pu!").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "one\ntwo\ntwo\nthree");

        let mut ed = editor(&["alpha", "beta", "gamma"]);
        run_line(&mut ed, "1y").unwrap();
        run_line(&mut ed, "0pu").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "alpha\nalpha\nbeta\ngamma");
    }

    #[test]
    fn join_single_address_and_explicit_pair() {
        // ":2j" joins lines 2-3; ":2,2j" is a no-op (ex_docmd.c:8074-8091).
        let mut ed = editor(&["one", "two", "three", "four"]);
        run_line(&mut ed, "2j").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "one\ntwo three\nfour");

        let mut ed = editor(&["one", "two", "three"]);
        run_line(&mut ed, "2,2j").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "one\ntwo\nthree");

        // ":2j 3" joins three lines starting at line 2.
        let mut ed = editor(&["one", "two", "three", "four", "five"]);
        run_line(&mut ed, "2j 3").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "one\ntwo three four\nfive");
    }

    #[test]
    fn backwards_range_reports_e493() {
        // ex_docmd.c:2146-2162: a backwards range in a script / Ex mode is
        // E493 instead of being silently swapped.
        let mut ed = editor(&["one", "two", "three"]);
        ed.exmode = true;
        let err = run_line(&mut ed, "3,1d").unwrap_err();
        assert_eq!(err.code, "E493");
        assert_eq!(ed.current_buffer().full_text(), "one\ntwo\nthree");
    }

    #[test]
    fn substitute_e_flag_count_and_ampersand() {
        // "e" suppresses E486 (ex_cmds.c:3895-3896).
        let mut ed = editor(&["cat", "cat cat"]);
        run_line(&mut ed, "1s/zzz/y/e").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "cat\ncat cat");

        // A trailing count means "N lines", not the Nth match
        // (ex_cmds.c:3928-3941).
        let mut ed = editor(&["cat bat cat", "cat cat", "zzz", "dog"]);
        run_line(&mut ed, "1s/cat/X/2").unwrap();
        assert_eq!(
            ed.current_buffer().full_text(),
            "X bat cat\nX cat\nzzz\ndog"
        );

        // "&" in the replacement is the matched text.
        let mut ed = editor(&["cat"]);
        run_line(&mut ed, "1s/cat/[&]/").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "[cat]");
    }

    #[test]
    fn empty_pattern_uses_the_last_search_pattern() {
        let mut ed = editor(&["cat cat", "cat", "zzz"]);
        run_line(&mut ed, "/cat").unwrap();
        run_line(&mut ed, "1,3s//dog/g").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "dog dog\ndog\nzzz");

        // ":g//d" reuses it too.
        let mut ed = editor(&["x1", "x2", "zzz"]);
        run_line(&mut ed, "/x").unwrap();
        run_line(&mut ed, "g//d").unwrap();
        assert_eq!(ed.current_buffer().full_text(), "zzz");
    }

    #[test]
    fn tilde_uses_the_last_search_pattern_and_last_replacement() {
        // ex_cmds.c:3715-3718: :~ uses RE_LAST (last search), :& uses RE_SUBST.
        let mut ed = editor(&["cat bat cat", "cat cat", "zzz", "dog"]);
        run_line(&mut ed, "1s/cat/dog/").unwrap();
        run_line(&mut ed, "/zzz").unwrap();
        run_line(&mut ed, "1,4~").unwrap();
        assert_eq!(
            ed.current_buffer().full_text(),
            "dog bat cat\ncat cat\ndog\ndog"
        );
    }

    #[test]
    fn user_command_bar_count_and_range_defaults() {
        let mut ed = editor(&["one", "two", "three"]);
        let dir = std::env::temp_dir();
        let state = dir.join(format!("rvim_uc_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&state);
        let path = state.display().to_string().replace('\\', "/");
        // -bar: "BarC | cmd" runs the next command.
        run_line(&mut ed, "command -bar BarC let g:x=1").unwrap();
        run_line(
            &mut ed,
            &format!("BarC | call writefile(['ran'], '{path}')"),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&state).unwrap().trim(), "ran");
        // -count=7 exposes <count> (typed value wins).
        let _ = std::fs::remove_file(&state);
        run_line(
            &mut ed,
            &format!("command -count=7 Cnt call writefile(['<count>'], '{path}')"),
        )
        .unwrap();
        run_line(&mut ed, "Cnt 3").unwrap();
        assert_eq!(std::fs::read_to_string(&state).unwrap().trim(), "3");
        // -range=% defaults to 1,$.
        let _ = std::fs::remove_file(&state);
        run_line(
            &mut ed,
            &format!("command -range=% Rng call writefile(['<line1>,<line2>'], '{path}')"),
        )
        .unwrap();
        run_line(&mut ed, "Rng").unwrap();
        assert_eq!(
            std::fs::read_to_string(&state).unwrap().trim(),
            format!("1,{}", ed.line_count())
        );
        let _ = std::fs::remove_file(&state);
    }

    #[test]
    fn set_modified_reads_the_buffer_flag() {
        let mut ed = editor(&["data"]);
        run_line(&mut ed, "1d").unwrap();
        assert!(ed.current_buffer().is_changed());
        let history = ed.messages.history().len();
        run_line(&mut ed, "set modified?").unwrap();
        assert_eq!(ed.messages.history()[history], "modified");
        run_line(&mut ed, "set nomodified").unwrap();
        assert!(!ed.current_buffer().is_changed());
    }
}
