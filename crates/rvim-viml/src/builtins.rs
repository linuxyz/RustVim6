//! Pure builtin functions (reference: evalfunc.c `global_functions[]`).
//!
//! Host-dependent functions (`getline`, `expand`, `exists`, `has`, ...) are
//! implemented by the Ex layer and called first; everything here is pure enough
//! to live in the language crate. `IMPLEMENTED_FUNCS` is the coverage list
//! checked against the generated 543-entry table.

use std::rc::Rc;

use rvim_core::{VimError, VimResult};

use crate::env::{Env, Host};
use crate::eval::{call_value, eval};
use crate::expr::parse_expression;
use crate::func_table::FUNCTIONS;
use crate::regex_mvp;
use crate::value::{FuncRef, Value, ValueResult, VimDict, compare_values, tv_equal};

fn err(code: &'static str, msg: impl Into<String>) -> VimError {
    VimError::new(code, msg.into())
}

/// `tv_equal()` with the optional ignore-case flag used by index()/count().
fn tv_equal_with(a: &Value, b: &Value, ignore_case: bool) -> bool {
    if ignore_case {
        if let (Value::String(x), Value::String(y)) = (a, b) {
            return x.to_lowercase() == y.to_lowercase();
        }
    }
    tv_equal(a, b)
}

/// Map a Vim-style -1/0/1 comparison result to a sort ordering.
fn ord_from(n: i64) -> std::cmp::Ordering {
    match n {
        n if n < 0 => std::cmp::Ordering::Less,
        n if n > 0 => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    }
}

/// sort() with the "i" (ignore case) / "l" flag: strings fold case.
fn cmp_ignore_case_func(a: &Value, b: &Value) -> std::cmp::Ordering {
    if let (Value::String(x), Value::String(y)) = (a, b) {
        return x.to_lowercase().cmp(&y.to_lowercase());
    }
    ord_from(compare_values(a, b))
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Null)
}

fn sarg(args: &[Value], i: usize) -> String {
    arg(args, i).to_string_value()
}

fn narg(args: &[Value], i: usize) -> i64 {
    arg(args, i).to_number()
}

pub(crate) fn check_arity(name: &str, args: &[Value]) -> VimResult<()> {
    if let Some(def) = FUNCTIONS.iter().find(|f| f.name == name) {
        let n = args.len() as u32;
        if n < def.min_args {
            return Err(err(
                "E119",
                format!("Not enough arguments for function: {name}"),
            ));
        }
        if def.max_args != 0 && n > def.max_args {
            return Err(err(
                "E118",
                format!("Too many arguments for function: {name}"),
            ));
        }
    }
    Ok(())
}

/// Builtin functions implemented so far (kept in sync with the dispatch below;
/// a test asserts both directions).
pub const IMPLEMENTED_FUNCS: &[&str] = &[
    "abs",
    "acos",
    "add",
    "and",
    "asin",
    "atan",
    "atan2",
    "call",
    "ceil",
    "char2nr",
    "copy",
    "cos",
    "count",
    "deepcopy",
    "empty",
    "escape",
    "eval",
    "exp",
    "extend",
    "filter",
    "float2nr",
    "floor",
    "fmod",
    "fnameescape",
    "function",
    "get",
    "has_key",
    "index",
    "insert",
    "invert",
    "isinf",
    "isnan",
    "items",
    "join",
    "keys",
    "len",
    "log",
    "log10",
    "map",
    "match",
    "matchend",
    "matchstr",
    "max",
    "min",
    "nr2char",
    "or",
    "pow",
    "printf",
    "range",
    "remove",
    "repeat",
    "reverse",
    "round",
    "sin",
    "sort",
    "split",
    "sqrt",
    "str2float",
    "str2nr",
    "strcharpart",
    "strchars",
    "stridx",
    "string",
    "strlen",
    "strpart",
    "strridx",
    "strwidth",
    "submatch",
    "substitute",
    "tan",
    "tolower",
    "toupper",
    "trim",
    "trunc",
    "type",
    "uniq",
    "values",
    "xor",
];

