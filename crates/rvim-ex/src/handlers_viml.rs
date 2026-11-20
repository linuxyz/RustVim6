//! VimL-backed Ex commands: `:let`, `:echo`, `:execute`, `:call`, `:function`,
//! `:unlet`, `:delfunction` and the filetype/syntax state commands.

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_viml::value::Value;
use rvim_viml::{Env, eval as vl_eval, expr::parse_expression};

use crate::exec::{ExOutcome, run_line_with};
use crate::parser::ParsedEx;
use crate::script::{
    ScriptState, UserFunc, assign_var, eval_expr, eval_expr_prefix, resolve_function_name,
};

fn echo_text(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    args: &str,
) -> VimResult<String> {
    // `:echo a b` prints several expressions separated by a space. Try the whole
    // argument text first (the common case), then fall back to whitespace-split
    // expressions when there are several.
    match eval_expr(ed, env, state, args) {
        Ok(value) => return Ok(value.to_string_value()),
        Err(e) if e.code == "E488" => {}
        Err(e) => return Err(e),
    }
    let mut parts = Vec::new();
    for piece in args.split_whitespace() {
        let value = eval_expr(ed, env, state, piece)?;
        parts.push(value.to_string_value());
    }
    Ok(parts.join(" "))
}

/// `:echo` / `:echomsg` / `:echon` / `:echoerr` with expression arguments.
pub fn cmd_echo(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    if p.args.trim().is_empty() {
        return Ok(ExOutcome::Continue);
    }
    let text = echo_text(ed, env, state, &p.args)?;
    match name {
        "echomsg" => ed.messages.info(text),
        "echoerr" => ed.emsg(text),
        // echo/echon print to the "screen"; the batch front-end renders the
        // message log, so they go there as well.
        _ => ed.messages.info(text),
    }
    Ok(ExOutcome::Continue)
}

/// Locate the assignment operator at top level (outside brackets/strings).
fn find_assign_op(input: &str) -> Option<(usize, &'static str)> {
    let bytes: Vec<char> = input.chars().collect();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' {
                i += 1;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
            '=' if depth == 0 => {
                // Multi-character operators ending at this '='.
                let prev = input.chars().take(i).last();
                let op_len = match prev {
                    Some('+') | Some('-') | Some('*') | Some('/') | Some('%') | Some('.') => 2,
                    _ => 1,
                };
                let start = input
                    .char_indices()
                    .nth(i + 1 - op_len)
                    .map(|(b, _)| b)
                    .unwrap_or(0);
                let op = match op_len {
                    1 => "=",
                    2 => match prev.unwrap() {
                        '+' => "+=",
                        '-' => "-=",
                        '*' => "*=",
                        '/' => "/=",
                        '%' => "%=",
                        _ => ".=",
                    },
                    _ => "=",
                };
                return Some((start, op));
            }
            '=' if depth > 0 => {}
            _ => {}
        }
        i += 1;
    }
    None
}

/// Apply `:let` assignment, including compound operators and container targets.
pub fn cmd_let(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let args = p.args.trim();
    if args.is_empty() {
        // `:let` with no arguments lists variables (abbreviated form).
        let mut names: Vec<String> = env.globals.keys().cloned().collect();
        names.sort();
        for name in names {
            let value = env
                .get_var(&name)
                .map(|v| v.to_string_value())
                .unwrap_or_default();
            ed.messages.info(format!("{name:<20} {value}"));
        }
        return Ok(ExOutcome::Continue);
    }
    match find_assign_op(args) {
        None => {
            // `:let name` displays one variable's value.
            let name = args.split_whitespace().next().unwrap_or("");
            match env.get_var(name) {
                Some(value) => {
                    ed.messages
                        .info(format!("{name:<20} {}", value.to_string_value()));
                    Ok(ExOutcome::Continue)
                }
                None => Err(VimError::new("E121", format!("Undefined variable: {name}"))),
            }
        }
        Some((op_at, op)) => {
            let target_src = args[..op_at].trim().to_string();
            let expr_src = args[op_at + op.len()..].trim().to_string();
            if target_src.is_empty() {
                return Err(VimError::new("E461", "Illegal variable name".to_string()));
            }
            let rhs = eval_expr(ed, env, state, &expr_src)?;
            let new_value = if op == "=" {
                rhs
            } else {
                let current = eval_expr(ed, env, state, &target_src)?;
                compound(op, &current, &rhs)?
            };
            assign_to_target(ed, env, state, &target_src, new_value)?;
            Ok(ExOutcome::Continue)
        }
    }
}

