//! rvim: RustVim executable.
//!
//! Two front-ends share one session:
//!
//! * silent/batch Ex mode (M1) for tests and compatibility diffing:
//!   rvim -es -c "set et" -c "1,3p" file.txt
//! * the interactive TUI (M4), plus a headless --script driver that replays key
//!   notation and prints the final screen so the TUI can be verified without a
//!   terminal:
//!   rvim --script keys.txt --cols 40 --rows 10 file.txt

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_ex::{ExOutcome, version_banner};
use rvim_term::{ChannelKeySource, KeySource, ScriptedKeys};
use rvim_termio::Terminal;

/// One queued start-up command. Vim keeps `+cmd`, `-c cmd` and `-S file` in a
/// single list executed in argument order (main.c:1967-1975, 2440-2458, 3131).
enum QueuedCmd {
    /// `-c {cmd}` or `+{cmd}`.
    Ex(String),
    /// `-S {file}`, queued the way vim does (`so {file}`).
    Source(PathBuf),
}

/// How files beyond the first are opened (-o / -O).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitKind {
    Horizontal,
    Vertical,
}

struct Cli {
    /// Vim's -d: open every file in its own window and diff them.
    diff_mode: bool,
    /// Vim's -p: open every file in its own tabpage.
    tab_files: bool,
    /// Vim's -o / -O.
    split_files: Option<SplitKind>,
    /// Vim's -i {viminfo}: the viminfo file to read and write.
    viminfo: Option<PathBuf>,
    /// Ex mode (`-e`/`-E`), vim's `exmode_active`.
    exmode: bool,
    /// Silent Ex batch mode (`-es`): also reads commands from stdin.
    silent_ex: bool,
    /// `--cmd {cmd}`: executed *before* the vimrc (main.c:416, 2463).
    pre_commands: Vec<String>,
    commands: Vec<QueuedCmd>,
    /// `-s {scriptin}`: read Ex commands from a file.
    scriptin: Option<PathBuf>,
    files: Vec<PathBuf>,
    show_version: bool,
    /// `-u {vimrc}`; `None` means "do not load a vimrc".
    user_vimrc: Option<PathBuf>,
    /// `--clean`: skip vimrc and plugin loading.
    clean: bool,
    /// `--script {file}`: replay key notation headlessly and dump the screen.
    script: Option<PathBuf>,
    /// `--keys {notation}`: replay key notation given inline.
    keys: Option<String>,
    /// Screen geometry for the scripted driver (default 80x24).
    cols: usize,
    rows: usize,
}

