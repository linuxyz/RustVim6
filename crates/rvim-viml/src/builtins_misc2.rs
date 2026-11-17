//! Miscellaneous pure builtins that do not need editor state.
//!
//! Implemented here:
//!
//! * `balloon_split({text})`: splits balloon text into display lines.
//!   Vim's real implementation (`popupmenu.c: split_message()`, used by
//!   `evalfunc.c: f_balloon_split()`) targets gdb output: it breaks at `", "`,
//!   `{`/`}` points, honours quotes and indentation, and only hard-wraps items
//!   at `BALLOON_MIN_WIDTH` (50) cells when the screen height allows. This
//!   runtime has no screen/bell/mouse layer, so the approximation here is the
//!   documented behaviour: explicit newlines are kept, every line is wrapped
//!   greedily at spaces to at most 50 characters, and a word longer than that is
//!   hard-split. The leading/trailing blank items Vim adds are not returned
//!   (Vim skips them too), so `balloon_split("")` is `[]`.
//!
//! * `iconv({string}, {from}, {to})`: a correct conversion for the encodings
//!   this runtime can represent without external tables: `utf-8`/`utf8`,
//!   `latin1` (plus `iso-8859-1`, `ansi`, ...), and `ascii`. Characters that
//!   cannot be represented in the target encoding become `?`, matching Vim's
//!   documented iconv() behaviour. For any other encoding pair an empty string
//!   is returned: the reference Vim returns the input unchanged when it is
//!   built without +iconv (no converter), but claiming success for a
//!   conversion that did not happen would be dishonest, and Vim itself
//!   documents the empty string as the "conversion failed" result.
//!
//!   Because [`Value::String`] holds Unicode text (not a byte string), a latin1
//!   result is represented by the corresponding Unicode code points
//!   (`U+0000..=U+00FF`); `len()` therefore counts UTF-8 bytes where Vim would
//!   count latin1 bytes.
//!
//! Deliberately absent from [`FUNCS`]:
//!
//! * `soundfold()` requires the spell/soundfold subsystem (`spell.c`,
//!   `soundfold.c` and the generated spellfile tables); returning the input or
//!   an empty string would fake a spelling transformation.
//! * `getcompletion()` needs the Ex command table and the command-line
//!   completion machinery, which live in `rvim-ex` and are not reachable from
//!   the pure language layer.

use rvim_core::VimError;

use crate::builtins::check_arity;
use crate::env::{Env, Host};
use crate::value::{Value, ValueResult};

/// Names implemented in this module (coverage source of truth).
pub const FUNCS: &[&str] = &["balloon_split", "iconv"];

