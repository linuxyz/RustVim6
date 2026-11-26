//! M9 integration tests: the Vim9 script engine (compiler, evaluator, imports,
//! autoloads) driven through the headless session.

use std::path::{Path, PathBuf};

use rvim_app::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rvim_m9_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn global(s: &Session, name: &str) -> Option<rvim_viml::Value> {
    s.viml.env.globals.get(name).cloned()
}

fn number(s: &Session, name: &str) -> i64 {
    global(s, name)
        .unwrap_or_else(|| panic!("missing g:{name}"))
        .to_number()
}

fn reference_runtime() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/vim/runtime");
    path.is_dir().then(|| path.canonicalize().unwrap_or(path))
}

// ------------------------------------------------------------ core execution

#[test]
fn vim9_script_runs_functions_and_script_vars() {
    let dir = temp_dir("core");
    let path = write(
        &dir,
        "core.vim",
        r#"vim9script
var counter = 2
def Add(a: number): number
  counter += a
  return counter
enddef
g:first = Add(3)
g:second = Add(4)
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(number(&s, "first"), 5);
    assert_eq!(number(&s, "second"), 9);
    assert_eq!(s.report.error_count, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vim9_enforces_declared_types_and_argument_types() {
    let dir = temp_dir("types");
    let bad_var = write(&dir, "bad_var.vim", "vim9script\nvar x: number = 'text'\n");
    let mut s = Session::new();
    let error = s.source(&bad_var).unwrap_err();
    assert_eq!(error.code, "E1012");

    let bad_arg = write(
        &dir,
        "bad_arg.vim",
        r#"vim9script
def g:Bump(a: number): number
  return a + 1
enddef
"#,
    );
    s.source(&bad_arg).unwrap();
    let error = s.ex("call g:Bump('x')").unwrap_err();
    assert_eq!(error.code, "E1013");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vim9_control_flow_collections_and_lambdas() {
    let dir = temp_dir("flow");
    let path = write(
        &dir,
        "flow.vim",
        r#"vim9script
var out = []
for i in range(3)
  out->add(i * 2)
endfor
var n = 0
while n < 2
  n += 1
endwhile
var Twice = (x: number): number => x * 2
var words = ['a', 'b']
g:out = out->map((_, v) => v + 1)
g:n = n
g:twice = Twice(6)
g:joined = words->join('-')
g:message = $"n={n} twice={Twice(6)}"
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    let out = global(&s, "out").unwrap();
    assert_eq!(out.to_string_value(), "[1, 3, 5]", "map() over a Vim9 list");
    assert_eq!(number(&s, "n"), 2);
    assert_eq!(number(&s, "twice"), 12);
    assert_eq!(global(&s, "joined").unwrap().to_string_value(), "a-b");
    assert_eq!(
        global(&s, "message").unwrap().to_string_value(),
        "n=2 twice=12"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vim9_try_catch_finally_and_throw() {
    let dir = temp_dir("try");
    let path = write(
        &dir,
        "try.vim",
        r#"vim9script
var text = ''
try
  throw 'boom'
catch /boom/
  text ..= 'caught'
finally
  text ..= '+finally'
endtry
g:text = text
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(
        global(&s, "text").unwrap().to_string_value(),
        "caught+finally"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vim9_imports_exports_functions_and_variables() {
    let dir = temp_dir("import");
    write(
        &dir,
        "mod.vim",
        r#"vim9script
export var answer = 42
export def Twice(n: number): number
  return n * 2
enddef
"#,
    );
    let path = write(
        &dir,
        "main.vim",
        r#"vim9script
import './mod.vim' as m
import {Twice} from './mod.vim'
g:total = m.Twice(3) + Twice(2)
g:answer = m.answer
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(number(&s, "total"), 10);
    assert_eq!(number(&s, "answer"), 42);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vim9_def_blocks_work_in_legacy_scripts() {
    let dir = temp_dir("legacy_def");
    let path = write(
        &dir,
        "legacy.vim",
        r#"def Sum(a: number, b: number): number
  return a + b
enddef
let g:result = Sum(2, 3)
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(number(&s, "result"), 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vim9cmd_and_has_vim9script() {
    let mut s = Session::new();
    s.ex("vim9cmd var g:value = 1 + 2").unwrap();
    assert_eq!(number(&s, "value"), 3);
    s.ex("let g:has_vim9 = has('vim9script')").unwrap();
    assert_eq!(number(&s, "has_vim9"), 1);
    s.ex("let g:has_float = has('float')").unwrap();
    assert_eq!(number(&s, "has_float"), 1);
}

// ---------------------------------------------------------------- autoloads

#[test]
fn runtime_vim9_autoloads_source_without_errors() {
    let Some(runtime) = reference_runtime() else {
        eprintln!("skipped: reference runtime missing");
        return;
    };
    let files = [
        runtime.join("autoload/dist/ft.vim"),
        runtime.join("autoload/dist/script.vim"),
        runtime.join("autoload/ccomplete.vim"),
    ];
    for path in files {
        let mut s = Session::new();
        s.source(&path)
            .unwrap_or_else(|e| panic!("sourcing {} failed: {e}", path.display()));
        assert_eq!(
            s.report.error_count,
            0,
            "{} reported errors",
            path.display()
        );
    }
}

#[test]
fn vim9_autoload_function_sets_the_filetype() {
    let Some(runtime) = reference_runtime() else {
        eprintln!("skipped: reference runtime missing");
        return;
    };
    if !runtime.join("autoload/dist/ft.vim").is_file() {
        eprintln!("skipped: dist/ft.vim missing");
        return;
    }
    let mut s = Session::new();
    s.editor
        .replace_lines(0, 1, vec!["# assembly-looking comment".to_string()])
        .unwrap();
    s.ex("call dist#ft#FTasm()").unwrap();
    assert_eq!(s.editor.options.str_or("filetype", ""), "asm");
    assert_eq!(s.report.error_count, 0);
}

// ------------------------------------------------------ Vim 9.2 classes

#[test]
fn vim9_classes_objects_methods_and_statics() {
    let dir = temp_dir("class");
    let path = write(
        &dir,
        "class.vim",
        r#"vim9script
class Counter
  var count: number = 0
  public var label: string = 'x'
  static var total: number = 0
  def new()
    this.count = 1
    total += 1
  enddef
  def Inc(): number
    this.count += 1
    return this.count
  enddef
  static def GetTotal(): number
    return total
  enddef
endclass
var c = Counter.new()
g:inc = c.Inc()
g:label = c.label
c.label = 'y'
g:label2 = c.label
g:total = Counter.GetTotal()
g:t_class = type(Counter)
g:t_object = type(c)
g:n_class = typename(Counter)
g:n_object = typename(c)
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(number(&s, "inc"), 2);
    assert_eq!(global(&s, "label").unwrap().to_string_value(), "x");
    assert_eq!(global(&s, "label2").unwrap().to_string_value(), "y");
    assert_eq!(number(&s, "total"), 1);
    assert_eq!(number(&s, "t_class"), 12);
    assert_eq!(number(&s, "t_object"), 13);
    assert_eq!(
        global(&s, "n_class").unwrap().to_string_value(),
        "class<Counter>"
    );
    assert_eq!(
        global(&s, "n_object").unwrap().to_string_value(),
        "object<Counter>"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vim9_enums_and_type_aliases() {
    let dir = temp_dir("enum");
    let path = write(
        &dir,
        "enum.vim",
        r#"vim9script
type Pair = list<number>
enum Color
  Red,
  Green = 5,
  Blue
endenum
g:red = Color.Red
g:green = Color.Green
g:blue = Color.Blue
g:type = type(Color)
g:name = typename(Color)
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    assert_eq!(number(&s, "red"), 0);
    assert_eq!(number(&s, "green"), 5);
    assert_eq!(number(&s, "blue"), 6);
    assert_eq!(number(&s, "type"), 15);
    assert_eq!(global(&s, "name").unwrap().to_string_value(), "enum<Color>");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vim9_92_autoloads_source_when_available() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/vim9-v9.2.1398");
    if !dir.is_dir() {
        eprintln!(
            "skipped: reference/vim9-v9.2.1398 not fetched (scripts/fetch-vim9-reference.ps1)"
        );
        return;
    }
    for name in [
        "runtime/autoload/dist/ft.vim",
        "runtime/autoload/dist/script.vim",
        "runtime/autoload/ccomplete.vim",
        "runtime/autoload/dist/vim9.vim",
        "runtime/autoload/dist/vim.vim",
        "runtime/autoload/dist/json.vim",
        "runtime/autoload/dist/man.vim",
    ] {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let mut s = Session::new();
        s.source(&path)
            .unwrap_or_else(|e| panic!("sourcing {name} failed: {e}"));
        assert_eq!(s.report.error_count, 0, "{name} reported errors");
    }
}

#[test]
fn official_vim9_expression_subset_runs() {
    let Some(runtime) = reference_runtime() else {
        eprintln!("skipped: reference runtime missing");
        return;
    };
    let testdir = runtime.parent().unwrap().join("src/testdir");
    let file = testdir.join("test_vim9_expr.vim");
    if !file.is_file() {
        eprintln!("skipped: test_vim9_expr.vim missing");
        return;
    }
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(&testdir).unwrap();
    let mut s = Session::new();
    let result = s.source(&file);
    std::env::set_current_dir(previous).unwrap();
    result.unwrap();
    assert_eq!(s.report.error_count, 0);

    // These Test_ functions are part of the subset that passes cleanly; each
    // one runs many assertions through v:errors.
    for name in [
        "Test_expr2",
        "Test_expr3",
        "Test_expr9_number",
        "Test_expr9_negate",
    ] {
        let mut s = Session::new();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&testdir).unwrap();
        s.source(&file).unwrap();
        s.ex(&format!("call {name}()")).unwrap();
        std::env::set_current_dir(previous).unwrap();
        let errors = s
            .viml
            .env
            .vvars
            .get("errors")
            .map(|value| value.to_string_value())
            .unwrap_or_else(|| "[]".to_string());
        assert_eq!(errors, "[]", "{name} produced assertion failures: {errors}");
    }
}

#[test]
fn vim9_try_catches_legacy_errors_and_always_runs_finally() {
    let dir = temp_dir("legacy_error");
    let path = write(
        &dir,
        "legacy_error.vim",
        r#"vim9script
func LegacyFail()
  call MissingFunctionForVim9TryTest()
  let g:continued_after_error = 1
endfunc
try
  LegacyFail()
catch /E117/
  g:caught = true
finally
  g:cleaned_up = true
endtry
g:after = true
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    for name in ["caught", "cleaned_up", "after"] {
        assert_eq!(global(&s, name), Some(rvim_viml::Value::Bool(true)));
    }
    assert!(global(&s, "continued_after_error").is_none());
    assert_eq!(s.viml.env.try_depth, 0);
    assert_eq!(s.viml.env.call_depth, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_calls_use_vim9_varargs_and_lambda_argument_checks() {
    let dir = temp_dir("legacy_args");
    let path = write(
        &dir,
        "legacy_args.vim",
        r#"vim9script
def g:Collect(first: number = 7, ...rest: list<number>): list<number>
  return [first] + rest
enddef
g:Vim9Lambda = (n: number): number => n + 1
"#,
    );
    let mut s = Session::new();
    s.source(&path).unwrap();
    s.ex("let g:empty = g:Collect()").unwrap();
    s.ex("let g:many = g:Collect(1, 2, 3)").unwrap();
    assert_eq!(global(&s, "empty").unwrap().to_string_value(), "[7]");
    assert_eq!(global(&s, "many").unwrap().to_string_value(), "[1, 2, 3]");
    assert_eq!(s.ex("call g:Collect(1, 'bad')").unwrap_err().code, "E1013");
    assert_eq!(s.ex("call g:Vim9Lambda()").unwrap_err().code, "E119");
    assert_eq!(s.ex("call g:Vim9Lambda(1, 2)").unwrap_err().code, "E118");
    s.ex("let g:valid = g:Vim9Lambda(41)").unwrap();
    assert_eq!(number(&s, "valid"), 42);
    assert!(s.viml.env.frames.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
