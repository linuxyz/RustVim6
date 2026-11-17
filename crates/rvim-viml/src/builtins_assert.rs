//! `assert_*` and `test_*` builtins (test-harness helpers).
//!
//! These are pure functions: they compare values and report failures through
//! `v:errors` / return values, exactly like Vim's own test suite helpers
//! (reference: `reference/vim/src/testing.c`, `evalfunc.c`).
//!
//! ## Failure format
//!
//! Vim's `prepare_assert_error()` prefixes each message with the sourcing
//! position (`file line N:`); the language layer does not track the current
//! script line, so messages here start directly with the assertion text. Every
//! other part follows `fill_assert_error()`: the optional `{msg}: ` prefix,
//! quoting/escaping of values, the `Expected ... but got ...` wording and the
//! dictionary "equal item(s) omitted" summary.
//!
//! The return value matches Vim: `1` when an item was appended to `v:errors`,
//! `0` otherwise. `v:errors` is created on demand and is reset when something
//! else (e.g. a test) put a non-List into it, like Vim's `assert_error()`.
//!
//! ## Host dependencies
//!
//! `assert_beeps()`, `assert_nobeep()` and `assert_fails()` execute an Ex
//! command through [`Host::execute_command`]. A host without a command runner
//! gets an explicit error instead of a fabricated verdict.
//!
//! RustVim does not track beeps, so "did beep" is inferred from the command
//! failing: a failing command is treated as having beeped (the error reaction
//! is the only observable analogue), while a succeeding command is treated as
//! not beeping. Consequently `assert_beeps('call NoSuchFunc()')` passes, and
//! commands that Vim rejects without beeping are reported as beeps.
//!
//! ## `test_*` deviations (documented, deliberate)
//!
//! * `test_garbagecollect_now/soon()`: Vim triggers a tracing-GC pass; values
//!   here are refcounted (`Rc`), so both are no-ops returning 0. Vim's E1142
//!   guard ("while v:testing is not set") is not enforced because `v:testing`
//!   cannot be assigned in this runtime (writable `v:` variables live in the
//!   Ex layer).
//! * `test_alloc_fail()`: validates its arguments and records the request in
//!   `v:test_alloc_fail`, but there is no allocator failure-injection hook.
//! * `test_feedinput()`: records the string in `v:test_feedinput`; the language
//!   layer has no input buffer to inject into.
//! * `test_setmouse()`: records row/col in `v:test_setmouse`; no mouse state.
//! * `test_override()`: validates name/value and records them in
//!   `v:test_override`; the internal debug switches it controls (`redraw`,
//!   `char_avail`, ...) do not exist in this runtime.
//! * `test_ignore_error()`: records the message in `v:test_ignore_error`; the
//!   error-emission path lives in the Ex layer.
//! * `test_option_not_set()`: checks the option exists via `Host::option_get`
//!   (invalid name → E475) and is otherwise a no-op: option "was set"
//!   bookkeeping is not modelled.
//! * `test_getvalue()`: Vim's only key, `"need_fileinfo"`, is always 0 here
//!   (there is no GUI file-info dialog); any other name raises E475.
//! * `test_settime()`: stores seconds in `v:test_settime`, but cannot influence
//!   `localtime()`/`strftime()` (host builtins in rvim-ex) or `reltime()` (a
//!   monotonic clock in `builtins_mr`) from this layer.
//! * `test_srand_seed()`: stores the seed in `v:test_srand_seed`; `rand()` uses
//!   its own thread-local xoshiro state in `builtins_mr` and cannot be reached
//!   from here.
//! * `test_refcount()`: -1 for value types, 0 for containers/funcrefs; `Rc`
//!   reference counts are not Vim refcounts (the interpreter clones values by
//!   design), so exact counts cannot be reported.
//! * `test_null_*()`: [`Value`] has no typed-null variants. Blob/Dict/List/
//!   String nulls become empty containers/strings (Vim treats a NULL list/dict
//!   as empty and `test_null_string()` compares equal to `""`); Function/
//!   Partial become a Funcref with an empty name (`string()` prints
//!   `function('')`, like Vim); Channel/Job/Unknown/Void have no counterpart
//!   and become `v:null`.
//! * `test_gui_event()`: always returns `v:false` (no GUI); event names are
//!   still validated, an unknown event raises E475 like Vim.

use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

use rvim_core::{VimError, VimResult};

use crate::builtins::check_arity;
use crate::env::{Env, Host};
use crate::regex_mvp;
use crate::value::{FuncRef, Value, ValueResult, VimDict};

/// Names implemented in this module (coverage source of truth).
pub const FUNCS: &[&str] = &[
    "assert_beeps",
    "assert_equal",
    "assert_equalfile",
    "assert_exception",
    "assert_fails",
    "assert_false",
    "assert_inrange",
    "assert_match",
    "assert_nobeep",
    "assert_notequal",
    "assert_notmatch",
    "assert_report",
    "assert_true",
    "test_alloc_fail",
    "test_autochdir",
    "test_feedinput",
    "test_garbagecollect_now",
    "test_garbagecollect_soon",
    "test_getvalue",
    "test_gui_event",
    "test_ignore_error",
    "test_null_blob",
    "test_null_channel",
    "test_null_dict",
    "test_null_function",
    "test_null_job",
    "test_null_list",
    "test_null_partial",
    "test_null_string",
    "test_option_not_set",
    "test_override",
    "test_refcount",
    "test_setmouse",
    "test_settime",
    "test_srand_seed",
    "test_unknown",
    "test_void",
];