fn compound(op: &str, current: &Value, rhs: &Value) -> VimResult<Value> {
    Ok(match op {
        "+=" => match (current, rhs) {
            (Value::List(a), Value::List(b)) => {
                let mut out = a.borrow().clone();
                out.extend(b.borrow().iter().cloned());
                Value::list(out)
            }
            (Value::Float(_), _) | (_, Value::Float(_)) => {
                Value::Float(current.to_float()? + rhs.to_float()?)
            }
            _ => Value::Number(current.as_number()? + rhs.as_number()?),
        },
        "-=" => Value::Number(current.as_number()? - rhs.as_number()?),
        "*=" => Value::Number(current.as_number()? * rhs.as_number()?),
        "/=" => {
            let d = rhs.as_number()?;
            if d == 0 {
                return Err(VimError::new(
                    "E805",
                    "Attempt to divide by zero".to_string(),
                ));
            }
            Value::Number(current.as_number()? / d)
        }
        "%=" => {
            let d = rhs.as_number()?;
            if d == 0 {
                return Err(VimError::new(
                    "E804",
                    "Attempt to divide by zero".to_string(),
                ));
            }
            Value::Number(current.as_number()? % d)
        }
        ".=" => Value::String(format!(
            "{}{}",
            current.to_string_value(),
            rhs.to_string_value()
        )),
        _ => rhs.clone(),
    })
}