/// Dispatch a call; `None` when the name is not handled here.
pub fn call(
    name: &str,
    args: &[Value],
    _env: &mut Env,
    _host: &mut dyn Host,
) -> Option<ValueResult> {
    if !FUNCS.contains(&name) {
        return None;
    }
    Some((|| {
        check_arity(name, args)?;
        match name {
            "balloon_split" => balloon_split(args),
            "iconv" => iconv(args),
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
fn string_arg(value: &Value) -> Result<String, VimError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Float(f) => Ok(crate::value::format_float(*f)),
        Value::Bool(b) => Ok(if *b { "v:true" } else { "v:false" }.to_string()),
        Value::Null => Ok("v:null".to_string()),
        Value::Funcref(_) => Err(err("E729", "Using a Funcref as a String")),
        Value::List(_) => Err(err("E730", "Using a List as a String")),
        Value::Dict(_) => Err(err("E731", "Using a Dictionary as a String")),
        Value::Blob(_) => Err(err("E976", "Using a Blob as a String")),
    }
}

// ---------------------------------------------------------------------------
// balloon_split()
// ---------------------------------------------------------------------------

/// Maximum display width of one balloon line (`BALLOON_MIN_WIDTH` in
/// `popupmenu.c`).
const BALLOON_MAX_WIDTH: usize = 50;

fn balloon_split(args: &[Value]) -> ValueResult {
    let text = string_arg(&arg(args, 0))?;
    Ok(Value::list(
        balloon_lines(&text)
            .into_iter()
            .map(Value::String)
            .collect(),
    ))
}

/// Split `text` into balloon lines: explicit newlines are line breaks (an empty
/// line between two newlines stays empty), every resulting line is wrapped.
fn balloon_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut segments: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        // A trailing newline terminates the last line instead of adding one.
        segments.pop();
    }
    let mut lines = Vec::new();
    for segment in segments {
        wrap_line(segment, &mut lines);
    }
    lines
}

/// Greedy word wrap at spaces, hard-splitting words longer than the limit.
fn wrap_line(line: &str, out: &mut Vec<String>) {
    if line.is_empty() {
        out.push(String::new());
        return;
    }
    let mut rest: String = line.to_string();
    loop {
        let chars: Vec<char> = rest.chars().collect();
        if chars.len() <= BALLOON_MAX_WIDTH {
            let line = rest.trim_end().to_string();
            if !line.is_empty() {
                out.push(line);
            }
            return;
        }
        let mut break_at = None;
        for index in (1..=BALLOON_MAX_WIDTH).rev() {
            if chars[index - 1] == ' ' {
                break_at = Some(index - 1);
                break;
            }
        }
        let (chunk, tail): (String, String) = match break_at {
            Some(at) => (
                chars[..at].iter().collect(),
                chars[at + 1..].iter().collect(),
            ),
            None => (
                chars[..BALLOON_MAX_WIDTH].iter().collect(),
                chars[BALLOON_MAX_WIDTH..].iter().collect(),
            ),
        };
        let chunk = chunk.trim_end().to_string();
        if !chunk.is_empty() {
            out.push(chunk);
        }
        rest = tail.trim_start_matches(' ').to_string();
        if rest.is_empty() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// iconv()
// ---------------------------------------------------------------------------

/// The encodings this runtime can convert without external tables.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Latin1,
    Ascii,
}

/// Vim's `enc_canonize()` for the supported subset: lower-case, `_` → `-`,
/// skip `2byte-`/`8bit-` prefixes, and resolve the common aliases.
fn canonical_encoding(name: &str) -> Option<Encoding> {
    let lower: String = name
        .trim()
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .collect::<String>()
        .replace('_', "-");
    let name = lower
        .strip_prefix("2byte-")
        .or_else(|| lower.strip_prefix("8bit-"))
        .unwrap_or(&lower);
    match name {
        // Vim's compiled-in default encoding is UTF-8.
        "utf-8" | "utf8" | "default" => Some(Encoding::Utf8),
        "latin1" | "latin-1" | "iso-8859-1" | "iso8859-1" | "iso-8859" | "ansi" => {
            Some(Encoding::Latin1)
        }
        "ascii" | "us-ascii" => Some(Encoding::Ascii),
        _ => None,
    }
}

fn iconv(args: &[Value]) -> ValueResult {
    let text = string_arg(&arg(args, 0))?;
    let from_name = string_arg(&arg(args, 1))?;
    let to_name = string_arg(&arg(args, 2))?;
    let (Some(from), Some(to)) = (canonical_encoding(&from_name), canonical_encoding(&to_name))
    else {
        // No converter available: Vim documents an empty result on failure.
        return Ok(Value::string(""));
    };
    if from == to {
        return Ok(Value::String(text));
    }
    let mut converted = String::with_capacity(text.len());
    for c in text.chars() {
        let representable = match (from, to) {
            // UTF-8 -> single-byte encodings: anything above the code page
            // becomes '?' (Vim: "some characters could not be converted").
            (Encoding::Utf8, Encoding::Latin1) => (c as u32) <= 0xFF,
            (Encoding::Utf8, Encoding::Ascii) => c.is_ascii(),
            // latin1 input code points map 1:1 into Unicode.
            (Encoding::Latin1, Encoding::Utf8) => (c as u32) <= 0xFF,
            (Encoding::Latin1, Encoding::Ascii) => c.is_ascii(),
            // ASCII is a subset of both targets.
            (Encoding::Ascii, Encoding::Utf8) | (Encoding::Ascii, Encoding::Latin1) => c.is_ascii(),
            (Encoding::Utf8, Encoding::Utf8)
            | (Encoding::Latin1, Encoding::Latin1)
            | (Encoding::Ascii, Encoding::Ascii) => true,
        };
        converted.push(if representable { c } else { '?' });
    }
    Ok(Value::String(converted))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct H;

    impl Host for H {}

    fn call_with(name: &str, args: &[Value]) -> ValueResult {
        let mut env = Env::new();
        let mut host = H;
        call(name, args, &mut env, &mut host).expect("function should be handled")
    }

    fn s(text: &str) -> Value {
        Value::string(text)
    }

    fn split(text: &str) -> Vec<String> {
        match call_with("balloon_split", &[s(text)]).unwrap() {
            Value::List(list) => list.borrow().iter().map(Value::to_string_value).collect(),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn unknown_names_return_none() {
        let mut env = Env::new();
        let mut host = H;
        assert!(call("strlen", &[s("x")], &mut env, &mut host).is_none());
        assert!(call("soundfold", &[s("x")], &mut env, &mut host).is_none());
        assert!(call("getcompletion", &[s(""), s("")], &mut env, &mut host).is_none());
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
    fn balloon_split_short_text_is_one_line() {
        assert_eq!(split("hello world"), vec!["hello world"]);
        assert_eq!(split(""), Vec::<String>::new());
    }

    #[test]
    fn balloon_split_keeps_explicit_newlines() {
        assert_eq!(split("one\ntwo"), vec!["one", "two"]);
        assert_eq!(split("one\n\ntwo"), vec!["one", "", "two"]);
        assert_eq!(split("one\n"), vec!["one"]);
    }

    #[test]
    fn balloon_split_wraps_long_lines_at_spaces() {
        let text = "word ".repeat(20);
        let lines = split(&text);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.chars().count() <= BALLOON_MAX_WIDTH, "{line:?}");
            assert!(!line.ends_with(' '));
        }
        assert_eq!(lines.concat().replace(' ', ""), "word".repeat(20));
    }

    #[test]
    fn balloon_split_hard_splits_unbroken_words() {
        let text = "x".repeat(BALLOON_MAX_WIDTH + 10);
        let lines = split(&text);
        assert_eq!(lines, vec!["x".repeat(BALLOON_MAX_WIDTH), "x".repeat(10)]);
    }

    #[test]
    fn balloon_split_requires_a_string() {
        let error = call_with("balloon_split", &[Value::list(Vec::new())]).unwrap_err();
        assert_eq!(error.code, "E730");
    }

    #[test]
    fn iconv_identity_for_same_encoding() {
        assert_eq!(
            call_with("iconv", &[s("héllo"), s("utf-8"), s("UTF-8")])
                .unwrap()
                .to_string_value(),
            "héllo"
        );
        assert_eq!(
            call_with("iconv", &[s("héllo"), s("latin1"), s("iso-8859-1")])
                .unwrap()
                .to_string_value(),
            "héllo"
        );
    }

    #[test]
    fn iconv_utf8_to_latin1_and_ascii() {
        // Representable latin1 characters survive.
        assert_eq!(
            call_with("iconv", &[s("héllo"), s("utf-8"), s("latin1")])
                .unwrap()
                .to_string_value(),
            "héllo"
        );
        // Non-latin1 characters become '?'.
        assert_eq!(
            call_with("iconv", &[s("日本"), s("utf-8"), s("latin1")])
                .unwrap()
                .to_string_value(),
            "??"
        );
        assert_eq!(
            call_with("iconv", &[s("héllo"), s("utf-8"), s("ascii")])
                .unwrap()
                .to_string_value(),
            "h?llo"
        );
    }

    #[test]
    fn iconv_latin1_to_utf8() {
        // A latin1 String is represented by its Unicode code points here.
        assert_eq!(
            call_with("iconv", &[s("h\u{e9}llo"), s("latin1"), s("utf-8")])
                .unwrap()
                .to_string_value(),
            "héllo"
        );
        assert_eq!(
            call_with("iconv", &[s("h\u{e9}llo"), s("latin1"), s("ascii")])
                .unwrap()
                .to_string_value(),
            "h?llo"
        );
    }

    #[test]
    fn iconv_unsupported_encodings_return_empty() {
        assert_eq!(
            call_with("iconv", &[s("abc"), s("bogus-enc"), s("utf-8")])
                .unwrap()
                .to_string_value(),
            ""
        );
        assert_eq!(
            call_with("iconv", &[s("abc"), s("utf-8"), s("cp932")])
                .unwrap()
                .to_string_value(),
            ""
        );
    }

    #[test]
    fn iconv_requires_strings() {
        let error =
            call_with("iconv", &[Value::list(Vec::new()), s("utf-8"), s("latin1")]).unwrap_err();
        assert_eq!(error.code, "E730");
        let error = call_with("iconv", &[s("x"), s("utf-8")]).unwrap_err();
        assert_eq!(error.code, "E119");
    }
}
