//! `Value`: the VimL type system (reference: typval.h / typval.c).
//!
//! Lists, dicts and blobs use `Rc<RefCell<..>>` so they have reference identity
//! (Vim's "same list" semantics, needed by `:let l2 = l1`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use rvim_core::{VimError, VimResult};

/// Insertion-ordered dictionary, matching Vim dict iteration order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VimDict {
    entries: Vec<(String, Value)>,
    index: HashMap<String, usize>,
}

impl VimDict {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.index.get(key).map(|i| &self.entries[*i].1)
    }

    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        match self.index.get(&key) {
            Some(idx) => self.entries[*idx].1 = value,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
            }
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let idx = self.index.remove(key)?;
        let (_k, v) = self.entries.remove(idx);
        // Reindex the entries after the removed one.
        for (i, (k, _)) in self.entries.iter().enumerate().skip(idx) {
            self.index.insert(k.clone(), i);
        }
        Some(v)
    }

    pub fn keys(&self) -> Vec<String> {
        self.entries.iter().map(|(k, _)| k.clone()).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

/// A lambda body captured at definition time (`{x -> x + 1}`).
#[derive(Clone, Debug)]
pub struct Lambda {
    pub params: Vec<String>,
    pub varargs: bool,
    pub body: crate::expr::Expr,
    /// Captured local variables (closures see the defining scope).
    pub captured: HashMap<String, Value>,
    /// Script id the lambda was defined in, for `s:` resolution.
    pub sid: usize,
}

/// A Funcref value: a function name plus bound arguments (partial).
#[derive(Clone, Debug)]
pub struct FuncRef {
    /// Canonical function name, e.g. `s:Helper`, `strlen`, `printf`, `<lambda>`.
    pub name: String,
    /// Bound arguments from `function('name', [args])`.
    pub partial: Vec<Value>,
    /// Optional dict for `dict`-functions / `self`.
    pub dict: Option<Value>,
    /// Present for lambdas.
    pub lambda: Option<Rc<Lambda>>,
    /// Present for compiled Vim9 lambdas.
    pub vim9_lambda: Option<Rc<crate::vim9::runtime::Vim9Lambda>>,
}

impl PartialEq for FuncRef {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.partial == other.partial
    }
}

impl FuncRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            partial: Vec::new(),
            dict: None,
            lambda: None,
            vim9_lambda: None,
        }
    }

    pub fn from_lambda(lambda: Lambda) -> Self {
        Self {
            name: "<lambda>".to_string(),
            partial: Vec::new(),
            dict: None,
            lambda: Some(Rc::new(lambda)),
            vim9_lambda: None,
        }
    }
}

/// A VimL value.
#[derive(Clone, Debug)]
pub enum Value {
    Number(i64),
    Float(f64),
    String(String),
    List(Rc<RefCell<Vec<Value>>>),
    Dict(Rc<RefCell<VimDict>>),
    Funcref(Rc<FuncRef>),
    Blob(Rc<RefCell<Vec<u8>>>),
    Bool(bool),
    Null,
}

/// Vim-style value equality (`==` semantics, with reference containers
/// compared by content).
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        values_equal(self, other, 0)
    }
}

pub type ValueResult = VimResult<Value>;

impl Value {
    pub fn string(s: impl Into<String>) -> Self {
        Value::String(s.into())
    }

    pub fn list(items: Vec<Value>) -> Self {
        Value::List(Rc::new(RefCell::new(items)))
    }

    pub fn dict(d: VimDict) -> Self {
        Value::Dict(Rc::new(RefCell::new(d)))
    }