/// Dispatch a call; `None` when the name is not handled here.
pub fn call(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> Option<ValueResult> {
    if !FUNCS.contains(&name) {
        return None;
    }
    Some((|| {
        check_arity(name, args)?;
        dispatch(name, args, env, host)
    })())
}

fn dispatch(name: &str, args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    match name {
        // -------------------------------------------------- assertions
        "assert_beeps" => assert_beeps(args, env, host, true),
        "assert_nobeep" => assert_beeps(args, env, host, false),
        "assert_equal" => assert_equal_common(args, env, true),
        "assert_notequal" => assert_equal_common(args, env, false),
        "assert_true" => assert_bool(args, env, true),
        "assert_false" => assert_bool(args, env, false),
        "assert_match" => assert_match_common(args, env, true),
        "assert_notmatch" => assert_match_common(args, env, false),
        "assert_inrange" => assert_inrange(args, env),
        "assert_equalfile" => assert_equalfile(args, env),
        "assert_exception" => assert_exception(args, env),
        "assert_fails" => assert_fails(args, env, host),
        "assert_report" => {
            let message = string_arg(&arg(args, 0))?;
            assert_error(env, message);
            Ok(Value::Number(1))
        }

        // ------------------------------------------- test harness hooks
        "test_alloc_fail" => test_alloc_fail(args, env),
        "test_autochdir" => {
            env.vvars
                .insert("test_autochdir".to_string(), Value::Number(1));
            Ok(Value::Number(0))
        }
        "test_feedinput" => {
            let text = string_arg(&arg(args, 0))?;
            env.vvars
                .insert("test_feedinput".to_string(), Value::String(text));
            Ok(Value::Number(0))
        }
        "test_garbagecollect_now" | "test_garbagecollect_soon" => {
            // No tracing GC exists; see the module docs.
            Ok(Value::Number(0))
        }
        "test_getvalue" => test_getvalue(args),
        "test_gui_event" => test_gui_event(args),
        "test_ignore_error" => {
            let message = match arg(args, 0) {
                Value::String(text) => text,
                _ => return Err(err("E474", "Invalid argument")),
            };
            env.vvars
                .insert("test_ignore_error".to_string(), Value::String(message));
            Ok(Value::Number(0))
        }
        "test_null_blob" => Ok(Value::blob(Vec::new())),
        "test_null_channel" | "test_null_job" => Ok(Value::Null),
        "test_null_dict" => Ok(Value::dict(VimDict::new())),
        "test_null_function" | "test_null_partial" => Ok(Value::Funcref(Rc::new(FuncRef::new("")))),
        "test_null_list" => Ok(Value::list(Vec::new())),
        "test_null_string" => Ok(Value::string("")),
        "test_option_not_set" => test_option_not_set(args, host),
        "test_override" => test_override(args, env),
        "test_refcount" => Ok(Value::Number(match arg(args, 0) {
            Value::List(_) | Value::Dict(_) | Value::Blob(_) | Value::Funcref(_) => 0,
            _ => -1,
        })),
        "test_setmouse" => test_setmouse(args, env),
        "test_settime" => {
            let seconds = arg(args, 0).as_number()?;
            env.vvars
                .insert("test_settime".to_string(), Value::Number(seconds));
            Ok(Value::Number(0))
        }
        "test_srand_seed" => {
            let seed = if args.is_empty() {
                0
            } else {
                i64::from(arg(args, 0).as_number()? as u32)
            };
            env.vvars
                .insert("test_srand_seed".to_string(), Value::Number(seed));
            Ok(Value::Number(0))
        }
        "test_unknown" | "test_void" => Ok(Value::Null),
        other => Err(err("E117", format!("Unknown function: {other}"))),
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn err(code: &'static str, message: impl Into<String>) -> VimError {
    VimError::new(code, message)
}

fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).cloned().unwrap_or(Value::Null)
}

/// Vim's legacy `tv_get_string_chk()`: scalars are coerced, containers and
/// blobs raise the matching E-code (`E729`/`E730`/`E731`/`E976`).
fn string_arg(value: &Value) -> VimResult<String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(_) | Value::Float(_) | Value::Bool(_) | Value::Null => {
            Ok(value.to_string_value())
        }
        Value::Funcref(_) => Err(err("E729", "Using a Funcref as a String")),
        Value::List(_) => Err(err("E730", "Using a List as a String")),
        Value::Dict(_) => Err(err("E731", "Using a Dictionary as a String")),
        Value::Blob(_) => Err(err("E976", "Using a Blob as a String")),
    }
}

/// Return the `v:errors` list, replacing a non-List value with a new list the
/// way Vim's `assert_error()` does.
fn errors_list(env: &mut Env) -> Rc<RefCell<Vec<Value>>> {
    if let Some(Value::List(list)) = env.vvars.get("errors") {
        return list.clone();
    }
    let list: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
    env.vvars
        .insert("errors".to_string(), Value::List(list.clone()));
    list
}

/// Append one failure message to `v:errors` (Vim's `assert_error()`).
fn assert_error(env: &mut Env, message: String) {
    errors_list(env).borrow_mut().push(Value::String(message));
}

/// Which assertion produced a failure; mirrors Vim's `assert_type_T`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AssertType {
    Equal,
    Notequal,
    Match,
    Notmatch,
    /// `assert_true()`/`assert_false()`/`assert_inrange()`/`assert_exception()`
    /// and `assert_fails()`: "Expected X but got Y".
    Other,
}

/// Append one character the way `ga_concat_esc()` does: control characters,
/// backslashes and quotes-as-typed are escaped for display.
fn push_escaped_char(out: &mut String, c: char) {
    match c {
        '\u{08}' => out.push_str("\\b"),
        '\u{1b}' => out.push_str("\\e"),
        '\u{0c}' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '\\' => out.push_str("\\\\"),
        c if c < '\u{20}' || c == '\u{7f}' => {
            out.push_str(&format!("\\x{:02x}", c as u32));
        }
        c => out.push(c),
    }
}

/// Append `text` like `ga_concat_shorten_esc()`: escape unprintable
/// characters and compress runs of more than 20 identical characters into
/// `\[x occurs N times]`.
fn push_escaped(out: &mut String, text: &str) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut same = 1;
        while i + same < chars.len() && chars[i + same] == c {
            same += 1;
        }
        if same > 20 {
            out.push_str("\\[");
            push_escaped_char(out, c);
            out.push_str(&format!(" occurs {same} times]"));
            i += same;
        } else {
            push_escaped_char(out, c);
            i += 1;
        }
    }
}

/// Build the message of a failed assertion (Vim's `fill_assert_error()`).
///
/// `exp_str` is used by the boolean checks: Vim renders them as a quoted
/// literal (`Expected 'True' but got 0`). `got` is `None` only for
/// `assert_notequal()`.
fn fill_assert_error(
    msg: Option<&Value>,
    atype: AssertType,
    exp_str: Option<&str>,
    expected: &Value,
    got: Option<&Value>,
) -> String {
    let mut out = String::new();
    if let Some(msg) = msg {
        let skip = matches!(msg, Value::String(text) if text.is_empty());
        if !skip {
            out.push_str(&msg.to_string_value());
            out.push_str(": ");
        }
    }
    match atype {
        AssertType::Match | AssertType::Notmatch => out.push_str("Pattern "),
        AssertType::Notequal => out.push_str("Expected not equal to "),
        AssertType::Equal | AssertType::Other => out.push_str("Expected "),
    }
    match exp_str {
        Some(text) => {
            out.push('\'');
            push_escaped(&mut out, text);
            out.push('\'');
        }
        None => push_escaped(&mut out, &expected.to_repr()),
    }
    match atype {
        AssertType::Notequal => {}
        AssertType::Match => {
            out.push_str(" does not match ");
            if let Some(got) = got {
                push_escaped(&mut out, &got.to_repr());
            }
        }
        AssertType::Notmatch => {
            out.push_str(" does match ");
            if let Some(got) = got {
                push_escaped(&mut out, &got.to_repr());
            }
        }
        AssertType::Equal | AssertType::Other => {
            out.push_str(" but got ");
            if let Some(got) = got {
                push_escaped(&mut out, &got.to_repr());
            }
        }
    }
    out
}

/// Vim's `tv_equal()`: strict by type (numbers and floats differ, a string is
/// never equal to a number), recursive for containers, `VAR_BOOL`/`VAR_SPECIAL`
/// compare by number. Used by `assert_equal()`/`assert_notequal()`.
fn tv_equal(left: &Value, right: &Value, depth: usize) -> bool {
    // Vim's tv_equal() assumes deeply recursive values are equal to stop an
    // endless loop; our values can be cyclic too.
    if depth > 100 {
        return true;
    }
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Bool(a), Value::Null) | (Value::Null, Value::Bool(a)) => i64::from(*a) == 0,
        (Value::Null, Value::Null) => true,
        (Value::Blob(a), Value::Blob(b)) => *a.borrow() == *b.borrow(),
        (Value::Funcref(a), Value::Funcref(b)) => a.name == b.name && a.partial == b.partial,
        (Value::List(a), Value::List(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(a, b)| tv_equal(a, b, depth + 1))
        }
        (Value::Dict(a), Value::Dict(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len()
                && a.iter().all(|(key, value)| match b.get(key) {
                    Some(other) => tv_equal(value, other, depth + 1),
                    None => false,
                })
        }
        // Different types are unequal (Vim's tv_equal() is stricter than the
        // `==` operator).
        _ => false,
    }
}

