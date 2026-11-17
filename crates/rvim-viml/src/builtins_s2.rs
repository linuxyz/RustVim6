//! Builtin functions, group si..z (M3 coverage batch).
//!
//! See `docs/m3_builtins_plan.md` for the candidate list, semantics rules and
//! the skip list (subsystems that are not built yet).
//!
//! Implemented in this batch: `tr()`, `typename()`, `undofile()`,
//! `wildmenumode()` and `writefile()`.  Every other name of the s2 candidate
//! list depends on state the language layer cannot reach through
//! [`crate::env::Host`] yet (editor buffers/windows, the undo tree, terminal
//! channels, the timer/event loop, or the test-harness overrides), so they are
//! intentionally absent from [`FUNCS`]: calling them fails with `E117: Unknown
//! function` instead of inventing a result.
//!
//! Deliberate simplifications, both forced by the current runtime:
//! * `typename()` reports `func(...): unknown` for every Funcref: the
//!   interpreter does not track function signatures, while Vim can name the
//!   signature of known functions.
//! * `wildmenumode()` returns 0: the language layer has no UI hook and the
//!   current runtime is headless (`-es`), where the wildmenu is never active.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use rvim_core::{VimError, VimResult};

use crate::builtins::check_arity;
use crate::env::{Env, Host};
use crate::value::{Value, ValueResult};

/// Names implemented in this module (coverage source of truth).
pub const FUNCS: &[&str] = &["tr", "typename", "undofile", "wildmenumode", "writefile"];

/// Dispatch a call; returns None when the name is not handled here.
pub fn call(
    name: &str,
    args: &[Value],
    _env: &mut Env,
    host: &mut dyn Host,
) -> Option<ValueResult> {
    if !FUNCS.contains(&name) {
        return None;
    }
    Some((|| {
        check_arity(name, args)?;
        match name {
            "tr" => tr(args),
            "typename" => typename(args),
            "undofile" => undofile(args, host),
            "wildmenumode" => Ok(Value::Number(0)),
            "writefile" => writefile(args),
            other => Err(err("E117", format!("Unknown function: {other}"))),
        }
    })())
}

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
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Float(f) => Ok(format!("{f}")),
        Value::Bool(b) => Ok(if *b { "v:true" } else { "v:false" }.to_string()),
        Value::Null => Ok("v:null".to_string()),
        Value::Funcref(_) => Err(err("E729", "Using a Funcref as a String")),
        Value::List(_) => Err(err("E730", "Using a List as a String")),
        Value::Dict(_) => Err(err("E731", "Using a Dictionary as a String")),
        Value::Blob(_) => Err(err("E976", "Using a Blob as a String")),
    }
}

// ---------------------------------------------------------------- tr()

/// `tr({src}, {fromstr}, {tostr})` (strings.c: `f_tr()`).
///
/// Translates every character of {src} that occurs in {fromstr} to the
/// character at the same (character, not byte) position in {tostr}; a
/// character that occurs more than once in {fromstr} uses the first mapping.
fn tr(args: &[Value]) -> ValueResult {
    let input = string_arg(&arg(args, 0))?;
    let from_text = string_arg(&arg(args, 1))?;
    let to_text = string_arg(&arg(args, 2))?;
    let from: Vec<char> = from_text.chars().collect();
    let to: Vec<char> = to_text.chars().collect();
    // fromstr and tostr have to contain the same number of characters.
    if from.len() != to.len() {
        return Err(err("E475", format!("Invalid argument: {from_text}")));
    }
    let mut map: HashMap<char, char> = HashMap::new();
    for (source, target) in from.into_iter().zip(to) {
        map.entry(source).or_insert(target);
    }
    let translated: String = input
        .chars()
        .map(|c| map.get(&c).copied().unwrap_or(c))
        .collect();
    Ok(Value::String(translated))
}

// ---------------------------------------------------------------- typename()

/// `typename({expr})` (vim9type.c: `f_typename()`).
///
/// The string mirrors Vim's type printer: `list<number>`, `dict<any>`,
/// `list<unknown>` for empty containers, `special` for `v:null`, etc.
fn typename(args: &[Value]) -> ValueResult {
    let value = arg(args, 0);
    let ty = typename_of(&value, &mut Vec::new(), 0);
    Ok(Value::String(type_name(&ty)))
}