    pub fn blob(bytes: Vec<u8>) -> Self {
        Value::Blob(Rc::new(RefCell::new(bytes)))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Number(_) => "number",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::List(_) => "list",
            Value::Dict(_) => "dict",
            Value::Funcref(_) => "func",
            Value::Blob(_) => "blob",
            Value::Bool(_) => "bool",
            Value::Null => "null",
        }
    }

    /// Vim's truthiness for `:if`/`:while`/`!`/`&&`/`||`.
    ///
    /// Legacy Vim goes through `tv_get_number_chk()` (eval.c:182,
    /// `eval_to_bool()`), so a String is converted to a Number first
    /// (`'x'` is false, `'1'` is true) and a Float is truncated to an integer.
    /// Lists/dicts/blobs/funcrefs are an error in Vim (`E745`); we treat them
    /// as "non-empty is true" and register that in docs/compat_deviations.md.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Number(n) => *n != 0,
            Value::Float(f) => (*f as i64) != 0,
            Value::String(s) => str_to_number(s) != 0,
            Value::List(l) => !l.borrow().is_empty(),
            Value::Dict(d) => !d.borrow().is_empty(),
            Value::Blob(b) => !b.borrow().is_empty(),
            Value::Funcref(_) => true,
            Value::Bool(b) => *b,
            Value::Null => false,
        }
    }

    /// Convert to a number (Vim's weak string→number conversion).
    pub fn to_number(&self) -> i64 {
        match self {
            Value::Number(n) => *n,
            Value::Float(f) => *f as i64,
            Value::Bool(b) => i64::from(*b),
            Value::String(s) => str_to_number(s),
            Value::Null => 0,
            other => other.to_number_slow(),
        }
    }

    fn to_number_slow(&self) -> i64 {
        // Lists/dicts/funcrefs are not numbers; Vim raises E745. Callers that
        // need that distinction use `as_number()` instead.
        0
    }

    pub fn as_number(&self) -> VimResult<i64> {
        match self {
            Value::Number(n) => Ok(*n),
            Value::Float(f) => Ok(*f as i64),
            Value::Bool(b) => Ok(i64::from(*b)),
            Value::String(s) => Ok(str_to_number(s)),
            Value::Null => Ok(0),
            Value::List(_) | Value::Dict(_) | Value::Funcref(_) | Value::Blob(_) => Err(
                VimError::new("E745", format!("Using a {} as a Number", self.type_name())),
            ),
        }
    }

    pub fn to_float(&self) -> VimResult<f64> {
        match self {
            Value::Float(f) => Ok(*f),
            Value::Number(n) => Ok(*n as f64),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::String(s) => Ok(str_to_number(s) as f64),
            Value::Null => Ok(0.0),
            other => Err(VimError::new(
                "E891",
                format!("Using a {} as a Float", other.type_name()),
            )),
        }
    }

    /// Convert to a string for `:echo`, `string()` and concatenation.
    pub fn to_string_value(&self) -> String {
        match self {
            Value::Number(n) => n.to_string(),
            Value::Float(f) => format_float(*f),
            Value::String(s) => s.clone(),
            Value::Bool(b) => if *b { "v:true" } else { "v:false" }.to_string(),
            Value::Null => "v:null".to_string(),
            Value::List(l) => {
                let items: Vec<String> = l.borrow().iter().map(Value::to_repr).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Dict(d) => {
                let items: Vec<String> = d
                    .borrow()
                    .iter()
                    .map(|(k, v)| format!("{}: {}", quote_string(k), v.to_repr()))
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
            Value::Blob(b) => {
                let items: Vec<String> = b.borrow().iter().map(|x| format!("{x:02X}")).collect();
                format!("0z{}", items.join(""))
            }
            Value::Funcref(f) => format!("function('{}')", f.name),
        }
    }

    /// Representation used *inside* containers: strings are quoted.
    pub fn to_repr(&self) -> String {
        match self {
            Value::String(s) => quote_string(s),
            other => other.to_string_value(),
        }
    }

    /// Numeric equality/comparison helper used by the operators.
    pub fn compare(&self, other: &Value) -> i64 {
        compare_values(self, other)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_value())
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Number(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

/// `str2nr({expr} [, {base} [, {quote}]])` (strings.c:1143-1192).
///
/// Unlike `tv_get_number()` this skips leading white space and accepts `+`.
/// Returns `None` for a base other than 2/8/10/16 (`E474`).
pub fn str2nr_vim(text: &str, base: u32, quote: bool) -> Option<i64> {
    if !matches!(base, 2 | 8 | 10 | 16) {
        return None;
    }
    let p = text.trim_start();
    let (negative, rest) = if let Some(r) = p.strip_prefix('-') {
        (true, r.trim_start())
    } else if let Some(r) = p.strip_prefix('+') {
        (false, r.trim_start())
    } else {
        (false, p)
    };
    let n = parse_vim_number(rest, Some(base), quote);
    Some(if negative { -n } else { n })
}

/// Parse a number the way Vim's `vim_str2nr()` does for `tv_get_number()`:
/// no leading whitespace is skipped, a `-` is only recognized at position 0,
/// and overflow clamps to `VARNUM_MAX`/`VARNUM_MIN`.
///
/// Reference: charset.c:1883-2010 (`vim_str2nr`), typval.c:201-210
/// (`tv_get_number` calls it with `STR2NR_ALL`, `strict = FALSE`).
pub fn str_to_number(s: &str) -> i64 {
    parse_vim_number(s, None, false)
}

/// The shared `vim_str2nr()` core. `force_base` selects `str2nr()` semantics
/// (`STR2NR_FORCE`: only prefixes valid for that base are recognized, digits are
/// taken in that base); `None` is the `STR2NR_ALL` conversion used for strings.
/// `quote` enables `STR2NR_QUOTE` embedded single quotes (`str2nr(..., 1)`).
pub fn parse_vim_number(text: &str, force_base: Option<u32>, quote: bool) -> i64 {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut negative = false;
    if force_base.is_none() {
        // vim_str2nr: "if (ptr[0] == '-') { negative = TRUE; ++ptr; }" — a '+'
        // or any whitespace is *not* accepted there.
        if bytes.first() == Some(&b'-') {
            negative = true;
            i = 1;
        }
    }
    let mut base = force_base.unwrap_or(10);
    // Prefix detection still happens with STR2NR_FORCE, but only the flags in
    // "what" are honored: base 16 -> STR2NR_HEX, base 2 -> STR2NR_BIN,
    // base 8 -> STR2NR_OCT|STR2NR_OOCT, base 10 -> nothing (strings.c:1143-1192).
    let allow_hex = matches!(force_base, None | Some(16));
    let allow_bin = matches!(force_base, None | Some(2));
    let allow_ooct = matches!(force_base, None | Some(8));
    let allow_oct_guess = matches!(force_base, None | Some(8));
    // Prefix detection: only when ptr[0] == '0' and ptr[1] != '8'/'9'.
    if bytes.get(i) == Some(&b'0') && !matches!(bytes.get(i + 1), Some(b'8') | Some(b'9')) {
        let c1 = bytes.get(i + 1).copied();
        match c1 {
            Some(b'x') | Some(b'X')
                if allow_hex && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit) =>
            {
                base = 16;
                i += 2;
            }
            Some(b'b') | Some(b'B')
                if allow_bin && matches!(bytes.get(i + 2), Some(b'0' | b'1')) =>
            {
                base = 2;
                i += 2;
            }
            Some(b'o') | Some(b'O')
                if allow_ooct && matches!(bytes.get(i + 2), Some(b'0'..=b'7')) =>
            {
                base = 8;
                i += 2;
            }
            _ => {
                // Octal only if every following digit is 0-7 ("08"/"0129"
                // stay decimal, charset.c:1878-1894).
                if allow_oct_guess {
                    let mut j = i + 1;
                    let mut octal = false;
                    while let Some(&d) = bytes.get(j) {
                        if !d.is_ascii_digit() {
                            break;
                        }
                        if d > b'7' {
                            octal = false;
                            break;
                        }
                        octal = true;
                        j += 1;
                    }
                    if octal {
                        base = 8;
                        // Keep i at the leading "0" so it is consumed as a digit.
                    }
                }
            }
        }
    }
    // Accumulate overflow-safe like vim (& un = UVARNUM_MAX on overflow).
    let mut un: u64 = 0;
    while let Some(&c) = bytes.get(i) {
        let Some(d) = (c as char).to_digit(base) else {
            break;
        };
        un = un
            .saturating_mul(u64::from(base))
            .saturating_add(u64::from(d));
        i += 1;
        if quote && bytes.get(i) == Some(&b'\'') {
            // STR2NR_QUOTE: disambiguate digit groups like 1'000.
            let next_ok = bytes
                .get(i + 1)
                .and_then(|b| (*b as char).to_digit(base))
                .is_some();
            if next_ok {
                i += 1;
            }
        }
    }
    if negative {
        // charset.c:1993-2001: avoid UB, "-VARNUM_MAX-1" clamps to VARNUM_MIN.
        if un > i64::MAX as u64 {
            i64::MIN
        } else {
            -(un as i64)
        }
    } else if un > i64::MAX as u64 {
        i64::MAX
    } else {
        un as i64
    }
}

/// Vim's `vim_snprintf("%g")` (reference strings.c:2678-2790): the same format
/// spec is used for `string()`/`:echo` of a Float (eval.c:497) and for
/// `printf('%g')`. Unlike C's `%g` it always keeps a decimal point, switches to
/// exponent form only outside [0.001, 1e7) and drops `+`/leading zeroes from
/// the exponent.
pub fn format_float(f: f64) -> String {
    format_vim_g(f, None)
}

/// `printf("%.*g", prec, f)` with Vim's `%g` rules.
pub fn format_g(f: f64, prec: usize) -> String {
    format_vim_g(f, Some(prec.max(1)))
}

fn format_vim_g(f: f64, precision: Option<usize>) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    let abs = f.abs();
    let use_f = (0.001..10_000_000.0).contains(&abs) || abs == 0.0;
    let digits = precision.unwrap_or(6);
    if use_f {
        let mut s = format!("{f:.digits$}");
        if precision.is_none() {
            trim_float_zeros(&mut s, false);
        }
        s
    } else {
        let mut s = format!("{f:.digits$e}");
        strip_exponent_sign_and_zeros(&mut s);
        if precision.is_none() {
            trim_float_zeros(&mut s, true);
        }
        s
    }
}

/// Rust formats exponents as `1.5e3`; C uses `1.5e+03`.
pub fn fix_exponent(s: &str) -> String {
    match s.find('e') {
        Some(pos) => {
            let (mantissa, exp) = s.split_at(pos);
            let digits = &exp[1..];
            let (sign, digits) = match digits.strip_prefix('-') {
                Some(rest) => ('-', rest),
                None => ('+', digits),
            };
            let digits = if digits.len() < 2 {
                format!("0{digits}")
            } else {
                digits.to_string()
            };
            format!("{mantissa}e{sign}{digits}")
        }
        None => s.to_string(),
    }
}

/// Change `1.000000e+20` into `1.000000e20` (vim drops the exponent sign and
/// leading zeroes for `%g`; strings.c:2757-2777).
fn strip_exponent_sign_and_zeros(s: &mut String) {
    let Some(pos) = s.find('e') else { return };
    let mantissa = s[..pos].to_string();
    let mut exp = s[pos + 1..].to_string();
    if let Some(rest) = exp.strip_prefix('+') {
        exp = rest.to_string();
    }
    let sign = if let Some(rest) = exp.strip_prefix('-') {
        exp = rest.to_string();
        "-"
    } else {
        ""
    };
    let trimmed = exp.trim_start_matches('0');
    let exp = if trimmed.is_empty() { "0" } else { trimmed };
    *s = format!("{mantissa}e{sign}{exp}");
}

/// Remove trailing zeroes, keeping the digit right after the decimal point
/// (`1.000000` -> `1.0`, `0.500000` -> `0.5`). strings.c:2780-2789.
fn trim_float_zeros(s: &mut String, has_exponent: bool) {
    let end = if has_exponent {
        s.find('e').unwrap_or(s.len())
    } else {
        s.len()
    };
    let mut mantissa = s[..end].to_string();
    while mantissa.len() > 2
        && mantissa.ends_with('0')
        && !mantissa[..mantissa.len() - 1].ends_with('.')
    {
        mantissa.pop();
    }
    let rest = &s[end..];
    *s = format!("{mantissa}{rest}");
}

/// Quote a string for display inside containers (single quotes, '' escaping).
pub fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Vim comparison returning -1/0/1 (used by operators and sort()).
pub fn compare_values(a: &Value, b: &Value) -> i64 {
    // Any float operand makes the comparison floating point; two strings
    // compare lexicographically; everything else is numeric.
    if matches!(a, Value::Float(_)) || matches!(b, Value::Float(_)) {
        let fa = a.to_float().unwrap_or(0.0);
        let fb = b.to_float().unwrap_or(0.0);
        return if fa < fb {
            -1
        } else if fa > fb {
            1
        } else {
            0
        };
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => match x.as_str().cmp(y.as_str()) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        },
        _ => {
            let na = a.to_number();
            let nb = b.to_number();
            if na < nb {
                -1
            } else if na > nb {
                1
            } else {
                0
            }
        }
    }
}