/// Reduce two unequal dicts to the differing entries for the failure message
/// (Vim's `fill_assert_error()`): equal items are omitted and counted.
fn dict_diff(expected: &Rc<RefCell<VimDict>>, got: &Rc<RefCell<VimDict>>) -> (Value, Value, usize) {
    let expected = expected.borrow();
    let got = got.borrow();
    let mut filtered_expected = VimDict::new();
    let mut filtered_got = VimDict::new();
    let mut omitted = 0;
    for (key, value) in expected.iter() {
        match got.get(key) {
            Some(other) if tv_equal(value, other, 0) => omitted += 1,
            Some(other) => {
                filtered_expected.insert(key.clone(), value.clone());
                filtered_got.insert(key.clone(), other.clone());
            }
            None => {
                filtered_expected.insert(key.clone(), value.clone());
            }
        }
    }
    // Items only present in "got".
    for (key, value) in got.iter() {
        if expected.get(key).is_none() {
            filtered_got.insert(key.clone(), value.clone());
        }
    }
    (
        Value::dict(filtered_expected),
        Value::dict(filtered_got),
        omitted,
    )
}

// ---------------------------------------------------------------------------
// assert_equal() / assert_notequal()
// ---------------------------------------------------------------------------

/// Shared by `assert_equal()` and `assert_notequal()` (testing.c
/// `assert_equal_common()`); `equal` selects the expected relation.
fn assert_equal_common(args: &[Value], env: &mut Env, equal: bool) -> ValueResult {
    let expected = arg(args, 0);
    let actual = arg(args, 1);
    let same = tv_equal(&expected, &actual, 0);
    if same == equal {
        return Ok(Value::Number(0));
    }
    let atype = if equal {
        AssertType::Equal
    } else {
        AssertType::Notequal
    };
    let message = if equal {
        let (expected_for_msg, actual_for_msg, omitted) = match (&expected, &actual) {
            (Value::Dict(expected), Value::Dict(actual)) => dict_diff(expected, actual),
            _ => (expected.clone(), actual.clone(), 0),
        };
        let mut message = fill_assert_error(
            args.get(2),
            atype,
            None,
            &expected_for_msg,
            Some(&actual_for_msg),
        );
        if omitted != 0 {
            message.push_str(&format!(
                " - {omitted} equal item{} omitted",
                if omitted == 1 { "" } else { "s" }
            ));
        }
        message
    } else {
        fill_assert_error(args.get(2), atype, None, &expected, None)
    };
    assert_error(env, message);
    Ok(Value::Number(1))
}

// ---------------------------------------------------------------------------
// assert_true() / assert_false()
// ---------------------------------------------------------------------------

/// Shared by `assert_true()` and `assert_false()` (testing.c `assert_bool()`).
///
/// Like Vim only Numbers and Bools count: `assert_true("x")` fails, while
/// `assert_true(123)` passes.
fn assert_bool(args: &[Value], env: &mut Env, is_true: bool) -> ValueResult {
    let actual = arg(args, 0);
    let pass = match &actual {
        Value::Bool(value) => *value == is_true,
        Value::Number(number) => (*number != 0) == is_true,
        _ => false,
    };
    if pass {
        return Ok(Value::Number(0));
    }
    let expected = if is_true { "True" } else { "False" };
    let message = fill_assert_error(
        args.get(1),
        AssertType::Other,
        Some(expected),
        &actual,
        Some(&actual),
    );
    assert_error(env, message);
    Ok(Value::Number(1))
}

// ---------------------------------------------------------------------------
// assert_match() / assert_notmatch()
// ---------------------------------------------------------------------------

/// Shared by `assert_match()` and `assert_notmatch()` (testing.c
/// `assert_match_common()`). `{pattern}` and `{text}` must be strings; the
/// crate's `regex_mvp` engine supplies the Vim-regex subset.
fn assert_match_common(args: &[Value], env: &mut Env, want_match: bool) -> ValueResult {
    let pattern = string_arg(&arg(args, 0))?;
    let text = string_arg(&arg(args, 1))?;
    let matched = regex_mvp::is_match(&pattern, &text, false);
    if matched == want_match {
        return Ok(Value::Number(0));
    }
    let atype = if want_match {
        AssertType::Match
    } else {
        AssertType::Notmatch
    };
    let message = fill_assert_error(
        args.get(2),
        atype,
        None,
        &Value::String(pattern),
        Some(&Value::String(text)),
    );
    assert_error(env, message);
    Ok(Value::Number(1))
}

// ---------------------------------------------------------------------------
// assert_inrange()
// ---------------------------------------------------------------------------

/// `assert_inrange({lower}, {upper}, {actual} [, {msg}])` (testing.c
/// `assert_inrange()`): compares as Float when any argument is a Float,
/// otherwise as Numbers. `{msg}`, when given, replaces the whole message and is
/// rendered with `tv2string()` (so strings are quoted).
fn assert_inrange(args: &[Value], env: &mut Env) -> ValueResult {
    let float_mode = matches!(arg(args, 0), Value::Float(_))
        || matches!(arg(args, 1), Value::Float(_))
        || matches!(arg(args, 2), Value::Float(_));
    let failed;
    let mut default_message = String::new();
    if float_mode {
        let lower = arg(args, 0).to_float()?;
        let upper = arg(args, 1).to_float()?;
        let actual = arg(args, 2).to_float()?;
        failed = actual < lower || actual > upper;
        if failed {
            default_message = format!(
                "Expected range {} - {}, but got {}",
                crate::value::format_float(lower),
                crate::value::format_float(upper),
                crate::value::format_float(actual)
            );
        }
    } else {
        let lower = arg(args, 0).as_number()?;
        let upper = arg(args, 1).as_number()?;
        let actual = arg(args, 2).as_number()?;
        failed = actual < lower || actual > upper;
        if failed {
            default_message = format!("Expected range {lower} - {upper}, but got {actual}");
        }
    }
    if !failed {
        return Ok(Value::Number(0));
    }
    let message = match args.get(3) {
        Some(msg) => msg.to_repr(),
        None => default_message,
    };
    assert_error(env, message);
    Ok(Value::Number(1))
}

// ---------------------------------------------------------------------------
// assert_exception()
// ---------------------------------------------------------------------------

/// `assert_exception({error} [, {msg}])` (testing.c `f_assert_exception()`):
/// `{error}` must be a substring of `v:exception`.
fn assert_exception(args: &[Value], env: &mut Env) -> ValueResult {
    let expected = string_arg(&arg(args, 0))?;
    let exception = env
        .vvars
        .get("exception")
        .map(Value::to_string_value)
        .unwrap_or_default();
    if exception.is_empty() {
        assert_error(env, "v:exception is not set".to_string());
        return Ok(Value::Number(1));
    }
    if exception.contains(&expected) {
        return Ok(Value::Number(0));
    }
    let message = fill_assert_error(
        args.get(1),
        AssertType::Other,
        None,
        &Value::String(expected),
        Some(&Value::String(exception)),
    );
    assert_error(env, message);
    Ok(Value::Number(1))
}

// ---------------------------------------------------------------------------
// assert_report()
// ---------------------------------------------------------------------------

// `assert_report()` is dispatched inline above: it always reports and returns 1.

// ---------------------------------------------------------------------------
// assert_beeps() / assert_nobeep()
// ---------------------------------------------------------------------------

/// Shared by `assert_beeps()` and `assert_nobeep()` (testing.c
/// `assert_beeps()`).
///
/// The command is run through [`Host::execute_command`]. RustVim has no beep
/// tracking, so a failing command is treated as the beep: `expect_beep` is the
/// assertion ("did beep" / "did not beep").
fn assert_beeps(
    args: &[Value],
    env: &mut Env,
    host: &mut dyn Host,
    expect_beep: bool,
) -> ValueResult {
    let command = string_arg(&arg(args, 0))?;
    let Some(executed) = host.execute_command(&command, env) else {
        return Err(err(
            "E117",
            "assert_beeps()/assert_nobeep() require the Ex command runner (host builtin)",
        ));
    };
    let beeped = executed.is_err();
    if beeped == expect_beep {
        return Ok(Value::Number(0));
    }
    let mut message = if expect_beep {
        "command did not beep: "
    } else {
        "command did beep: "
    }
    .to_string();
    message.push_str(&command);
    assert_error(env, message);
    Ok(Value::Number(1))
}

