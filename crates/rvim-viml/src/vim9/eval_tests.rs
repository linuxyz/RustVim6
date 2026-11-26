use super::*;
use crate::vim9::parser::parse_script;

struct NoHost;
impl Host for NoHost {
    fn maxfuncdepth(&self) -> usize {
        8
    }
}

fn run(source: &str) -> (Env, VimResult<()>) {
    let lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    let script = parse_script(&lines, "unit.vim", 1, "").expect("valid Vim9 syntax");
    let mut env = Env::new();
    let result = run_script(&script, &mut env, &mut NoHost);
    assert!(env.frames.is_empty(), "call frames must be restored");
    assert_eq!(env.call_depth, 0, "call depth must be restored");
    assert_eq!(env.current_sid, 0, "script ID must be restored");
    assert_eq!(env.try_depth, 0, "try depth must be restored");
    (env, result)
}

fn global(env: &Env, name: &str) -> String {
    env.globals
        .get(name)
        .unwrap_or_else(|| panic!("missing g:{name}"))
        .to_string_value()
}

#[test]
fn catch_consumes_a_runtime_error_and_continues_after_finally() {
    let (env, result) = run(r#"vim9script
try
  var bad = 1 / 0
catch /E1154/
  g:caught = true
finally
  g:finally = true
endtry
g:after = true
"#);
    result.unwrap();
    for name in ["caught", "finally", "after"] {
        assert_eq!(global(&env, name), "v:true");
    }
}

#[test]
fn finally_runs_when_the_catch_body_raises_another_error() {
    let (env, result) = run(r#"vim9script
try
  throw 'original'
catch
  var bad = 1 / 0
finally
  g:finally = true
endtry
"#);
    assert_eq!(result.unwrap_err().code, "E1154");
    assert_eq!(global(&env, "finally"), "v:true");
}

#[test]
fn nested_catches_restore_the_outer_exception() {
    let (env, result) = run(r#"vim9script
try
  throw 'outer'
catch
  try
    throw 'inner'
  catch
    g:inner = v:exception
  endtry
  g:outer = v:exception
endtry
"#);
    result.unwrap();
    assert_eq!(global(&env, "inner"), "inner");
    assert_eq!(global(&env, "outer"), "outer");
}

#[test]
fn unmatched_errors_survive_finally() {
    let (env, result) = run(r#"vim9script
try
  var bad = 1 / 0
catch /no match/
  g:caught = true
finally
  g:finally = true
endtry
"#);
    assert_eq!(result.unwrap_err().code, "E1154");
    assert!(!env.globals.contains_key("caught"));
    assert_eq!(global(&env, "finally"), "v:true");
}

#[test]
fn lambda_rejects_extra_arguments() {
    let (_, result) = run("vim9script\nvar F = (x) => x\ng:r = F(1, 2)\n");
    assert_eq!(result.unwrap_err().code, "E118");
}

#[test]
fn lambda_rejects_missing_arguments() {
    let (_, result) = run("vim9script\nvar F = (x) => x\ng:r = F()\n");
    assert_eq!(result.unwrap_err().code, "E119");
}

#[test]
fn lambda_checks_its_declared_return_type() {
    let (_, result) = run("vim9script\nvar F = (x: number): number => 'bad'\ng:r = F(1)\n");
    assert_eq!(result.unwrap_err().code, "E1012");
}

#[test]
fn omitted_defaults_with_anonymous_varargs_do_not_panic() {
    let (env, result) = run(r#"vim9script
def F(a: number = 7, ...): number
  return a
enddef
g:r = F()
"#);
    result.unwrap();
    assert_eq!(global(&env, "r"), "7");
}

#[test]
fn named_varargs_collect_and_check_each_extra_argument() {
    let (env, result) = run(r#"vim9script
def F(a: number = 7, ...rest: list<number>): list<number>
  return [a] + rest
enddef
g:empty = F()
g:many = F(1, 2, 3)
"#);
    result.unwrap();
    assert_eq!(global(&env, "empty"), "[7]");
    assert_eq!(global(&env, "many"), "[1, 2, 3]");
    let (_, result) = run(r#"vim9script
def F(...rest: list<number>): number
  return 1
enddef
g:r = F('bad')
"#);
    assert_eq!(result.unwrap_err().code, "E1013");
}

#[test]
fn optional_arguments_can_reference_previous_parameters() {
    let (env, result) = run(r#"vim9script
def F(a: number = 7, b: number = a + 2): number
  return b
enddef
g:r = F()
"#);
    result.unwrap();
    assert_eq!(global(&env, "r"), "9");
}

#[test]
fn integer_comparison_does_not_subtract_and_overflow() {
    for (a, b, expected) in [
        (i64::MAX, -1, 1),
        (i64::MIN, 1, -1),
        (i64::MIN, i64::MAX, -1),
    ] {
        assert_eq!(
            compare_values(&Value::Number(a), &Value::Number(b)).unwrap(),
            expected
        );
    }
}

#[test]
fn finally_return_overrides_a_pending_runtime_error() {
    let (env, result) = run(r#"vim9script
def F(): number
  try
    var bad = 1 / 0
  finally
    return 42
  endtry
enddef
g:r = F()
"#);
    result.unwrap();
    assert_eq!(global(&env, "r"), "42");
}

#[test]
fn finally_error_overrides_a_pending_throw() {
    let (_, result) = run(r#"vim9script
try
  throw 'original'
finally
  var bad = 1 / 0
endtry
"#);
    assert_eq!(result.unwrap_err().code, "E1154");
}

#[test]
fn rethrow_runs_finally_and_preserves_the_new_exception() {
    let (env, result) = run(r#"vim9script
try
  try
    throw 'original'
  catch
    throw 'replacement'
  finally
    g:finally = true
  endtry
catch
  g:caught = v:exception
endtry
"#);
    result.unwrap();
    assert_eq!(global(&env, "finally"), "v:true");
    assert_eq!(global(&env, "caught"), "replacement");
}

#[test]
fn throws_cross_function_and_unmatched_try_boundaries_unchanged() {
    let (env, result) = run(r#"vim9script
def F()
  throw 'original'
enddef
try
  try
    F()
  catch /no match/
    g:wrong = true
  endtry
catch
  g:caught = v:exception
endtry
"#);
    result.unwrap();
    assert_eq!(global(&env, "caught"), "original");
    assert!(!env.globals.contains_key("wrong"));
}

#[test]
fn recursive_default_arguments_obey_maxfuncdepth() {
    let (_, result) = run(r#"vim9script
def F(n: number = F()): number
  return n
enddef
g:r = F()
"#);
    assert_eq!(result.unwrap_err().code, "E132");
}

#[test]
fn function_calls_recover_after_argument_binding_errors() {
    let (env, result) = run(r#"vim9script
def F(n: number): number
  return n + 1
enddef
try
  F('bad')
catch /E1013/
  g:r = F(41)
endtry
"#);
    result.unwrap();
    assert_eq!(global(&env, "r"), "42");
}

#[test]
fn variadic_lambda_accepts_empty_and_multiple_arguments() {
    let (env, result) =
        run("vim9script\nvar F = (...) => 42\ng:empty = F()\ng:many = F(1, 'two')\n");
    result.unwrap();
    assert_eq!(global(&env, "empty"), "42");
    assert_eq!(global(&env, "many"), "42");
}

#[test]
fn typed_lambda_still_uses_captured_locals() {
    let (env, result) = run(r#"vim9script
def Make(): func
  var offset = 10
  return (x: number): number => x + offset
enddef
var F = Make()
g:r = F(2)
"#);
    result.unwrap();
    assert_eq!(global(&env, "r"), "12");
}