/// Vim's `tv_equal()` (typval.c:1748): compare items like `==` does, except
/// that Strings and Numbers are different types and so are Floats and Numbers.
/// Used recursively for container comparison and by `index()`/`count()`/`uniq()`.
pub fn tv_equal(a: &Value, b: &Value) -> bool {
    const MAX_DEPTH: usize = 1000;
    fn go(a: &Value, b: &Value, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return true; // vim guesses "equal" past its recursion limit
        }
        // VAR_BOOL/VAR_SPECIAL compare against numbers by value.
        let special = |v: &Value| matches!(v, Value::Bool(_) | Value::Null);
        if std::mem::discriminant(a) != std::mem::discriminant(b) {
            return (special(a) || special(b)) && a.to_number() == b.to_number();
        }
        match (a, b) {
            (Value::List(x), Value::List(y)) => {
                let x = x.borrow();
                let y = y.borrow();
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| go(a, b, depth + 1))
            }
            (Value::Dict(x), Value::Dict(y)) => {
                let x = x.borrow();
                let y = y.borrow();
                x.len() == y.len()
                    && x.iter()
                        .all(|(k, v)| y.get(k).is_some_and(|w| go(v, w, depth + 1)))
            }
            (Value::Blob(x), Value::Blob(y)) => *x.borrow() == *y.borrow(),
            (Value::Funcref(x), Value::Funcref(y)) => x.name == y.name && x.partial == y.partial,
            (Value::Float(x), Value::Float(y)) => x == y,
            (Value::String(x), Value::String(y)) => x == y,
            _ => a.to_number() == b.to_number(),
        }
    }
    go(a, b, 0)
}