// ---------------------------------------------------------------------------
// assert_equalfile()
// ---------------------------------------------------------------------------

/// Size of Vim's per-line display buffer in `assert_equalfile()` (200 bytes,
/// sliding by 100).
const EQUALFILE_LINE_BUF: usize = 199;

/// `assert_equalfile({fname-one}, {fname-two} [, {msg}])` (testing.c
/// `assert_equalfile()`): a byte-wise comparison that reports the first
/// difference together with the tail of the two lines.
fn assert_equalfile(args: &[Value], env: &mut Env) -> ValueResult {
    let first_name = string_arg(&arg(args, 0))?;
    let second_name = string_arg(&arg(args, 1))?;
    let prefix = match args.get(2) {
        Some(msg) => format!("{}: ", msg.to_string_value()),
        None => String::new(),
    };
    let read_error = |name: &str| format!("E485: Can't read file {name}");
    let first = match fs::read(&first_name) {
        Ok(bytes) => bytes,
        Err(_) => {
            assert_error(env, format!("{prefix}{}", read_error(&first_name)));
            return Ok(Value::Number(1));
        }
    };
    let second = match fs::read(&second_name) {
        Ok(bytes) => bytes,
        Err(_) => {
            assert_error(env, format!("{prefix}{}", read_error(&second_name)));
            return Ok(Value::Number(1));
        }
    };
    // Port of the reference loop: walk both files byte by byte, keeping the
    // current line's tail for the "after ..." part of the message.
    let mut problem: Option<String> = None;
    let mut line1: Vec<u8> = Vec::new();
    let mut line2: Vec<u8> = Vec::new();
    let mut lineidx = 0usize;
    let mut line = 1usize;
    let mut index = 0usize;
    loop {
        let c1 = first.get(index).copied();
        let c2 = second.get(index).copied();
        match (c1, c2) {
            (None, None) => break,
            (None, Some(_)) => {
                problem = Some("first file is shorter".to_string());
                break;
            }
            (Some(_), None) => {
                problem = Some("second file is shorter".to_string());
                break;
            }
            (Some(a), Some(b)) => {
                line1.push(a);
                line2.push(b);
                lineidx += 1;
                if a != b {
                    problem = Some(format!("difference at byte {index}, line {line}"));
                    break;
                }
                if a == b'\n' {
                    line += 1;
                    lineidx = 0;
                    line1.clear();
                    line2.clear();
                } else if line1.len() + 2 == EQUALFILE_LINE_BUF + 1 {
                    line1.drain(..100);
                    line2.drain(..100);
                    lineidx -= 100;
                }
            }
        }
        index += 1;
    }
    let Some(mut problem) = problem else {
        return Ok(Value::Number(0));
    };
    if lineidx > 0 {
        let line1 = String::from_utf8_lossy(&line1);
        let line2 = String::from_utf8_lossy(&line2);
        problem.push_str(&format!(" after \"{line1}\""));
        if line1 != line2 {
            problem.push_str(&format!(" vs \"{line2}\""));
        }
    }
    assert_error(env, format!("{prefix}{problem}"));
    Ok(Value::Number(1))
}

// ---------------------------------------------------------------------------
// assert_fails()
// ---------------------------------------------------------------------------

/// What `assert_append_cmd_or_arg()` appends: the message when both `{error}`
/// and `{msg}` are given, otherwise the command itself.
fn command_or_message(args: &[Value], command: &str) -> String {
    if args.len() > 2 {
        arg(args, 2).to_string_value()
    } else {
        command.to_string()
    }
}

/// `assert_fails({cmd} [, {error} [, {msg} [, {lnum} [, {context}]]]])`
/// (testing.c `f_assert_fails()`).
///
/// `{error}` is a plain substring (String) or a one/two item List of Vim
/// patterns matched against the error message and `v:errmsg`. `{lnum}` and
/// `{context}` verification needs the command-line tracking of the Ex
/// dispatcher, which [`Host::execute_command`] does not expose; explicitly
/// asking for it raises E475 rather than silently passing.
fn assert_fails(args: &[Value], env: &mut Env, host: &mut dyn Host) -> ValueResult {
    let command = match arg(args, 0) {
        Value::String(command) => command,
        Value::Number(number) => number.to_string(),
        _ => return Err(err("E1210", "String or Number required for argument 1")),
    };
    if args.len() > 1 && !matches!(args[1], Value::String(_) | Value::List(_)) {
        return Err(err("E1222", "String or List required for argument 2"));
    }
    if args.len() > 3 && !matches!(args[3], Value::Number(_)) {
        return Err(err(
            "E1115",
            "\"assert_fails()\" fourth argument must be a number",
        ));
    }
    if args.len() > 4 && !matches!(args[4], Value::String(_)) {
        return Err(err(
            "E1116",
            "\"assert_fails()\" fifth argument must be a string",
        ));
    }
    let Some(executed) = host.execute_command(&command, env) else {
        return Err(err(
            "E117",
            "assert_fails() requires the Ex command runner (host builtin)",
        ));
    };
    let error = match executed {
        Ok(()) => {
            let message = format!(
                "command did not fail: {}",
                command_or_message(args, &command)
            );
            assert_error(env, message);
            return Ok(Value::Number(1));
        }
        Err(error) => error,
    };
    if args.len() < 2 {
        return Ok(Value::Number(0));
    }
    let actual = error.to_string();
    let mut error_found = false;
    let mut expected_str: Option<String> = None;
    match &args[1] {
        Value::String(expected) => {
            error_found = !actual.contains(expected.as_str());
        }
        Value::List(list) => {
            let items = list.borrow().clone();
            if items.is_empty() || items.len() > 2 {
                return Err(err(
                    "E856",
                    "\"assert_fails()\" second argument must be a string or a list with one or two strings",
                ));
            }
            let first = string_arg(&items[0])?;
            if !regex_mvp::is_match(&first, &actual, false) {
                error_found = true;
                expected_str = Some(first);
            } else if items.len() == 2 {
                let second = string_arg(&items[1])?;
                let errmsg = env
                    .vvars
                    .get("errmsg")
                    .map(Value::to_string_value)
                    .unwrap_or_default();
                if !regex_mvp::is_match(&second, &errmsg, false) {
                    error_found = true;
                    expected_str = Some(second);
                }
            }
        }
        _ => unreachable!("argument 2 type was validated above"),
    }
    if !error_found && args.len() > 3 {
        // Vim only verifies {lnum}/{context} when {msg} is given as well.
        if args.len() > 2 {
            let lnum = args[3].to_number();
            if lnum >= 0 {
                return Err(err(
                    "E475",
                    "assert_fails() cannot verify the {lnum} argument in this runtime",
                ));
            }
            if args.len() > 4 && !matches!(&args[4], Value::String(context) if context.is_empty()) {
                return Err(err(
                    "E475",
                    "assert_fails() cannot verify the {context} argument in this runtime",
                ));
            }
        }
    }
    if !error_found {
        return Ok(Value::Number(0));
    }
    let mut message = fill_assert_error(
        args.get(2),
        AssertType::Other,
        expected_str.as_deref(),
        &arg(args, 1),
        Some(&Value::String(actual)),
    );
    message.push_str(": ");
    message.push_str(&command_or_message(args, &command));
    assert_error(env, message);
    Ok(Value::Number(1))
}

// ---------------------------------------------------------------------------
// test_* hooks
// ---------------------------------------------------------------------------