fn parse_args(argv: &[String]) -> Result<Cli, String> {
    let mut cli = Cli {
        diff_mode: false,
        tab_files: false,
        split_files: None,
        viminfo: None,
        exmode: false,
        silent_ex: false,
        pre_commands: Vec::new(),
        commands: Vec::new(),
        scriptin: None,
        files: Vec::new(),
        show_version: false,
        user_vimrc: None,
        clean: false,
        script: None,
        keys: None,
        cols: 80,
        rows: 24,
    };
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--version" | "-v" => cli.show_version = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            // "-es" is the classic silent batch invocation; "-e"/"-E" start Ex
            // mode without silencing.
            "-es" | "-Es" => {
                cli.exmode = true;
                cli.silent_ex = true;
            }
            "-e" | "-E" => cli.exmode = true,
            // -d opens every file in its own window and starts diff mode.
            "-d" => cli.diff_mode = true,
            // -p puts every file in its own tabpage.
            "-p" => cli.tab_files = true,
            "-o" => cli.split_files = Some(SplitKind::Horizontal),
            "-O" => cli.split_files = Some(SplitKind::Vertical),
            "-i" => {
                let value = argv.get(i).ok_or("-i requires a file name")?;
                cli.viminfo = Some(PathBuf::from(value));
                i += 1;
            }
            "-u" => {
                i += 1;
                let value = argv.get(i).ok_or("-u requires an argument")?;
                // Vim's "-u NONE" / "-u NORC" disable vimrc loading.
                cli.user_vimrc =
                    if value.eq_ignore_ascii_case("NONE") || value.eq_ignore_ascii_case("NORC") {
                        None
                    } else {
                        Some(PathBuf::from(value))
                    };
            }
            "--clean" => {
                cli.clean = true;
                cli.user_vimrc = None;
            }
            "--cmd" => {
                i += 1;
                let value = argv.get(i).ok_or("--cmd requires an argument")?;
                cli.pre_commands.push(value.clone());
            }
            // M4: headless replay of key notation (verification/smoke driver).
            "--script" => {
                i += 1;
                cli.script = Some(PathBuf::from(
                    argv.get(i).ok_or("--script requires a file")?,
                ));
            }
            "--keys" => {
                i += 1;
                cli.keys = Some(argv.get(i).ok_or("--keys requires a value")?.clone());
            }
            "--cols" => {
                i += 1;
                cli.cols = argv
                    .get(i)
                    .ok_or("--cols requires a number")?
                    .parse()
                    .map_err(|_| "--cols requires a number".to_string())?;
            }
            "--rows" => {
                i += 1;
                cli.rows = argv
                    .get(i)
                    .ok_or("--rows requires a number")?
                    .parse()
                    .map_err(|_| "--rows requires a number".to_string())?;
            }
            "-s" => {
                // In Ex mode "-s" means silent (already implied by -es); outside
                // it, "-s {scriptin}" reads commands from a file (main.c:2296).
                i += 1;
                let value = argv.get(i).ok_or("-s requires a script file")?;
                cli.scriptin = Some(PathBuf::from(value));
            }
            // Accepted, no-op compatibility flags
            "-U" => {
                i += 1; // consume the gvimrc value (no GUI in M1/M2)
            }
            "--noplugin" | "-N" | "-n" | "-Z" | "--not-a-term" | "--nofork" => {}
            "-c" => {
                i += 1;
                cli.commands.push(QueuedCmd::Ex(
                    argv.get(i).ok_or("-c requires an argument")?.clone(),
                ));
            }
            "-S" => {
                i += 1;
                cli.commands.push(QueuedCmd::Source(PathBuf::from(
                    argv.get(i).ok_or("-S requires a file")?,
                )));
            }
            other => {
                if let Some(cmd) = other.strip_prefix('+') {
                    // "+" alone jumps to the last line (main.c:1972), "+N" is
                    // ":N", anything else is an Ex command.
                    if cmd.is_empty() {
                        cli.commands.push(QueuedCmd::Ex("$".to_string()));
                    } else {
                        cli.commands.push(QueuedCmd::Ex(cmd.to_string()));
                    }
                } else if other.starts_with('-') && other.len() > 1 {
                    // Unknown flag: ignore rather than crash, mirroring vim's tolerance
                    eprintln!("rvim: warning: unknown option {other} (ignored)");
                } else {
                    cli.files.push(PathBuf::from(other));
                }
            }
        }
        i += 1;
    }
    Ok(cli)
}

fn print_help() {
    println!(
        "rvim - RustVim (Vim-compatible editor, headless M1)\n\
         Usage: rvim [options] [file ...]\n\n\
         Main options:\n  \
         -es            silent Ex batch mode (read commands from -c or stdin)\n  \
         -e, -E         Ex mode (ranges without a command print in Ex mode)\n  \
         -c <cmd>       execute Ex command (repeatable, runs in argument order)\n  \
         -S <file>      source Ex/VimL script file\n  \
         +<cmd>         same as -c <cmd> (\"+\" alone jumps to the last line)\n  \
         --cmd <cmd>    like -c but executed before the vimrc\n  \
         -s <file>      read Ex commands from <file>\n  \
         --script <f>   replay key notation from <f> and dump the screen\n  \
         --keys <text>  replay key notation given inline\n  \
         --cols/--rows  screen size for --script (default 80x24)\n  \
         -u <vimrc>     source a vimrc (-u NONE: skip)\n  \
         --clean        skip vimrc/plugin loading\n  \
         --version      show version and Vim baseline\n  \
         --help         this help"
    );
}

