//! Host builtins for spell checking (spellbadword(), spellsuggest(),
//! soundfold()).
//!
//! The three functions use the word lists cached in
//! rvim_editor::spell::SpellState. spellbadword() and spellsuggest() work
//! whether or not 'spell' is set, like Vim.

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_editor::spell::{self, WordStatus};
use rvim_viml::Env;
use rvim_viml::value::Value;

use crate::script::ScriptState;

/// Names implemented in this module.
pub const FUNCS: &[&str] = &["soundfold", "spellbadword", "spellsuggest"];

/// Dispatch a call; None when the name is not handled here.
pub fn call(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    _env: &mut Env,
    _state: &mut ScriptState,
) -> Option<rvim_core::VimResult<Value>> {
    if !FUNCS.contains(&name) {
        return None;
    }
    Some(match name {
        "soundfold" => Ok(Value::string(spell::soundfold(&string_arg(args, 0)))),
        "spellbadword" => spellbadword(args, ed),
        "spellsuggest" => spellsuggest(args, ed),
        _ => Err(VimError::internal(format!(
            "unhandled spell builtin {name}"
        ))),
    })
}

fn string_arg(args: &[Value], index: usize) -> String {
    args.get(index)
        .map(Value::to_string_value)
        .unwrap_or_default()
}

fn number_arg(args: &[Value], index: usize) -> Option<i64> {
    args.get(index).map(Value::to_number)
}

/// spellbadword([{str}]).
fn spellbadword(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let found = match args.first() {
        None | Some(Value::Null) => ed.spell.bad_word_at_cursor(ed),
        Some(value) => ed.spell.first_bad_in(ed, &value.to_string_value()),
    };
    let (word, status) = found.unwrap_or((String::new(), WordStatus::Good));
    Ok(Value::list(vec![
        Value::string(word),
        Value::string(status.as_str()),
    ]))
}

/// spellsuggest({word} [, {max} [, {capital}]).
fn spellsuggest(args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    let Some(word) = args.first() else {
        return Err(VimError::new(
            "E119",
            "Not enough arguments for function: spellsuggest".to_string(),
        ));
    };
    let word = word.to_string_value();
    let max = match number_arg(args, 1) {
        Some(value) => value.clamp(0, 10_000) as usize,
        None => 25,
    };
    let capital_only = number_arg(args, 2).is_some_and(|value| value != 0);
    let suggestions = ed.spell.suggest(ed, &word, max, capital_only);
    Ok(Value::list(
        suggestions.into_iter().map(Value::string).collect(),
    ))
}