/// Assign to a variable, option, register, or container element/slice.
pub fn assign_to_target(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    target_src: &str,
    value: Value,
) -> VimResult<()> {
    let target = target_src.trim();
    // Destructuring: let [a, b] = list
    if target.starts_with('[') && target.ends_with(']') {
        let inner = &target[1..target.len() - 1];
        let items = match &value {
            Value::List(l) => l.borrow().clone(),
            other => {
                return Err(VimError::new(
                    "E714",
                    format!("List required for destructuring, got {}", other.type_name()),
                ));
            }
        };
        for (i, part) in inner.split(',').map(str::trim).enumerate() {
            let item = items.get(i).cloned().unwrap_or(Value::Null);
            assign_to_target(ed, env, state, part, item)?;
        }
        return Ok(());
    }
    if target.starts_with('&') || target.starts_with('@') || !target.contains(['[', '.']) {
        return assign_var(ed, env, state, target, value);
    }
    let expr = parse_expression(target)?;
    match expr {
        rvim_viml::Expr::Index(base, index) => {
            let container = eval_expr(ed, env, state, &expr_to_src(&base))?;
            let idx = eval_expr(ed, env, state, &expr_to_src(&index))?;
            let sync_target = container.clone();
            match container {
                Value::List(list) => {
                    let n = idx.as_number()?;
                    let mut items = list.borrow_mut();
                    let len = items.len() as i64;
                    let real = if n < 0 { len + n } else { n };
                    if real < 0 || real >= len {
                        return Err(VimError::new(
                            "E684",
                            format!("List index out of range: {n}"),
                        ));
                    }
                    items[real as usize] = value;
                    Ok(())
                }
                Value::Dict(dict) => {
                    dict.borrow_mut().insert(idx.to_string_value(), value);
                    // `let g:[...] = ...` through a scope dictionary must land
                    // in the real variable table (evalvars.c:3203-3236).
                    env.sync_scope_dict(&sync_target);
                    Ok(())
                }
                Value::Blob(blob) => {
                    let n = idx.as_number()?;
                    let mut bytes = blob.borrow_mut();
                    let len = bytes.len() as i64;
                    let real = if n < 0 { len + n } else { n };
                    if real < 0 || real >= len {
                        return Err(VimError::new("E684", "Blob index out of range".to_string()));
                    }
                    bytes[real as usize] = value.as_number()? as u8;
                    Ok(())
                }
                other => Err(VimError::new(
                    "E689",
                    format!(
                        "Can only index a List, Dictionary or Blob, got {}",
                        other.type_name()
                    ),
                )),
            }
        }
        rvim_viml::Expr::Slice(base, start, end) => {
            let container = eval_expr(ed, env, state, &expr_to_src(&base))?;
            let s = match &start {
                Some(e) => Some(eval_expr(ed, env, state, &expr_to_src(e))?.as_number()?),
                None => None,
            };
            let e = match &end {
                Some(e) => Some(eval_expr(ed, env, state, &expr_to_src(e))?.as_number()?),
                None => None,
            };
            match container {
                Value::List(list) => {
                    let items = match &value {
                        Value::List(l) => l.borrow().clone(),
                        other => vec![other.clone()],
                    };
                    let mut list_ref = list.borrow_mut();
                    let len = list_ref.len() as i64;
                    let norm = |v: i64| if v < 0 { len + v } else { v };
                    let from = s.map(norm).unwrap_or(0).max(0) as usize;
                    let to = e.map(norm).unwrap_or(len - 1).max(-1);
                    if to < 0 {
                        list_ref.splice(from..from, items);
                    } else {
                        let to = (to as usize + 1).min(list_ref.len());
                        list_ref.splice(from..to.max(from), items);
                    }
                    Ok(())
                }
                other => Err(VimError::new(
                    "E689",
                    format!("Cannot slice-assign a {}", other.type_name()),
                )),
            }
        }
        rvim_viml::Expr::Attr(base, key) => {
            let container = eval_expr(ed, env, state, &expr_to_src(&base))?;
            let sync_target = container.clone();
            match container {
                Value::Dict(dict) => {
                    dict.borrow_mut().insert(key, value);
                    // `let g:d.x = ...` on a scope dictionary writes globals.
                    env.sync_scope_dict(&sync_target);
                    Ok(())
                }
                other => Err(VimError::new(
                    "E715",
                    format!("Dictionary required, got {}", other.type_name()),
                )),
            }
        }
        rvim_viml::Expr::Var(name) => assign_var(ed, env, state, &name, value),
        _ => Err(VimError::new(
            "E461",
            format!("Illegal variable name: {target}"),
        )),
    }
}

