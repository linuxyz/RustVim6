//! Runtime schemes, command completion and visible palette refresh.
use rvim_app::{Session, tui::Tui};
use rvim_editor::OptValue;
use rvim_syntax::highlight::HlColor;
use rvim_term::{Color, ScriptedKeys};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rvim_colors_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("runtime with spaces/colors")).unwrap();
        Self(root)
    }
    fn write(&self, name: &str, text: &str) {
        std::fs::write(
            self.0
                .join("runtime with spaces/colors")
                .join(format!("{name}.vim")),
            text,
        )
        .unwrap();
    }
    fn session(&self) -> Session {
        let mut s = Session::new();
        s.editor
            .options
            .set(
                "runtimepath",
                OptValue::Str(self.0.join("runtime with spaces").display().to_string()),
            )
            .unwrap();
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let (Ok(path), Ok(base)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize()) {
            if path != base && path.starts_with(base) {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
}

#[test]
fn loads_script_variables_events_and_reports_the_current_name() {
    let f = Fixture::new();
    f.write(
        "sample",
        "hi clear\nlet g:colors_name = 'sample'\nhi Normal guifg=#abcdef guibg=#112233\n",
    );
    let mut s = f.session();
    s.ex("colorscheme").unwrap();
    assert_eq!(s.editor.messages.history().last().unwrap(), "default");
    s.ex("let g:events = []").unwrap();
    s.ex("autocmd ColorSchemePre * call add(g:events, 'pre:' . expand('<amatch>'))")
        .unwrap();
    s.ex("autocmd ColorScheme * call add(g:events, 'post:' . expand('<amatch>'))")
        .unwrap();
    s.ex("colo sample").unwrap();
    assert_eq!(
        s.viml
            .env
            .get_var("g:colors_name")
            .unwrap()
            .to_string_value(),
        "sample"
    );
    assert_eq!(
        s.viml.env.get_var("g:events").unwrap().to_string_value(),
        "['pre:sample', 'post:sample']"
    );
    assert_eq!(
        s.editor.syntax_highlight("Normal").unwrap().bg,
        Some(HlColor::Rgb(17, 34, 51))
    );
    s.ex("colorscheme").unwrap();
    assert_eq!(s.editor.messages.history().last().unwrap(), "sample");
}

#[test]
fn alias_completion_and_chained_commands_work() {
    let f = Fixture::new();
    f.write("ocean", "let g:colors_name = 'ocean'\n");
    f.write("orchid", "let g:colors_name = 'orchid'\n");
    let mut tui = Tui::new(f.session());
    let completion =
        rvim_cmdline::complete_colorscheme("colorschema o", &tui.session.editor, 20).unwrap();
    assert_eq!(
        completion,
        (12, vec!["ocean".to_string(), "orchid".to_string()])
    );
    tui.run(
        &mut ScriptedKeys::from_notation(":colorschema oce<Tab><CR>"),
        &mut Vec::new(),
        80,
        24,
    )
    .unwrap();
    assert_eq!(
        tui.session
            .viml
            .env
            .get_var("g:colors_name")
            .unwrap()
            .to_string_value(),
        "ocean"
    );
    tui.session
        .ex("colorschema orchid | let g:after = 1")
        .unwrap();
    assert_eq!(
        tui.session.viml.env.get_var("g:after").unwrap().to_number(),
        1
    );
}

#[test]
fn missing_scheme_preserves_palette_and_script_errors_propagate() {
    let f = Fixture::new();
    f.write("broken", "throw 'broken theme'\n");
    let mut s = f.session();
    let before = s.editor.syntax_highlight("Normal");
    assert_eq!(s.ex("colorscheme absent").unwrap_err().code, "E185");
    assert_eq!(s.editor.syntax_highlight("Normal"), before);
    assert_eq!(s.ex("colorscheme broken").unwrap_err().code, "E605");
    f.write("valid", "let g:colors_name = 'valid'\n");
    s.ex("colorscheme valid").unwrap();
    assert_eq!(
        s.viml
            .env
            .get_var("g:colors_name")
            .unwrap()
            .to_string_value(),
        "valid"
    );
}

#[test]
fn recursive_scheme_autocommand_does_not_recurse_forever() {
    let f = Fixture::new();
    f.write("sample", "let g:colors_name = 'sample'\n");
    let mut s = f.session();
    s.ex("autocmd ColorScheme * colorscheme sample").unwrap();
    s.ex("colorscheme sample").unwrap();
    assert_eq!(s.editor.error_count, 0);
}

#[test]
fn runtime_order_and_optional_package_discovery_match_completion() {
    let f = Fixture::new();
    f.write("sample", "let g:source = 'first'\n");
    let second = f.0.join("second");
    std::fs::create_dir_all(second.join("colors")).unwrap();
    std::fs::write(
        second.join("colors/sample.vim"),
        "let g:source = 'second'\n",
    )
    .unwrap();
    let opt = f.0.join("packages/pack/vendor/opt/theme/colors");
    std::fs::create_dir_all(&opt).unwrap();
    std::fs::write(opt.join("optional.vim"), "let g:source = 'optional'\n").unwrap();
    let mut s = f.session();
    let paths = format!(
        "{},{}",
        s.editor.options.str_or("runtimepath", ""),
        second.display()
    );
    s.editor
        .options
        .set("runtimepath", OptValue::Str(paths))
        .unwrap();
    s.editor
        .options
        .set(
            "packpath",
            OptValue::Str(f.0.join("packages").display().to_string()),
        )
        .unwrap();
    s.ex("colorscheme sample").unwrap();
    assert_eq!(
        s.viml.env.get_var("g:source").unwrap().to_string_value(),
        "first"
    );
    assert_eq!(
        rvim_ex::handlers_colors::complete_schemes(&s.editor, "", 10),
        ["optional", "sample"]
    );
    s.ex("colorscheme optional").unwrap();
    assert_eq!(
        s.viml.env.get_var("g:source").unwrap().to_string_value(),
        "optional"
    );
}

#[test]
fn gui_and_terminal_palettes_and_explicit_none_are_independent() {
    let mut s = Session::new();
    s.ex("set notermguicolors").unwrap();
    s.ex("hi Sample guifg=#123456 guibg=#234567 gui=bold,italic ctermfg=Red cterm=underline")
        .unwrap();
    s.ex("hi Sample guibg=NONE gui=NONE").unwrap();
    let gui = s
        .editor
        .syntax
        .highlights
        .resolve_for("Sample", true)
        .unwrap();
    assert_eq!(gui.fg, Some(HlColor::Rgb(18, 52, 86)));
    assert_eq!(gui.bg, None);
    assert!(!gui.bold && !gui.italic && !gui.underline);
    let term = s.editor.syntax_highlight("Sample").unwrap();
    assert_eq!(term.fg, Some(HlColor::Indexed(9)));
    assert!(term.underline);
    s.ex("set termguicolors").unwrap();
    assert_eq!(s.editor.syntax_highlight("Sample"), Some(gui));
    s.ex("set notermguicolors").unwrap();
    s.editor.gui_running = true;
    assert_eq!(s.editor.syntax_highlight("Sample"), Some(gui));
}

#[test]
fn forced_links_override_existing_groups_and_default_does_not() {
    let mut s = Session::new();
    s.ex("hi Target guifg=#123456").unwrap();
    s.ex("hi! link Comment Target").unwrap();
    assert_eq!(
        s.editor.syntax_highlight("Comment").unwrap().fg,
        Some(HlColor::Rgb(18, 52, 86))
    );
    s.ex("hi def Comment guifg=#ffffff").unwrap();
    s.ex("hi def link Comment Normal").unwrap();
    assert_eq!(
        s.editor.syntax_highlight("Comment"),
        s.editor.syntax_highlight("Target")
    );
}

#[test]
fn switching_schemes_updates_text_background_and_status_without_syntax() {
    let f = Fixture::new();
    for (name, color) in [("dark", "#112233"), ("light", "#ddeeff")] {
        f.write(name, &format!("hi clear\nlet g:colors_name = '{name}'\nhi Normal guifg=#abcdef guibg={color}\nhi StatusLine guifg=#010203 guibg={color}\n"));
    }
    let mut tui = Tui::new(f.session());
    for (name, color) in [
        ("dark", Color::Rgb(17, 34, 51)),
        ("light", Color::Rgb(221, 238, 255)),
    ] {
        tui.session.ex(&format!("colorscheme {name}")).unwrap();
        let grid = tui.grid(40, 12);
        assert_eq!(grid.cell(0, 39).unwrap().style.bg, color);
        assert_eq!(grid.cell(5, 0).unwrap().style.bg, color);
        assert_eq!(grid.cell(11, 39).unwrap().style.bg, color);
        assert_eq!(tui.theme.text.bg, color);
        assert_eq!(tui.theme.status.bg, color);
    }
}

#[test]
fn bundled_runtime_schemes_load_without_errors() {
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/vim/runtime");
    if !runtime.join("colors/default.vim").is_file() {
        return;
    }
    for name in [
        "default",
        "desert",
        "elflord",
        "evening",
        "industry",
        "murphy",
        "pablo",
        "peachpuff",
        "ron",
        "slate",
        "torte",
        "zellner",
        "blue",
        "darkblue",
        "delek",
        "koehler",
        "morning",
        "shine",
    ] {
        for gui in [false, true] {
            let mut s = Session::new();
            s.editor.gui_running = gui;
            s.ex("set notermguicolors").unwrap();
            s.editor
                .options
                .set("runtimepath", OptValue::Str(runtime.display().to_string()))
                .unwrap();
            s.ex(&format!("colorscheme {name}"))
                .unwrap_or_else(|err| panic!("{name}: {err}"));
            assert_eq!(s.editor.error_count, 0, "{name}");
            assert_eq!(
                s.viml
                    .env
                    .get_var("g:colors_name")
                    .unwrap()
                    .to_string_value(),
                name
            );
            if name == "desert" {
                assert_eq!(
                    s.editor.syntax_highlight("Normal").unwrap().bg,
                    Some(if gui {
                        HlColor::Rgb(51, 51, 51)
                    } else {
                        HlColor::Indexed(236)
                    })
                );
                assert!(!s.editor.syntax_highlight("Comment").unwrap().italic);
            }
        }
    }
}