/// Persist marks/registers/history on exit when -i was given (viminfo.c).
fn save_viminfo(session: &Session, path: &Option<PathBuf>) {
    let Some(path) = path else {
        return;
    };
    if let Err(e) =
        rvim_ex::handlers_session::write_viminfo(&session.editor, &session.viml.state, path)
    {
        eprintln!("rvim: cannot write {}: {e}", path.display());
    }
}

fn main() -> ExitCode {
    // VimL function calls recurse deeply (Vim's default 'maxfuncdepth' is 100)
    // and each interpreted call frame is large in a debug build. Run the whole
    // editor on a thread with a roomy stack so recursion hits the E132 guard
    // instead of overflowing the native stack (userfunc.c:2536-2544).
    rvim_ex::script::run_with_deep_stack(run)
}

fn run() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_args(&argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rvim: {e}");
            return ExitCode::from(2);
        }
    };
    if cli.show_version {
        println!("{}", version_banner());
        return ExitCode::SUCCESS;
    }

    let mut session = Session::new();
    session.editor.exmode = cli.exmode || cli.silent_ex;

    // Positional files go on the argument list; Vim edits the first one and
    // leaves the others reachable with :next/:args (main.c create_windows()).
    if let Some(first) = cli.files.first() {
        session.editor.arglist = cli.files.clone();
        session.editor.arg_index = 0;
        if let Err(e) = session.open_file(first) {
            eprintln!("rvim: {e}");
            session.count_error();
        }
    }

    // -p / -o / -O / -d: the remaining files get their own tabpage, split
    // window or diff window (main.c:2500-2560, diff.c ex_diffsplit).
    if cli.files.len() > 1 {
        for path in cli.files.iter().skip(1) {
            let escaped = path.display().to_string().replace(' ', "\\ ");
            // vimdiff (-d) lays the files out side by side like :vertical split.
            let command = if cli.tab_files {
                format!("tabedit {escaped}")
            } else if cli.split_files == Some(SplitKind::Vertical) || cli.diff_mode {
                format!("vsplit {escaped}")
            } else {
                format!("split {escaped}")
            };
            if let Err(e) = session.ex(&command) {
                eprintln!("rvim: {e}");
                session.count_error();
            }
        }
    }
    if cli.diff_mode {
        // Every window of the first tabpage enters diff mode (vimdiff).
        if let Err(e) = session.ex("windo diffthis") {
            eprintln!("rvim: {e}");
            session.count_error();
        }
    }

    // -i {viminfo}: restore marks, registers and history (viminfo.c).
    if let Some(path) = &cli.viminfo {
        let path = path.clone();
        let _ = rvim_ex::handlers_session::read_viminfo(
            &mut session.editor,
            &mut session.viml.state,
            &path,
        );
    }

    // MS-Windows: a single fully-qualified file argument means we were probably
    // launched from Explorer, so change to the file's directory
    // (reference vim main.c:275-288, `vim_chdirfile()`).
    #[cfg(windows)]
    if cli.files.len() == 1 && cli.files[0].is_absolute() {
        if let Some(dir) = cli.files[0].parent() {
            if !dir.as_os_str().is_empty() && std::env::set_current_dir(dir).is_ok() {
                session.editor.cwd = dir.to_path_buf();
            }
        }
    }

    // "Ex starts at last line of the file" (main.c:719-721).
    if cli.exmode || cli.silent_ex {
        session.editor.start_ex_mode_cursor();
    }

    // --cmd arguments run before any vimrc (main.c:416).
    for cmd in &cli.pre_commands {
        match session.ex(cmd) {
            Ok(ExOutcome::Continue) => {}
            Ok(_) => return ExitCode::from(final_exit_code(&session.report)),
            Err(_) => continue,
        }
    }

    // -u {vimrc}: source the user configuration before the +cmd/-c queue.
    if let Some(vimrc) = &cli.user_vimrc {
        if let Err(e) = session.source(vimrc) {
            eprintln!("rvim: error while sourcing {}: {e}", vimrc.display());
            session.sync_errors();
        }
    }

    // "+cmd", "-c cmd" and "-S file" arguments, in command-line order.
    let mut quit = false;
    for queued in &cli.commands {
        let result = match queued {
            QueuedCmd::Ex(cmd) => session.ex(cmd),
            QueuedCmd::Source(path) => match session.source(path) {
                Ok(outcome) => Ok(outcome),
                Err(e) => {
                    eprintln!("rvim: cannot source {}: {e}", path.display());
                    session.sync_errors();
                    continue;
                }
            },
        };
        match result {
            Ok(ExOutcome::Continue) => {}
            Ok(_) => {
                quit = true;
                break;
            }
            Err(_) => continue,
        }
    }

    // M4 key replay: feed key notation through the real key loop and print the
    // final screen. This is how the TUI is verified without a terminal.
    let script_notation = match (&cli.keys, &cli.script) {
        (Some(inline), _) => Some(inline.clone()),
        (None, Some(path)) => match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) => {
                eprintln!("rvim: cannot read script {}: {e}", path.display());
                session.count_error();
                None
            }
        },
        (None, None) => None,
    };
    if let Some(notation) = script_notation {
        let mut tui = Tui::new(session);
        let mut keys = ScriptedKeys::from_notation(&notation);
        let mut sink: Vec<u8> = Vec::new();
        let _ = tui.run(&mut keys, &mut sink, cli.cols, cli.rows);
        let screen = tui.screen_text(cli.cols, cli.rows);
        println!("{screen}");
        tui.session.sync_errors();
        let code = if tui.exit_code != 0 {
            tui.exit_code as u8
        } else {
            final_exit_code(&tui.session.report)
        };
        return ExitCode::from(code);
    }

    // Interactive TUI (M4) whenever a console is attached and Ex batch mode was
    // not requested.
    if !quit && !cli.silent_ex && !cli.exmode {
        if Terminal::is_available() {
            // ChannelKeySource owns the raw-mode terminal and polls the console
            // size so a window resize re-renders the TUI.
            let mut source = match ChannelKeySource::open() {
                Ok(source) => source,
                Err(e) => {
                    eprintln!("rvim: cannot open terminal: {e}");
                    return ExitCode::from(1);
                }
            };
            let size = source
                .size()
                .map(|(cols, rows)| rvim_termio::Size::new(cols, rows))
                .unwrap_or(rvim_termio::Size::new(cli.cols, cli.rows));
            let mut tui = Tui::new(session);
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let code = tui
                .run(&mut source, &mut out, size.cols, size.rows)
                .unwrap_or(1);
            let _ = out.flush();
            save_viminfo(&tui.session, &cli.viminfo);
            tui.session.sync_errors();
            let exit = if code != 0 {
                code as u8
            } else {
                final_exit_code(&tui.session.report)
            };
            return ExitCode::from(exit);
        }
        eprintln!("rvim: no terminal available; use -es for batch mode");
        return ExitCode::from(1);
    }
    // In silent Ex mode with no -c, consume commands from stdin (scripted use).
    if cli.silent_ex && !quit && !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_ok() && !buf.is_empty() {
            let lines: Vec<String> = buf.lines().map(str::to_string).collect();
            session.run_script(&lines);
        }
    }

    // "-s {scriptin}": read Ex commands from a file (main.c:2296).
    if let Some(script) = &cli.scriptin {
        match std::fs::read_to_string(script) {
            Ok(text) => {
                let lines: Vec<String> = text.lines().map(str::to_string).collect();
                session.run_script(&lines);
            }
            Err(e) => {
                eprintln!("rvim: cannot read script {}: {e}", script.display());
                session.count_error();
            }
        }
    }

    // Batch output: emit message history produced by :p/:echo/:ls ...
    for line in session.message_history() {
        if !line.is_empty() {
            println!("{line}");
        }
    }

    session.sync_errors();
    save_viminfo(&session, &cli.viminfo);
    ExitCode::from(final_exit_code(&session.report))
}