/// Vim's `type_T` for the subset of types our values can have.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ty {
    Any,
    Unknown,
    Number,
    Float,
    String,
    Bool,
    Blob,
    Special,
    Func,
    List(Box<Ty>),
    Dict(Box<Ty>),
}

/// `typval2type()`: derive the type of a value, looking inside containers.
///
/// `seen` holds the container addresses currently being inspected, mirroring
/// Vim's `lv_copyID`/`dv_copyID` recursion guard: a self-referencing container
/// contributes `list<any>`/`dict<any>` at the point of the cycle.
fn typename_of(value: &Value, seen: &mut Vec<usize>, depth: usize) -> Ty {
    if depth > 100 {
        return Ty::Any;
    }
    match value {
        Value::Number(_) => Ty::Number,
        Value::Float(_) => Ty::Float,
        Value::String(_) => Ty::String,
        Value::Bool(_) => Ty::Bool,
        Value::Blob(_) => Ty::Blob,
        Value::Null => Ty::Special,
        Value::Funcref(_) => Ty::Func,
        Value::List(list) => {
            let id = Rc::as_ptr(list) as usize;
            if seen.contains(&id) {
                return Ty::List(Box::new(Ty::Any));
            }
            seen.push(id);
            let member = {
                let items = list.borrow();
                let mut member = Ty::Unknown;
                for (index, item) in items.iter().enumerate() {
                    let item_type = typename_of(item, seen, depth + 1);
                    member = if index == 0 {
                        item_type
                    } else {
                        common_type(member, item_type)
                    };
                }
                member
            };
            seen.pop();
            Ty::List(Box::new(member))
        }
        Value::Dict(dict) => {
            let id = Rc::as_ptr(dict) as usize;
            if seen.contains(&id) {
                return Ty::Dict(Box::new(Ty::Any));
            }
            seen.push(id);
            let member = {
                let entries = dict.borrow();
                let mut member = Ty::Unknown;
                for (index, (_, item)) in entries.iter().enumerate() {
                    let item_type = typename_of(item, seen, depth + 1);
                    member = if index == 0 {
                        item_type
                    } else {
                        common_type(member, item_type)
                    };
                }
                member
            };
            seen.pop();
            Ty::Dict(Box::new(member))
        }
    }
}

/// `common_type()`: the type shared by two members of one container.
fn common_type(first: Ty, second: Ty) -> Ty {
    if first == second {
        return first;
    }
    match (first, second) {
        // An "unknown" member (empty list/dict at top level) carries no
        // information, the other member wins.
        (Ty::Unknown, other) | (other, Ty::Unknown) => other,
        (Ty::List(a), Ty::List(b)) => Ty::List(Box::new(common_type(*a, *b))),
        (Ty::Dict(a), Ty::Dict(b)) => Ty::Dict(Box::new(common_type(*a, *b))),
        (Ty::Func, Ty::Func) => Ty::Func,
        _ => Ty::Any,
    }
}

/// `type_name()`: render a type the way `typename()` does.
fn type_name(ty: &Ty) -> String {
    match ty {
        Ty::Any => "any".to_string(),
        Ty::Unknown => "unknown".to_string(),
        Ty::Number => "number".to_string(),
        Ty::Float => "float".to_string(),
        Ty::String => "string".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Blob => "blob".to_string(),
        Ty::Special => "special".to_string(),
        // Vim's `t_func_unknown`.
        Ty::Func => "func(...): unknown".to_string(),
        Ty::List(member) => format!("list<{}>", type_name(member)),
        Ty::Dict(member) => format!("dict<{}>", type_name(member)),
    }
}

// ---------------------------------------------------------------- undofile()

/// `undofile({name})` (undo.c: `f_undofile()`).
///
/// Returns the undo file name for {name}: the full path is computed first
/// (relative names are resolved against the current directory), then the
/// 'undodir' option decides where the file lives.  The default `"."` uses the
/// directory of the file itself and prefixes the tail with a dot, so
/// `undofile('abc')` ends in `.abc.un~`.
fn undofile(args: &[Value], host: &mut dyn Host) -> ValueResult {
    let name = string_arg(&arg(args, 0))?;
    if name.is_empty() {
        // A buffer without a name will not write an undo file.
        return Ok(Value::String(String::new()));
    }
    let full = full_path(&name);
    let undodir = match host.option_get("undodir") {
        Some(Value::String(dir)) => dir,
        _ => ".".to_string(),
    };
    Ok(Value::String(undo_file_name(&full, &undodir)))
}