/// `test_alloc_fail({id}, {countdown}, {repeat})` (testing.c
/// `f_test_alloc_fail()`): argument validation is Vim's; the allocator has no
/// failure-injection hook, so the request is only recorded in
/// `v:test_alloc_fail`.
fn test_alloc_fail(args: &[Value], env: &mut Env) -> ValueResult {
    let (Value::Number(id), Value::Number(countdown), Value::Number(repeat)) =
        (arg(args, 0), arg(args, 1), arg(args, 2))
    else {
        return Err(err("E474", "Invalid argument"));
    };
    if id <= 0 || countdown < 0 {
        return Err(err("E474", "Invalid argument"));
    }
    env.vvars.insert(
        "test_alloc_fail".to_string(),
        Value::list(vec![
            Value::Number(id),
            Value::Number(countdown),
            Value::Number(repeat),
        ]),
    );
    Ok(Value::Number(0))
}

/// `test_getvalue({name})` (testing.c `f_test_getvalue()`): the only internal
/// value Vim exposes, `"need_fileinfo"`, is 0 for a headless runtime.
fn test_getvalue(args: &[Value]) -> ValueResult {
    let Value::String(name) = arg(args, 0) else {
        return Err(err("E474", "Invalid argument"));
    };
    if name == "need_fileinfo" {
        Ok(Value::Number(0))
    } else {
        Err(err("E475", format!("Invalid argument: {name}")))
    }
}

/// `test_gui_event({event}, {args})` (testing.c `f_test_gui_event()`): no GUI
/// exists, so every known event returns `v:false`; unknown event names raise
/// E475 and a non-dict second argument (or no GUI) keeps Vim's "false" result.
fn test_gui_event(args: &[Value]) -> ValueResult {
    if !matches!(arg(args, 1), Value::Dict(_)) {
        return Ok(Value::Bool(false));
    }
    let event = string_arg(&arg(args, 0))?;
    match event.as_str() {
        "dropfiles" | "findrepl" | "mouse" | "scrollbar" | "tabline" | "tabmenu" => {
            Ok(Value::Bool(false))
        }
        other => Err(err("E475", format!("Invalid argument: {other}"))),
    }
}

/// `test_option_not_set({name})` (testing.c `f_test_option_not_set()`): the
/// option must exist, but this runtime does not track "was set" flags.
fn test_option_not_set(args: &[Value], host: &mut dyn Host) -> ValueResult {
    let Value::String(name) = arg(args, 0) else {
        return Err(err("E474", "Invalid argument"));
    };
    if host.option_get(&name).is_none() {
        return Err(err("E475", format!("Invalid argument: {name}")));
    }
    Ok(Value::Number(0))
}

/// Names accepted by `test_override()` in the reference Vim 8.2 (testing.c
/// `f_test_override()`).
const TEST_OVERRIDE_NAMES: &[&str] = &[
    "ALL",
    "autoload",
    "char_avail",
    "nfa_fail",
    "no_query_mouse",
    "no_wait_return",
    "redraw",
    "redraw_flag",
    "starting",
    "term_props",
    "ui_delay",
    "uptime",
    "vterm_title",
];

/// `test_override({name}, {val})` (testing.c `f_test_override()`): validates
/// the pair like Vim and records it in `v:test_override`; the internal debug
/// switches themselves do not exist in this runtime.
fn test_override(args: &[Value], env: &mut Env) -> ValueResult {
    let (Value::String(name), Value::Number(value)) = (arg(args, 0), arg(args, 1)) else {
        return Err(err("E474", "Invalid argument"));
    };
    if !TEST_OVERRIDE_NAMES.contains(&name.as_str()) {
        return Err(err("E475", format!("Invalid argument: {name}")));
    }
    let dict = match env.vvars.get("test_override") {
        Some(Value::Dict(dict)) => dict.clone(),
        _ => {
            let dict: Rc<RefCell<VimDict>> = Rc::new(RefCell::new(VimDict::new()));
            env.vvars
                .insert("test_override".to_string(), Value::Dict(dict.clone()));
            dict
        }
    };
    if name == "ALL" {
        *dict.borrow_mut() = VimDict::new();
    }
    dict.borrow_mut().insert(name, Value::Number(value));
    Ok(Value::Number(0))
}

