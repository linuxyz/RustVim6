//! Expression evaluation (reference: eval.c `eval1()`..`eval9()`).

use rvim_core::{VimError, VimResult};

use crate::builtins;
use crate::env::{Env, Host};
use crate::expr::Expr;
use crate::regex_mvp;
use crate::value::{FuncRef, Lambda, Value, ValueResult, VimDict, compare_values, values_equal};

/// Evaluate an expression.
pub fn eval(expr: &Expr, env: &mut Env, host: &mut dyn Host) -> ValueResult {
    match expr {
        Expr::Number(n) => Ok(Value::Number(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::Str(s) => Ok(Value::String(s.clone())),
        Expr::Blob(b) => Ok(Value::blob(b.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),
        Expr::Var(name) => eval_var(name, env, host),
        Expr::Option(name) => eval_option(name, env, host),
        Expr::Env(name) => Ok(Value::String(std::env::var(name).unwrap_or_default())),
        Expr::Register(r) => host
            .host_register(*r)
            .unwrap_or_else(|| Ok(Value::String(String::new()))),
        Expr::List(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(eval(item, env, host)?);
            }
            Ok(Value::list(values))
        }
        Expr::Dict(entries) => {
            let mut dict = VimDict::new();
            for (k, v) in entries {
                let key = eval(k, env, host)?.to_string_value();
                let value = eval(v, env, host)?;
                dict.insert(key, value);
            }
            Ok(Value::dict(dict))
        }
        Expr::Unary(op, inner) => {
            let v = eval(inner, env, host)?;
            eval_unary(op, v)
        }
        Expr::Binary(op, lhs, rhs) => eval_binary(op, lhs, rhs, env, host),
        Expr::Ternary(cond, then_branch, else_branch) => {
            let c = eval(cond, env, host)?;
            if c.truthy() {
                eval(then_branch, env, host)
            } else {
                eval(else_branch, env, host)
            }
        }
        Expr::Index(target, index) => {
            let t = eval(target, env, host)?;
            let i = eval(index, env, host)?;
            eval_index(&t, &i)
        }
        Expr::Slice(target, start, end) => {
            let t = eval(target, env, host)?;
            let s = match start {
                Some(e) => Some(eval(e, env, host)?.as_number()?),
                None => None,
            };
            let e = match end {
                Some(e) => Some(eval(e, env, host)?.as_number()?),
                None => None,
            };
            eval_slice(&t, s, e)
        }
        Expr::Attr(target, name) => {
            let t = eval(target, env, host)?;
            if let Value::Dict(d) = &t {
                return d.borrow().get(name).cloned().ok_or_else(|| {
                    VimError::new("E716", format!("Key not present in Dictionary: \"{name}\""))
                });
            }
            // Not a dict: Vim treats `expr.name` as string concatenation with
            // the variable `name`.
            let rhs = eval_var(name, env, host)?;
            Ok(Value::String(format!(
                "{}{}",
                t.to_string_value(),
                rhs.to_string_value()
            )))
        }
        Expr::Call(callee, args) => {
            // `expr.name(args)`: vim decides between a dict member call and
            // ".name" being string concatenation by looking at the *evaluated*
            // left value — only a Dictionary gets member access (eval.c:6420).
            // So `'a' . strlen('b')` calls strlen and concatenates, while
            // `d.Func(1)` calls the dictionary's function.
            if let Expr::Attr(target, name) = callee.as_ref() {
                let base = eval(target, env, host)?;
                if let Value::Dict(d) = &base {
                    let member = d.borrow().get(name).cloned().ok_or_else(|| {
                        VimError::new("E716", format!("Key not present in Dictionary: \"{name}\""))
                    })?;
                    let values = eval_args(args, env, host)?;
                    return call_value(&member, values, env, host);
                }
                let values = eval_args(args, env, host)?;
                let rhs = call_function(name, values, env, host)?;
                return Ok(Value::String(format!(
                    "{}{}",
                    base.to_string_value(),
                    rhs.to_string_value()
                )));
            }
            let values = eval_args(args, env, host)?;
            // A plain function name is resolved by name (E117 when unknown),
            // not as a variable (E121).
            if let Expr::Var(name) = callee.as_ref() {
                return call_function(name, values, env, host);
            }
            let target = eval(callee, env, host)?;
            call_value(&target, values, env, host)
        }
        Expr::Method(receiver, name, args) => {
            let this = eval(receiver, env, host)?;
            let mut values = vec![this];
            values.extend(eval_args(args, env, host)?);
            call_function(name, values, env, host)
        }
        Expr::Lambda {
            params,
            varargs,
            body,
        } => {
            let mut captured = std::collections::HashMap::new();
            if let Some(frame) = env.frames.last() {
                for (k, v) in &frame.locals {
                    captured.insert(k.clone(), v.clone());
                }
                for (k, v) in &frame.args {
                    captured.insert(k.clone(), v.clone());
                }
            }
            Ok(Value::Funcref(std::rc::Rc::new(FuncRef::from_lambda(
                Lambda {
                    params: params.clone(),
                    varargs: *varargs,
                    body: (**body).clone(),
                    captured,
                    sid: env.current_sid,
                },
            ))))
        }
    }
}

fn eval_args(args: &[Expr], env: &mut Env, host: &mut dyn Host) -> VimResult<Vec<Value>> {
    let mut values = Vec::with_capacity(args.len());
    for a in args {
        values.push(eval(a, env, host)?);
    }
    Ok(values)
}

fn eval_var(name: &str, env: &mut Env, host: &mut dyn Host) -> ValueResult {
    // Bare scope dictionaries for the editor-owned scopes; the language-owned
    // ones are handled by Env::get_var below via Env::scope_dict.
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
    if name == "self" {
        if let Some(frame) = env.frames.last() {
            if let Some(v) = frame.locals.get("self") {
                return Ok(v.clone());
            }
        }
    }
    env.get_var(name).ok_or_else(|| undefined(name))
}

fn undefined(name: &str) -> VimError {
    VimError::new("E121", format!("Undefined variable: {name}"))
}

fn eval_option(name: &str, _env: &mut Env, host: &mut dyn Host) -> ValueResult {
    host.option_get(name)
        .ok_or_else(|| VimError::new("E113", format!("Unknown option: {name}")))
}

fn eval_unary(op: &str, v: Value) -> ValueResult {
    match op {
        // eval9_leader() keeps the Float type for "!" (eval.c:4126-4135).
        "!" => match v {
            Value::Float(f) => Ok(Value::Float(if f == 0.0 { 1.0 } else { 0.0 })),
            other => Ok(Value::Number(if other.truthy() { 0 } else { 1 })),
        },
        "-" => match v {
            Value::Float(f) => Ok(Value::Float(-f)),
            other => Ok(Value::Number(other.as_number()?.wrapping_neg())),
        },
        "+" => match v {
            Value::Float(f) => Ok(Value::Float(f)),
            other => Ok(Value::Number(other.as_number()?)),
        },
        other => Err(VimError::new(
            "E15",
            format!("Unknown unary operator {other}"),
        )),
    }
}

fn case_flags(env: &mut Env, host: &mut dyn Host, op: &str) -> (bool, bool) {
    // Returns (ignore_case, smartcase) for comparison operators.
    if op.ends_with('#') {
        return (false, false);
    }
    if op.ends_with('?') {
        return (true, false);
    }
    let ignore_case = host
        .option_get("ignorecase")
        .map(|v| v.truthy())
        .unwrap_or(false);
    let smartcase = host
        .option_get("smartcase")
        .map(|v| v.truthy())
        .unwrap_or(false);
    let _ = env;
    (ignore_case, smartcase)
}

fn regex_flags(_env: &mut Env, host: &mut dyn Host, op: &str) -> bool {
    if op.ends_with('#') {
        return false;
    }
    if op.ends_with('?') {
        return true;
    }
    // Default follows 'ignorecase'; 'smartcase' only matters when the pattern
    // itself contains upper case letters (checked below).
    host.option_get("ignorecase")
        .map(|v| v.truthy())
        .unwrap_or(false)
}

fn eval_binary(
    op: &str,
    lhs: &Expr,
    rhs: &Expr,
    env: &mut Env,
    host: &mut dyn Host,
) -> ValueResult {
    // Short-circuit logical operators first.
    if op == "&&" || op == "||" {
        let left = eval(lhs, env, host)?;
        if op == "&&" && !left.truthy() {
            return Ok(Value::Number(0));
        }
        if op == "||" && left.truthy() {
            return Ok(Value::Number(1));
        }
        let right = eval(rhs, env, host)?;
        return Ok(Value::Number(if right.truthy() { 1 } else { 0 }));
    }

    let left = eval(lhs, env, host)?;
    let right = eval(rhs, env, host)?;
    let base = op.trim_end_matches(['#', '?']);

    // Regex match operators.
    if base == "=~" || base == "!~" {
        let ignore_case = regex_flags(env, host, op);
        let pattern = right.to_string_value();
        let text = left.to_string_value();
        let hit = regex_mvp::is_match(&pattern, &text, ignore_case);
        return Ok(Value::Number(if (base == "=~") == hit { 1 } else { 0 }));
    }

    // Identity: `is` / `isnot` require the same type and, for containers,
    // the very same container (typval.c typval_compare(), EXPR_IS branch).
    if base == "is" || base == "isnot" {
        let same = identity_equal(&left, &right);
        let hit = if base == "is" { same } else { !same };
        return Ok(Value::Number(if hit { 1 } else { 0 }));
    }

    match base {
        // "." and the 8.2 ".." alias both concatenate (eval.c:3143-3144).
        "." | ".." => Ok(Value::String(format!(
            "{}{}",
            left.to_string_value(),
            right.to_string_value()
        ))),
        "+" => match (&left, &right) {
            (Value::List(a), Value::List(b)) => {
                let mut out = a.borrow().clone();
                out.extend(b.borrow().iter().cloned());
                Ok(Value::list(out))
            }
            (Value::Float(_), _) | (_, Value::Float(_)) => {
                Ok(Value::Float(left.to_float()? + right.to_float()?))
            }
            _ => Ok(Value::Number(
                left.as_number()?.wrapping_add(right.as_number()?),
            )),
        },
        "-" => match (&left, &right) {
            (Value::Float(_), _) | (_, Value::Float(_)) => {
                Ok(Value::Float(left.to_float()? - right.to_float()?))
            }
            _ => Ok(Value::Number(
                left.as_number()?.wrapping_sub(right.as_number()?),
            )),
        },
        "*" => match (&left, &right) {
            (Value::Float(_), _) | (_, Value::Float(_)) => {
                Ok(Value::Float(left.to_float()? * right.to_float()?))
            }
            _ => Ok(Value::Number(
                left.as_number()?.wrapping_mul(right.as_number()?),
            )),
        },
        "/" => match (&left, &right) {
            (Value::Float(_), _) | (_, Value::Float(_)) => {
                Ok(Value::Float(left.to_float()? / right.to_float()?))
            }
            // Legacy Vim never errors on integer division by zero; it returns
            // VARNUM_MAX / -VARNUM_MAX / VARNUM_MIN (eval.c:65-80 num_divide()).
            _ => {
                let divisor = right.as_number()?;
                let dividend = left.as_number()?;
                Ok(Value::Number(if divisor == 0 {
                    if dividend == 0 {
                        i64::MIN
                    } else if dividend < 0 {
                        i64::MIN + 1
                    } else {
                        i64::MAX
                    }
                } else {
                    dividend / divisor
                }))
            }
        },
        "%" => {
            // num_modulus(): modulus by zero is 0, without an error.
            let divisor = right.as_number()?;
            let dividend = left.as_number()?;
            Ok(Value::Number(if divisor == 0 {
                0
            } else {
                dividend % divisor
            }))
        }
        "==" | "!=" | ">" | ">=" | "<" | "<=" => {
            // Ordering comparisons are only defined for numbers, floats and
            // strings; everything else is an explicit error
            // (typval.c typval_compare(): E692/E736/E978/E694).
            if base != "==" && base != "!=" {
                if let Some(err) = invalid_ordering(&left, &right) {
                    return Err(err);
                }
            }
            let (ignore_case, smartcase) = case_flags(env, host, op);
            let ord = if let (Value::String(a), Value::String(b)) = (&left, &right) {
                let fold = ignore_case && !(smartcase && a.chars().any(|c| c.is_uppercase()));
                let (a, b) = if fold {
                    (a.to_lowercase(), b.to_lowercase())
                } else {
                    (a.clone(), b.clone())
                };
                match a.cmp(&b) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                }
            } else {
                compare_values(&left, &right)
            };
            Ok(Value::Number(match base {
                "==" => i64::from(values_equal_with(&left, &right, ignore_case)),
                "!=" => i64::from(!values_equal_with(&left, &right, ignore_case)),
                ">" => i64::from(ord > 0),
                ">=" => i64::from(ord >= 0),
                "<" => i64::from(ord < 0),
                "<=" => i64::from(ord <= 0),
                _ => 0,
            }))
        }
        "<<" | ">>" => {
            // eval5(): both operands must be Numbers, the shift count must be
            // non-negative, a count past the word width yields 0 and ">>" is a
            // *logical* shift on the unsigned representation (eval.c:3069-3130).
            let (Value::Number(value), Value::Number(shift)) = (&left, &right) else {
                return Err(VimError::new(
                    "E745",
                    "Bitshift operations must be Number".to_string(),
                ));
            };
            if *shift < 0 {
                return Err(VimError::new(
                    "E1709",
                    "Shift count must be a positive Number".to_string(),
                ));
            }
            let out = if *shift > 63 {
                0i64
            } else if base == "<<" {
                ((*value as u64) << *shift) as i64
            } else {
                ((*value as u64) >> *shift) as i64
            };
            Ok(Value::Number(out))
        }
        other => Err(VimError::new("E15", format!("Invalid operator: {other}"))),
    }
}

/// `is`/`isnot`: same type required; containers compare by identity.
fn identity_equal(a: &Value, b: &Value) -> bool {
    if std::mem::discriminant(a) != std::mem::discriminant(b) {
        return false;
    }
    match (a, b) {
        (Value::List(x), Value::List(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Dict(x), Value::Dict(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Blob(x), Value::Blob(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Funcref(x), Value::Funcref(y)) => x.name == y.name && x.partial == y.partial,
        _ => values_equal(a, b, 0),
    }
}

/// Vim rejects ordering comparisons on containers/funcrefs.
fn invalid_ordering(a: &Value, b: &Value) -> Option<VimError> {
    let name = match (a, b) {
        (Value::List(_), _) | (_, Value::List(_)) => ("E692", "List"),
        (Value::Dict(_), _) | (_, Value::Dict(_)) => ("E736", "Dictionary"),
        (Value::Blob(_), _) | (_, Value::Blob(_)) => ("E978", "Blob"),
        (Value::Funcref(_), _) | (_, Value::Funcref(_)) => ("E694", "Funcrefs"),
        _ => return None,
    };
    Some(VimError::new(
        name.0,
        format!("Invalid operation for {}", name.1),
    ))
}

fn values_equal_with(left: &Value, right: &Value, ignore_case: bool) -> bool {
    if ignore_case {
        if let (Value::String(a), Value::String(b)) = (left, right) {
            return a.to_lowercase() == b.to_lowercase();
        }
    }
    values_equal(left, right, 0)
}

fn eval_index(target: &Value, index: &Value) -> ValueResult {
    match target {
        Value::List(list) => {
            let items = list.borrow();
            let idx = resolve_index(index.as_number()?, items.len())?;
            items
                .get(idx)
                .cloned()
                .ok_or_else(|| index_out_of_range(idx))
        }
        Value::Blob(blob) => {
            let bytes = blob.borrow();
            let idx = resolve_index(index.as_number()?, bytes.len())?;
            bytes
                .get(idx)
                .map(|b| Value::Number(*b as i64))
                .ok_or_else(|| index_out_of_range(idx))
        }
        Value::Dict(dict) => {
            let key = index.to_string_value();
            dict.borrow().get(&key).cloned().ok_or_else(|| {
                VimError::new("E716", format!("Key not present in Dictionary: \"{key}\""))
            })
        }
        Value::String(s) => {
            // A string index that is negative or past the end yields an empty
            // string, not an error (eval.c:4711-4718).
            let chars: Vec<char> = s.chars().collect();
            let raw = index.as_number()?;
            if raw < 0 || raw as usize >= chars.len() {
                Ok(Value::String(String::new()))
            } else {
                Ok(Value::String(chars[raw as usize].to_string()))
            }
        }
        other => Err(VimError::new(
            "E909",
            format!("Cannot index a {} value", other.type_name()),
        )),
    }
}

/// Negative indexes count from the end (Vim semantics).
fn resolve_index(raw: i64, len: usize) -> VimResult<usize> {
    let idx = if raw < 0 { len as i64 + raw } else { raw };
    if idx < 0 || idx as usize >= len {
        return Err(index_out_of_range(raw));
    }
    Ok(idx as usize)
}

fn index_out_of_range(idx: impl std::fmt::Display) -> VimError {
    VimError::new("E684", format!("List index out of range: {idx}"))
}

fn eval_slice(target: &Value, start: Option<i64>, end: Option<i64>) -> ValueResult {
    // List/clob range semantics (list.c:1150-1184 via eval.c:4730-4738):
    // missing start is 0, missing end is VARNUM_MAX, a start before the list
    // collapses to len and an end before the start yields an empty list.
    let list_bounds = |len: usize| -> Option<(usize, usize)> {
        let len_i = len as i64;
        let mut n1 = start.unwrap_or(0);
        let mut n2 = end.unwrap_or(i64::MAX);
        if n1 < 0 {
            n1 += len_i;
        }
        if n1 < 0 || n1 >= len_i {
            n1 = len_i;
        }
        if n2 < 0 {
            n2 += len_i;
        } else if n2 >= len_i {
            n2 = len_i - 1;
        }
        if n2 < 0 || n2 + 1 < n1 || n1 >= len_i {
            return None;
        }
        Some((n1 as usize, n2 as usize))
    };
    // String range semantics (eval.c:4691-4708): a negative start clamps to
    // 0 (not len) and the end clamps to len, not len-1.
    let string_bounds = |len: usize| -> Option<(usize, usize)> {
        let len_i = len as i64;
        let mut n1 = start.unwrap_or(0);
        let mut n2 = end.unwrap_or(i64::MAX);
        if n1 < 0 {
            n1 += len_i;
            if n1 < 0 {
                n1 = 0;
            }
        }
        if n2 < 0 {
            n2 += len_i;
        } else if n2 >= len_i {
            n2 = len_i;
        }
        if n1 >= len_i || n2 < 0 || n1 > n2 {
            return None;
        }
        let last = if n2 >= len_i { len_i - 1 } else { n2 };
        Some((n1 as usize, last as usize))
    };
    match target {
        Value::List(list) => {
            let items = list.borrow();
            match list_bounds(items.len()) {
                Some((s, e)) => Ok(Value::list(items[s..=e].to_vec())),
                None => Ok(Value::list(Vec::new())),
            }
        }
        Value::String(text) => {
            let chars: Vec<char> = text.chars().collect();
            match string_bounds(chars.len()) {
                Some((s, e)) => Ok(Value::String(chars[s..=e].iter().collect())),
                None => Ok(Value::String(String::new())),
            }
        }
        Value::Blob(blob) => {
            let bytes = blob.borrow();
            match list_bounds(bytes.len()) {
                Some((s, e)) => Ok(Value::blob(bytes[s..=e].to_vec())),
                None => Ok(Value::blob(Vec::new())),
            }
        }
        other => Err(VimError::new(
            "E709",
            format!("Cannot slice a {} value", other.type_name()),
        )),
    }
}

/// Call a function by name, resolving builtins, host builtins and user functions.
///
/// A variable that holds a Funcref can also be called by name (`let F = {x -> x}
/// | echo F(1)`), which Vim supports.
pub fn call_function(
    name: &str,
    args: Vec<Value>,
    env: &mut Env,
    host: &mut dyn Host,
) -> ValueResult {
    if let Some(Value::Funcref(_)) | Some(Value::String(_)) = env.get_var(name) {
        let target = env.get_var(name).unwrap();
        return call_value(&target, args, env, host);
    }
    // Ask the host first with a *cheap name check*: entering the big host /
    // builtin dispatchers allocates large stack frames in debug builds and that
    // cost is paid on every nested user-function call (deep recursion used to
    // overflow the native stack long before the E132 depth guard).
    if host.is_host_builtin(name) {
        if let Some(result) = host.call_host_builtin(name, args.clone(), env) {
            return result;
        }
    }
    if builtins::is_builtin(name) {
        if let Some(result) = builtins::call_builtin(name, &args, env, host) {
            return result;
        }
    }
    if let Some(result) = host.call_user_function(name, args, env) {
        return result;
    }
    Err(VimError::new("E117", format!("Unknown function: {name}")))
}

/// Call a Funcref/lambda value (or a function name in a variable).
pub fn call_value(
    target: &Value,
    mut args: Vec<Value>,
    env: &mut Env,
    host: &mut dyn Host,
) -> ValueResult {
    match target {
        Value::Funcref(f) => {
            if let Some(lambda) = &f.lambda {
                let mut bound = f.partial.clone();
                bound.append(&mut args);
                return call_lambda(lambda, bound, env, host);
            }
            let mut all = f.partial.clone();
            all.append(&mut args);
            call_function(&f.name, all, env, host)
        }
        Value::String(name) => call_function(name, args, env, host),
        other => Err(VimError::new(
            "E1085",
            format!("Not a callable value: {}", other.type_name()),
        )),
    }
}

fn call_lambda(
    lambda: &Lambda,
    args: Vec<Value>,
    env: &mut Env,
    host: &mut dyn Host,
) -> ValueResult {
    let mut frame = crate::env::Frame {
        args: std::collections::HashMap::new(),
        locals: lambda.captured.clone(),
        sid: lambda.sid,
    };
    for (i, param) in lambda.params.iter().enumerate() {
        let value = args.get(i).cloned().unwrap_or(Value::Null);
        frame.args.insert(param.clone(), value.clone());
        // Lambda parameters are also visible without the a: prefix: Vim adds
        // them to the l: scope as well (userfunc.c:2767-2773 addlocal).
        frame.locals.insert(param.clone(), value);
    }
    let extra = args.len().saturating_sub(lambda.params.len());
    frame
        .args
        .insert("0".to_string(), Value::Number(extra as i64));
    frame.args.insert(
        "000".to_string(),
        Value::list(args.iter().skip(lambda.params.len()).cloned().collect()),
    );
    // Only varargs get numeric a:1..a:N keys; named parameters are addressed by
    // name (userfunc.c:2769-2771).
    for (i, value) in args.iter().enumerate().skip(lambda.params.len()) {
        frame.args.insert((i + 1).to_string(), value.clone());
    }
    if !lambda.varargs && args.len() > lambda.params.len() {
        return Err(VimError::new(
            "E118",
            format!(
                "Too many arguments for <lambda>: expected {}",
                lambda.params.len()
            ),
        ));
    }
    // funcdepth_increment() rejects the call when the depth is already at
    // 'maxfuncdepth' (userfunc.c:2536-2544).
    if env.call_depth >= host.maxfuncdepth() {
        return Err(VimError::new(
            "E132",
            "Function call depth is higher than 'maxfuncdepth'".to_string(),
        ));
    }
    env.call_depth += 1;
    env.frames.push(frame);
    let saved_sid = env.current_sid;
    env.current_sid = lambda.sid;
    let result = eval(&lambda.body, env, host);
    env.current_sid = saved_sid;
    env.frames.pop();
    env.call_depth -= 1;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoHost;
    impl Host for NoHost {}

    fn ev(src: &str) -> Value {
        let mut env = Env::new();
        let expr = crate::expr::parse_expression(src).unwrap();
        eval(&expr, &mut env, &mut NoHost).unwrap()
    }

    #[test]
    fn slices_return_empty_for_invalid_ranges() {
        // list.c:1150-1184: an out-of-range start or a reversed range is empty.
        let list = Value::list(vec![Value::Number(1), Value::Number(2), Value::Number(3)]);
        let show = |v: &Value| v.to_string_value();
        assert_eq!(show(&eval_slice(&list, Some(2), Some(1)).unwrap()), "[]");
        assert_eq!(show(&eval_slice(&list, Some(3), None).unwrap()), "[]");
        assert_eq!(show(&eval_slice(&list, Some(5), Some(6)).unwrap()), "[]");
        assert_eq!(show(&eval_slice(&list, Some(-10), Some(1)).unwrap()), "[]");
        assert_eq!(show(&eval_slice(&list, Some(1), None).unwrap()), "[2, 3]");
        // eval.c:4691-4708: strings clamp differently (negative start -> 0).
        let text = Value::string("abcdef");
        assert_eq!(show(&eval_slice(&text, Some(4), Some(2)).unwrap()), "");
        assert_eq!(show(&eval_slice(&text, Some(5), Some(6)).unwrap()), "f");
        assert_eq!(show(&eval_slice(&text, Some(6), Some(6)).unwrap()), "");
        assert_eq!(show(&eval_slice(&text, Some(-2), None).unwrap()), "ef");
        assert_eq!(show(&eval_slice(&text, Some(-10), Some(1)).unwrap()), "ab");
    }

    #[test]
    fn dotdot_concatenates_like_dot() {
        // eval.c:3143-3144.
        assert_eq!(ev("'a' .. 'b'").to_string_value(), "ab");
        assert_eq!(ev("'a' .. (1 + 2)").to_string_value(), "a3");
    }

    #[test]
    fn null_and_bool_comparisons() {
        // typval.c:1170-1239 / 1414.
        assert_eq!(ev("v:null == []").to_number(), 0);
        assert_eq!(ev("v:null == ''").to_number(), 0);
        assert_eq!(ev("v:null == 0").to_number(), 1);
        assert_eq!(ev("v:true == 'v:true'").to_number(), 1);
        assert_eq!(ev("v:false == 'v:false'").to_number(), 1);
    }
}