/// Deep equality (`==` / `is` share this for scalars).
///
/// Mirrors the branch order of `typval_compare()` (typval.c:1170-1370):
/// `v:null` has special rules, floats compare as floats, plain numbers as
/// numbers, and everything else (including Bool/Special against strings) as
/// strings. Containers compare with type-sensitive element equality.
pub fn values_equal(a: &Value, b: &Value, depth: usize) -> bool {
    if depth > 100 {
        return false;
    }
    // typval_compare_null(): v:null equals a null container/string pointer or
    // the number/float zero; it never equals a materialized List/Dict/String.
    if matches!(a, Value::Null) || matches!(b, Value::Null) {
        if matches!(a, Value::Null) && matches!(b, Value::Null) {
            return true;
        }
        let other = if matches!(a, Value::Null) { b } else { a };
        return match other {
            Value::Number(n) => *n == 0,
            Value::Float(f) => *f == 0.0,
            // Materialized containers/strings/funcrefs have a non-NULL pointer.
            Value::List(_)
            | Value::Dict(_)
            | Value::Blob(_)
            | Value::Funcref(_)
            | Value::String(_) => false,
            Value::Bool(_) | Value::Null => false,
        };
    }
    match (a, b) {
        (Value::List(_), Value::List(_))
        | (Value::Dict(_), Value::Dict(_))
        | (Value::Blob(_), Value::Blob(_)) => tv_equal(a, b),
        // A container compared against another type is an error in Vim
        // (E691/E735/E977); report "not equal" rather than inventing equality.
        (Value::List(_) | Value::Dict(_) | Value::Blob(_), _)
        | (_, Value::List(_) | Value::Dict(_) | Value::Blob(_)) => false,
        (Value::Funcref(x), Value::Funcref(y)) => x.name == y.name && x.partial == y.partial,
        (Value::Funcref(_), _) | (_, Value::Funcref(_)) => false,
        (Value::Float(_), _) | (_, Value::Float(_)) => {
            a.to_float().unwrap_or(0.0) == b.to_float().unwrap_or(0.0)
        }
        // "If one of the two variables is a number, compare as a number."
        (Value::Number(_), _) | (_, Value::Number(_)) => a.to_number() == b.to_number(),
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        // Everything else compares through tv_get_string(): this is what makes
        // `v:true == 'v:true'` true in legacy script (typval.c:1362-1370).
        _ => a.to_string_value() == b.to_string_value(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_parse_vim_style() {
        assert_eq!(str_to_number("42"), 42);
        assert_eq!(str_to_number("-7xyz"), -7);
        assert_eq!(str_to_number("0x1f"), 31);
        assert_eq!(str_to_number("0b101"), 5);
        assert_eq!(str_to_number("010"), 8);
        assert_eq!(str_to_number("08"), 8);
        assert_eq!(str_to_number("0o17"), 15);
        assert_eq!(str_to_number("abc"), 0);
        // vim_str2nr does not skip leading white space and only accepts '-'
        // at position 0 (charset.c:1926-1931; typval.c:201-210).
        assert_eq!(str_to_number(" 12"), 0);
        assert_eq!(str_to_number("+7"), 0);
        assert_eq!(str_to_number("-9223372036854775808"), i64::MIN);
        assert_eq!(str_to_number("0xffffffffffffffff"), i64::MAX);
        assert_eq!(str_to_number("99999999999999999999"), i64::MAX);
    }

    #[test]
    fn str2nr_matches_f_str2nr() {
        // strings.c:1143-1192: white space and a leading sign are skipped, the
        // base forces the digit set (prefixes still honored).
        assert_eq!(str2nr_vim(" 12", 10, false), Some(12));
        assert_eq!(str2nr_vim("+7", 10, false), Some(7));
        assert_eq!(str2nr_vim("-ff", 16, false), Some(-255));
        assert_eq!(str2nr_vim("0x1f", 16, false), Some(31));
        assert_eq!(str2nr_vim("101", 2, false), Some(5));
        assert_eq!(str2nr_vim("0o17", 8, false), Some(15));
        assert_eq!(str2nr_vim("0x17", 8, false), Some(0));
        assert_eq!(str2nr_vim("017", 8, false), Some(15));
        assert_eq!(str2nr_vim("017", 10, false), Some(17));
        assert_eq!(str2nr_vim("1'000", 10, true), Some(1000));
        assert_eq!(
            str2nr_vim("99999999999999999999", 10, false),
            Some(i64::MAX)
        );
        assert_eq!(str2nr_vim("12", 7, false), None);
    }

    #[test]
    fn comparison_follows_typval_compare() {
        // typval_compare_null(): v:null is only equal to a null pointer, the
        // number/float zero (typval.c:1414-1445).
        assert!(!values_equal(&Value::Null, &Value::list(vec![]), 0));
        assert!(!values_equal(&Value::string(""), &Value::Null, 0));
        assert!(values_equal(&Value::Null, &Value::Number(0), 0));
        assert!(values_equal(&Value::Null, &Value::Null, 0));
        // Bool/Special compares through tv_get_string() in legacy script.
        assert!(values_equal(
            &Value::Bool(true),
            &Value::string("v:true"),
            0
        ));
        assert!(values_equal(
            &Value::Bool(false),
            &Value::string("v:false"),
            0
        ));
        assert!(!values_equal(&Value::Bool(true), &Value::Number(2), 0));
        // A container against a scalar is not "equal" (it is E691 in Vim).
        assert!(!values_equal(
            &Value::Number(0),
            &Value::list(vec![Value::Number(1)]),
            0
        ));
        assert!(!values_equal(
            &Value::dict(VimDict::new()),
            &Value::Number(0),
            0
        ));
    }

    #[test]
    fn dict_is_insertion_ordered_and_supports_remove() {
        let mut d = VimDict::new();
        d.insert("b", Value::Number(2));
        d.insert("a", Value::Number(1));
        d.insert("c", Value::Number(3));
        assert_eq!(d.keys(), vec!["b", "a", "c"]);
        d.remove("a");
        assert_eq!(d.keys(), vec!["b", "c"]);
        d.insert("a", Value::Number(9));
        assert_eq!(d.keys(), vec!["b", "c", "a"]);
        assert!(d.get("c").is_some());
    }

    #[test]
    fn display_quotes_strings_inside_containers() {
        let v = Value::list(vec![
            Value::string("a'b"),
            Value::Number(2),
            Value::list(vec![Value::string("inner")]),
        ]);
        assert_eq!(v.to_string_value(), "['a''b', 2, ['inner']]");
        // Top-level string prints raw for :echo.
        assert_eq!(Value::string("x").to_string_value(), "x");
    }

    #[test]
    fn floats_render_with_decimal_point() {
        // vim_snprintf("%g") keeps one decimal digit and uses exponent form
        // outside [0.001, 1e7) (strings.c:2678-2790).
        assert_eq!(format_float(1.0), "1.0");
        assert_eq!(format_float(1.5), "1.5");
        assert_eq!(format_float(1e20), "1.0e20");
        assert_eq!(format_float(1.0 / 3.0), "0.333333");
        assert_eq!(format_float(0.000001), "1.0e-6");
        assert_eq!(format_float(100.0), "100.0");
        assert_eq!(format_float(0.0), "0.0");
        assert_eq!(format_float(-2.5), "-2.5");
    }

    #[test]
    fn equality_and_comparison() {
        assert!(values_equal(&Value::Number(1), &Value::Number(1), 0));
        assert!(values_equal(
            &Value::string("abc"),
            &Value::string("abc"),
            0
        ));
        assert!(!values_equal(&Value::string("abc"), &Value::Number(1), 0));
        assert_eq!(compare_values(&Value::Number(1), &Value::Number(2)), -1);
        assert_eq!(compare_values(&Value::string("b"), &Value::string("a")), 1);
        let l1 = Value::list(vec![Value::Number(1)]);
        let l2 = Value::list(vec![Value::Number(1)]);
        assert!(values_equal(&l1, &l2, 0));
    }
}
