//! Vim9 evaluator: expressions, statements, functions, imports and script
//! loading.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rvim_core::{VimError, VimResult};

use crate::env::{Env, Frame, Host};
use crate::regex_mvp;
use crate::value::{FuncRef, Value, VimDict, values_equal};
use crate::vim9::ast::*;
use crate::vim9::parser::{StmtParser, parse_expression_prefix, prepare_lines};
use crate::vim9::runtime::{ImportBinding, Module, Vim9Lambda, VimClassState, module_key};
use crate::vim9::types::Vim9Type;

/// Control-flow result of executing a statement list.
#[derive(Debug)]
pub enum Flow {
    Next,
    Break,
    Continue,
    Return(Value),
    Throw(Value),
}

/// Per-call evaluation context.
pub struct Ctx {
    pub sid: usize,
    pub script_path: String,
    pub prefix: String,
    pub types: HashMap<String, Vim9Type>,
    pub script_types: HashMap<String, Vim9Type>,
    pub at_script_level: bool,
    pub return_type: Vim9Type,
    pub loop_depth: usize,
    /// Canonical class whose method is currently executing (access control).
    pub current_class: Option<String>,
}

impl Ctx {
    fn new(sid: usize, script_path: &str, prefix: &str, at_script_level: bool) -> Self {
        Self {
            sid,
            script_path: script_path.to_string(),
            prefix: prefix.to_string(),
            types: HashMap::new(),
            script_types: HashMap::new(),
            at_script_level,
            return_type: Vim9Type::Any,
            loop_depth: 0,
            current_class: None,
        }
    }
}

/// Canonicalize a script path for module lookup.
fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

// ---------------------------------------------------------------------------
// Script loading
// ---------------------------------------------------------------------------

/// Register a parsed script and execute its top-level statements.
pub fn run_script(script: &Vim9Script, env: &mut Env, host: &mut dyn Host) -> VimResult<()> {
    // Script-local variable declarations are visible to all functions, even
    // those defined before the var statement (Vim compiles the whole script).
    let mut script_types: HashMap<String, Vim9Type> = HashMap::new();
    let mut exported_vars: Vec<(String, Vim9Type)> = Vec::new();
    for stmt in &script.top {
        if let Stmt::Var {
            name,
            ty,
            value: _,
            export,
            ..
        } = stmt
        {
            let inferred = ty.clone().unwrap_or_default();
            script_types.insert(name.clone(), inferred.clone());
            if *export {
                exported_vars.push((name.clone(), inferred));
            }
        }
    }
    env.vim9
        .script_types
        .insert(script.sid, script_types.clone());
    env.vim9
        .script_prefix
        .insert(script.sid, script.prefix.clone());

    // Register classes (and their static members) before functions run.
    for class in &script.classes {
        let mut statics = HashMap::new();
        let mut class_ctx = Ctx::new(script.sid, &script.path, &script.prefix, true);
        class_ctx.script_types = script_types.clone();
        for member in &class.members {
            if !member.static_member {
                continue;
            }
            let value = match &member.default {
                Some(expr) => eval_expr(expr, &mut class_ctx, env, host)?,
                None => default_value(Some(&member.ty)),
            };
            statics.insert(member.name.clone(), value);
        }
        let state = Rc::new(VimClassState {
            def: class.clone(),
            statics: std::cell::RefCell::new(statics),
        });
        env.vim9.classes.insert(class.canonical.clone(), state);
    }
    for item in &script.enums {
        env.vim9.enums.insert(item.canonical.clone(), item.clone());
    }
    for alias in &script.type_aliases {
        env.vim9
            .type_aliases
            .insert(alias.canonical.clone(), alias.ty.clone());
    }

    // Register every function before running the body.
    for func in &script.funcs {
        env.vim9.add_function(func.clone());
    }

    // Build the importable module description.
    let mut module = Module {
        path: absolute(Path::new(&script.path)),
        sid: script.sid,
        prefix: script.prefix.clone(),
        funcs: HashMap::new(),
        vars: HashMap::new(),
        classes: HashMap::new(),
        enums: HashMap::new(),
        aliases: HashMap::new(),
    };
    for func in &script.funcs {
        if func.is_export {
            module
                .funcs
                .insert(func.name.clone(), func.canonical.clone());
        }
    }
    for (name, ty) in exported_vars {
        module.vars.insert(name, ty);
    }
    for class in &script.classes {
        if class.is_export {
            module
                .classes
                .insert(class.name.clone(), class.canonical.clone());
        }
    }
    for item in &script.enums {
        if item.is_export {
            module
                .enums
                .insert(item.name.clone(), item.canonical.clone());
        }
    }
    for alias in &script.type_aliases {
        if alias.is_export {
            module
                .aliases
                .insert(alias.name.clone(), alias.canonical.clone());
        }
    }
    // Legacy :func blocks defined inside a Vim9 script are global functions.
    for stmt in &script.top {
        if let Stmt::LegacyFunc {
            header,
            export: true,
            ..
        } = stmt
        {
            if let Some(name) = legacy_func_name(header) {
                module.funcs.entry(name.clone()).or_insert(name);
            }
        }
    }
    let key = module_key(&module.path.to_string_lossy());
    env.vim9.modules.insert(key, Rc::new(module));

    let mut ctx = Ctx::new(script.sid, &script.path, &script.prefix, true);
    ctx.script_types = script_types;
    let saved_sid = env.current_sid;
    env.current_sid = script.sid;
    let result = exec_stmts(&script.top, &mut ctx, env, host);
    env.current_sid = saved_sid;
    match result? {
        Flow::Next => Ok(()),
        Flow::Return(_) => Ok(()),
        Flow::Throw(value) => {
            env.vvars.insert("exception".to_string(), value.clone());
            Err(VimError::aborting(
                "E605",
                format!("Exception not caught: {}", value.to_string_value()),
            ))
        }
        Flow::Break | Flow::Continue => Ok(()),
    }
}

fn legacy_func_name(header: &str) -> Option<String> {
    let name = header.split('(').next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.trim_start_matches("s:").to_string())
    }
}