/// Compute the process exit code from a finished session report.
///
/// Mirrors vim's `getout()` (reference vim main.c:1563-1575): an explicit
/// `:cq N`/`:q N` code is the base value, then `ex_exitval` — 1 as soon as any
/// error was given in Ex mode — is *added* (`exitval += ex_exitval`). With no
/// explicit code an error makes a batch run exit 1, which POSIX requires for
/// silent Ex mode.
fn final_exit_code(report: &rvim_app::SessionReport) -> u8 {
    let mut code = report.exit_code.max(0) as u8;
    if report.error_count > 0 {
        code = code.saturating_add(1).max(1);
    }
    code
}

#[cfg(test)]
mod tests {
    use super::{QueuedCmd, final_exit_code, parse_args};
    use rvim_app::SessionReport;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plus_arguments_map_to_last_line_or_ex_command() {
        // main.c:1972: a bare "+" is "$", "+N" is the Ex command "N".
        let cli = parse_args(&argv(&["+", "file.txt"])).unwrap();
        match &cli.commands[0] {
            QueuedCmd::Ex(cmd) => assert_eq!(cmd, "$"),
            QueuedCmd::Source(_) => panic!("expected an Ex command"),
        }
        let cli = parse_args(&argv(&["+5"])).unwrap();
        match &cli.commands[0] {
            QueuedCmd::Ex(cmd) => assert_eq!(cmd, "5"),
            QueuedCmd::Source(_) => panic!("expected an Ex command"),
        }
    }

