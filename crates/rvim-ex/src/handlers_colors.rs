//! Runtime color schemes, matching highlight.c load_colors().
use crate::{ExOutcome, autodispatch, handlers_script, script::ScriptState};
use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_viml::Env;

pub fn cmd_colorscheme(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    args: &str,
) -> VimResult<ExOutcome> {
    let name = args.trim();
    if name.is_empty() {
        let current = env
            .get_var("g:colors_name")
            .map(|value| value.to_string_value())
            .unwrap_or_else(|| "default".into());
        ed.messages.info(current);
        return Ok(ExOutcome::Continue);
    }
    if name.chars().any(char::is_whitespace) {
        return Err(VimError::new("E488", "Trailing characters"));
    }
    // Names denote a single colors/*.vim file, never an arbitrary script path.
    if name.contains(['/', '\\', '*', '?', '[', ']']) || name == "." || name == ".." {
        return Err(VimError::new(
            "E185",
            format!("Cannot find color scheme '{name}'"),
        ));
    }
    if state.colors_loading {
        return Ok(ExOutcome::Continue);
    }
    state.colors_loading = true;
    autodispatch::fire(ed, env, state, "ColorSchemePre", name);
    let files = handlers_script::color_files(ed, &format!("colors/{name}.vim"));
    let result = if let Some(path) = files.first() {
        handlers_script::source_file(ed, env, state, path)
    } else {
        Err(VimError::new(
            "E185",
            format!("Cannot find color scheme '{name}'"),
        ))
    };
    autodispatch::fire(ed, env, state, "ColorScheme", name);
    state.colors_loading = false;
    result
}

pub fn complete_schemes(ed: &Editor, prefix: &str, max: usize) -> Vec<String> {
    let mut names = handlers_script::color_files(ed, "colors/*.vim")
        .into_iter()
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_string))
        .filter(|name| name.starts_with(prefix))
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names.truncate(max);
    names
}