/// `FullName_save(fname, TRUE)`: expand `~` and make the path absolute with
/// `.`/`..` components resolved.
fn full_path(name: &str) -> String {
    let expanded = expand_tilde(name);
    let path = Path::new(&expanded);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    simplify_path(&absolute).to_string_lossy().into_owned()
}

fn simplify_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn expand_tilde(name: &str) -> String {
    let Some(rest) = name.strip_prefix('~') else {
        return name.to_string();
    };
    if rest.is_empty() {
        return home_dir().unwrap_or_default();
    }
    let mut chars = rest.chars();
    if chars.next().is_some_and(is_path_sep) {
        if let Some(home) = home_dir() {
            let home = home.trim_end_matches(is_path_sep);
            return format!("{home}{rest}");
        }
    }
    name.to_string()
}

fn home_dir() -> Option<String> {
    for variable in ["HOME", "USERPROFILE"] {
        if let Ok(value) = std::env::var(variable) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn is_path_sep(c: char) -> bool {
    if cfg!(windows) {
        c == '/' || c == '\\'
    } else {
        c == '/'
    }
}

/// `u_get_undo_file_name(full, FALSE)`: walk 'undodir', use the first entry
/// that can be written to.  `"."` keeps the file next to the original.
fn undo_file_name(full: &str, undodir: &str) -> String {
    for entry in split_option_list(undodir) {
        if entry.is_empty() {
            continue;
        }
        let entry = expand_tilde(&entry);
        if entry == "." {
            // "dir/name" -> "dir/.name.un~"
            return match full.rfind(is_path_sep) {
                Some(index) => {
                    let (dir, base) = full.split_at(index + 1);
                    format!("{dir}.{base}.un~")
                }
                None => format!(".{full}.un~"),
            };
        }
        // For other directories the file name is the full path with the path
        // separators replaced with '%'.
        if !Path::new(&entry).is_dir() {
            continue;
        }
        let mut name = entry;
        if !name.ends_with(is_path_sep) {
            name.push(std::path::MAIN_SEPARATOR);
        }
        name.extend(full.chars().map(|c| if is_path_sep(c) { '%' } else { c }));
        return name;
    }
    String::new()
}

/// Split a comma separated `P_COMMA` option value like `copy_option_part()`:
/// a backslash only escapes a comma (on Windows a backslash is also a path
/// separator, so it must otherwise stay intact), and spaces right after a
/// comma are skipped.
fn split_option_list(text: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut skipping_spaces = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&',') => {
                chars.next();
                if let Some(current) = parts.last_mut() {
                    current.push(',');
                }
                skipping_spaces = false;
            }
            ',' => {
                parts.push(String::new());
                skipping_spaces = true;
            }
            ' ' if skipping_spaces => {}
            _ => {
                if let Some(current) = parts.last_mut() {
                    current.push(c);
                }
                skipping_spaces = false;
            }
        }
    }
    parts
}

// ---------------------------------------------------------------- wildmenumode()

// `wildmenumode()` (evalfunc.c: `f_wildmenumode()`): Vim returns 1 only while
// the wildmenu is showing.  The language layer cannot see UI state and the
// current runtime is headless, so the answer is always 0 (see the module docs).

// ---------------------------------------------------------------- writefile()

/// `writefile({object}, {fname} [, {flags}])` (filepath.c: `f_writefile()`).
///
/// `{object}` is a List (each item becomes one line) or a Blob (written
/// unmodified).  Flags: `b` binary (no NL after the last line), `a` append.
/// Returns 0 on success; creation/write failures raise `E482`/`E80` like Vim.
fn writefile(args: &[Value]) -> ValueResult {
    let object = arg(args, 0);
    // Vim validates the object before looking at the other arguments.
    match &object {
        Value::List(list) => {
            for item in list.borrow().iter() {
                string_arg(item)?;
            }
        }
        Value::Blob(_) => {}
        _ => {
            return Err(err(
                "E475",
                "Invalid argument: writefile() first argument must be a List or a Blob",
            ));
        }
    }

    let flags = if args.len() > 2 {
        string_arg(&arg(args, 2))?
    } else {
        String::new()
    };
    let binary = flags.contains('b');
    let append = flags.contains('a');
    let fname = string_arg(&arg(args, 1))?;

    let file = if append {
        OpenOptions::new().append(true).create(true).open(&fname)
    } else {
        File::create(&fname)
    };
    let mut file = file.map_err(|_| cant_create_file(&fname))?;

    let mut bytes: Vec<u8> = Vec::new();
    match &object {
        Value::Blob(blob) => bytes.extend_from_slice(&blob.borrow()),
        Value::List(list) => {
            let items = list.borrow().clone();
            let last = items.len().saturating_sub(1);
            for (index, item) in items.iter().enumerate() {
                let text = string_arg(item)?;
                for c in text.chars() {
                    // Vim writes a NUL byte for a NL inside an item.
                    if c == '\n' {
                        bytes.push(0);
                    } else {
                        let mut buffer = [0u8; 4];
                        bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                    }
                }
                if !binary || index != last {
                    bytes.push(b'\n');
                }
            }
        }
        _ => unreachable!("the object type was validated above"),
    }

    file.write_all(&bytes)
        .map_err(|_| err("E80", "Error while writing"))?;
    Ok(Value::Number(0))
}