    #[test]
    fn dash_cmd_is_separate_and_command_order_is_preserved() {
        // "--cmd" runs before the vimrc; -c/+cmd/-S share one ordered queue.
        let cli = parse_args(&argv(&[
            "--cmd", "set et", "-c", "echo 1", "-S", "s.vim", "+2",
        ]))
        .unwrap();
        assert_eq!(cli.pre_commands, vec!["set et".to_string()]);
        let shape: Vec<&str> = cli
            .commands
            .iter()
            .map(|q| match q {
                QueuedCmd::Ex(_) => "ex",
                QueuedCmd::Source(_) => "source",
            })
            .collect();
        assert_eq!(shape, vec!["ex", "source", "ex"]);
    }

    #[test]
    fn e_and_es_select_ex_mode() {
        let cli = parse_args(&argv(&["-e"])).unwrap();
        assert!(cli.exmode && !cli.silent_ex);
        let cli = parse_args(&argv(&["-es"])).unwrap();
        assert!(cli.exmode && cli.silent_ex);
        let cli = parse_args(&argv(&["-s", "cmds.vim"])).unwrap();
        assert_eq!(
            cli.scriptin.as_deref(),
            Some(std::path::Path::new("cmds.vim"))
        );
    }

    #[test]
    fn clean_session_exits_zero() {
        assert_eq!(final_exit_code(&SessionReport::default()), 0);
    }

    #[test]
    fn ex_error_forces_nonzero_even_after_quit() {
        // smoke case 5: unknown command then :q must not exit 0.
        let r = SessionReport {
            quit: true,
            exit_code: 0,
            error_count: 1,
        };
        assert_eq!(final_exit_code(&r), 1);
    }

    #[test]
    fn explicit_cquit_code_is_increased_by_errors() {
        // getout(): "exitval += ex_exitval" (main.c:1574-1575). ex_exitval is
        // 1 once any error was given, so ":cq 3" after an error exits 4.
        let r = SessionReport {
            quit: true,
            exit_code: 1,
            error_count: 4,
        };
        assert_eq!(final_exit_code(&r), 2);
        let r = SessionReport {
            quit: true,
            exit_code: 3,
            error_count: 1,
        };
        assert_eq!(final_exit_code(&r), 4);
    }

    #[test]
    fn error_without_quit_still_nonzero() {
        let r = SessionReport {
            quit: false,
            exit_code: 0,
            error_count: 2,
        };
        assert_eq!(final_exit_code(&r), 1);
    }
}
