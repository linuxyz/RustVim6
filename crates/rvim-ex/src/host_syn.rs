//! Host builtins for syntax highlighting (synID(), synIDattr(), synstack(),
//! synIDtrans(), synconcealed()).  Reference: syntax.c, highlight.c.

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_syntax::highlight::HlColor;
use rvim_viml::Env;
use rvim_viml::value::Value;

use crate::script::ScriptState;

/// Names implemented in this module.
pub const FUNCS: &[&str] = &[
    "synID",
    "synIDattr",
    "synIDtrans",
    "synconcealed",
    "synstack",
];

/// Dispatch a call; None when the name is not handled here.
pub fn call(
    name: &str,
    args: &[Value],
    ed: &mut Editor,
    _env: &mut Env,
    _state: &mut ScriptState,
) -> Option<VimResult<Value>> {
    if !FUNCS.contains(&name) {
        return None;
    }
    Some(dispatch(name, args, ed))
}

fn err(code: &'static str, message: impl Into<String>) -> VimError {
    VimError::new(code, message)
}

fn num(args: &[Value], index: usize) -> i64 {
    args.get(index).map(Value::to_number).unwrap_or(0)
}

fn text(args: &[Value], index: usize) -> String {
    args.get(index)
        .map(Value::to_string_value)
        .unwrap_or_default()
}

fn dispatch(name: &str, args: &[Value], ed: &mut Editor) -> VimResult<Value> {
    match name {
        "synID" => {
            let lnum = num(args, 0);
            let col = num(args, 1);
            let trans = num(args, 2) != 0;
            let bufnr = ed.current_bufnr();
            let stack = ed.syntax_stack(bufnr, lnum, col);
            let id = if trans {
                stack
                    .iter()
                    .rev()
                    .copied()
                    .find(|id| !ed.syntax_options_of(*id).transparent)
                    .unwrap_or(0)
            } else {
                stack.last().copied().unwrap_or(0)
            };
            Ok(Value::Number(id))
        }
        "synIDattr" => {
            let id = num(args, 0);
            let what = text(args, 1).to_ascii_lowercase();
            let mode = text(args, 2).to_ascii_lowercase();
            if id == 0 {
                return Ok(Value::String(String::new()));
            }
            let group = ed.syntax_group_of(id).map(str::to_string);
            let attr = group.as_deref().and_then(|g| ed.syntax_highlight(g));
            let out = match what.as_str() {
                "name" => group.unwrap_or_default(),
                "fg" | "bg" => {
                    let color = if what == "fg" {
                        attr.and_then(|a| a.fg)
                    } else {
                        attr.and_then(|a| a.bg)
                    };
                    match (color, mode.as_str()) {
                        (Some(HlColor::Rgb(r, g, b)), "" | "gui") => {
                            format!("#{r:02x}{g:02x}{b:02x}")
                        }
                        (Some(HlColor::Indexed(index)), "cterm") => index.to_string(),
                        (Some(HlColor::Indexed(index)), "") => index.to_string(),
                        (Some(HlColor::Rgb(..)), "cterm") => String::new(),
                        _ => String::new(),
                    }
                }
                "bold" => flag(attr.is_some_and(|a| a.bold)),
                "italic" => flag(attr.is_some_and(|a| a.italic)),
                "underline" => flag(attr.is_some_and(|a| a.underline)),
                "reverse" | "inverse" => flag(attr.is_some_and(|a| a.reverse)),
                "trans" => ed.syntax_trans(id).to_string(),
                // Attributes RustVim does not model.
                "undercurl" | "strikethrough" | "nocombine" | "sp" | "font" => String::new(),
                _ => String::new(),
            };
            Ok(Value::String(out))
        }
        "synIDtrans" => Ok(Value::Number(ed.syntax_trans(num(args, 0)))),
        "synstack" => {
            let lnum = num(args, 0);
            let col = num(args, 1);
            let bufnr = ed.current_bufnr();
            let stack = ed.syntax_stack(bufnr, lnum, col);
            if lnum < 1 || col < 1 {
                return Err(err("E16", "Invalid range"));
            }
            Ok(Value::list(stack.into_iter().map(Value::Number).collect()))
        }
        "synconcealed" => Ok(Value::list(vec![
            Value::Number(0),
            Value::String(String::new()),
            Value::Number(0),
        ])),
        other => Err(err("E117", format!("Unknown function: {other}"))),
    }
}

fn flag(on: bool) -> String {
    if on { "1".to_string() } else { String::new() }
}