fn cant_create_file(fname: &str) -> VimError {
    let shown = if fname.is_empty() { "<empty>" } else { fname };
    err("E482", format!("Can't create file {shown}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::VimDict;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Dummy host: every Host method has a default, so this is enough for the
    /// pure builtins in this module.
    struct H;
    impl Host for H {}

    /// Host that reports a given 'undodir' value.
    struct UndodirHost(String);
    impl Host for UndodirHost {
        fn option_get(&mut self, name: &str) -> Option<Value> {
            if name == "undodir" {
                Some(Value::String(self.0.clone()))
            } else {
                None
            }
        }
    }

    fn call_fn(name: &str, args: Vec<Value>) -> ValueResult {
        let mut env = Env::new();
        let mut host = H;
        call(name, &args, &mut env, &mut host).expect("builtin should be handled")
    }

    fn call_host(host: &mut dyn Host, name: &str, args: Vec<Value>) -> ValueResult {
        let mut env = Env::new();
        call(name, &args, &mut env, host).expect("builtin should be handled")
    }

    fn temp_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("rvim_s2_{}_{}_{}.tmp", std::process::id(), tag, n))
    }

    fn file_arg(path: &Path) -> Value {
        Value::String(path.to_string_lossy().into_owned())
    }

    fn dict_insert(value: &Value, key: &str, item: Value) {
        match value {
            Value::Dict(dict) => dict.borrow_mut().insert(key, item),
            _ => panic!("not a dictionary"),
        }
    }

    fn list_push(value: &Value, item: Value) {
        match value {
            Value::List(list) => list.borrow_mut().push(item),
            _ => panic!("not a list"),
        }
    }

    #[test]
    fn dispatch_handles_exactly_the_listed_names() {
        let mut env = Env::new();
        let mut host = H;
        // Names from this group that need unbuilt subsystems stay unknown.
        assert!(call("winheight", &[Value::Number(1)], &mut env, &mut host).is_none());
        assert!(call("term_start", &[], &mut env, &mut host).is_none());
        assert!(call("timer_start", &[], &mut env, &mut host).is_none());
        for name in FUNCS {
            let result = call(name, &[], &mut env, &mut host).expect("listed in FUNCS");
            if *name == "wildmenumode" {
                assert!(result.is_ok(), "wildmenumode takes no argument");
            } else {
                assert_eq!(result.unwrap_err().code, "E119", "arity check for {name}");
            }
        }
    }

    #[test]
    fn functions_are_reachable_through_the_builtin_table() {
        // `call_builtin()` dispatches pure builtins first and then falls
        // through to the group modules; a name listed in FUNCS but not wired
        // up would report E117 "listed but not dispatched".
        let mut env = Env::new();
        let mut host = H;
        for name in FUNCS {
            let result = crate::builtins::call_builtin(name, &[], &mut env, &mut host)
                .unwrap_or_else(|| panic!("{name} is not reachable through call_builtin"));
            match result {
                Ok(_) => assert_eq!(*name, "wildmenumode", "{name} accepts no arguments"),
                Err(error) => assert_eq!(error.code, "E119", "{name}"),
            }
            assert!(
                crate::builtins::is_builtin(name),
                "{name} not in is_builtin"
            );
            assert!(
                crate::builtins::implemented_funcs().contains(name),
                "{name} missing from implemented_funcs()"
            );
        }
    }

    #[test]
    fn arity_rejects_extra_arguments() {
        let error = call_fn("typename", vec![Value::Number(1), Value::Number(2)]).unwrap_err();
        assert_eq!(error.code, "E118");
    }

    #[test]
    fn tr_translates_and_keeps_other_characters() {
        let value = call_fn(
            "tr",
            vec![
                Value::string("abcabc"),
                Value::string("ab"),
                Value::string("xy"),
            ],
        )
        .unwrap();
        assert_eq!(value.to_string_value(), "xycxyc");
    }

    #[test]
    fn tr_handles_multibyte_characters_by_character() {
        let value = call_fn(
            "tr",
            vec![
                Value::string("äöü"),
                Value::string("äö"),
                Value::string("ao"),
            ],
        )
        .unwrap();
        assert_eq!(value.to_string_value(), "aoü");
    }

    #[test]
    fn tr_uses_the_first_mapping_for_duplicates() {
        let value = call_fn(
            "tr",
            vec![
                Value::string("ab"),
                Value::string("aa"),
                Value::string("xy"),
            ],
        )
        .unwrap();
        assert_eq!(value.to_string_value(), "xb");
    }

    #[test]
    fn tr_with_empty_translation_lists_keeps_the_input() {
        let value = call_fn(
            "tr",
            vec![Value::string("abc"), Value::string(""), Value::string("")],
        )
        .unwrap();
        assert_eq!(value.to_string_value(), "abc");
    }

    #[test]
    fn tr_rejects_mismatched_character_counts() {
        let error = call_fn(
            "tr",
            vec![
                Value::string("abc"),
                Value::string("ab"),
                Value::string("x"),
            ],
        )
        .unwrap_err();
        assert_eq!(error.code, "E475");
        assert!(error.message.contains("Invalid argument"));
    }

    #[test]
    fn typename_reports_scalar_types() {
        let cases: Vec<(Value, &str)> = vec![
            (Value::Number(123), "number"),
            (Value::Float(1.5), "float"),
            (Value::string("x"), "string"),
            (Value::Bool(true), "bool"),
            (Value::Null, "special"),
            (Value::blob(vec![1, 2]), "blob"),
        ];
        for (value, expected) in cases {
            let actual = call_fn("typename", vec![value]).unwrap();
            assert_eq!(actual.to_string_value(), expected);
        }
    }

    #[test]
    fn typename_reports_container_types() {
        let dict_number = Value::dict(VimDict::new());
        dict_insert(&dict_number, "key", Value::Number(123));
        let cases: Vec<(Value, &str)> = vec![
            (Value::list(Vec::new()), "list<unknown>"),
            (Value::dict(VimDict::new()), "dict<unknown>"),
            (
                Value::list(vec![Value::Number(1), Value::Number(2)]),
                "list<number>",
            ),
            (
                Value::list(vec![Value::string("a"), Value::string("b")]),
                "list<string>",
            ),
            (
                Value::list(vec![Value::list(vec![Value::Number(1)])]),
                "list<list<number>>",
            ),
            (Value::list(vec![dict_number]), "list<dict<number>>"),
            (
                Value::list(vec![Value::Number(1), Value::string("b")]),
                "list<any>",
            ),
        ];
        for (value, expected) in cases {
            let actual = call_fn("typename", vec![value]).unwrap();
            assert_eq!(actual.to_string_value(), expected);
        }
    }

    #[test]
    fn typename_handles_recursive_containers() {
        // let l = [] | let d = #{a: 0} | let l = [d] | let l[0].e = #{b: l}
        // typename(l) is "list<dict<any>>", typename(d) is "dict<any>".
        let list = Value::list(Vec::new());
        let dict = Value::dict(VimDict::new());
        list_push(&list, dict.clone());
        {
            dict_insert(&dict, "a", Value::Number(0));
            let inner = Value::dict(VimDict::new());
            dict_insert(&inner, "b", list.clone());
            dict_insert(&dict, "e", inner);
        }
        let list_name = call_fn("typename", vec![list]).unwrap();
        assert_eq!(list_name.to_string_value(), "list<dict<any>>");
        let dict_name = call_fn("typename", vec![dict]).unwrap();
        assert_eq!(dict_name.to_string_value(), "dict<any>");
    }

    #[test]
    fn typename_reports_funcref_without_signature() {
        let funcref = Value::Funcref(Rc::new(crate::value::FuncRef::new("strlen")));
        let value = call_fn("typename", vec![funcref]).unwrap();
        assert_eq!(value.to_string_value(), "func(...): unknown");
    }

    #[test]
    fn undofile_of_empty_name_is_empty() {
        let value = call_fn("undofile", vec![Value::string("")]).unwrap();
        assert_eq!(value.to_string_value(), "");
    }

    #[test]
    fn undofile_uses_the_default_undodir() {
        // The default 'undodir' is ".", so the undo file sits next to the file
        // with a leading dot and a ".un~" suffix (Vim's Test_undofile).
        let value = call_fn("undofile", vec![Value::string("abc")]).unwrap();
        let name = value.to_string_value();
        let path = Path::new(&name);
        assert_eq!(path.file_name().unwrap().to_string_lossy(), ".abc.un~");
        assert_eq!(
            path.parent().unwrap(),
            std::env::current_dir().unwrap().as_path()
        );
    }

    #[test]
    fn undofile_uses_an_existing_undodir() {
        let directory = std::env::temp_dir();
        let directory_text = directory.to_string_lossy().into_owned();
        let mut host = UndodirHost(directory_text.clone());
        let value = call_host(&mut host, "undofile", vec![Value::string("abc")]).unwrap();
        let name = value.to_string_value();
        assert!(name.starts_with(&directory_text), "{name}");
        // The full path gets its path separators replaced with '%'.
        assert!(name.contains('%'), "{name}");
        assert!(name.ends_with("abc"), "{name}");
    }

    #[test]
    fn undofile_ignores_undodir_entries_that_do_not_exist() {
        let missing = temp_path("missing_dir");
        let mut host = UndodirHost(missing.to_string_lossy().into_owned());
        let value = call_host(&mut host, "undofile", vec![Value::string("abc")]).unwrap();
        assert_eq!(value.to_string_value(), "");
    }

    #[test]
    fn wildmenumode_is_never_active_in_the_headless_runtime() {
        let value = call_fn("wildmenumode", Vec::new()).unwrap();
        assert_eq!(value.to_number(), 0);
    }

    #[test]
    fn writefile_writes_one_line_per_item() {
        let path = temp_path("lines");
        let value = call_fn(
            "writefile",
            vec![
                Value::list(vec![Value::string("a"), Value::Number(5)]),
                file_arg(&path),
            ],
        )
        .unwrap();
        assert_eq!(value.to_number(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"a\n5\n");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn writefile_supports_binary_and_append_flags() {
        let path = temp_path("flags");
        call_fn(
            "writefile",
            vec![
                Value::list(vec![Value::string("a"), Value::string("b")]),
                file_arg(&path),
            ],
        )
        .unwrap();
        // Append one more line.
        call_fn(
            "writefile",
            vec![
                Value::list(vec![Value::string("c")]),
                file_arg(&path),
                Value::string("a"),
            ],
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"a\nb\nc\n");
        // Binary mode: no NL after the last item.
        call_fn(
            "writefile",
            vec![
                Value::list(vec![Value::string("x"), Value::string("y")]),
                file_arg(&path),
                Value::string("b"),
            ],
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"x\ny");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn writefile_replaces_nl_inside_items_with_nul() {
        let path = temp_path("nl");
        call_fn(
            "writefile",
            vec![Value::list(vec![Value::string("a\nb")]), file_arg(&path)],
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"a\0b\n");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn writefile_writes_blob_bytes_unmodified() {
        let path = temp_path("blob");
        let value = call_fn(
            "writefile",
            vec![Value::blob(vec![0x41, 0x42, 0x00, 0x43]), file_arg(&path)],
        )
        .unwrap();
        assert_eq!(value.to_number(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), vec![0x41, 0x42, 0x00, 0x43]);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn writefile_rejects_invalid_objects() {
        let error = call_fn("writefile", vec![Value::Number(1), Value::string("x")]).unwrap_err();
        assert_eq!(error.code, "E475");

        let error = call_fn(
            "writefile",
            vec![
                Value::list(vec![Value::list(Vec::new())]),
                Value::string("x"),
            ],
        )
        .unwrap_err();
        assert_eq!(error.code, "E730");
    }

    #[test]
    fn writefile_reports_files_it_cannot_create() {
        let error = call_fn(
            "writefile",
            vec![Value::list(Vec::new()), Value::string("")],
        )
        .unwrap_err();
        assert_eq!(error.code, "E482");

        let missing = temp_path("no_such_dir").join("file.txt");
        let error = call_fn(
            "writefile",
            vec![Value::list(Vec::new()), file_arg(&missing)],
        )
        .unwrap_err();
        assert_eq!(error.code, "E482");
    }
}