/// All pure builtin names implemented across the group modules.
pub fn implemented_funcs() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = IMPLEMENTED_FUNCS.to_vec();
    for group in [
        crate::builtins_ae::FUNCS,
        crate::builtins_assert::FUNCS,
        crate::builtins_fl::FUNCS,
        crate::builtins_misc2::FUNCS,
        crate::builtins_mr::FUNCS,
        crate::builtins_s1::FUNCS,
        crate::builtins_s2::FUNCS,
    ] {
        names.extend_from_slice(group);
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// True when the builtin has a pure implementation here or in a group module.
pub fn is_builtin(name: &str) -> bool {
    if IMPLEMENTED_FUNCS.contains(&name) {
        return true;
    }
    crate::builtins_ae::FUNCS.contains(&name)
        || crate::builtins_assert::FUNCS.contains(&name)
        || crate::builtins_misc2::FUNCS.contains(&name)
        || crate::builtins_fl::FUNCS.contains(&name)
        || crate::builtins_mr::FUNCS.contains(&name)
        || crate::builtins_s1::FUNCS.contains(&name)
        || crate::builtins_s2::FUNCS.contains(&name)
}

/// Dispatch a pure builtin; `None` means "not a pure builtin here".
pub fn call_builtin(
    name: &str,
    args: &[Value],
    env: &mut Env,
    host: &mut dyn Host,
) -> Option<ValueResult> {
    if IMPLEMENTED_FUNCS.contains(&name) {
        return Some((|| {
            check_arity(name, args)?;
            dispatch(name, args, env, host)
        })());
    }
    for group in [
        crate::builtins_ae::call,
        crate::builtins_assert::call,
        crate::builtins_fl::call,
        crate::builtins_misc2::call,
        crate::builtins_mr::call,
        crate::builtins_s1::call,
        crate::builtins_s2::call,
    ] {
        if let Some(result) = group(name, args, env, host) {
            return Some(result);
        }
    }
    None
}

fn dispatch(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    match name {
        // ---------------------------------------------------------- types
        "type" => Ok(Value::Number(match arg(args, 0) {
            Value::Number(_) => 0,
            Value::String(_) => 1,
            Value::Funcref(_) => 2,
            Value::List(_) => 3,
            Value::Dict(_) => 4,
            Value::Float(_) => 5,
            Value::Bool(_) => 6,
            Value::Null => 7,
            Value::Blob(_) => 10,
        })),
        "empty" => Ok(Value::Number(if is_empty(&arg(args, 0)) { 1 } else { 0 })),
        "len" => Ok(Value::Number(match arg(args, 0) {
            Value::List(l) => l.borrow().len() as i64,
            Value::Dict(d) => d.borrow().len() as i64,
            // `len()` of a String is its *byte* length (STRLEN).
            Value::String(s) => s.len() as i64,
            Value::Blob(b) => b.borrow().len() as i64,
            // For a Number the length of its decimal representation is used
            // (f_len() calls tv_get_string() first, evalfunc.c:7083-7087).
            Value::Number(n) => n.to_string().len() as i64,
            // Floats/Bools/Specials keep the initial 0 in legacy Vim.
            _ => 0,
        })),
        "string" => Ok(Value::String(arg(args, 0).to_repr())),
        "copy" => Ok(shallow_copy(&arg(args, 0))),
        "deepcopy" => Ok(deep_copy(&arg(args, 0), 0)),
        "function" => {
            let func_name = sarg(args, 0);
            let mut f = FuncRef::new(func_name);
            if let Some(Value::List(partial)) = args.get(1) {
                f.partial = partial.borrow().clone();
            }
            Ok(Value::Funcref(Rc::new(f)))
        }
        "call" => {
            let target = arg(args, 0);
            let call_args = match arg(args, 1) {
                Value::List(l) => l.borrow().clone(),
                Value::Null => Vec::new(),
                _ => return Err(err("E714", "List required for argument 2")),
            };
            call_value(&target, call_args, env, host)
        }
        "eval" => {
            let src = sarg(args, 0);
            let expr = parse_expression(&src)?;
            eval(&expr, env, host)
        }

        // -------------------------------------------------------- numbers
        "abs" => match arg(args, 0) {
            Value::Float(f) => Ok(Value::Float(f.abs())),
            v => Ok(Value::Number(v.as_number()?.abs())),
        },
        "ceil" => Ok(Value::Float(arg(args, 0).to_float()?.ceil())),
        "floor" => Ok(Value::Float(arg(args, 0).to_float()?.floor())),
        "trunc" => Ok(Value::Float(arg(args, 0).to_float()?.trunc())),
        "round" => Ok(Value::Float(arg(args, 0).to_float()?.round())),
        "float2nr" => Ok(Value::Number(arg(args, 0).to_float()? as i64)),
        "str2float" => Ok(Value::Float(
            sarg(args, 0).trim().parse::<f64>().unwrap_or(0.0),
        )),
        "str2nr" => {
            let base = if args.len() > 1 {
                let b = narg(args, 1);
                if b == 0 { 10 } else { b as u32 }
            } else {
                10
            };
            let quote = args.len() > 2 && arg(args, 2).truthy();
            let text = sarg(args, 0);
            match crate::value::str2nr_vim(&text, base, quote) {
                Some(n) => Ok(Value::Number(n)),
                None => Err(err("E474", "Invalid argument")),
            }
        }
        "cos" => Ok(Value::Float(arg(args, 0).to_float()?.cos())),
        "sin" => Ok(Value::Float(arg(args, 0).to_float()?.sin())),
        "tan" => Ok(Value::Float(arg(args, 0).to_float()?.tan())),
        "acos" => Ok(Value::Float(arg(args, 0).to_float()?.acos())),
        "asin" => Ok(Value::Float(arg(args, 0).to_float()?.asin())),
        "atan" => Ok(Value::Float(arg(args, 0).to_float()?.atan())),
        "atan2" => Ok(Value::Float(
            arg(args, 0).to_float()?.atan2(arg(args, 1).to_float()?),
        )),
        "exp" => Ok(Value::Float(arg(args, 0).to_float()?.exp())),
        "log" => Ok(Value::Float(arg(args, 0).to_float()?.ln())),
        "log10" => Ok(Value::Float(arg(args, 0).to_float()?.log10())),
        "pow" => Ok(Value::Float(
            arg(args, 0).to_float()?.powf(arg(args, 1).to_float()?),
        )),
        "sqrt" => Ok(Value::Float(arg(args, 0).to_float()?.sqrt())),
        "fmod" => Ok(Value::Float(
            arg(args, 0).to_float()? % arg(args, 1).to_float()?,
        )),
        "isinf" => Ok(Value::Number(if arg(args, 0).to_float()?.is_infinite() {
            1
        } else {
            0
        })),
        "isnan" => Ok(Value::Number(if arg(args, 0).to_float()?.is_nan() {
            1
        } else {
            0
        })),
        "and" => Ok(Value::Number(narg(args, 0) & narg(args, 1))),
        "or" => Ok(Value::Number(narg(args, 0) | narg(args, 1))),
        "xor" => Ok(Value::Number(narg(args, 0) ^ narg(args, 1))),
        "invert" => Ok(Value::Number(!narg(args, 0))),
        "char2nr" => Ok(Value::Number(
            sarg(args, 0).chars().next().map(|c| c as i64).unwrap_or(0),
        )),
        "nr2char" => {
            let n = narg(args, 0);
            Ok(Value::String(
                char::from_u32(n as u32)
                    .map(String::from)
                    .unwrap_or_default(),
            ))
        }

        // -------------------------------------------------------- strings
        "strlen" => Ok(Value::Number(sarg(args, 0).len() as i64)),
        "strchars" => Ok(Value::Number(sarg(args, 0).chars().count() as i64)),
        "strwidth" => Ok(Value::Number(sarg(args, 0).chars().count() as i64)),
        "stridx" => {
            let hay = sarg(args, 0);
            let needle = sarg(args, 1);
            let start = if args.len() > 2 {
                (narg(args, 2).max(0) as usize).min(hay.len())
            } else {
                0
            };
            // Byte index into the string.
            let mut from = start;
            while from <= hay.len() && !hay.is_char_boundary(from) {
                from += 1;
            }
            Ok(Value::Number(
                hay[from..]
                    .find(&needle)
                    .map(|i| (i + from) as i64)
                    .unwrap_or(-1),
            ))
        }
        "strridx" => {
            let hay = sarg(args, 0);
            let needle = sarg(args, 1);
            let limit = if args.len() > 2 {
                (narg(args, 2).max(0) as usize).min(hay.len())
            } else {
                hay.len()
            };
            let mut end = limit;
            while end > 0 && !hay.is_char_boundary(end) {
                end -= 1;
            }
            Ok(Value::Number(
                hay[..end].rfind(&needle).map(|i| i as i64).unwrap_or(-1),
            ))
        }
        "strpart" => {
            let text = sarg(args, 0);
            let start = narg(args, 1).max(0) as usize;
            let len = if args.len() > 2 {
                narg(args, 2).max(0) as usize
            } else {
                text.len().saturating_sub(start)
            };
            Ok(Value::String(byte_slice(&text, start, len)))
        }
        "strcharpart" => {
            let chars: Vec<char> = sarg(args, 0).chars().collect();
            let start = narg(args, 1).max(0) as usize;
            let len = if args.len() > 2 {
                narg(args, 2).max(0) as usize
            } else {
                chars.len()
            };
            let slice: String = chars.iter().skip(start).take(len).collect();
            Ok(Value::String(slice))
        }
        "toupper" => Ok(Value::String(sarg(args, 0).to_uppercase())),
        "tolower" => Ok(Value::String(sarg(args, 0).to_lowercase())),
        "trim" => {
            let text = sarg(args, 0);
            let mask = if args.len() > 1 {
                sarg(args, 1)
            } else {
                String::new()
            };
            let dir = if args.len() > 2 { narg(args, 2) } else { 0 };
            Ok(Value::String(trim_text(&text, &mask, dir)))
        }
        "escape" => {
            let text = sarg(args, 0);
            let chars = sarg(args, 1);
            let mut out = String::new();
            for c in text.chars() {
                if chars.contains(c) {
                    out.push('\\');
                }
                out.push(c);
            }
            Ok(Value::String(out))
        }
        "fnameescape" => Ok(Value::String(fnameescape(&sarg(args, 0)))),
        "repeat" => {
            if let Value::List(l) = arg(args, 0) {
                let times = narg(args, 1).max(0) as usize;
                let items = l.borrow().clone();
                let mut out = Vec::new();
                for _ in 0..times {
                    out.extend(items.iter().cloned());
                }
                return Ok(Value::list(out));
            }
            let text = sarg(args, 0);
            let times = narg(args, 1).max(0) as usize;
            Ok(Value::String(text.repeat(times)))
        }
        "split" => {
            let text = sarg(args, 0);
            let pattern = if args.len() > 1 {
                sarg(args, 1)
            } else {
                String::new()
            };
            // split(): {keepempty} defaults to TRUE (evalfunc.c f_split).
            let keepempty = if args.len() > 2 {
                narg(args, 2) != 0
            } else {
                true
            };
            let parts = split_text(&text, &pattern, keepempty);
            Ok(Value::list(parts.into_iter().map(Value::String).collect()))
        }
        "join" => {
            let items = match arg(args, 0) {
                Value::List(l) => l.borrow().clone(),
                other => {
                    return Err(err(
                        "E714",
                        format!("List required, got {}", other.type_name()),
                    ));
                }
            };
            let sep = if args.len() > 1 {
                sarg(args, 1)
            } else {
                " ".to_string()
            };
            let parts: Vec<String> = items.iter().map(Value::to_string_value).collect();
            Ok(Value::String(parts.join(&sep)))
        }
        "submatch" => {
            let nr = narg(args, 0).max(0) as usize;
            Ok(Value::String(regex_mvp::submatch(nr)))
        }
        "substitute" => {
            let text = sarg(args, 0);
            let pat = sarg(args, 1);
            let sub = sarg(args, 2);
            let flags = if args.len() > 3 {
                sarg(args, 3)
            } else {
                String::new()
            };
            let global = flags.contains('g');
            // Expand \1..\9 from the (single) match group is not supported yet;
            // plain replacement plus & (whole match) are.
            Ok(Value::String(substitute_text(&text, &pat, &sub, global)))
        }
        "match" => {
            let text = sarg(args, 0);
            let pat = sarg(args, 1);
            let start = if args.len() > 2 {
                narg(args, 2).max(0) as usize
            } else {
                0
            };
            let start = byte_start(&text, start);
            match regex_mvp::find(&pat, &text[start..], false) {
                Some((b, _)) => Ok(Value::Number((start + b) as i64)),
                None => Ok(Value::Number(-1)),
            }
        }
        "matchend" => {
            let text = sarg(args, 0);
            let pat = sarg(args, 1);
            let start = if args.len() > 2 {
                narg(args, 2).max(0) as usize
            } else {
                0
            };
            let start = byte_start(&text, start);
            match regex_mvp::find(&pat, &text[start..], false) {
                Some((_, e)) => Ok(Value::Number((start + e) as i64)),
                None => Ok(Value::Number(-1)),
            }
        }
        "matchstr" => {
            let text = sarg(args, 0);
            let pat = sarg(args, 1);
            let start = if args.len() > 2 {
                narg(args, 2).max(0) as usize
            } else {
                0
            };
            let start = byte_start(&text, start);
            match regex_mvp::find(&pat, &text[start..], false) {
                Some((b, e)) => Ok(Value::String(text[start + b..start + e].to_string())),
                None => Ok(Value::String(String::new())),
            }
        }
        "printf" => Ok(Value::String(printf(&sarg(args, 0), &args[1..])?)),

        // ----------------------------------------------------------- lists
        "add" => {
            let Value::List(list) = arg(args, 0) else {
                return Err(err("E897", "List required for add()"));
            };
            list.borrow_mut().push(arg(args, 1));
            Ok(arg(args, 0))
        }
        "insert" => {
            let Value::List(list) = arg(args, 0) else {
                return Err(err("E897", "List required for insert()"));
            };
            let idx = if args.len() > 2 { narg(args, 2) } else { 0 };
            let len = list.borrow().len() as i64;
            let at = if idx < 0 {
                (len + idx).max(0)
            } else {
                idx.min(len)
            } as usize;
            list.borrow_mut().insert(at, arg(args, 1));
            Ok(arg(args, 0))
        }
        "remove" => {
            let target = arg(args, 0);
            match &target {
                Value::List(list) => {
                    let idx = narg(args, 1);
                    let mut items = list.borrow_mut();
                    let len = items.len() as i64;
                    let real = if idx < 0 { len + idx } else { idx };
                    if real < 0 || real >= len {
                        return Err(err("E684", format!("List index out of range: {idx}")));
                    }
                    Ok(items.remove(real as usize))
                }
                Value::Dict(dict) => {
                    let key = sarg(args, 1);
                    let removed = dict.borrow_mut().remove(&key).ok_or_else(|| {
                        err("E716", format!("Key not present in Dictionary: \"{key}\""))
                    })?;
                    env.sync_scope_dict(&target);
                    Ok(removed)
                }
                Value::Blob(blob) => {
                    let idx = narg(args, 1);
                    let mut bytes = blob.borrow_mut();
                    let len = bytes.len() as i64;
                    let real = if idx < 0 { len + idx } else { idx };
                    if real < 0 || real >= len {
                        return Err(err("E684", format!("Blob index out of range: {idx}")));
                    }
                    Ok(Value::Number(bytes.remove(real as usize) as i64))
                }
                other => Err(err(
                    "E896",
                    format!(
                        "Argument of remove() must be a List, Dictionary or Blob, not {}",
                        other.type_name()
                    ),
                )),
            }
        }
        "get" => {
            // Without an explicit default, get() returns 0 (not v:null).
            let default = if args.len() > 2 {
                arg(args, 2)
            } else {
                Value::Number(0)
            };
            match arg(args, 0) {
                Value::List(list) => {
                    let items = list.borrow();
                    let idx = narg(args, 1);
                    let real = if idx < 0 {
                        items.len() as i64 + idx
                    } else {
                        idx
                    };
                    if real < 0 || real as usize >= items.len() {
                        Ok(default)
                    } else {
                        Ok(items[real as usize].clone())
                    }
                }
                Value::Dict(dict) => {
                    let key = sarg(args, 1);
                    Ok(dict.borrow().get(&key).cloned().unwrap_or(default))
                }
                Value::Blob(blob) => {
                    let bytes = blob.borrow();
                    let idx = narg(args, 1);
                    let real = if idx < 0 {
                        bytes.len() as i64 + idx
                    } else {
                        idx
                    };
                    if real < 0 || real as usize >= bytes.len() {
                        Ok(default)
                    } else {
                        Ok(Value::Number(bytes[real as usize] as i64))
                    }
                }
                other => Err(err(
                    "E896",
                    format!(
                        "Argument of get() must be a List, Dictionary or Blob, not {}",
                        other.type_name()
                    ),
                )),
            }
        }
        "index" => {
            let items = match arg(args, 0) {
                Value::List(l) => l.borrow().clone(),
                other => {
                    return Err(err(
                        "E714",
                        format!("List required, got {}", other.type_name()),
                    ));
                }
            };
            let needle = arg(args, 1);
            let start = if args.len() > 2 {
                narg(args, 2).max(0) as usize
            } else {
                0
            };
            let ignore_case = args.len() > 3 && narg(args, 3) != 0;
            Ok(Value::Number(
                items
                    .iter()
                    .enumerate()
                    .skip(start)
                    .find(|(_, v)| tv_equal_with(v, &needle, ignore_case))
                    .map(|(i, _)| i as i64)
                    .unwrap_or(-1),
            ))
        }
        "count" => {
            // count({comp}, {expr} [, {ic} [, {start}]]) — {start} only exists
            // for Lists (list.c:2639 f_count).
            let needle = arg(args, 1);
            let ignore_case = args.len() > 2 && narg(args, 2) != 0;
            match arg(args, 0) {
                Value::List(list) => {
                    let start = if args.len() > 3 {
                        narg(args, 3).max(0) as usize
                    } else {
                        0
                    };
                    let items = list.borrow();
                    let mut count = 0i64;
                    for item in items.iter().skip(start) {
                        if tv_equal_with(item, &needle, ignore_case) {
                            count += 1;
                        }
                    }
                    Ok(Value::Number(count))
                }
                Value::Dict(dict) => {
                    if args.len() > 3 {
                        return Err(err(
                            "E474",
                            "Invalid argument: count() with a Dictionary and a start index"
                                .to_string(),
                        ));
                    }
                    let d = dict.borrow();
                    let mut count = 0i64;
                    for (_, item) in d.iter() {
                        if tv_equal_with(item, &needle, ignore_case) {
                            count += 1;
                        }
                    }
                    Ok(Value::Number(count))
                }
                Value::String(s) => Ok(Value::Number(count_in_string(
                    &s,
                    &needle.to_string_value(),
                    ignore_case,
                ))),
                other => Err(err(
                    "E714",
                    format!("List required, got {}", other.type_name()),
                )),
            }
        }
        "reverse" => {
            match arg(args, 0) {
                Value::List(list) => list.borrow_mut().reverse(),
                Value::Blob(blob) => blob.borrow_mut().reverse(),
                other => {
                    return Err(err(
                        "E896",
                        format!(
                            "Argument of reverse() must be a List or Blob, not {}",
                            other.type_name()
                        ),
                    ));
                }
            }
            Ok(arg(args, 0))
        }
        "sort" => {
            let target = arg(args, 0);
            let mut items = match &target {
                Value::List(l) => l.borrow().clone(),
                other => {
                    return Err(err(
                        "E714",
                        format!("List required, got {}", other.type_name()),
                    ));
                }
            };
            // Second argument: a Funcref comparator, or a one-letter how-string
            // ("n" numeric, "N" numbers-in-strings, "f" float, "i" ignore case,
            // "l" locale) — list.c:2091 parse_sort_uniq_args().
            match args.get(1).cloned().unwrap_or(Value::Null) {
                Value::Null => items.sort_by(|a, b| ord_from(compare_values(a, b))),
                Value::Number(1) => {
                    items.sort_by(cmp_ignore_case_func);
                }
                Value::Number(0) => items.sort_by(|a, b| ord_from(compare_values(a, b))),
                Value::Number(_) => return Err(err("E474", "Invalid argument: sort()")),
                Value::String(how) => match how.as_str() {
                    "" => items.sort_by(|a, b| ord_from(compare_values(a, b))),
                    // "n": numbers in strings compared as doubles (strtod).
                    "n" => items.sort_by(|a, b| {
                        let x = a.to_float().unwrap_or(0.0);
                        let y = b.to_float().unwrap_or(0.0);
                        x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
                    }),
                    // "N": compare as Numbers (tv_get_number).
                    "N" => items.sort_by_key(|a| a.to_number()),
                    "f" => items.sort_by(|a, b| {
                        let x = a.to_float().unwrap_or(0.0);
                        let y = b.to_float().unwrap_or(0.0);
                        x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
                    }),
                    "i" | "l" => items.sort_by(cmp_ignore_case_func),
                    name => {
                        // Anything else is a comparison function name.
                        let cmp = Value::string(name);
                        let mut failed: Option<VimError> = None;
                        items.sort_by(|a, b| {
                            let r = call_value(&cmp, vec![a.clone(), b.clone()], env, host);
                            match r {
                                Ok(v) => ord_from(v.as_number().unwrap_or(0)),
                                Err(e) => {
                                    failed = Some(e);
                                    std::cmp::Ordering::Equal
                                }
                            }
                        });
                        if let Some(e) = failed {
                            return Err(e);
                        }
                    }
                },
                Value::Funcref(_) => {
                    let cmp = args.get(1).cloned().unwrap_or(Value::Null);
                    let mut failed: Option<VimError> = None;
                    items.sort_by(|a, b| {
                        let r = call_value(&cmp, vec![a.clone(), b.clone()], env, host);
                        match r {
                            Ok(v) => ord_from(v.as_number().unwrap_or(0)),
                            Err(e) => {
                                failed = Some(e);
                                std::cmp::Ordering::Equal
                            }
                        }
                    });
                    if let Some(e) = failed {
                        return Err(e);
                    }
                }
                other => {
                    return Err(err(
                        "E474",
                        format!("Invalid argument: sort() got {}", other.type_name()),
                    ));
                }
            }
            if args.len() > 2 && narg(args, 2) != 0 {
                items.reverse();
            }
            if let Value::List(l) = &target {
                *l.borrow_mut() = items;
            }
            Ok(target)
        }
        "uniq" => {
            let Value::List(list) = arg(args, 0) else {
                return Err(err("E714", "List required"));
            };
            let mut out: Vec<Value> = Vec::new();
            for item in list.borrow().iter() {
                if out.last().map(|last| !tv_equal(last, item)).unwrap_or(true) {
                    out.push(item.clone());
                }
            }
            *list.borrow_mut() = out;
            Ok(arg(args, 0))
        }
        "range" => {
            // range({end}) is 0..end-1; range({start}, {end}) is start..=end
            // (evalfunc.c f_range(): "end" is exclusive for one argument).
            let (start, end, stride) = match args.len() {
                1 => (0, narg(args, 0) - 1, 1),
                2 => (narg(args, 0), narg(args, 1), 1),
                _ => (narg(args, 0), narg(args, 1), narg(args, 2)),
            };
            if stride == 0 {
                return Err(err("E726", "Stride is zero".to_string()));
            }
            let mut out = Vec::new();
            let mut cur = start;
            if stride > 0 {
                while cur <= end {
                    out.push(Value::Number(cur));
                    cur = cur.wrapping_add(stride);
                }
            } else {
                while cur >= end {
                    out.push(Value::Number(cur));
                    cur = cur.wrapping_add(stride);
                }
            }
            Ok(Value::list(out))
        }
        "max" => {
            let items = match arg(args, 0) {
                Value::List(l) => l.borrow().clone(),
                Value::Dict(d) => d.borrow().iter().map(|(_, v)| v.clone()).collect(),
                other => return Ok(other),
            };
            Ok(items
                .into_iter()
                .max_by(|a, b| compare_values(a, b).cmp(&0))
                .unwrap_or(Value::Number(0)))
        }
        "min" => {
            let items = match arg(args, 0) {
                Value::List(l) => l.borrow().clone(),
                Value::Dict(d) => d.borrow().iter().map(|(_, v)| v.clone()).collect(),
                other => return Ok(other),
            };
            Ok(items
                .into_iter()
                .min_by(|a, b| compare_values(a, b).cmp(&0))
                .unwrap_or(Value::Number(0)))
        }
        "map" | "filter" => map_filter(name == "map", args, env, host),
        "extend" => {
            let target = arg(args, 0);
            extend(args)?;
            env.sync_scope_dict(&target);
            Ok(target)
        }

        // ----------------------------------------------------------- dicts
        "keys" => {
            let Value::Dict(d) = arg(args, 0) else {
                return Err(err("E715", "Dictionary required"));
            };
            let keys = d.borrow().keys();
            Ok(Value::list(keys.into_iter().map(Value::String).collect()))
        }
        "values" => {
            let Value::Dict(d) = arg(args, 0) else {
                return Err(err("E715", "Dictionary required"));
            };
            let values: Vec<Value> = d.borrow().iter().map(|(_, v)| v.clone()).collect();
            Ok(Value::list(values))
        }
        "items" => {
            let Value::Dict(d) = arg(args, 0) else {
                return Err(err("E715", "Dictionary required"));
            };
            let items: Vec<Value> = d
                .borrow()
                .iter()
                .map(|(k, v)| Value::list(vec![Value::String(k.clone()), v.clone()]))
                .collect();
            Ok(Value::list(items))
        }
        "has_key" => {
            let Value::Dict(d) = arg(args, 0) else {
                return Err(err("E715", "Dictionary required"));
            };
            let hit = d.borrow().get(&sarg(args, 1)).is_some();
            Ok(Value::Number(if hit { 1 } else { 0 }))
        }
        other => Err(err(
            "E117",
            format!("builtin {other} is listed but not dispatched"),
        )),
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Number(n) => *n == 0,
        Value::Float(f) => *f == 0.0,
        Value::String(s) => s.is_empty(),
        Value::List(l) => l.borrow().is_empty(),
        Value::Dict(d) => d.borrow().is_empty(),
        Value::Blob(b) => b.borrow().is_empty(),
        Value::Funcref(_) => false,
        Value::Bool(b) => !*b,
        Value::Null => true,
    }
}

fn shallow_copy(v: &Value) -> Value {
    match v {
        Value::List(l) => Value::list(l.borrow().clone()),
        Value::Dict(d) => Value::dict(d.borrow().clone()),
        Value::Blob(b) => Value::blob(b.borrow().clone()),
        other => other.clone(),
    }
}

fn deep_copy(v: &Value, depth: usize) -> Value {
    if depth > 100 {
        return v.clone();
    }
    match v {
        Value::List(l) => Value::list(l.borrow().iter().map(|x| deep_copy(x, depth + 1)).collect()),
        Value::Dict(d) => {
            let mut out = VimDict::new();
            for (k, v) in d.borrow().iter() {
                out.insert(k.clone(), deep_copy(v, depth + 1));
            }
            Value::dict(out)
        }
        Value::Blob(b) => Value::blob(b.borrow().clone()),
        other => other.clone(),
    }
}

fn map_filter(is_map: bool, args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    let expr_src = match args.get(1) {
        Some(Value::Funcref(_)) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => {
            return Err(err(
                "E731",
                "Second argument of map() must be a Funcref or a string",
            ));
        }
    };
    let funcref = match args.get(1) {
        Some(Value::Funcref(_)) => args.get(1).cloned(),
        _ => None,
    };
    let target = arg(args, 0);
    // Apply to a list or dict, collecting into a new container.
    let mut run = |v: &Value, key: Option<&str>, index: Option<usize>| -> VimResult<Value> {
        if let Some(f) = &funcref {
            // Vim calls the Funcref with (key, value): the list index or the
            // dictionary key first.
            let first = match (key, index) {
                (Some(k), _) => Value::String(k.to_string()),
                (None, Some(i)) => Value::Number(i as i64),
                _ => Value::Number(0),
            };
            return call_value(f, vec![first, v.clone()], env, host);
        }
        // String expression form: v:val / v:key are available.
        let saved_val = env.vvars.get("val").cloned();
        let saved_key = env.vvars.get("key").cloned();
        env.vvars.insert("val".to_string(), v.clone());
        env.vvars.insert(
            "key".to_string(),
            key.map(|k| Value::String(k.to_string()))
                .unwrap_or(Value::Null),
        );
        let result = parse_expression(expr_src.as_deref().unwrap_or("v:val"))
            .and_then(|e| eval(&e, env, host));
        match saved_val {
            Some(val) => {
                env.vvars.insert("val".to_string(), val);
            }
            None => {
                env.vvars.remove("val");
            }
        }
        match saved_key {
            Some(k) => {
                env.vvars.insert("key".to_string(), k);
            }
            None => {
                env.vvars.remove("key");
            }
        }
        result
    };

    match &target {
        Value::List(list) => {
            let items = list.borrow().clone();
            let mut out = Vec::with_capacity(items.len());
            for (idx, item) in items.iter().enumerate() {
                let mapped = run(item, None, Some(idx))?;
                if is_map {
                    out.push(mapped);
                } else if mapped.truthy() {
                    out.push(item.clone());
                }
            }
            // Legacy map()/filter() modify the list *in place* and return the
            // same list (list.c:2395-2424: map() replaces li_tv, filter()
            // removes items from l; the returned rettv is the same list).
            *list.borrow_mut() = out;
            Ok(target)
        }
        Value::Dict(dict) => {
            let entries: Vec<(String, Value)> = dict
                .borrow()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut out = VimDict::new();
            for (k, v) in entries {
                let mapped = run(&v, Some(&k), None)?;
                if is_map {
                    out.insert(k, mapped);
                } else if mapped.truthy() {
                    out.insert(k, v);
                }
            }
            // dict_filter_map() keeps the same dictionary (dict.c:1302-1400).
            *dict.borrow_mut() = out;
            env.sync_scope_dict(&target);
            Ok(target)
        }
        other => Err(err(
            "E896",
            format!(
                "Argument of map()/filter() must be a List or Dictionary, not {}",
                other.type_name()
            ),
        )),
    }
}

fn extend(args: &[Value]) -> VimResult<()> {
    let target = arg(args, 0);
    let source = arg(args, 1);
    match (&target, &source) {
        (Value::List(a), Value::List(b)) => {
            let idx = if args.len() > 2 { narg(args, 2) } else { -1 };
            let extra = b.borrow().clone();
            let mut a = a.borrow_mut();
            if idx < 0 {
                a.extend(extra);
            } else {
                let at = (idx as usize).min(a.len());
                for (i, item) in extra.into_iter().enumerate() {
                    a.insert(at + i, item);
                }
            }
            Ok(())
        }
        (Value::Dict(a), Value::Dict(b)) => {
            let action = if args.len() > 2 {
                sarg(args, 2)
            } else {
                "force".to_string()
            };
            let entries: Vec<(String, Value)> = b
                .borrow()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (k, v) in entries {
                let exists = a.borrow().get(&k).is_some();
                match action.as_str() {
                    "keep" if exists => continue,
                    "error" if exists => {
                        return Err(err("E737", format!("Key already exists: {k}")));
                    }
                    _ => {}
                }
                a.borrow_mut().insert(k, v);
            }
            Ok(())
        }
        _ => Err(err("E712", "Argument must be a List or Dictionary")),
    }
}

fn byte_slice(text: &str, start: usize, len: usize) -> String {
    let mut s = start.min(text.len());
    while s < text.len() && !text.is_char_boundary(s) {
        s += 1;
    }
    let mut e = (start + len).min(text.len());
    while e > s && !text.is_char_boundary(e) {
        e -= 1;
    }
    text[s..e].to_string()
}

fn byte_start(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

fn trim_text(text: &str, mask: &str, dir: i64) -> String {
    let is_trim = |c: char| -> bool {
        if mask.is_empty() {
            c.is_whitespace()
        } else {
            mask.contains(c)
        }
    };
    let mut result = text;
    if dir == 0 || dir == 1 {
        result = result.trim_start_matches(is_trim);
    }
    if dir == 0 || dir == 2 {
        result = result.trim_end_matches(is_trim);
    }
    result.to_string()
}

fn fnameescape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if " \t\n*?[{`$\\%#'\"|!<".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn split_text(text: &str, pattern: &str, keepempty: bool) -> Vec<String> {
    if pattern.is_empty() {
        // Vim splits on runs of whitespace when the pattern is empty.
        return text.split_whitespace().map(str::to_string).collect();
    }
    if pattern == "\\zs" {
        return text.chars().map(String::from).collect();
    }
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        match regex_mvp::find(pattern, rest, false) {
            Some((b, e)) if e > b => {
                let piece = rest[..b].to_string();
                if keepempty || !piece.is_empty() {
                    out.push(piece);
                }
                rest = &rest[e..];
            }
            _ => {
                if keepempty || !rest.is_empty() {
                    out.push(rest.to_string());
                }
                break;
            }
        }
    }
    // f_split(): leading and trailing empty items are always discarded, even
    // with {keepempty} (only empty items saw_a between separators are kept).
    while out.first().is_some_and(|s| s.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|s| s.is_empty()) {
        out.pop();
    }
    out
}

fn substitute_text(text: &str, pattern: &str, replacement: &str, global: bool) -> String {
    let mut out = String::new();
    let mut rest = text;
    loop {
        match regex_mvp::find(pattern, rest, false) {
            Some((b, e)) => {
                out.push_str(&rest[..b]);
                out.push_str(&expand_replacement(replacement, &rest[b..e]));
                if e == 0 && b == 0 {
                    // Zero-width match: copy one char to make progress.
                    if let Some(c) = rest.chars().next() {
                        out.push(c);
                        rest = &rest[c.len_utf8()..];
                        if !global {
                            out.push_str(rest);
                            break;
                        }
                        continue;
                    }
                    break;
                }
                rest = &rest[e..];
                if !global {
                    out.push_str(rest);
                    break;
                }
                if rest.is_empty() {
                    break;
                }
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

fn expand_replacement(replacement: &str, matched: &str) -> String {
    let mut out = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.push_str(matched),
            '\\' => match chars.next() {
                Some('n') => out.push('\0'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('&') => out.push('&'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '\u{0}' => out.push('\n'),
            other => out.push(other),
        }
    }
    out
}

fn count_in_string(hay: &str, needle: &str, ignore_case: bool) -> i64 {
    if needle.is_empty() {
        return 0;
    }
    let (mut h, mut n) = (hay.to_string(), needle.to_string());
    if ignore_case {
        h = h.to_lowercase();
        n = n.to_lowercase();
    }
    h.matches(&n).count() as i64
}

/// One parsed C-style conversion specification.
struct FmtSpec {
    minus: bool,
    plus: bool,
    space: bool,
    hash: bool,
    zero: bool,
    width: usize,
    precision: Option<usize>,
}

impl FmtSpec {
    /// Apply the field width, honouring "-" (left) and "0" (zero pad).
    fn pad(&self, text: String, numeric: bool) -> String {
        let len = text.chars().count();
        if len >= self.width {
            return text;
        }
        let fill = self.width - len;
        if self.minus {
            return format!("{}{}", text, " ".repeat(fill));
        }
        if numeric && self.zero && self.precision.is_none() {
            let (prefix, rest) = split_numeric_prefix(&text);
            return format!("{}{}{}", prefix, "0".repeat(fill), rest);
        }
        format!("{}{}", " ".repeat(fill), text)
    }
}

/// Split a rendered number into its sign/`0x` prefix and the digits.
fn split_numeric_prefix(s: &str) -> (&str, &str) {
    for prefix in ["-", "+", " "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return (&s[..prefix.len()], rest);
        }
    }
    if s.starts_with("0x") || s.starts_with("0X") {
        return s.split_at(2);
    }
    ("", s)
}

/// Render digits with the precision as a minimum digit count (C rules).
fn digits_with_precision(digits: String, precision: Option<usize>, zero_value: bool) -> String {
    match precision {
        Some(0) if zero_value => String::new(),
        Some(p) if digits.len() < p => format!("{}{}", "0".repeat(p - digits.len()), digits),
        _ => digits,
    }
}

fn format_signed_number(n: i64, spec: &FmtSpec) -> String {
    let negative = n < 0;
    let digits = digits_with_precision(n.unsigned_abs().to_string(), spec.precision, n == 0);
    let sign = if negative {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };
    spec.pad(format!("{sign}{digits}"), true)
}

fn format_unsigned_number(n: u64, base: u32, upper: bool, spec: &FmtSpec) -> String {
    let body = match base {
        10 => n.to_string(),
        16 => {
            if upper {
                format!("{n:X}")
            } else {
                format!("{n:x}")
            }
        }
        8 => format!("{n:o}"),
        2 => format!("{n:b}"),
        _ => n.to_string(),
    };
    let body = digits_with_precision(body, spec.precision, n == 0);
    let prefix = if spec.hash && n != 0 {
        match base {
            16 if upper => "0X",
            16 => "0x",
            8 => "0",
            _ => "",
        }
    } else {
        ""
    };
    spec.pad(format!("{prefix}{body}"), true)
}

/// Render a float conversion (C printf semantics) for f/F/e/E/g/G.
fn format_float_conversion(f: f64, conv: char, spec: &FmtSpec) -> String {
    let precision = spec.precision.unwrap_or(6);
    let sign = if f.is_sign_negative() && !f.is_nan() {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };
    let abs = f.abs();
    let body = match conv {
        'f' | 'F' => format!("{abs:.precision$}"),
        'e' | 'E' => crate::value::fix_exponent(&format!("{abs:.precision$e}")),
        _ => {
            // "%g" uses Vim's own %g rules: no explicit precision means the
            // default 6 significant digits *with* trailing-zero trimming.
            let mut s = match spec.precision {
                Some(p) => crate::value::format_g(abs, p),
                None => crate::value::format_float(abs),
            };
            if spec.hash && !s.contains('.') && !s.contains('e') {
                s.push('.');
            }
            s
        }
    };
    let body = if conv.is_ascii_uppercase() {
        body.to_uppercase()
    } else {
        body
    };
    spec.pad(format!("{sign}{body}"), true)
}

fn printf(fmt: &str, args: &[Value]) -> VimResult<String> {
    let mut out = String::new();
    let mut args_iter = args.iter();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= chars.len() {
            break;
        }
        if chars[i] == '%' {
            out.push('%');
            i += 1;
            continue;
        }
        let mut spec = FmtSpec {
            minus: false,
            plus: false,
            space: false,
            hash: false,
            zero: false,
            width: 0,
            precision: None,
        };
        while i < chars.len() && "-+ #0".contains(chars[i]) {
            match chars[i] {
                '-' => spec.minus = true,
                '+' => spec.plus = true,
                ' ' => spec.space = true,
                '#' => spec.hash = true,
                '0' => spec.zero = true,
                _ => {}
            }
            i += 1;
        }
        while i < chars.len() && chars[i].is_ascii_digit() {
            spec.width = spec.width.saturating_mul(10) + (chars[i] as usize - '0' as usize);
            i += 1;
        }
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            let mut precision = 0usize;
            while i < chars.len() && chars[i].is_ascii_digit() {
                precision = precision
                    .saturating_mul(10)
                    .saturating_add(chars[i] as usize - '0' as usize);
                i += 1;
            }
            spec.precision = Some(precision);
        }
        if i >= chars.len() {
            break;
        }
        let conv = chars[i];
        i += 1;
        let mut next_value = || -> VimResult<Value> {
            args_iter
                .next()
                .cloned()
                .ok_or_else(|| err("E766", "Insufficient arguments for printf()"))
        };
        let rendered = match conv {
            'd' | 'i' => format_signed_number(next_value()?.to_number(), &spec),
            'u' => format_unsigned_number(next_value()?.to_number() as u64, 10, false, &spec),
            'x' => format_unsigned_number(next_value()?.to_number() as u64, 16, false, &spec),
            'X' => format_unsigned_number(next_value()?.to_number() as u64, 16, true, &spec),
            'o' => format_unsigned_number(next_value()?.to_number() as u64, 8, false, &spec),
            'b' => format_unsigned_number(next_value()?.to_number() as u64, 2, false, &spec),
            'c' => {
                let n = next_value()?.to_number();
                let c = char::from_u32(n as u32).unwrap_or('\u{fffd}');
                // Numeric conversion: honour zero padding like an integer.
                spec.pad(c.to_string(), true)
            }
            's' => {
                let mut text = next_value()?.to_string_value();
                if let Some(p) = spec.precision {
                    if p == 0 {
                        text.clear();
                    } else if let Some((cut, _)) = text.char_indices().nth(p) {
                        text.truncate(cut);
                    }
                }
                spec.pad(text, false)
            }
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
                format_float_conversion(next_value()?.to_float()?, conv, &spec)
            }
            other => {
                return Err(err(
                    "E767",
                    format!("Unsupported format specifier: {other}"),
                ));
            }
        };
        out.push_str(&rendered);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Host;

    struct NoHost;
    impl Host for NoHost {}

    fn run(src: &str, env: &mut Env) -> Value {
        let expr = parse_expression(src).unwrap();
        eval(&expr, env, &mut NoHost).unwrap()
    }

    #[test]
    fn map_and_filter_mutate_in_place() {
        // list.c:2305-2424 / dict.c:1302: legacy map()/filter() change the
        // container and return the very same container.
        let mut env = Env::new();
        env.set_var("l", Value::list(vec![Value::Number(1), Value::Number(2)]))
            .unwrap();
        let mapped = run("map(l, {i, v -> v})", &mut env);
        if let Value::List(items) = &mapped {
            items.borrow_mut().push(Value::Number(9));
        }
        assert_eq!(env.get_var("l").unwrap().to_string_value(), "[1, 2, 9]");
        let filtered = run("filter(l, {i, v -> v != 2})", &mut env);
        assert!(matches!(
            (&filtered, env.get_var("l").unwrap()),
            (Value::List(a), Value::List(b)) if std::rc::Rc::ptr_eq(a, &b)
        ));
        assert_eq!(env.get_var("l").unwrap().to_string_value(), "[1, 9]");

        let mut env = Env::new();
        env.set_var(
            "d",
            Value::dict({
                let mut d = VimDict::new();
                d.insert("a", Value::Number(1));
                d.insert("b", Value::Number(2));
                d
            }),
        )
        .unwrap();
        let mapped = run("map(d, {k, v -> v * 10})", &mut env);
        assert!(matches!(
            (&mapped, env.get_var("d").unwrap()),
            (Value::Dict(a), Value::Dict(b)) if std::rc::Rc::ptr_eq(a, &b)
        ));
        assert_eq!(
            env.get_var("d").unwrap().to_string_value(),
            "{'a': 10, 'b': 20}"
        );
        let _ = run("filter(d, {k, v -> v > 10})", &mut env);
        assert_eq!(env.get_var("d").unwrap().to_string_value(), "{'b': 20}");
    }

    #[test]
    fn len_counts_bytes_digits_and_zero_for_other_types() {
        // evalfunc.c:7079-7095.
        let mut env = Env::new();
        assert_eq!(run("len('abc')", &mut env).to_number(), 3);
        assert_eq!(run("len('h\u{e9}llo')", &mut env).to_number(), 6);
        assert_eq!(run("len(123)", &mut env).to_number(), 3);
        assert_eq!(run("len(-5)", &mut env).to_number(), 2);
        assert_eq!(run("len(1.5)", &mut env).to_number(), 0);
        assert_eq!(run("len(v:true)", &mut env).to_number(), 0);
        assert_eq!(run("len([1, 2])", &mut env).to_number(), 2);
        assert_eq!(run("len({'a': 1})", &mut env).to_number(), 1);
    }
}