/// `test_setmouse({row}, {col})` (testing.c `f_test_setmouse()`): validates
/// Numbers and records the position in `v:test_setmouse`; mouse handling is not
/// modelled in this runtime.
fn test_setmouse(args: &[Value], env: &mut Env) -> ValueResult {
    let (Value::Number(row), Value::Number(col)) = (arg(args, 0), arg(args, 1)) else {
        return Err(err("E474", "Invalid argument"));
    };
    env.vvars.insert(
        "test_setmouse".to_string(),
        Value::list(vec![Value::Number(row), Value::Number(col)]),
    );
    Ok(Value::Number(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Default host: no buffers, options or command runner.
    struct H;

    impl Host for H {}

    /// Host that runs (or refuses) every Ex command line.
    struct CmdHost {
        fail: bool,
        /// `None` makes `execute_command` unavailable (like the dummy host).
        available: bool,
        lines: Vec<String>,
    }

    impl CmdHost {
        fn new(fail: bool) -> Self {
            Self {
                fail,
                available: true,
                lines: Vec::new(),
            }
        }
    }

    impl Host for CmdHost {
        fn execute_command(&mut self, line: &str, _env: &mut Env) -> Option<VimResult<()>> {
            if !self.available {
                return None;
            }
            self.lines.push(line.to_string());
            if self.fail {
                Some(Err(VimError::new(
                    "E492",
                    format!("Not an editor command: {line}"),
                )))
            } else {
                Some(Ok(()))
            }
        }
    }

    /// Host with a single known option, for `test_option_not_set()`.
    struct OptionHost;

    impl Host for OptionHost {
        fn option_get(&mut self, name: &str) -> Option<Value> {
            (name == "ignorecase").then_some(Value::Number(0))
        }
    }

    fn call_with(name: &str, args: &[Value]) -> (ValueResult, Env) {
        let mut env = Env::new();
        let mut host = H;
        let result = call(name, args, &mut env, &mut host).expect("function should be handled");
        (result, env)
    }

    fn call_host(name: &str, args: &[Value], host: &mut dyn Host) -> (ValueResult, Env) {
        let mut env = Env::new();
        let result = call(name, args, &mut env, host).expect("function should be handled");
        (result, env)
    }

    fn number(name: &str, args: &[Value]) -> i64 {
        call_with(name, args).0.unwrap().to_number()
    }

    fn errors(env: &Env) -> Vec<String> {
        match env.vvars.get("errors") {
            Some(Value::List(list)) => list.borrow().iter().map(Value::to_string_value).collect(),
            _ => Vec::new(),
        }
    }

    fn failed(name: &str, args: &[Value]) -> (i64, Vec<String>) {
        let (result, env) = call_with(name, args);
        (result.unwrap().to_number(), errors(&env))
    }

    fn s(text: &str) -> Value {
        Value::string(text)
    }

    fn n(number: i64) -> Value {
        Value::Number(number)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rvim_assert_{}_{}_{}",
            std::process::id(),
            tag,
            count
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unknown_names_return_none() {
        let mut env = Env::new();
        let mut host = H;
        assert!(call("strlen", &[s("x")], &mut env, &mut host).is_none());
        assert!(call("assert_equal", &[n(1)], &mut env, &mut host).is_some());
    }

    #[test]
    fn funcs_are_registered_in_the_builtin_table() {
        for name in FUNCS {
            assert!(
                crate::builtins::is_builtin(name),
                "{name} is not aggregated"
            );
        }
    }

    #[test]
    fn arity_is_checked() {
        let (result, _) = call_with("assert_equal", &[n(1)]);
        assert_eq!(result.unwrap_err().code, "E119");
        let (result, _) = call_with("assert_equal", &[n(1), n(1), s("m"), s("extra")]);
        assert_eq!(result.unwrap_err().code, "E118");
    }

    #[test]
    fn assert_equal_passes_when_values_match() {
        assert_eq!(number("assert_equal", &[n(4), n(4)]), 0);
        assert_eq!(number("assert_equal", &[s("hi"), s("hi")]), 0);
        assert_eq!(
            number(
                "assert_equal",
                &[
                    Value::list(vec![n(1), s("a")]),
                    Value::list(vec![n(1), s("a")])
                ]
            ),
            0
        );
        // v:true == 1 by number in legacy Vim? No: tv_equal keeps types apart.
        assert_eq!(number("assert_equal", &[Value::Bool(true), n(1)]), 1);
    }

    #[test]
    fn assert_equal_failure_pushes_expected_but_got() {
        let (result, errors) = failed("assert_equal", &[s("abc"), s("abd")]);
        assert_eq!(result, 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Expected 'abc' but got 'abd'"));
        assert_eq!(results_with_msg(), vec!["mymsg: Expected 'a' but got 'b'"]);
    }

    fn results_with_msg() -> Vec<String> {
        let (result, env) = call_with("assert_equal", &[s("a"), s("b"), s("mymsg")]);
        assert_eq!(result.unwrap().to_number(), 1);
        errors(&env)
    }

    #[test]
    fn assert_equal_repr_quotes_and_escapes() {
        let (_, errors) = failed(
            "assert_equal",
            &[
                s("XxxxxxxxxxxxxxxxxxxxxxX"),
                s("XyyyyyyyyyyyyyyyyyyyyyyyyyX"),
            ],
        );
        assert!(errors[0].contains("Expected 'X\\[x occurs 21 times]X'"));
        assert!(errors[0].contains("but got 'X\\[y occurs 25 times]X'"));

        let (_, errors) = failed("assert_equal", &[s("\u{8}\u{1b}\u{c}\n\t\r\\"), s("x")]);
        assert!(errors[0].contains("Expected '\\b\\e\\f\\n\\t\\r\\\\'"));
    }

    #[test]
    fn assert_equal_dict_omits_equal_items() {
        let mut expected = VimDict::new();
        expected.insert("a", n(1));
        expected.insert("b", n(2));
        let mut actual = VimDict::new();
        actual.insert("a", n(1));
        actual.insert("b", n(3));
        let (result, errors) = failed(
            "assert_equal",
            &[Value::dict(expected), Value::dict(actual)],
        );
        assert_eq!(result, 1);
        assert_eq!(
            errors[0],
            "Expected {'b': 2} but got {'b': 3} - 1 equal item omitted"
        );
    }

    #[test]
    fn assert_notequal() {
        assert_eq!(number("assert_notequal", &[s("foo"), s("bar")]), 0);
        let (result, errors) = failed("assert_notequal", &[s("foo"), s("foo")]);
        assert_eq!(result, 1);
        assert!(errors[0].contains("Expected not equal to 'foo'"));
    }

    #[test]
    fn assert_true_and_false() {
        assert_eq!(number("assert_true", &[n(1)]), 0);
        assert_eq!(number("assert_true", &[Value::Bool(true)]), 0);
        assert_eq!(number("assert_false", &[n(0)]), 0);
        assert_eq!(number("assert_false", &[Value::Bool(false)]), 0);

        let (result, errors) = failed("assert_true", &[n(0)]);
        assert_eq!(result, 1);
        assert!(errors[0].contains("Expected 'True' but got 0"));

        let (result, errors) = failed("assert_false", &[n(123)]);
        assert_eq!(result, 1);
        assert!(errors[0].contains("Expected 'False' but got 123"));

        // Only Numbers and Bools are considered.
        assert_eq!(number("assert_true", &[s("x")]), 1);
    }

    #[test]
    fn assert_match_and_notmatch() {
        assert_eq!(number("assert_match", &[s("^f.*b.*r$"), s("foobar")]), 0);
        assert_eq!(number("assert_notmatch", &[s("foo"), s("bar")]), 0);

        let (result, errors) = failed("assert_match", &[s("bar.*foo"), s("foobar")]);
        assert_eq!(result, 1);
        assert!(errors[0].contains("Pattern 'bar.*foo' does not match 'foobar'"));

        let (result, errors) = failed("assert_notmatch", &[s("foo"), s("foobar")]);
        assert_eq!(result, 1);
        assert!(errors[0].contains("Pattern 'foo' does match 'foobar'"));

        // Containers cannot be used as patterns/text.
        let (result, _) = call_with("assert_match", &[Value::list(vec![]), s("x")]);
        assert_eq!(result.unwrap_err().code, "E730");
    }

    #[test]
    fn assert_inrange() {
        assert_eq!(number("assert_inrange", &[n(7), n(7), n(7)]), 0);
        assert_eq!(number("assert_inrange", &[n(5), n(7), n(6)]), 0);
        assert_eq!(
            number("assert_inrange", &[n(5), n(7), Value::Float(5.0)]),
            0
        );

        let (result, errors) = failed("assert_inrange", &[n(5), n(7), n(4)]);
        assert_eq!(result, 1);
        assert!(errors[0].contains("Expected range 5 - 7, but got 4"));

        // Float formatting matches Vim's %g rendering.
        let (result, errors) = failed("assert_inrange", &[n(5), n(7), Value::Float(4.0)]);
        assert_eq!(result, 1);
        assert!(errors[0].contains("Expected range 5.0 - 7.0, but got 4.0"));

        // An explicit message replaces the default one (tv2string form).
        let (result, errors) = failed("assert_inrange", &[n(1), n(2), n(5), s("bad")]);
        assert_eq!(result, 1);
        assert_eq!(errors[0], "'bad'");
    }

    #[test]
    fn assert_report_always_reports() {
        let (result, errors) = failed("assert_report", &[s("something is wrong")]);
        assert_eq!(result, 1);
        assert_eq!(errors, vec!["something is wrong"]);
    }

    #[test]
    fn assert_exception() {
        let (result, env) = call_with("assert_exception", &[s("E123:")]);
        assert_eq!(result.unwrap().to_number(), 1);
        assert!(errors(&env)[0].contains("v:exception is not set"));

        // Matching exception passes.
        let mut env = Env::new();
        let mut host = H;
        env.vvars.insert(
            "exception".to_string(),
            s("Vim:E492: Not an editor command"),
        );
        let result = call("assert_exception", &[s("E492:")], &mut env, &mut host)
            .unwrap()
            .unwrap();
        assert_eq!(result.to_number(), 0);

        // Non-matching exception reports expected/got.
        let result = call("assert_exception", &[s("E12345:")], &mut env, &mut host)
            .unwrap()
            .unwrap();
        assert_eq!(result.to_number(), 1);
        assert!(errors(&env)[0].contains("Expected 'E12345:' but got 'Vim:E492:"));
    }

    #[test]
    fn assert_equalfile_equal_and_differing() {
        let dir = temp_dir("equal");
        let one = dir.join("one.txt");
        let two = dir.join("two.txt");
        fs::write(&one, "one\ntwo\nthree\n").unwrap();
        fs::write(&two, "one\ntwo\nthree\n").unwrap();
        assert_eq!(
            number(
                "assert_equalfile",
                &[
                    Value::String(one.to_string_lossy().into_owned()),
                    Value::String(two.to_string_lossy().into_owned()),
                ]
            ),
            0
        );

        fs::write(&one, "1234X89").unwrap();
        fs::write(&two, "1234Y89").unwrap();
        let (result, errors) = failed(
            "assert_equalfile",
            &[
                Value::String(one.to_string_lossy().into_owned()),
                Value::String(two.to_string_lossy().into_owned()),
            ],
        );
        assert_eq!(result, 1);
        assert!(
            errors[0].contains("difference at byte 4, line 1 after \"1234X\" vs \"1234Y\""),
            "{errors:?}"
        );

        fs::write(&one, "only\n").unwrap();
        fs::write(&two, "only\ntwo\n").unwrap();
        let (_, errors) = failed(
            "assert_equalfile",
            &[
                Value::String(one.to_string_lossy().into_owned()),
                Value::String(two.to_string_lossy().into_owned()),
            ],
        );
        assert!(errors[0].contains("first file is shorter"));

        fs::write(&one, "one\ntwo\n").unwrap();
        fs::write(&two, "one\n").unwrap();
        let (_, errors) = failed(
            "assert_equalfile",
            &[
                Value::String(one.to_string_lossy().into_owned()),
                Value::String(two.to_string_lossy().into_owned()),
            ],
        );
        assert!(errors[0].contains("second file is shorter"));

        fs::write(&one, "1234X89").unwrap();
        let (_, errors) = failed(
            "assert_equalfile",
            &[
                Value::String(one.to_string_lossy().into_owned()),
                s("does-not-exist"),
                s("a message"),
            ],
        );
        assert!(errors[0].starts_with("a message: E485: Can't read file"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn assert_beeps_uses_command_failure_as_beep_proxy() {
        // A failing command is the "beep": the assertion passes.
        let mut host = CmdHost::new(true);
        let (result, env) = call_host("assert_beeps", &[s("call SomeBadFunc()")], &mut host);
        assert_eq!(result.unwrap().to_number(), 0);
        assert!(errors(&env).is_empty());
        assert_eq!(host.lines, vec!["call SomeBadFunc()"]);

        // A command that does not fail "did not beep".
        let mut host = CmdHost::new(false);
        let (result, env) = call_host("assert_beeps", &[s("echo 1")], &mut host);
        assert_eq!(result.unwrap().to_number(), 1);
        assert!(errors(&env)[0].contains("command did not beep: echo 1"));

        // Host without a command runner: explicit error, no fake verdict.
        let mut host = CmdHost {
            fail: false,
            available: false,
            lines: Vec::new(),
        };
        let (result, _) = call_host("assert_beeps", &[s("echo 1")], &mut host);
        assert_eq!(result.unwrap_err().code, "E117");
    }

    #[test]
    fn assert_nobeep() {
        let mut host = CmdHost::new(false);
        let (result, env) = call_host("assert_nobeep", &[s("echo 1")], &mut host);
        assert_eq!(result.unwrap().to_number(), 0);
        assert!(errors(&env).is_empty());

        let mut host = CmdHost::new(true);
        let (result, env) = call_host("assert_nobeep", &[s("call SomeBadFunc()")], &mut host);
        assert_eq!(result.unwrap().to_number(), 1);
        assert!(errors(&env)[0].contains("command did beep: call SomeBadFunc()"));
    }

    #[test]
    fn assert_fails() {
        let mut host = CmdHost::new(true);
        let (result, env) = call_host("assert_fails", &[s("xxx")], &mut host);
        assert_eq!(result.unwrap().to_number(), 0);
        assert!(errors(&env).is_empty());
        assert_eq!(host.lines, vec!["xxx"]);

        // Matching expected error passes.
        let mut host = CmdHost::new(true);
        let (result, _) = call_host("assert_fails", &[s("xxx"), s("E492:")], &mut host);
        assert_eq!(result.unwrap().to_number(), 0);

        // Mismatching expected error reports both values and the command.
        let mut host = CmdHost::new(true);
        let (result, env) = call_host("assert_fails", &[s("xxx"), s("E12345")], &mut host);
        assert_eq!(result.unwrap().to_number(), 1);
        assert_eq!(
            errors(&env)[0],
            "Expected 'E12345' but got 'E492: Not an editor command: xxx': xxx"
        );

        // A one-item List is matched with a Vim pattern.
        let mut host = CmdHost::new(true);
        let (result, _) = call_host(
            "assert_fails",
            &[s("xxx"), Value::list(vec![s("E4.*:")])],
            &mut host,
        );
        assert_eq!(result.unwrap().to_number(), 0);

        // A command that does not fail.
        let mut host = CmdHost::new(false);
        let (result, env) = call_host(
            "assert_fails",
            &[s("echo"), s(""), s("echo command")],
            &mut host,
        );
        assert_eq!(result.unwrap().to_number(), 1);
        assert_eq!(errors(&env)[0], "command did not fail: echo command");

        // Invalid argument types and list shapes.
        let mut host = CmdHost::new(true);
        let (result, _) = call_host("assert_fails", &[Value::Float(1.0)], &mut host);
        assert_eq!(result.unwrap_err().code, "E1210");
        let mut host = CmdHost::new(true);
        let (result, _) = call_host("assert_fails", &[s("xxx"), n(1)], &mut host);
        assert_eq!(result.unwrap_err().code, "E1222");
        let mut host = CmdHost::new(true);
        let (result, _) = call_host(
            "assert_fails",
            &[s("xxx"), Value::list(Vec::new())],
            &mut host,
        );
        assert_eq!(result.unwrap_err().code, "E856");
        let mut host = CmdHost::new(true);
        let (result, _) = call_host(
            "assert_fails",
            &[s("xxx"), s("E492"), s("m"), s("lnum")],
            &mut host,
        );
        assert_eq!(result.unwrap_err().code, "E1115");

        // Unverifiable {lnum}/{context} is an explicit error, not a fake pass.
        let mut host = CmdHost::new(true);
        let (result, _) = call_host(
            "assert_fails",
            &[s("xxx"), s("E492"), s("m"), n(3)],
            &mut host,
        );
        assert_eq!(result.unwrap_err().code, "E475");
    }

    #[test]
    fn test_alloc_fail() {
        let (result, env) = call_with("test_alloc_fail", &[n(3), n(2), n(1)]);
        assert_eq!(result.unwrap().to_number(), 0);
        match env.vvars.get("test_alloc_fail") {
            Some(Value::List(list)) => assert_eq!(list.borrow().len(), 3),
            other => panic!("expected recorded request, got {other:?}"),
        }
        let (result, _) = call_with("test_alloc_fail", &[n(0), n(1), n(1)]);
        assert_eq!(result.unwrap_err().code, "E474");
        let (result, _) = call_with("test_alloc_fail", &[n(1), n(-1), n(1)]);
        assert_eq!(result.unwrap_err().code, "E474");
        let (result, _) = call_with("test_alloc_fail", &[s("x"), n(1), n(1)]);
        assert_eq!(result.unwrap_err().code, "E474");
    }

    #[test]
    fn test_autochdir_and_feedinput() {
        let (result, env) = call_with("test_autochdir", &[]);
        assert_eq!(result.unwrap().to_number(), 0);
        assert_eq!(
            env.vvars.get("test_autochdir").map(Value::to_number),
            Some(1)
        );

        let (result, env) = call_with("test_feedinput", &[s("abc")]);
        assert_eq!(result.unwrap().to_number(), 0);
        assert_eq!(
            env.vvars.get("test_feedinput").map(Value::to_string_value),
            Some("abc".to_string())
        );
        let (result, _) = call_with("test_feedinput", &[Value::list(vec![])]);
        assert_eq!(result.unwrap_err().code, "E730");
    }

    #[test]
    fn test_garbagecollect_is_a_noop() {
        assert_eq!(number("test_garbagecollect_now", &[]), 0);
        assert_eq!(number("test_garbagecollect_soon", &[]), 0);
    }

    #[test]
    fn test_getvalue() {
        assert_eq!(number("test_getvalue", &[s("need_fileinfo")]), 0);
        let (result, _) = call_with("test_getvalue", &[s("bogus")]);
        assert_eq!(result.unwrap_err().code, "E475");
        let (result, _) = call_with("test_getvalue", &[n(1)]);
        assert_eq!(result.unwrap_err().code, "E474");
    }

    #[test]
    fn test_gui_event() {
        let mut dict = VimDict::new();
        dict.insert("row", n(1));
        let (result, _) = call_with("test_gui_event", &[s("mouse"), Value::dict(dict)]);
        assert!(matches!(result.unwrap(), Value::Bool(false)));

        // Non-dict arguments keep Vim's "false" result.
        let (result, _) = call_with("test_gui_event", &[s("mouse"), n(1)]);
        assert!(matches!(result.unwrap(), Value::Bool(false)));

        let (result, _) = call_with("test_gui_event", &[s("bogus"), Value::dict(VimDict::new())]);
        assert_eq!(result.unwrap_err().code, "E475");
    }

    #[test]
    fn test_ignore_error() {
        let (result, env) = call_with("test_ignore_error", &[s("E123")]);
        assert_eq!(result.unwrap().to_number(), 0);
        assert_eq!(
            env.vvars
                .get("test_ignore_error")
                .map(Value::to_string_value),
            Some("E123".to_string())
        );
        let (result, _) = call_with("test_ignore_error", &[n(1)]);
        assert_eq!(result.unwrap_err().code, "E474");
    }

    #[test]
    fn test_null_values() {
        match call_with("test_null_blob", &[]).0.unwrap() {
            Value::Blob(blob) => assert!(blob.borrow().is_empty()),
            other => panic!("expected blob, got {other:?}"),
        }
        match call_with("test_null_dict", &[]).0.unwrap() {
            Value::Dict(dict) => assert!(dict.borrow().is_empty()),
            other => panic!("expected dict, got {other:?}"),
        }
        match call_with("test_null_list", &[]).0.unwrap() {
            Value::List(list) => assert!(list.borrow().is_empty()),
            other => panic!("expected list, got {other:?}"),
        }
        assert_eq!(
            call_with("test_null_string", &[])
                .0
                .unwrap()
                .to_string_value(),
            ""
        );
        // A Funcref with an empty name prints like Vim's NULL funcref.
        assert_eq!(
            call_with("test_null_function", &[]).0.unwrap().to_repr(),
            "function('')"
        );
        assert_eq!(
            call_with("test_null_partial", &[]).0.unwrap().to_repr(),
            "function('')"
        );
        assert!(matches!(
            call_with("test_null_channel", &[]).0.unwrap(),
            Value::Null
        ));
        assert!(matches!(
            call_with("test_null_job", &[]).0.unwrap(),
            Value::Null
        ));
        assert!(matches!(
            call_with("test_void", &[]).0.unwrap(),
            Value::Null
        ));
        assert!(matches!(
            call_with("test_unknown", &[]).0.unwrap(),
            Value::Null
        ));
        // test_null_list() equals [] and test_null_dict() equals {}.
        assert_eq!(
            number(
                "assert_equal",
                &[
                    call_with("test_null_list", &[]).0.unwrap(),
                    Value::list(Vec::new())
                ]
            ),
            0
        );
        assert_eq!(
            number(
                "assert_equal",
                &[
                    call_with("test_null_dict", &[]).0.unwrap(),
                    Value::dict(VimDict::new())
                ]
            ),
            0
        );
    }

    #[test]
    fn test_option_not_set() {
        let mut host = OptionHost;
        let (result, _) = call_host("test_option_not_set", &[s("ignorecase")], &mut host);
        assert_eq!(result.unwrap().to_number(), 0);

        let mut host = H;
        let (result, _) = call_host("test_option_not_set", &[s("bogus_opt")], &mut host);
        assert_eq!(result.unwrap_err().code, "E475");
        let mut host = H;
        let (result, _) = call_host("test_option_not_set", &[n(1)], &mut host);
        assert_eq!(result.unwrap_err().code, "E474");
    }

    #[test]
    fn test_override() {
        let (result, env) = call_with("test_override", &[s("redraw"), n(1)]);
        assert_eq!(result.unwrap().to_number(), 0);
        match env.vvars.get("test_override") {
            Some(Value::Dict(dict)) => {
                assert_eq!(dict.borrow().get("redraw").map(Value::to_number), Some(1));
            }
            other => panic!("expected override dict, got {other:?}"),
        }
        let (result, _) = call_with("test_override", &[s("xxx"), n(1)]);
        assert_eq!(result.unwrap_err().code, "E475");
        let (result, _) = call_with("test_override", &[s("redraw"), s("yes")]);
        assert_eq!(result.unwrap_err().code, "E474");
    }

    #[test]
    fn test_refcount() {
        assert_eq!(number("test_refcount", &[n(1)]), -1);
        assert_eq!(number("test_refcount", &[s("x")]), -1);
        assert_eq!(number("test_refcount", &[Value::list(vec![])]), 0);
        assert_eq!(number("test_refcount", &[Value::dict(VimDict::new())]), 0);
        assert_eq!(number("test_refcount", &[Value::blob(vec![])]), 0);
    }

    #[test]
    fn test_setmouse() {
        let (result, env) = call_with("test_setmouse", &[n(2), n(5)]);
        assert_eq!(result.unwrap().to_number(), 0);
        match env.vvars.get("test_setmouse") {
            Some(Value::List(list)) => {
                let list = list.borrow();
                assert_eq!(list[0].to_number(), 2);
                assert_eq!(list[1].to_number(), 5);
            }
            other => panic!("expected mouse position, got {other:?}"),
        }
        let (result, _) = call_with("test_setmouse", &[s(""), n(2)]);
        assert_eq!(result.unwrap_err().code, "E474");
    }

    #[test]
    fn test_settime() {
        let (result, env) = call_with("test_settime", &[n(1234567890)]);
        assert_eq!(result.unwrap().to_number(), 0);
        assert_eq!(
            env.vvars.get("test_settime").map(Value::to_number),
            Some(1234567890)
        );
        // Vim coercion: a numeric string is accepted.
        let (result, _) = call_with("test_settime", &[s("42")]);
        assert_eq!(result.unwrap().to_number(), 0);
        let (result, _) = call_with("test_settime", &[Value::list(vec![])]);
        assert_eq!(result.unwrap_err().code, "E745");
    }

    #[test]
    fn test_srand_seed() {
        let (result, env) = call_with("test_srand_seed", &[n(1234)]);
        assert_eq!(result.unwrap().to_number(), 0);
        assert_eq!(
            env.vvars.get("test_srand_seed").map(Value::to_number),
            Some(1234)
        );
        // Without an argument the seed is reset to 0.
        let (result, env) = call_with("test_srand_seed", &[]);
        assert_eq!(result.unwrap().to_number(), 0);
        assert_eq!(
            env.vvars.get("test_srand_seed").map(Value::to_number),
            Some(0)
        );
        // The seed is truncated to 32 bits, like Vim's UINT32_T.
        let (result, env) = call_with("test_srand_seed", &[n(1 + (1 << 32))]);
        assert_eq!(result.unwrap().to_number(), 0);
        assert_eq!(
            env.vvars.get("test_srand_seed").map(Value::to_number),
            Some(1)
        );
    }

    #[test]
    fn v_errors_is_created_and_repaired() {
        let mut env = Env::new();
        let mut host = H;
        // A non-List v:errors is replaced, like Vim's assert_error().
        env.vvars.insert("errors".to_string(), s("oops"));
        let result = call("assert_report", &[s("boom")], &mut env, &mut host)
            .unwrap()
            .unwrap();
        assert_eq!(result.to_number(), 1);
        assert_eq!(errors(&env), vec!["boom"]);
    }
}