/// Execute a single Vim9 command line (used by :vim9cmd and :var).
pub fn run_command_line(
    line: &str,
    sid: usize,
    script_path: &str,
    prefix: &str,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    let raw = vec![line.to_string()];
    let lines = prepare_lines(&raw, 1)?;
    let mut parser = StmtParser {
        lines: &lines,
        pos: 0,
        sid,
        prefix,
    };
    let (stmts, term) = parser.parse_block(&[])?;
    if let Some(term) = term {
        return Err(VimError::new("E580", format!("Unexpected :{term}")));
    }
    let mut ctx = Ctx::new(sid, script_path, prefix, true);
    ctx.script_types = env.vim9.script_types.get(&sid).cloned().unwrap_or_default();
    let saved_sid = env.current_sid;
    env.current_sid = sid;
    let result = exec_stmts(&stmts, &mut ctx, env, host);
    env.current_sid = saved_sid;
    match result? {
        Flow::Next | Flow::Return(_) | Flow::Break | Flow::Continue => Ok(()),
        Flow::Throw(value) => Err(VimError::aborting(
            "E605",
            format!("Exception not caught: {}", value.to_string_value()),
        )),
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

pub fn exec_stmts(
    stmts: &[Stmt],
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<Flow> {
    for stmt in stmts {
        let flow = exec_stmt(stmt, ctx, env, host)?;
        if !matches!(flow, Flow::Next) {
            return Ok(flow);
        }
    }
    Ok(Flow::Next)
}

fn exec_stmt(stmt: &Stmt, ctx: &mut Ctx, env: &mut Env, host: &mut dyn Host) -> VimResult<Flow> {
    match stmt {
        Stmt::Var {
            name,
            ty,
            value,
            constant,
            line,
            ..
        } => {
            let value = match value {
                Some(expr) => eval_expr(expr, ctx, env, host)?,
                // A typed declaration without a value gets Vim9's default for
                // the type (empty string/list/dict, zero, false).
                None => default_value(ty.as_ref()),
            };
            if let Some(ty) = ty {
                if !value_is(ty, &value) {
                    return Err(type_mismatch(ty, &value, *line));
                }
            }
            declare_variable(name, ty.clone(), value, *constant, ctx, env, host)?;
            Ok(Flow::Next)
        }
        Stmt::Destructure {
            names,
            value,
            constant,
            line,
        } => {
            let collection = eval_expr(value, ctx, env, host)?;
            let Value::List(list) = &collection else {
                return Err(VimError::new(
                    "E1098",
                    format!("Cannot destructure a {} value", collection.type_name()),
                ));
            };
            let items = list.borrow().clone();
            for (index, (name, ty)) in names.iter().enumerate() {
                let value = items.get(index).cloned().unwrap_or(Value::Null);
                if let Some(ty) = ty {
                    if !value_is(ty, &value) {
                        return Err(type_mismatch(ty, &value, *line));
                    }
                }
                declare_variable(name, ty.clone(), value, *constant, ctx, env, host)?;
            }
            Ok(Flow::Next)
        }
        Stmt::Assign {
            target,
            op,
            value,
            line,
        } => {
            let new_value = eval_expr(value, ctx, env, host)?;
            let new_value = if op == "=" {
                new_value
            } else {
                let current = eval_expr(target, ctx, env, host)?;
                let base = op.trim_end_matches('=');
                apply_binary(base, current, new_value, *line)?
            };
            assign_target(target, new_value, *line, ctx, env, host)?;
            Ok(Flow::Next)
        }
        Stmt::IncDec {
            target,
            delta,
            line,
        } => {
            let current = eval_expr(target, ctx, env, host)?;
            let new_value = match current {
                Value::Number(n) => Value::Number(n.wrapping_add(*delta)),
                Value::Float(f) => Value::Float(f + *delta as f64),
                other => {
                    return Err(VimError::new(
                        "E1012",
                        format!(
                            "Type mismatch; expected number but got {} (line {line})",
                            other.type_name()
                        ),
                    ));
                }
            };
            assign_target(target, new_value, *line, ctx, env, host)?;
            Ok(Flow::Next)
        }
        Stmt::If {
            branches,
            else_body,
            line: _,
        } => {
            for (cond, body) in branches {
                let value = eval_expr(cond, ctx, env, host)?;
                if condition(&value)? {
                    return exec_stmts(body, ctx, env, host);
                }
            }
            match else_body {
                Some(body) => exec_stmts(body, ctx, env, host),
                None => Ok(Flow::Next),
            }
        }
        Stmt::While { cond, body, .. } => {
            ctx.loop_depth += 1;
            let result = (|| {
                loop {
                    let value = eval_expr(cond, ctx, env, host)?;
                    if !condition(&value)? {
                        break;
                    }
                    match exec_stmts(body, ctx, env, host)? {
                        Flow::Next | Flow::Continue => {}
                        Flow::Break => break,
                        other => return Ok(other),
                    }
                }
                Ok(Flow::Next)
            })();
            ctx.loop_depth -= 1;
            result
        }
        Stmt::For {
            vars,
            iter,
            body,
            line,
        } => {
            let collection = eval_expr(iter, ctx, env, host)?;
            let items: Vec<Value> = match collection {
                Value::List(list) => list.borrow().clone(),
                Value::String(s) => s.chars().map(|c| Value::string(c.to_string())).collect(),
                Value::Blob(blob) => blob
                    .borrow()
                    .iter()
                    .map(|b| Value::Number(i64::from(*b)))
                    .collect(),
                other => {
                    return Err(VimError::new(
                        "E1098",
                        format!("Cannot iterate over {}", other.type_name()),
                    ));
                }
            };
            ctx.loop_depth += 1;
            let result = (|| {
                for item in items {
                    bind_for_vars(vars, item, *line, ctx, env, host)?;
                    match exec_stmts(body, ctx, env, host)? {
                        Flow::Next | Flow::Continue => {}
                        Flow::Break => break,
                        other => return Ok(other),
                    }
                }
                Ok(Flow::Next)
            })();
            ctx.loop_depth -= 1;
            result
        }
        Stmt::Try {
            body,
            catches,
            finally,
            line,
        } => exec_try(body, catches, finally, *line, ctx, env, host),
        Stmt::Return { value, line } => {
            let value = match value {
                Some(expr) => eval_expr(expr, ctx, env, host)?,
                None => Value::Null,
            };
            if !matches!(ctx.return_type, Vim9Type::Any | Vim9Type::Void)
                && !value_is(&ctx.return_type, &value)
            {
                return Err(type_mismatch(&ctx.return_type, &value, *line));
            }
            Ok(Flow::Return(value))
        }
        Stmt::Throw { value, line } => {
            let value = eval_expr(value, ctx, env, host)?;
            let _ = line;
            Ok(Flow::Throw(value))
        }
        Stmt::Break { .. } => {
            if ctx.loop_depth == 0 {
                return Err(VimError::new("E587", ":break without :while or :for"));
            }
            Ok(Flow::Break)
        }
        Stmt::Continue { .. } => {
            if ctx.loop_depth == 0 {
                return Err(VimError::new("E586", ":continue without :while or :for"));
            }
            Ok(Flow::Continue)
        }
        Stmt::Ex { line, number } => {
            exec_ex_line(line, *number, ctx, env, host)?;
            Ok(Flow::Next)
        }
        Stmt::ExprStmt { expr, .. } => {
            eval_expr(expr, ctx, env, host)?;
            Ok(Flow::Next)
        }
        Stmt::Def(func) => {
            env.vim9.add_function(func.clone());
            Ok(Flow::Next)
        }
        Stmt::LegacyFunc {
            header,
            body,
            export,
            ..
        } => {
            if let Some(result) = host.define_legacy_function(header, body, ctx.sid, env) {
                result?;
            } else {
                let _ = export;
            }
            Ok(Flow::Next)
        }
        Stmt::ClassDef(_) | Stmt::EnumDef(_) | Stmt::TypeAlias(_) => Ok(Flow::Next),
        Stmt::Import { kind, .. } => {
            handle_import(kind, ctx, env, host)?;
            Ok(Flow::Next)
        }
        Stmt::Nop => Ok(Flow::Next),
        Stmt::Block(inner) => exec_stmts(inner, ctx, env, host),
    }
}

fn exec_try(
    body: &[Stmt],
    catches: &[CatchClause],
    finally: &[Stmt],
    _line: usize,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<Flow> {
    let mut pending_throw: Option<Value> = None;
    let mut pending_error: Option<VimError> = None;
    let mut result: Option<Flow> = None;
    match exec_stmts(body, ctx, env, host) {
        Ok(Flow::Next) => {}
        Ok(flow @ (Flow::Return(_) | Flow::Break | Flow::Continue)) => {
            result = Some(flow);
        }
        Ok(Flow::Throw(value)) => pending_throw = Some(value),
        Err(error) => {
            pending_throw = Some(exception_value(&error, env));
            pending_error = Some(error);
        }
    }
    if let Some(value) = pending_throw.clone() {
        env.vvars.insert("exception".to_string(), value.clone());
        let text = value.to_string_value();
        let mut handled = false;
        for clause in catches {
            if catch_matches(clause.pattern.as_deref(), &text) {
                handled = true;
                result = Some(exec_stmts(&clause.body, ctx, env, host)?);
                break;
            }
        }
        if !handled && pending_error.is_none() {
            result = Some(Flow::Throw(value));
        }
    }
    let finally_flow = exec_stmts(finally, ctx, env, host)?;
    if !matches!(finally_flow, Flow::Next) {
        return Ok(finally_flow);
    }
    if let Some(error) = pending_error {
        return Err(error);
    }
    Ok(result.unwrap_or(Flow::Next))
}

fn exception_value(error: &VimError, env: &mut Env) -> Value {
    if error.code == "E605" {
        if let Some(value) = env.pending_throw.take() {
            return value;
        }
        if let Some(text) = error.message.strip_prefix("Exception not caught: ") {
            return Value::String(text.to_string());
        }
    }
    Value::String(format!("Vim:{}", error))
}

fn catch_matches(pattern: Option<&str>, text: &str) -> bool {
    match pattern {
        None => true,
        Some(pattern) => {
            if regex_mvp::is_match(pattern, text, false) {
                return true;
            }
            // Allow a plain error code (E123) as a catch pattern.
            if pattern.starts_with('E') && text.contains(pattern) {
                return true;
            }
            false
        }
    }
}

fn bind_for_vars(
    vars: &[(String, Option<Vim9Type>)],
    item: Value,
    line: usize,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    if vars.len() == 1 {
        let (name, ty) = &vars[0];
        if let Some(ty) = ty {
            if !value_is(ty, &item) {
                return Err(type_mismatch(ty, &item, line));
            }
        }
        declare_variable(name, ty.clone(), item, false, ctx, env, host)?;
        return Ok(());
    }
    let Value::List(list) = &item else {
        return Err(VimError::new(
            "E1098",
            "Cannot destructure a non-list value".to_string(),
        ));
    };
    let values = list.borrow().clone();
    for (i, (name, ty)) in vars.iter().enumerate() {
        let value = values.get(i).cloned().unwrap_or(Value::Null);
        if let Some(ty) = ty {
            if !value_is(ty, &value) {
                return Err(type_mismatch(ty, &value, line));
            }
        }
        declare_variable(name, ty.clone(), value, false, ctx, env, host)?;
    }
    Ok(())
}

fn declare_variable(
    name: &str,
    ty: Option<Vim9Type>,
    value: Value,
    constant: bool,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    if let Some(rest) = name.strip_prefix("g:") {
        env.globals.insert(rest.to_string(), value);
        return Ok(());
    }
    if let Some(rest) = name.strip_prefix("b:") {
        return host
            .buffer_var_set(rest, value)
            .unwrap_or_else(|| Err(unknown_scope(name)));
    }
    if let Some(rest) = name.strip_prefix("w:") {
        return host
            .window_var_set(rest, value)
            .unwrap_or_else(|| Err(unknown_scope(name)));
    }
    if let Some(rest) = name.strip_prefix("t:") {
        return host
            .tab_var_set(rest, value)
            .unwrap_or_else(|| Err(unknown_scope(name)));
    }
    if let Some(rest) = name.strip_prefix("s:") {
        script_vars_mut(env, ctx.sid).insert(rest.to_string(), value);
        if let Some(ty) = ty {
            env.vim9
                .script_types
                .entry(ctx.sid)
                .or_default()
                .insert(rest.to_string(), ty);
        }
        if constant {
            env.vim9.constants.insert(format!("{}:{}", ctx.sid, rest));
        }
        return Ok(());
    }
    if let Some(rest) = name.strip_prefix("l:") {
        if let Some(frame) = env.frames.last_mut() {
            frame.locals.insert(rest.to_string(), value);
            if let Some(ty) = ty {
                ctx.types.insert(rest.to_string(), ty);
            }
            return Ok(());
        }
        return Err(unknown_scope(name));
    }
    if let Some(rest) = name.strip_prefix("a:") {
        if let Some(frame) = env.frames.last_mut() {
            frame.args.insert(rest.to_string(), value);
            return Ok(());
        }
        return Err(unknown_scope(name));
    }
    if let Some(rest) = name.strip_prefix("v:") {
        return env.set_var(&format!("v:{rest}"), value);
    }
    if ctx.at_script_level {
        script_vars_mut(env, ctx.sid).insert(name.to_string(), value);
        if let Some(ty) = ty {
            ctx.script_types.insert(name.to_string(), ty.clone());
            env.vim9
                .script_types
                .entry(ctx.sid)
                .or_default()
                .insert(name.to_string(), ty);
        }
        if constant {
            env.vim9.constants.insert(format!("{}:{}", ctx.sid, name));
        }
        return Ok(());
    }
    if let Some(frame) = env.frames.last_mut() {
        frame.locals.insert(name.to_string(), value);
        if let Some(ty) = ty {
            ctx.types.insert(name.to_string(), ty);
        }
        Ok(())
    } else {
        script_vars_mut(env, ctx.sid).insert(name.to_string(), value);
        Ok(())
    }
}

fn script_vars_mut(env: &mut Env, sid: usize) -> &mut HashMap<String, Value> {
    if sid >= env.scripts.len() {
        env.scripts.resize_with(sid + 1, HashMap::new);
    }
    &mut env.scripts[sid]
}

fn unknown_scope(name: &str) -> VimError {
    VimError::new("E461", format!("Illegal variable name: {name}"))
}

fn assign_target(
    target: &Expr,
    value: Value,
    line: usize,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    match target {
        Expr::Var(name) => set_variable(name, value, line, ctx, env, host),
        Expr::Index(container, index) => {
            let target_value = eval_expr(container, ctx, env, host)?;
            let key = eval_expr(index, ctx, env, host)?;
            match &target_value {
                Value::List(list) => {
                    let idx = key.as_number()?;
                    let mut items = list.borrow_mut();
                    let resolved = resolve_index(idx, items.len())?;
                    items[resolved] = value;
                    Ok(())
                }
                Value::Dict(dict) => {
                    dict.borrow_mut().insert(key.to_string_value(), value);
                    Ok(())
                }
                Value::Blob(blob) => {
                    let idx = key.as_number()?;
                    let mut bytes = blob.borrow_mut();
                    let resolved = resolve_index(idx, bytes.len())?;
                    bytes[resolved] = value.as_number()? as u8;
                    Ok(())
                }
                other => Err(VimError::new(
                    "E689",
                    format!(
                        "Can only index a List, Dictionary or Blob, not {}",
                        other.type_name()
                    ),
                )),
            }
        }
        Expr::Attr(base, key) => {
            if let Expr::Var(alias) = base.as_ref() {
                if let Some(class) = resolve_class(alias, ctx, env) {
                    let member = class
                        .def
                        .members
                        .iter()
                        .find(|member| &member.name == key)
                        .ok_or_else(|| {
                            VimError::new(
                                "E1326",
                                format!("Member not found on class {}: {key}", class.def.name),
                            )
                        })?;
                    if !member.static_member {
                        return Err(VimError::new(
                            "E1376",
                            format!("Cannot set object member using the class name: {key}"),
                        ));
                    }
                    if !member.public
                        && ctx.current_class.as_deref() != Some(class.def.canonical.as_str())
                    {
                        return Err(VimError::new(
                            "E1335",
                            format!("Cannot set read-only class member: {key}"),
                        ));
                    }
                    class.statics.borrow_mut().insert(key.clone(), value);
                    return Ok(());
                }
            }
            let target_value = eval_expr(base, ctx, env, host)?;
            if let Some(class) = env.vim9.object_class(&target_value) {
                let member = class
                    .def
                    .members
                    .iter()
                    .find(|member| &member.name == key)
                    .ok_or_else(|| {
                        VimError::new(
                            "E1326",
                            format!("Member not found on object {}: {key}", class.def.name),
                        )
                    })?;
                if member.name.starts_with('_')
                    && ctx.current_class.as_deref() != Some(class.def.canonical.as_str())
                {
                    return Err(VimError::new(
                        "E1332",
                        format!("Cannot access protected member: {key}"),
                    ));
                }
                if !member.public
                    && ctx.current_class.as_deref() != Some(class.def.canonical.as_str())
                {
                    return Err(VimError::new(
                        "E1335",
                        format!("Cannot set read-only member: {key}"),
                    ));
                }
            }
            match &target_value {
                Value::Dict(dict) => {
                    dict.borrow_mut().insert(key.clone(), value);
                    Ok(())
                }
                other => Err(VimError::new(
                    "E715",
                    format!("Cannot add to a {} value", other.type_name()),
                )),
            }
        }
        _ => Err(VimError::new(
            "E474",
            format!("Invalid assignment target at line {line}"),
        )),
    }
}

fn set_variable(
    name: &str,
    value: Value,
    line: usize,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    // Type check against the declaration, when known.
    let declared = declared_type(name, ctx);
    if let Some(ty) = declared {
        if !value_is(&ty, &value) {
            return Err(type_mismatch(&ty, &value, line));
        }
    }
    if let Some(rest) = name.strip_prefix("g:") {
        env.globals.insert(rest.to_string(), value);
        return Ok(());
    }
    if let Some(rest) = name.strip_prefix("b:") {
        return host
            .buffer_var_set(rest, value)
            .unwrap_or_else(|| Err(unknown_scope(name)));
    }
    if let Some(rest) = name.strip_prefix("w:") {
        return host
            .window_var_set(rest, value)
            .unwrap_or_else(|| Err(unknown_scope(name)));
    }
    if let Some(rest) = name.strip_prefix("t:") {
        return host
            .tab_var_set(rest, value)
            .unwrap_or_else(|| Err(unknown_scope(name)));
    }
    if let Some(rest) = name.strip_prefix("s:") {
        if env
            .vim9
            .constants
            .contains(&format!("{}:{}", ctx.sid, rest))
        {
            return Err(VimError::new(
                "E995",
                format!("Cannot change constant: s:{rest}"),
            ));
        }
        script_vars_mut(env, ctx.sid).insert(rest.to_string(), value);
        return Ok(());
    }
    if let Some(rest) = name.strip_prefix("l:") {
        if let Some(frame) = env.frames.last_mut() {
            frame.locals.insert(rest.to_string(), value);
            return Ok(());
        }
        return Err(unknown_scope(name));
    }
    if let Some(rest) = name.strip_prefix("a:") {
        if let Some(frame) = env.frames.last_mut() {
            frame.args.insert(rest.to_string(), value);
            return Ok(());
        }
        return Err(unknown_scope(name));
    }
    if let Some(rest) = name.strip_prefix("v:") {
        return env.set_var(&format!("v:{rest}"), value);
    }
    if let Some(canonical) = &ctx.current_class {
        if let Some(class) = env.vim9.class(canonical) {
            if class
                .def
                .members
                .iter()
                .any(|member| member.static_member && member.name == name)
            {
                class.statics.borrow_mut().insert(name.to_string(), value);
                return Ok(());
            }
        }
    }
    if env
        .frames
        .last()
        .is_some_and(|f| f.locals.contains_key(name))
    {
        if env
            .vim9
            .constants
            .contains(&format!("{}:{}", ctx.sid, name))
        {
            return Err(VimError::new(
                "E995",
                format!("Cannot change constant: {name}"),
            ));
        }
        if let Some(frame) = env.frames.last_mut() {
            frame.locals.insert(name.to_string(), value);
        }
        return Ok(());
    }
    if env
        .scripts
        .get(ctx.sid)
        .is_some_and(|m| m.contains_key(name))
    {
        script_vars_mut(env, ctx.sid).insert(name.to_string(), value);
        return Ok(());
    }
    if ctx.at_script_level {
        script_vars_mut(env, ctx.sid).insert(name.to_string(), value);
        Ok(())
    } else {
        Err(VimError::new(
            "E1001",
            format!("Variable not found: {name} (line {line})"),
        ))
    }
}

fn declared_type(name: &str, ctx: &Ctx) -> Option<Vim9Type> {
    if let Some(rest) = name.strip_prefix("s:") {
        return ctx.script_types.get(rest).cloned();
    }
    if name.contains(':') {
        return None;
    }
    ctx.types
        .get(name)
        .cloned()
        .or_else(|| ctx.script_types.get(name).cloned())
}

fn default_value(ty: Option<&Vim9Type>) -> Value {
    match ty {
        Some(Vim9Type::String) => Value::string(""),
        Some(Vim9Type::Number) => Value::Number(0),
        Some(Vim9Type::Float) => Value::Float(0.0),
        Some(Vim9Type::Bool) => Value::Bool(false),
        Some(Vim9Type::List(_)) => Value::list(Vec::new()),
        Some(Vim9Type::Dict(_)) => Value::dict(VimDict::new()),
        Some(Vim9Type::Blob) => Value::blob(Vec::new()),
        Some(Vim9Type::Named(name)) if name == "list" => Value::list(Vec::new()),
        Some(Vim9Type::Named(name)) if name == "dict" => Value::dict(VimDict::new()),
        _ => Value::Null,
    }
}

fn value_is(ty: &Vim9Type, value: &Value) -> bool {
    match (ty, value) {
        (Vim9Type::Any, _) | (Vim9Type::Void, _) => true,
        (Vim9Type::Bool, Value::Bool(_)) => true,
        // Builtins historically return Number for boolean results.
        (Vim9Type::Bool, Value::Number(0 | 1)) => true,
        (Vim9Type::Number, Value::Bool(_)) => true,
        _ => ty.accepts_value(value),
    }
}

fn type_mismatch(ty: &Vim9Type, value: &Value, line: usize) -> VimError {
    VimError::new(
        "E1012",
        format!(
            "Type mismatch; expected {} but got {} (line {line})",
            ty.name(),
            value.type_name()
        ),
    )
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

pub fn eval_expr(
    expr: &Expr,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    match expr {
        Expr::Number(n) => Ok(Value::Number(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::Str(s) => Ok(Value::String(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),
        Expr::Blob(bytes) => Ok(Value::blob(bytes.clone())),
        Expr::Var(name) => get_variable(name, ctx, env, host),
        Expr::Option(name) => host
            .option_get(name)
            .ok_or_else(|| VimError::new("E113", format!("Unknown option: {name}"))),
        Expr::Env(name) => Ok(Value::String(std::env::var(name).unwrap_or_default())),
        Expr::Register(r) => host
            .host_register(*r)
            .unwrap_or_else(|| Ok(Value::String(String::new()))),
        Expr::List(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(eval_expr(item, ctx, env, host)?);
            }
            Ok(Value::list(values))
        }
        Expr::Dict(entries) => {
            let mut dict = VimDict::new();
            for (key, value_expr) in entries {
                let key = match key {
                    DictKey::Literal(text) => text.clone(),
                    DictKey::Expr(expr) => eval_expr(expr, ctx, env, host)?.to_string_value(),
                };
                dict.insert(key, eval_expr(value_expr, ctx, env, host)?);
            }
            Ok(Value::dict(dict))
        }
        Expr::Unary(op, inner) => {
            let value = eval_expr(inner, ctx, env, host)?;
            match *op {
                "!" => Ok(Value::Bool(!truthy_any(&value))),
                "-" => match value {
                    Value::Float(f) => Ok(Value::Float(-f)),
                    Value::Number(n) => Ok(Value::Number(n.wrapping_neg())),
                    other => Err(VimError::new(
                        "E1012",
                        format!("Cannot negate a {} value", other.type_name()),
                    )),
                },
                "+" => match value {
                    Value::Float(_) | Value::Number(_) => Ok(value),
                    other => Err(VimError::new(
                        "E1012",
                        format!("Cannot apply unary + to {}", other.type_name()),
                    )),
                },
                _ => Err(VimError::new("E15", "Unknown unary operator")),
            }
        }
        Expr::Binary(op, lhs, rhs) => eval_binary_expr(op, lhs, rhs, ctx, env, host),
        Expr::Ternary(cond, then_expr, else_expr) => {
            let value = eval_expr(cond, ctx, env, host)?;
            if condition(&value)? {
                eval_expr(then_expr, ctx, env, host)
            } else {
                eval_expr(else_expr, ctx, env, host)
            }
        }
        Expr::Index(target, index) => {
            let target = eval_expr(target, ctx, env, host)?;
            let index = eval_expr(index, ctx, env, host)?;
            index_value(&target, &index)
        }
        Expr::Slice(target, start, end) => {
            let target = eval_expr(target, ctx, env, host)?;
            let start = match start {
                Some(expr) => Some(eval_expr(expr, ctx, env, host)?.as_number()?),
                None => None,
            };
            let end = match end {
                Some(expr) => Some(eval_expr(expr, ctx, env, host)?.as_number()?),
                None => None,
            };
            slice_value(&target, start, end)
        }
        Expr::Attr(base, name) => {
            if let Expr::Var(alias) = base.as_ref() {
                if let Some(class) = resolve_class(alias, ctx, env) {
                    return class_member_value(&class, name, ctx);
                }
                if let Some(item) = resolve_enum(alias, ctx, env) {
                    return item
                        .values
                        .iter()
                        .find(|(member, _)| member == name)
                        .map(|(_, value)| Value::Number(*value))
                        .ok_or_else(|| {
                            VimError::new(
                                "E1428",
                                format!("Member not found on enum {}: {name}", item.name),
                            )
                        });
                }
            }
            if let Some(value) = module_member(base, name, ctx, env)? {
                return Ok(value);
            }
            let value = eval_expr(base, ctx, env, host)?;
            if let Some(class) = env.vim9.object_class(&value) {
                let member = class
                    .def
                    .members
                    .iter()
                    .find(|member| &member.name == name)
                    .ok_or_else(|| {
                        VimError::new(
                            "E1326",
                            format!("Member not found on object \"{}\": {name}", class.def.name),
                        )
                    })?;
                if member.name.starts_with('_')
                    && ctx.current_class.as_deref() != Some(class.def.canonical.as_str())
                {
                    return Err(VimError::new(
                        "E1332",
                        format!("Cannot access protected member: {name}"),
                    ));
                }
                if let Value::Dict(dict) = &value {
                    return Ok(dict.borrow().get(name).cloned().unwrap_or(Value::Null));
                }
            }
            match &value {
                Value::Dict(dict) => dict.borrow().get(name).cloned().ok_or_else(|| {
                    VimError::new("E716", format!("Key not present in Dictionary: \"{name}\""))
                }),
                other => Err(VimError::new(
                    "E715",
                    format!("Cannot read member from a {} value", other.type_name()),
                )),
            }
        }
        Expr::Call(callee, args) => eval_call(callee, args, ctx, env, host),
        Expr::Method(receiver, name, args) => {
            let this = eval_expr(receiver, ctx, env, host)?;
            let mut values = vec![this];
            for arg in args {
                values.push(eval_expr(arg, ctx, env, host)?);
            }
            call_named(name, values, ctx, env, host)
        }
        Expr::Lambda(def) => {
            let mut captured = HashMap::new();
            if let Some(frame) = env.frames.last() {
                for (k, v) in &frame.locals {
                    captured.insert(k.clone(), v.clone());
                }
                for (k, v) in &frame.args {
                    captured.insert(k.clone(), v.clone());
                }
            }
            if let Some(script) = env.scripts.get(ctx.sid) {
                for (k, v) in script {
                    captured.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            let lambda = Vim9Lambda {
                def: def.clone(),
                captured,
                sid: ctx.sid,
            };
            Ok(Value::Funcref(Rc::new(FuncRef {
                name: "<lambda>".to_string(),
                partial: Vec::new(),
                dict: None,
                lambda: None,
                vim9_lambda: Some(Rc::new(lambda)),
            })))
        }
    }
}

fn module_member(base: &Expr, name: &str, ctx: &Ctx, env: &Env) -> Result<Option<Value>, VimError> {
    let Expr::Var(alias) = base else {
        return Ok(None);
    };
    let Some(binding) = env.vim9.import_binding(ctx.sid, alias) else {
        return Ok(None);
    };
    match binding {
        ImportBinding::Module(module) => {
            if module.vars.contains_key(name) {
                let value = env
                    .scripts
                    .get(module.sid)
                    .and_then(|map| map.get(name))
                    .cloned()
                    .ok_or_else(|| VimError::new("E716", format!("Variable not found: {name}")))?;
                return Ok(Some(value));
            }
            Err(VimError::new(
                "E716",
                format!("Key not present in script module: {name}"),
            ))
        }
        ImportBinding::Autoload(prefix) => Err(VimError::new(
            "E1048",
            format!("Cannot use autoload module member {prefix}#{name} as a value"),
        )),
        ImportBinding::Class(_) | ImportBinding::Enum(_) | ImportBinding::Alias(_) => Ok(None),
        ImportBinding::Func(_) | ImportBinding::Var { .. } => Ok(None),
    }
}

fn get_variable(
    name: &str,
    ctx: &Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    if name == "b:" {
        return host.buffer_var_dict().ok_or_else(|| undefined(name));
    }
    if name == "w:" {
        return host.window_var_dict().ok_or_else(|| undefined(name));
    }
    if name == "t:" {
        return host.tab_var_dict().ok_or_else(|| undefined(name));
    }
    if let Some(rest) = name.strip_prefix("b:") {
        return host.buffer_var_get(rest).ok_or_else(|| undefined(name));
    }
    if let Some(rest) = name.strip_prefix("w:") {
        return host.window_var_get(rest).ok_or_else(|| undefined(name));
    }
    if let Some(rest) = name.strip_prefix("t:") {
        return host.tab_var_get(rest).ok_or_else(|| undefined(name));
    }
    if let Some(rest) = name.strip_prefix("g:") {
        if let Some(value) = env.globals.get(rest) {
            return Ok(value.clone());
        }
        // Vim exposes global functions through the g: namespace too, which is
        // how 'g:FuncOne' is used as a funcref in the Vim9 tests.
        if env.vim9.funcs.contains_key(rest) {
            return Ok(Value::Funcref(Rc::new(FuncRef::new(rest))));
        }
        return Err(undefined(name));
    }
    if let Some(rest) = name.strip_prefix("v:") {
        return env.vvars.get(rest).cloned().ok_or_else(|| undefined(name));
    }
    if let Some(rest) = name.strip_prefix("s:") {
        return env
            .scripts
            .get(ctx.sid)
            .and_then(|map| map.get(rest))
            .cloned()
            .ok_or_else(|| undefined(name));
    }
    if let Some(rest) = name.strip_prefix("l:") {
        return env
            .frames
            .last()
            .and_then(|frame| frame.locals.get(rest))
            .cloned()
            .ok_or_else(|| undefined(name));
    }
    if let Some(rest) = name.strip_prefix("a:") {
        return env
            .frames
            .last()
            .and_then(|frame| frame.args.get(rest))
            .cloned()
            .ok_or_else(|| undefined(name));
    }
    if let Some(value) = env.vvars.get(name) {
        if name.starts_with("null_") {
            return Ok(value.clone());
        }
    }
    if !name.contains(':') {
        if let Some(class) = resolve_class(name, ctx, env) {
            return Ok(Value::Funcref(Rc::new(FuncRef::new(format!(
                "<class>{}",
                class.def.canonical
            )))));
        }
        if let Some(item) = resolve_enum(name, ctx, env) {
            return Ok(Value::Funcref(Rc::new(FuncRef::new(format!(
                "<enum>{}",
                item.canonical
            )))));
        }
    }
    if let Some(frame) = env.frames.last() {
        if let Some(value) = frame.locals.get(name) {
            return Ok(value.clone());
        }
    }
    if let Some(value) = env.scripts.get(ctx.sid).and_then(|map| map.get(name)) {
        return Ok(value.clone());
    }
    if let Some(ImportBinding::Var { sid, name, .. }) = env.vim9.import_binding(ctx.sid, name) {
        if let Some(value) = env.scripts.get(sid).and_then(|map| map.get(&name)) {
            return Ok(value.clone());
        }
    }
    if let Some(canonical) = &ctx.current_class {
        if let Some(class) = env.vim9.class(canonical) {
            if class
                .def
                .members
                .iter()
                .any(|member| member.static_member && member.name == name)
            {
                return Ok(class
                    .statics
                    .borrow()
                    .get(name)
                    .cloned()
                    .unwrap_or(Value::Null));
            }
        }
    }
    Err(undefined(name))
}

fn undefined(name: &str) -> VimError {
    VimError::new("E1001", format!("Variable not found: {name}"))
}

fn eval_call(
    callee: &Expr,
    args: &[Expr],
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(eval_expr(arg, ctx, env, host)?);
    }
    match callee {
        Expr::Var(name) => call_named(name, values, ctx, env, host),
        Expr::Attr(base, member) => {
            if let Expr::Var(alias) = base.as_ref() {
                if let Some(class) = resolve_class(alias, ctx, env) {
                    if member == "new" {
                        return new_object(&class, values, ctx, env, host);
                    }
                    if class.def.static_methods.iter().any(|name| name == member) {
                        let method = class
                            .def
                            .methods
                            .iter()
                            .find(|method| &method.name == member)
                            .cloned()
                            .ok_or_else(|| {
                                VimError::new(
                                    "E1326",
                                    format!(
                                        "Method not found on class {}: {member}",
                                        class.def.name
                                    ),
                                )
                            })?;
                        return call_method(&method, class.clone(), None, values, ctx, env, host);
                    }
                    if class
                        .def
                        .methods
                        .iter()
                        .any(|method| &method.name == member)
                    {
                        return Err(VimError::new(
                            "E1318",
                            format!("Cannot call non-static method using the class name: {member}"),
                        ));
                    }
                    return Err(VimError::new(
                        "E1326",
                        format!("Method not found on class {}: {member}", class.def.name),
                    ));
                }
            }
            if let Expr::Var(alias) = base.as_ref() {
                if let Some(binding) = env.vim9.import_binding(ctx.sid, alias) {
                    match binding {
                        ImportBinding::Module(module) => {
                            if let Some(canonical) = module.funcs.get(member) {
                                return call_named(canonical, values, ctx, env, host);
                            }
                            return Err(VimError::new(
                                "E117",
                                format!("Unknown function: {alias}.{member}"),
                            ));
                        }
                        ImportBinding::Autoload(prefix) => {
                            return call_named(
                                &format!("{prefix}#{member}"),
                                values,
                                ctx,
                                env,
                                host,
                            );
                        }
                        ImportBinding::Func(canonical) => {
                            return call_named(&canonical, values, ctx, env, host);
                        }
                        ImportBinding::Var { .. } => {
                            // fall through to a dictionary member call
                        }
                        ImportBinding::Class(_)
                        | ImportBinding::Enum(_)
                        | ImportBinding::Alias(_) => {
                            return Err(VimError::new(
                                "E1326",
                                format!("Member not found: {member}"),
                            ));
                        }
                    }
                }
            }
            let base_value = eval_expr(base, ctx, env, host)?;
            if let Some(class) = env.vim9.object_class(&base_value) {
                if let Some(method) = class
                    .def
                    .methods
                    .iter()
                    .find(|method| &method.name == member)
                    .cloned()
                {
                    return call_method(
                        &method,
                        class.clone(),
                        Some(base_value.clone()),
                        values,
                        ctx,
                        env,
                        host,
                    );
                }
                if class.def.members.iter().any(|m| &m.name == member) {
                    return Err(VimError::new("E1318", format!("Not a method: {member}")));
                }
                return Err(VimError::new(
                    "E1326",
                    format!("Method not found on object {}: {member}", class.def.name),
                ));
            }
            match &base_value {
                Value::Dict(dict) => {
                    let member_value = dict.borrow().get(member).cloned().ok_or_else(|| {
                        VimError::new(
                            "E716",
                            format!("Key not present in Dictionary: \"{member}\""),
                        )
                    })?;
                    crate::eval::call_value(&member_value, values, env, host)
                }
                other => Err(VimError::new(
                    "E1085",
                    format!("Not a callable member on {}", other.type_name()),
                )),
            }
        }
        other => {
            let target = eval_expr(other, ctx, env, host)?;
            crate::eval::call_value(&target, values, env, host)
        }
    }
}

fn resolve_class(name: &str, ctx: &Ctx, env: &Env) -> Option<Rc<VimClassState>> {
    if let Some(ImportBinding::Class(canonical)) = env.vim9.import_binding(ctx.sid, name) {
        return env.vim9.class(&canonical);
    }
    let mut candidates = vec![name.to_string(), format!("<SNR>{}_{}", ctx.sid, name)];
    if !ctx.prefix.is_empty() {
        candidates.push(format!("{}#{}", ctx.prefix, name));
    }
    for candidate in candidates {
        if let Some(class) = env.vim9.class(&candidate) {
            return Some(class);
        }
    }
    None
}

fn resolve_enum(name: &str, ctx: &Ctx, env: &Env) -> Option<Rc<VimEnum>> {
    if let Some(ImportBinding::Enum(canonical)) = env.vim9.import_binding(ctx.sid, name) {
        return env.vim9.enums.get(&canonical).cloned();
    }
    let mut candidates = vec![name.to_string(), format!("<SNR>{}_{}", ctx.sid, name)];
    if !ctx.prefix.is_empty() {
        candidates.push(format!("{}#{}", ctx.prefix, name));
    }
    for candidate in candidates {
        if let Some(item) = env.vim9.enums.get(&candidate) {
            return Some(item.clone());
        }
    }
    None
}

fn class_member_value(class: &VimClassState, name: &str, ctx: &Ctx) -> Result<Value, VimError> {
    let member = class
        .def
        .members
        .iter()
        .find(|member| member.name == name)
        .ok_or_else(|| {
            VimError::new(
                "E1326",
                format!("Member not found on class {}: {name}", class.def.name),
            )
        })?;
    if !member.static_member {
        return Err(VimError::new(
            "E1376",
            format!("Cannot access object member using the class name: {name}"),
        ));
    }
    if member.name.starts_with('_')
        && ctx.current_class.as_deref() != Some(class.def.canonical.as_str())
    {
        return Err(VimError::new(
            "E1332",
            format!("Cannot access protected member: {name}"),
        ));
    }
    Ok(class
        .statics
        .borrow()
        .get(name)
        .cloned()
        .unwrap_or(Value::Null))
}

fn call_named(
    name: &str,
    args: Vec<Value>,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    // A method of the class currently executing can be called without 'this.'.
    if let Some(canonical) = ctx.current_class.clone() {
        if let Some(class) = env.vim9.class(&canonical) {
            if let Some(method) = class
                .def
                .methods
                .iter()
                .find(|method| method.name == name)
                .cloned()
            {
                let this = env
                    .frames
                    .last()
                    .and_then(|frame| frame.locals.get("this"))
                    .cloned();
                return call_method(&method, class, this, args, ctx, env, host);
            }
        }
    }
    // A local Funcref shadows the function name.
    if let Some(frame) = env.frames.last() {
        if let Some(value @ Value::Funcref(_)) = frame.locals.get(name) {
            return crate::eval::call_value(&value.clone(), args, env, host);
        }
    }
    if let Some(binding) = env.vim9.import_binding(ctx.sid, name) {
        match binding {
            ImportBinding::Func(canonical) => {
                return call_resolved(&canonical, args, ctx, env, host);
            }
            ImportBinding::Var { .. } => {
                let value = get_variable(name, ctx, env, host)?;
                return crate::eval::call_value(&value, args, env, host);
            }
            _ => {}
        }
    }
    let candidate = format!("<SNR>{}_{}", ctx.sid, name);
    if env.vim9.funcs.contains_key(&candidate) {
        return call_resolved(&candidate, args, ctx, env, host);
    }
    if !ctx.prefix.is_empty() {
        let autoload = format!("{}#{}", ctx.prefix, name);
        if env.vim9.funcs.contains_key(&autoload) {
            return call_resolved(&autoload, args, ctx, env, host);
        }
    }
    if env.vim9.funcs.contains_key(name) {
        return call_resolved(name, args, ctx, env, host);
    }
    // Legacy functions / builtins.
    crate::eval::call_function(name, args, env, host)
}

fn call_resolved(
    canonical: &str,
    args: Vec<Value>,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    if let Some(func) = env.vim9.function(canonical) {
        return call_vim9(&func, args, env, host);
    }
    let _ = ctx;
    crate::eval::call_function(canonical, args, env, host)
}

/// Call a compiled Vim9 function.
pub fn call_vim9(
    func: &Rc<Vim9Func>,
    args: Vec<Value>,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    call_vim9_full(func, args, None, None, env, host)
}

/// Call a class method with 'this' bound and the class context recorded so
/// protected members can be accessed from inside the class.
fn call_method(
    func: &Rc<Vim9Func>,
    class: Rc<VimClassState>,
    this: Option<Value>,
    args: Vec<Value>,
    _caller: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    call_vim9_full(
        func,
        args,
        this,
        Some(class.def.canonical.clone()),
        env,
        host,
    )
}

/// Create an object from a class: initialize members, register the object and
/// run the constructor.
fn new_object(
    class: &Rc<VimClassState>,
    args: Vec<Value>,
    _caller: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    let mut dict = VimDict::new();
    let mut class_ctx = Ctx::new(class.def.sid, "", "", true);
    class_ctx.script_types = env
        .vim9
        .script_types
        .get(&class.def.sid)
        .cloned()
        .unwrap_or_default();
    for member in &class.def.members {
        if member.static_member {
            continue;
        }
        let value = match &member.default {
            Some(expr) => eval_expr(expr, &mut class_ctx, env, host)?,
            None => default_value(Some(&member.ty)),
        };
        dict.insert(member.name.clone(), value);
    }
    let object = Value::dict(dict);
    env.vim9.register_object(&object, class.clone());
    if let Some(constructor) = &class.def.constructor {
        call_method(
            constructor,
            class.clone(),
            Some(object.clone()),
            args,
            &mut class_ctx,
            env,
            host,
        )?;
    } else if !args.is_empty() {
        return Err(VimError::new(
            "E118",
            format!(
                "Too many arguments for class {}: no new() method",
                class.def.name
            ),
        ));
    }
    Ok(object)
}

fn call_vim9_full(
    func: &Rc<Vim9Func>,
    args: Vec<Value>,
    this: Option<Value>,
    current_class: Option<String>,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    if env.call_depth >= host.maxfuncdepth() {
        return Err(VimError::new(
            "E132",
            "Function call depth is higher than 'maxfuncdepth'".to_string(),
        ));
    }
    let mut frame = Frame {
        args: HashMap::new(),
        locals: HashMap::new(),
        sid: func.sid,
    };
    let mut types = HashMap::new();
    let mut index = 0usize;
    for param in &func.params {
        let value = if param.varargs {
            Value::list(args[index..].to_vec())
        } else if let Some(value) = args.get(index) {
            value.clone()
        } else if let Some(default) = &param.default {
            // Defaults may refer to earlier parameters: build a temporary ctx.
            let mut temp = Ctx::new(func.sid, "", "", false);
            temp.types = types.clone();
            temp.script_types = env
                .vim9
                .script_types
                .get(&func.sid)
                .cloned()
                .unwrap_or_default();
            env.frames.push(frame.clone());
            let result = eval_expr(default, &mut temp, env, host);
            env.frames.pop();
            result?
        } else {
            return Err(VimError::new(
                "E119",
                format!(
                    "Not enough arguments for function {}: expected {}",
                    func.name,
                    func.params.len()
                ),
            ));
        };
        if !value_is(&param.ty, &value) {
            return Err(VimError::new(
                "E1013",
                format!(
                    "Argument {} ({}): expected {} but got {}",
                    index + 1,
                    param.name,
                    param.ty.name(),
                    value.type_name()
                ),
            ));
        }
        frame.locals.insert(param.name.clone(), value.clone());
        // Named parameters are reachable with a: too (legacy interop).
        frame.args.insert(param.name.clone(), value);
        types.insert(param.name.clone(), param.ty.clone());
        index += 1;
    }
    if !func.varargs && args.len() > func.params.len() {
        return Err(VimError::new(
            "E118",
            format!(
                "Too many arguments for function {}: expected {}",
                func.name,
                func.params.len()
            ),
        ));
    }
    if let Some(object) = &this {
        frame.locals.insert("this".to_string(), object.clone());
    }
    if func.varargs {
        frame
            .args
            .insert("000".to_string(), Value::list(args[index..].to_vec()));
        frame.args.insert(
            "0".to_string(),
            Value::Number(args.len().saturating_sub(index) as i64),
        );
    }
    env.call_depth += 1;
    env.frames.push(frame);
    let saved_sid = env.current_sid;
    env.current_sid = func.sid;
    let mut ctx = Ctx::new(func.sid, "", "", false);
    ctx.prefix = env
        .vim9
        .script_prefix
        .get(&func.sid)
        .cloned()
        .unwrap_or_default();
    ctx.types = types;
    ctx.script_types = env
        .vim9
        .script_types
        .get(&func.sid)
        .cloned()
        .unwrap_or_default();
    ctx.return_type = func.ret.clone();
    ctx.current_class = current_class;
    let result = exec_stmts(&func.body, &mut ctx, env, host).map_err(|error| VimError {
        code: error.code,
        message: format!("{} (in {})", error.message, func.name),
        abort: error.abort,
    });
    env.current_sid = saved_sid;
    env.frames.pop();
    env.call_depth -= 1;
    match result? {
        Flow::Return(value) => Ok(value),
        Flow::Next => Ok(Value::Null),
        Flow::Throw(value) => {
            env.vvars.insert("exception".to_string(), value.clone());
            env.pending_throw = Some(value.clone());
            Err(VimError::aborting(
                "E605",
                format!("Exception not caught: {}", value.to_string_value()),
            ))
        }
        Flow::Break | Flow::Continue => Err(VimError::new(
            "E587",
            format!(":break/:continue outside a loop in {}", func.name),
        )),
    }
}

/// Call a compiled Vim9 lambda value.
pub fn call_vim9_lambda(
    lambda: &Vim9Lambda,
    args: Vec<Value>,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    if env.call_depth >= host.maxfuncdepth() {
        return Err(VimError::new(
            "E132",
            "Function call depth is higher than 'maxfuncdepth'".to_string(),
        ));
    }
    let mut frame = Frame {
        args: HashMap::new(),
        locals: lambda.captured.clone(),
        sid: lambda.sid,
    };
    let mut types = HashMap::new();
    for (i, param) in lambda.def.params.iter().enumerate() {
        let value = args.get(i).cloned().unwrap_or(Value::Null);
        if !value_is(&param.ty, &value) {
            return Err(VimError::new(
                "E1013",
                format!(
                    "Argument {} ({}): expected {} but got {}",
                    i + 1,
                    param.name,
                    param.ty.name(),
                    value.type_name()
                ),
            ));
        }
        frame.locals.insert(param.name.clone(), value);
        types.insert(param.name.clone(), param.ty.clone());
    }
    if lambda.def.params.is_empty() && !args.is_empty() {
        return Err(VimError::new(
            "E118",
            "Too many arguments for <lambda>".to_string(),
        ));
    }
    env.call_depth += 1;
    env.frames.push(frame);
    let saved_sid = env.current_sid;
    env.current_sid = lambda.sid;
    let mut ctx = Ctx::new(lambda.sid, "", "", false);
    ctx.prefix = env
        .vim9
        .script_prefix
        .get(&lambda.sid)
        .cloned()
        .unwrap_or_default();
    ctx.types = types;
    ctx.script_types = env
        .vim9
        .script_types
        .get(&lambda.sid)
        .cloned()
        .unwrap_or_default();
    let result = if let Some(expr) = &lambda.def.expr_body {
        eval_expr(expr, &mut ctx, env, host)
    } else {
        match exec_stmts(&lambda.def.body, &mut ctx, env, host) {
            Ok(Flow::Return(value)) | Ok(Flow::Throw(value)) => Ok(value),
            Ok(_) => Ok(Value::Null),
            Err(error) => Err(error),
        }
    };
    env.current_sid = saved_sid;
    env.frames.pop();
    env.call_depth -= 1;
    result
}

fn eval_binary_expr(
    op: &str,
    lhs: &Expr,
    rhs: &Expr,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> Result<Value, VimError> {
    if op == "??" {
        let left = eval_expr(lhs, ctx, env, host)?;
        if truthy_any(&left) {
            return Ok(left);
        }
        return eval_expr(rhs, ctx, env, host);
    }
    if op == "&&" || op == "||" {
        let left = eval_expr(lhs, ctx, env, host)?;
        let left = condition(&left)?;
        if op == "&&" && !left {
            return Ok(Value::Bool(false));
        }
        if op == "||" && left {
            return Ok(Value::Bool(true));
        }
        let right = eval_expr(rhs, ctx, env, host)?;
        let right = condition(&right)?;
        return Ok(Value::Bool(right));
    }
    let left = eval_expr(lhs, ctx, env, host)?;
    let right = eval_expr(rhs, ctx, env, host)?;
    apply_binary(op, left, right, 0)
}

fn apply_binary(op: &str, left: Value, right: Value, line: usize) -> Result<Value, VimError> {
    let base = op.trim_end_matches(['#', '?']);
    match base {
        ".." => Ok(Value::String(format!(
            "{}{}",
            vim9_string(&left),
            vim9_string(&right)
        ))),
        "==" => Ok(Value::Bool(strict_equal(&left, &right))),
        "!=" => Ok(Value::Bool(!strict_equal(&left, &right))),
        "is" => Ok(Value::Bool(identity_equal(&left, &right))),
        "isnot" => Ok(Value::Bool(!identity_equal(&left, &right))),
        "=~" | "!~" => {
            let pattern = match &right {
                Value::String(s) => s.clone(),
                other => vim9_string(other),
            };
            let text = vim9_string(&left);
            let hit = regex_mvp::is_match(&pattern, &text, base == "!~" && false);
            Ok(Value::Bool(if base == "=~" { hit } else { !hit }))
        }
        ">" | ">=" | "<" | "<=" => {
            let ordering = compare_values(&left, &right)?;
            Ok(Value::Bool(match base {
                ">" => ordering > 0,
                ">=" => ordering >= 0,
                "<" => ordering < 0,
                _ => ordering <= 0,
            }))
        }
        "+" | "-" | "*" | "/" | "%" => {
            if base == "+" {
                if let (Value::List(a), Value::List(b)) = (&left, &right) {
                    let mut items = a.borrow().clone();
                    items.extend(b.borrow().iter().cloned());
                    return Ok(Value::list(items));
                }
                if let (Value::Blob(a), Value::Blob(b)) = (&left, &right) {
                    let mut bytes = a.borrow().clone();
                    bytes.extend(b.borrow().iter().copied());
                    return Ok(Value::blob(bytes));
                }
            }
            if matches!((&left, &right), (Value::Float(_), _) | (_, Value::Float(_))) {
                let a = left.to_float()?;
                let b = right.to_float()?;
                let value = match base {
                    "+" => a + b,
                    "-" => a - b,
                    "*" => a * b,
                    "/" => {
                        if b == 0.0 {
                            return Err(VimError::new("E1154", "Divide by zero"));
                        }
                        a / b
                    }
                    _ => {
                        if b == 0.0 {
                            return Err(VimError::new("E1154", "Divide by zero"));
                        }
                        a % b
                    }
                };
                return Ok(Value::Float(value));
            }
            let a = integer(&left, line)?;
            let b = integer(&right, line)?;
            let value = match base {
                "+" => a.wrapping_add(b),
                "-" => a.wrapping_sub(b),
                "*" => a.wrapping_mul(b),
                "/" => {
                    if b == 0 {
                        return Err(VimError::new("E1154", "Divide by zero"));
                    }
                    a / b
                }
                _ => {
                    if b == 0 {
                        return Err(VimError::new("E1154", "Divide by zero"));
                    }
                    a % b
                }
            };
            Ok(Value::Number(value))
        }
        "<<" | ">>" => {
            let a = integer(&left, line)?;
            let b = integer(&right, line)?;
            if b < 0 {
                return Err(VimError::new("E1709", "Shift count must be positive"));
            }
            let value = if b > 63 {
                0
            } else if base == "<<" {
                ((a as u64) << b) as i64
            } else {
                ((a as u64) >> b) as i64
            };
            Ok(Value::Number(value))
        }
        other => Err(VimError::new("E15", format!("Invalid operator: {other}"))),
    }
}

fn integer(value: &Value, line: usize) -> Result<i64, VimError> {
    match value {
        Value::Number(n) => Ok(*n),
        Value::Bool(b) => Ok(i64::from(*b)),
        other => Err(VimError::new(
            "E1012",
            format!(
                "Expected number but got {} (line {line})",
                other.type_name()
            ),
        )),
    }
}

fn compare_values(left: &Value, right: &Value) -> Result<i64, VimError> {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => Ok((a - b).signum()),
        (Value::Float(a), Value::Float(b)) => Ok(if a < b {
            -1
        } else if a > b {
            1
        } else {
            0
        }),
        (Value::Number(a), Value::Float(b)) => Ok(if (*a as f64) < *b {
            -1
        } else if (*a as f64) > *b {
            1
        } else {
            0
        }),
        (Value::Float(a), Value::Number(b)) => Ok(if *a < (*b as f64) {
            -1
        } else if *a > (*b as f64) {
            1
        } else {
            0
        }),
        (Value::String(a), Value::String(b)) => Ok(match a.cmp(b) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        (Value::Bool(a), Value::Bool(b)) => Ok((i64::from(*a) - i64::from(*b)).signum()),
        _ => Err(VimError::new(
            "E1012",
            format!(
                "Cannot compare {} with {}",
                left.type_name(),
                right.type_name()
            ),
        )),
    }
}

fn identity_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::List(a), Value::List(b)) => Rc::ptr_eq(a, b),
        (Value::Dict(a), Value::Dict(b)) => Rc::ptr_eq(a, b),
        (Value::Blob(a), Value::Blob(b)) => Rc::ptr_eq(a, b),
        (Value::Funcref(a), Value::Funcref(b)) => Rc::ptr_eq(a, b),
        (Value::Number(_), Value::Number(_))
        | (Value::Float(_), Value::Float(_))
        | (Value::String(_), Value::String(_))
        | (Value::Bool(_), Value::Bool(_)) => values_equal(left, right, 0),
        _ => false,
    }
}

fn strict_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) => true,
        (Value::List(a), Value::List(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| strict_equal(x, y))
        }
        (Value::Dict(a), Value::Dict(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|other| strict_equal(v, other)))
        }
        (Value::Blob(a), Value::Blob(b)) => *a.borrow() == *b.borrow(),
        (Value::Funcref(a), Value::Funcref(b)) => Rc::ptr_eq(a, b),
        (Value::Number(a), Value::Bool(b)) | (Value::Bool(b), Value::Number(a)) => (*a != 0) == *b,
        _ => false,
    }
}

fn condition(value: &Value) -> Result<bool, VimError> {
    match value {
        Value::Bool(b) => Ok(*b),
        // Many builtins return Number for a boolean result in the legacy
        // layer; accepting 0/1 keeps them usable in Vim9 conditions.
        Value::Number(0) => Ok(false),
        Value::Number(1) => Ok(true),
        Value::Number(_) => Err(VimError::new(
            "E1030",
            format!("Using a Number as a Bool: {}", vim9_string(value)),
        )),
        Value::Null => Ok(false),
        other => Err(VimError::new(
            "E1030",
            format!(
                "Using a {} as a Bool: {}",
                other.type_name(),
                vim9_string(other)
            ),
        )),
    }
}

fn truthy_any(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => *n != 0,
        Value::Float(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::List(l) => !l.borrow().is_empty(),
        Value::Dict(d) => !d.borrow().is_empty(),
        Value::Blob(b) => !b.borrow().is_empty(),
        Value::Funcref(_) => true,
        Value::Null => false,
    }
}

fn index_value(target: &Value, index: &Value) -> Result<Value, VimError> {
    match target {
        Value::List(list) => {
            let raw = index.as_number()?;
            let items = list.borrow();
            let idx = resolve_index(raw, items.len())?;
            Ok(items[idx].clone())
        }
        Value::Dict(dict) => {
            let key = index.to_string_value();
            dict.borrow().get(&key).cloned().ok_or_else(|| {
                VimError::new("E716", format!("Key not present in Dictionary: \"{key}\""))
            })
        }
        Value::Blob(blob) => {
            let raw = index.as_number()?;
            let bytes = blob.borrow();
            let idx = resolve_index(raw, bytes.len())?;
            Ok(Value::Number(i64::from(bytes[idx])))
        }
        Value::String(s) => {
            let raw = index.as_number()?;
            let chars: Vec<char> = s.chars().collect();
            let idx = resolve_index(raw, chars.len())?;
            Ok(Value::string(chars[idx].to_string()))
        }
        other => Err(VimError::new(
            "E689",
            format!(
                "Can only index a List, Dictionary, Blob or String, not {}",
                other.type_name()
            ),
        )),
    }
}

fn resolve_index(raw: i64, len: usize) -> Result<usize, VimError> {
    let idx = if raw < 0 { len as i64 + raw } else { raw };
    if idx < 0 || idx as usize >= len {
        Err(VimError::new(
            "E684",
            format!("List index out of range: {raw}"),
        ))
    } else {
        Ok(idx as usize)
    }
}

fn slice_value(target: &Value, start: Option<i64>, end: Option<i64>) -> Result<Value, VimError> {
    match target {
        Value::List(list) => {
            let items = list.borrow();
            let (s, e) = slice_bounds(start, end, items.len());
            Ok(Value::list(items[s..e].to_vec()))
        }
        Value::String(text) => {
            let chars: Vec<char> = text.chars().collect();
            let (s, e) = slice_bounds(start, end, chars.len());
            Ok(Value::string(chars[s..e].iter().collect::<String>()))
        }
        Value::Blob(blob) => {
            let bytes = blob.borrow();
            let (s, e) = slice_bounds(start, end, bytes.len());
            Ok(Value::blob(bytes[s..e].to_vec()))
        }
        other => Err(VimError::new(
            "E1098",
            format!("Cannot slice a {} value", other.type_name()),
        )),
    }
}

fn slice_bounds(start: Option<i64>, end: Option<i64>, len: usize) -> (usize, usize) {
    let len_i = len as i64;
    let adjust = |n: i64| if n < 0 { len_i + n } else { n };
    let s = start.map(adjust).unwrap_or(0).clamp(0, len_i);
    let e = end.map(adjust).unwrap_or(len_i - 1);
    let e_inclusive = e.clamp(-1, len_i - 1);
    if e_inclusive < s {
        (s as usize, s as usize)
    } else {
        (s as usize, (e_inclusive + 1) as usize)
    }
}

fn vim9_string(value: &Value) -> String {
    match value {
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string_value(),
    }
}

// ---------------------------------------------------------------------------
// Ex command handling inside Vim9 functions
// ---------------------------------------------------------------------------

fn exec_ex_line(
    text: &str,
    line: usize,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    let trimmed = text.trim();
    let word = first_word(trimmed);
    match word.as_str() {
        "call" => {
            let rest = trimmed[4..].trim();
            let expr = crate::vim9::parser::parse_expression(rest)?;
            eval_expr(&expr, ctx, env, host)?;
            Ok(())
        }
        "execute" | "exe" => {
            let rest = trimmed[word.len()..].trim();
            let values = eval_arg_list(rest, ctx, env, host)?;
            let command = values.join(" ");
            host.execute_command(&command, env)
                .unwrap_or_else(|| Err(VimError::new("E492", "Not an editor command")))
        }
        "echo" | "echomsg" | "echon" | "echoerr" => {
            let rest = trimmed[word.len()..].trim();
            let values = eval_arg_list(rest, ctx, env, host)?;
            let message = values.join(" ");
            host.echo_message(&message, word == "echoerr")
                .unwrap_or(Ok(()))
        }
        "eval" => {
            let rest = trimmed[4..].trim();
            let expr = crate::vim9::parser::parse_expression(rest)?;
            eval_expr(&expr, ctx, env, host)?;
            Ok(())
        }
        "silent" | "silent!" | "noautocmd" => host
            .execute_command(trimmed, env)
            .unwrap_or_else(|| Err(VimError::new("E492", "Not an editor command"))),
        _ => host.execute_command(trimmed, env).unwrap_or_else(|| {
            Err(VimError::new(
                "E492",
                format!("Not an editor command: {trimmed} (line {line})"),
            ))
        }),
    }
}

fn eval_arg_list(
    text: &str,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<Vec<String>> {
    let mut rest = text;
    let mut out = Vec::new();
    loop {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            break;
        }
        let (expr, consumed) = parse_expression_prefix(trimmed)?;
        let value = eval_expr(&expr, ctx, env, host)?;
        out.push(vim9_string(&value));
        if consumed == 0 {
            break;
        }
        let byte_off = trimmed
            .char_indices()
            .nth(consumed)
            .map(|(o, _)| o)
            .unwrap_or(trimmed.len());
        rest = &trimmed[byte_off..];
    }
    Ok(out)
}

fn first_word(text: &str) -> String {
    let mut word = String::new();
    for c in text.trim_start().chars() {
        if c.is_ascii_alphabetic() || c == '!' {
            word.push(c.to_ascii_lowercase());
        } else {
            break;
        }
    }
    word
}

// ---------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------

/// Handle one :import statement for the current script.
pub fn handle_import(
    kind: &ImportKind,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    match kind {
        ImportKind::Export(inner) => {
            let _ = inner;
            Ok(())
        }
        ImportKind::Script { path, as_name } => {
            import_script(path, as_name.as_deref(), false, ctx, env, host)
        }
        ImportKind::Autoload { path, as_name } => {
            import_script(path, as_name.as_deref(), true, ctx, env, host)
        }
        ImportKind::From {
            names,
            path,
            autoload,
        } => import_from(names, path, *autoload, ctx, env, host),
    }
}

fn import_script(
    path: &str,
    alias: Option<&str>,
    autoload: bool,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    if autoload {
        let prefix = autoload_prefix(path);
        let name = alias.unwrap_or(&prefix).to_string();
        env.vim9
            .imports
            .entry(ctx.sid)
            .or_default()
            .insert(name, ImportBinding::Autoload(prefix));
        return Ok(());
    }
    let resolved = resolve_import_path(path, false, ctx, env, host)?;
    let module = load_module(&resolved, env, host)?;
    match alias {
        Some(alias) => {
            env.vim9
                .imports
                .entry(ctx.sid)
                .or_default()
                .insert(alias.to_string(), ImportBinding::Module(module));
        }
        None => {
            for (name, canonical) in &module.funcs {
                env.vim9
                    .imports
                    .entry(ctx.sid)
                    .or_default()
                    .insert(name.clone(), ImportBinding::Func(canonical.clone()));
            }
            for (name, ty) in &module.vars {
                env.vim9.imports.entry(ctx.sid).or_default().insert(
                    name.clone(),
                    ImportBinding::Var {
                        sid: module.sid,
                        name: name.clone(),
                        ty: ty.clone(),
                    },
                );
            }
            for (name, canonical) in &module.classes {
                env.vim9
                    .imports
                    .entry(ctx.sid)
                    .or_default()
                    .insert(name.clone(), ImportBinding::Class(canonical.clone()));
            }
            for (name, canonical) in &module.enums {
                env.vim9
                    .imports
                    .entry(ctx.sid)
                    .or_default()
                    .insert(name.clone(), ImportBinding::Enum(canonical.clone()));
            }
            for (name, canonical) in &module.aliases {
                env.vim9
                    .imports
                    .entry(ctx.sid)
                    .or_default()
                    .insert(name.clone(), ImportBinding::Alias(canonical.clone()));
            }
        }
    }
    Ok(())
}

fn import_from(
    names: &[String],
    path: &str,
    autoload: bool,
    ctx: &mut Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    let resolved = resolve_import_path(path, autoload, ctx, env, host)?;
    let module = load_module(&resolved, env, host)?;
    for name in names {
        if let Some(canonical) = module.funcs.get(name) {
            env.vim9
                .imports
                .entry(ctx.sid)
                .or_default()
                .insert(name.clone(), ImportBinding::Func(canonical.clone()));
        } else if let Some(ty) = module.vars.get(name) {
            env.vim9.imports.entry(ctx.sid).or_default().insert(
                name.clone(),
                ImportBinding::Var {
                    sid: module.sid,
                    name: name.clone(),
                    ty: ty.clone(),
                },
            );
        } else if let Some(canonical) = module.classes.get(name) {
            env.vim9
                .imports
                .entry(ctx.sid)
                .or_default()
                .insert(name.clone(), ImportBinding::Class(canonical.clone()));
        } else if let Some(canonical) = module.enums.get(name) {
            env.vim9
                .imports
                .entry(ctx.sid)
                .or_default()
                .insert(name.clone(), ImportBinding::Enum(canonical.clone()));
        } else if let Some(canonical) = module.aliases.get(name) {
            env.vim9
                .imports
                .entry(ctx.sid)
                .or_default()
                .insert(name.clone(), ImportBinding::Alias(canonical.clone()));
        } else {
            return Err(VimError::new(
                "E1049",
                format!("Importing name {name} not found in {path}"),
            ));
        }
    }
    Ok(())
}

fn load_module(path: &Path, env: &mut Env, host: &mut dyn Host) -> VimResult<Rc<Module>> {
    let key = module_key(&path.to_string_lossy());
    if let Some(module) = env.vim9.modules.get(&key) {
        return Ok(module.clone());
    }
    if let Some(result) = host.source_script(path, env) {
        result?;
    }
    env.vim9.modules.get(&key).cloned().ok_or_else(|| {
        VimError::new(
            "E1048",
            format!("Cannot find script file {} for :import", path.display()),
        )
    })
}

fn resolve_import_path(
    path: &str,
    autoload: bool,
    ctx: &Ctx,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<PathBuf> {
    let _ = env;
    let raw = Path::new(path);
    if path.starts_with("./") || path.starts_with("../") || raw.is_absolute() {
        let base = Path::new(&ctx.script_path)
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            base.join(raw)
        };
        return absolute(&candidate)
            .exists()
            .then(|| absolute(&candidate))
            .ok_or_else(|| VimError::new("E484", format!("Can't open file {path}")));
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(value) = host.option_get("runtimepath") {
        for part in value.to_string_value().split(',') {
            if !part.is_empty() {
                roots.push(PathBuf::from(part));
            }
        }
    }
    for root in roots {
        let candidates = if autoload {
            vec![root.join("autoload").join(path)]
        } else {
            vec![root.join("import").join(path), root.join(path)]
        };
        for candidate in candidates {
            if candidate.is_file() {
                return Ok(absolute(&candidate));
            }
        }
    }
    let base = Path::new(&ctx.script_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let candidate = base.join(path);
    if candidate.is_file() {
        return Ok(absolute(&candidate));
    }
    Err(VimError::new("E484", format!("Can't open file {path}")))
}

fn autoload_prefix(path: &str) -> String {
    let stem = path
        .trim_end_matches(".vim")
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches("autoload/")
        .to_string();
    stem.replace('/', "#")
}

/// Handle an :import line at the Ex layer (legacy scripts may import too).
pub fn run_import_line(
    text: &str,
    sid: usize,
    script_path: &str,
    prefix: &str,
    env: &mut Env,
    host: &mut dyn Host,
) -> VimResult<()> {
    let stmt = crate::vim9::parser::parse_import_line(text, 0)?;
    let Stmt::Import { kind, .. } = stmt else {
        return Err(VimError::new("E1049", "Invalid :import"));
    };
    let mut ctx = Ctx::new(sid, script_path, prefix, true);
    ctx.script_types = env.vim9.script_types.get(&sid).cloned().unwrap_or_default();
    handle_import(&kind, &mut ctx, env, host)
}

/// Register a script module after an external load (used by tests).
pub fn module_for(env: &Env, path: &Path) -> Option<Rc<Module>> {
    env.vim9
        .modules
        .get(&module_key(&absolute(path).to_string_lossy()))
        .cloned()
}

/// Names imported by a script (diagnostics).
pub fn imported_names(env: &Env, sid: usize) -> HashSet<String> {
    env.vim9
        .imports
        .get(&sid)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}