/// Render an expression back to source text (used to re-evaluate sub-parts).
fn expr_to_src(expr: &rvim_viml::Expr) -> String {
    use rvim_viml::Expr;
    match expr {
        Expr::Number(n) => n.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::Str(s) => format!("'{}'", s.replace('\'', "''")),
        Expr::Blob(_) => "0z".to_string(),
        Expr::Bool(true) => "v:true".to_string(),
        Expr::Bool(false) => "v:false".to_string(),
        Expr::Null => "v:null".to_string(),
        Expr::Var(name) => name.clone(),
        Expr::Option(name) => format!("&{name}"),
        Expr::Env(name) => format!("${name}"),
        Expr::Register(r) => format!("@{r}"),
        Expr::List(_) | Expr::Dict(_) | Expr::Lambda { .. } => "[]".to_string(),
        Expr::Unary(op, inner) => format!("{op}({})", expr_to_src(inner)),
        Expr::Binary(op, a, b) => format!("({}) {op} ({})", expr_to_src(a), expr_to_src(b)),
        Expr::Ternary(c, t, e) => format!(
            "({}) ? ({}) : ({})",
            expr_to_src(c),
            expr_to_src(t),
            expr_to_src(e)
        ),
        Expr::Index(a, b) => format!("{}[{}]", expr_to_src(a), expr_to_src(b)),
        Expr::Slice(a, s, e) => format!(
            "{}[{}:{}]",
            expr_to_src(a),
            s.as_ref().map(|x| expr_to_src(x)).unwrap_or_default(),
            e.as_ref().map(|x| expr_to_src(x)).unwrap_or_default()
        ),
        Expr::Attr(a, k) => format!("{}.{}", expr_to_src(a), k),
        Expr::Call(f, args) => format!(
            "{}({})",
            expr_to_src(f),
            args.iter().map(expr_to_src).collect::<Vec<_>>().join(", ")
        ),
        Expr::Method(r, name, args) => format!(
            "{}->{}({})",
            expr_to_src(r),
            name,
            args.iter().map(expr_to_src).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// `:unlet[!] {name} ...`
pub fn cmd_unlet(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let _ = ed;
    for name in p.args.split_whitespace() {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let result = if let Some(rest) = name.strip_prefix("b:") {
            let bufnr = 0;
            let _ = bufnr;
            let removed = state.bufvars.values_mut().any(|m| m.remove(rest).is_some());
            if removed {
                Ok(())
            } else {
                Err(VimError::new("E108", format!("No such variable: {name}")))
            }
        } else if let Some(rest) = name.strip_prefix("w:") {
            if state.winvars.remove(rest).is_some() {
                Ok(())
            } else {
                Err(VimError::new("E108", format!("No such variable: {name}")))
            }
        } else if let Some(rest) = name.strip_prefix("t:") {
            if state.tabvars.remove(rest).is_some() {
                Ok(())
            } else {
                Err(VimError::new("E108", format!("No such variable: {name}")))
            }
        } else {
            env.unlet_var(name)
        };
        if let Err(e) = result {
            if !p.bang {
                return Err(e);
            }
        }
    }
    Ok(ExOutcome::Continue)
}

/// `:execute {expr} ...` — evaluate and run the resulting command text.
///
/// Vim evaluates each expression in the argument list separately and joins the
/// resulting strings with a single space (eval.c:6699-6757, esp. 6749-6751), so
/// `execute 'let g:x =' 1 + 2` executes `let g:x = 3`.
pub fn cmd_execute(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let mut rest = p.args.trim_start();
    let mut parts: Vec<String> = Vec::new();
    while !rest.trim().is_empty() {
        let (value, consumed) = eval_expr_prefix(ed, env, state, rest)?;
        parts.push(value.to_string_value());
        rest = rest.get(consumed..).unwrap_or("").trim_start();
    }
    let text = parts.join(" ");
    if text.trim().is_empty() {
        return Ok(ExOutcome::Continue);
    }
    run_line_with(ed, env, state, &text)
}

/// `:eval {expr}` — evaluate for side effects.
pub fn cmd_eval(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let _ = eval_expr(ed, env, state, &p.args)?;
    Ok(ExOutcome::Continue)
}

/// `:call {func}(...)` — evaluate the call expression.
pub fn cmd_call(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let expr = parse_expression(&p.args)?;
    let mut host = crate::script::make_host(ed, state);
    vl_eval(&expr, env, &mut host)?;
    Ok(ExOutcome::Continue)
}

/// `:function` listing and `:function!` clearing (definitions are parsed by the
/// script runner).
pub fn cmd_function(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let name = p.args.trim();
    if name.is_empty() {
        if p.bang {
            state.funcs.clear();
            return Ok(ExOutcome::Continue);
        }
        let mut names: Vec<String> = state.funcs.values().map(display_func_name).collect();
        names.sort();
        for name in names {
            ed.messages.info(format!("function {name}"));
        }
        return Ok(ExOutcome::Continue);
    }
    let resolved = resolve_function_name(name, state, env.current_sid);
    match state.funcs.get(&resolved) {
        Some(func) => {
            ed.messages
                .info(format!("function {} ", display_func_name(func)));
            for line in &func.body {
                ed.messages.info(line.clone());
            }
            ed.messages.info("endfunction".to_string());
            Ok(ExOutcome::Continue)
        }
        None => Err(VimError::new("E123", format!("Function {name} not found"))),
    }
}

fn display_func_name(func: &UserFunc) -> String {
    let mut params = func.params.clone();
    if func.varargs {
        params.push("...".to_string());
    }
    format!("{}({})", func.name, params.join(", "))
}

/// `:delfunction[!] {name}`
pub fn cmd_delfunction(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let name = p.args.trim();
    if name.is_empty() {
        return Err(VimError::range_required());
    }
    let resolved = resolve_function_name(name, state, env.current_sid);
    if state.funcs.remove(&resolved).is_none() && !p.bang {
        return Err(VimError::new("E130", format!("Function {name} not found")));
    }
    let _ = ed;
    Ok(ExOutcome::Continue)
}

/// `:setf` / `:setfiletype` set the buffer 'filetype' and fire `FileType`.
///
/// Without `!` the option is only set when it is still empty, which is what the
/// runtime detection autocommands (`au BufRead *.c setf c`) rely on.
pub fn cmd_setfiletype(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let name = p.args.trim();
    if name.is_empty() {
        return Ok(ExOutcome::Continue);
    }
    if !p.bang && !ed.options.str_or("filetype", "").trim().is_empty() {
        return Ok(ExOutcome::Continue);
    }
    ed.options
        .set("filetype", rvim_editor::OptValue::Str(name.to_string()))?;
    // Setting 'filetype' triggers FileType with the filetype as the pattern.
    // vim applies it with "force", so it fires even from inside the BufRead
    // autocommand that called `:setf` (optionstr.c:2551).
    crate::autodispatch::fire_forced(ed, env, state, "FileType", name);
    Ok(ExOutcome::Continue)
}

/// `:filetype on|off|detect|plugin|indent ...`
pub fn cmd_filetype_state(
    ed: &mut Editor,
    env: &mut Env,
    state: &mut ScriptState,
    p: &ParsedEx,
) -> VimResult<ExOutcome> {
    let args = p.args.trim().to_ascii_lowercase();
    if args.is_empty() {
        ed.messages.info(format!(
            "filetype detection:{} plugin:{} indent:{}",
            if ed.filetype_detection { "ON" } else { "OFF" },
            if ed.filetype_plugin { "ON" } else { "OFF" },
            if ed.filetype_indent { "ON" } else { "OFF" }
        ));
        return Ok(ExOutcome::Continue);
    }
    for word in args.split_whitespace() {
        match word {
            "on" | "detect" => {
                ed.filetype_detection = true;
                // vim sources $VIMRUNTIME/filetype.vim the first time detection
                // is switched on (filetype.vim: `runtime! filetype.vim`).
                load_filetype_script(ed, env, state)?;
            }
            "off" => {
                ed.filetype_detection = false;
                ed.filetype_plugin = false;
                ed.filetype_indent = false;
            }
            "plugin" => ed.filetype_plugin = true,
            "indent" => ed.filetype_indent = true,
            _ => {}
        }
    }
    Ok(ExOutcome::Continue)
}

/// Source `filetype.vim` from 'runtimepath' once, the way `:filetype on` does.
fn load_filetype_script(ed: &mut Editor, env: &mut Env, state: &mut ScriptState) -> VimResult<()> {
    if ed.filetype_script_loaded {
        return Ok(());
    }
    let Some(path) = crate::handlers_script::find_in_runtime(ed, "filetype.vim") else {
        return Ok(());
    };
    ed.filetype_script_loaded = true;
    crate::handlers_script::source_file(ed, env, state, &path)?;
    Ok(())
}
