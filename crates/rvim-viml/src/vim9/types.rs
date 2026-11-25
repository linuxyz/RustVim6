//! Vim9 script type system (reference: vim9type.c).
//!
//! The type checker and the evaluator share this representation. Types are
//! erased at runtime (values are [Value]), but var/def declarations keep their
//! declared types so assignments and returns can be checked.

use std::fmt;

use crate::value::Value;

/// A Vim9 type.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Vim9Type {
    /// 'any': no checking (also used when inference gives up).
    #[default]
    Any,
    /// 'void': a function that returns nothing.
    Void,
    Bool,
    Number,
    Float,
    String,
    Blob,
    /// 'list<T>'.
    List(Box<Vim9Type>),
    /// 'dict<T>'.
    Dict(Box<Vim9Type>),
    /// 'func' / 'func(...)': any function reference.
    Func,
    /// 'tuple<...>' (baseline 8.2 has tuple types for 'for [a, b]').
    Tuple(Vec<Vim9Type>),
    /// Named opaque types ('job', 'channel', user classes in later Vims).
    Named(String),
}

impl Vim9Type {
    /// Parse a type annotation ('list<dict<any>>', 'func', 'number', ...).
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("empty type".to_string());
        }
        let lower = text.to_ascii_lowercase();
        let simple = match lower.as_str() {
            "any" => Some(Vim9Type::Any),
            "void" => Some(Vim9Type::Void),
            "bool" => Some(Vim9Type::Bool),
            "number" | "int" => Some(Vim9Type::Number),
            "float" => Some(Vim9Type::Float),
            "string" => Some(Vim9Type::String),
            "blob" => Some(Vim9Type::Blob),
            "func" | "funcref" => Some(Vim9Type::Func),
            "job" | "channel" | "dict" | "list" | "object" | "class" | "enum" => {
                Some(Vim9Type::Named(lower.clone()))
            }
            _ => None,
        };
        if let Some(ty) = simple {
            return Ok(ty);
        }
        if let Some(rest) = lower.strip_prefix("list<") {
            let inner = rest
                .strip_suffix('>')
                .ok_or_else(|| format!("invalid type: {text}"))?;
            return Ok(Vim9Type::List(Box::new(Self::parse(inner)?)));
        }
        if let Some(rest) = lower.strip_prefix("dict<") {
            let inner = rest
                .strip_suffix('>')
                .ok_or_else(|| format!("invalid type: {text}"))?;
            return Ok(Vim9Type::Dict(Box::new(Self::parse(inner)?)));
        }
        if let Some(rest) = lower.strip_prefix("tuple<") {
            let inner = rest
                .strip_suffix('>')
                .ok_or_else(|| format!("invalid type: {text}"))?;
            let mut items = Vec::new();
            for part in split_top_level(inner) {
                items.push(Self::parse(&part)?);
            }
            return Ok(Vim9Type::Tuple(items));
        }
        if lower.starts_with("func(") {
            // We do not model signatures; every function value has type func.
            return Ok(Vim9Type::Func);
        }
        // Unknown user type: keep the name, tolerate it (classes are not part of
        // the 8.2 baseline; this keeps forward-compatible scripts loadable).
        Ok(Vim9Type::Named(text.to_string()))
    }

    /// Vim's spelling ('list<string>'), used in error messages.
    pub fn name(&self) -> String {
        match self {
            Vim9Type::Any => "any".to_string(),
            Vim9Type::Void => "void".to_string(),
            Vim9Type::Bool => "bool".to_string(),
            Vim9Type::Number => "number".to_string(),
            Vim9Type::Float => "float".to_string(),
            Vim9Type::String => "string".to_string(),
            Vim9Type::Blob => "blob".to_string(),
            Vim9Type::List(inner) => format!("list<{}>", inner.name()),
            Vim9Type::Dict(inner) => format!("dict<{}>", inner.name()),
            Vim9Type::Func => "func".to_string(),
            Vim9Type::Tuple(items) => {
                let parts: Vec<String> = items.iter().map(Vim9Type::name).collect();
                format!("tuple<{}>", parts.join(", "))
            }
            Vim9Type::Named(name) => name.clone(),
        }
    }

    /// True when a value of self can be used where target is expected.
    pub fn assignable_to(&self, target: &Vim9Type) -> bool {
        if matches!(self, Vim9Type::Any) || matches!(target, Vim9Type::Any) {
            return true;
        }
        if self == target {
            return true;
        }
        // A bool is accepted for number contexts? Vim9 is strict, but our
        // builtins return Number where Vim returns Bool, so allow both ways.
        match (self, target) {
            (Vim9Type::Bool, Vim9Type::Number) | (Vim9Type::Number, Vim9Type::Bool) => true,
            (Vim9Type::List(a), Vim9Type::List(b)) => a.assignable_to(b),
            (Vim9Type::Dict(a), Vim9Type::Dict(b)) => a.assignable_to(b),
            (Vim9Type::Tuple(a), Vim9Type::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.assignable_to(y))
            }
            // 'null' is represented as Any when inferred from v:null.
            _ => false,
        }
    }

    /// Infer the runtime type of a value (used for 'var x = expr').
    pub fn of_value(value: &Value) -> Self {
        match value {
            Value::Number(_) => Vim9Type::Number,
            Value::Float(_) => Vim9Type::Float,
            Value::String(_) => Vim9Type::String,
            Value::List(items) => {
                let mut elem = Vim9Type::Any;
                for (i, item) in items.borrow().iter().enumerate() {
                    let ty = Self::of_value(item);
                    elem = if i == 0 { ty } else { join(&elem, &ty) };
                }
                Vim9Type::List(Box::new(elem))
            }
            Value::Dict(dict) => {
                let mut elem = Vim9Type::Any;
                for (i, (_, item)) in dict.borrow().iter().enumerate() {
                    let ty = Self::of_value(item);
                    elem = if i == 0 { ty } else { join(&elem, &ty) };
                }
                Vim9Type::Dict(Box::new(elem))
            }
            Value::Blob(_) => Vim9Type::Blob,
            Value::Funcref(_) => Vim9Type::Func,
            Value::Bool(_) => Vim9Type::Bool,
            Value::Null => Vim9Type::Any,
        }
    }

    /// True when a value satisfies this type (runtime check for arguments and
    /// declared assignments).
    pub fn accepts_value(&self, value: &Value) -> bool {
        match self {
            Vim9Type::Any | Vim9Type::Void => true,
            Vim9Type::Bool => matches!(value, Value::Bool(_)),
            Vim9Type::Number => matches!(value, Value::Number(_)),
            Vim9Type::Float => matches!(value, Value::Float(_)),
            Vim9Type::String => matches!(value, Value::String(_)),
            Vim9Type::Blob => matches!(value, Value::Blob(_)),
            Vim9Type::Func => matches!(value, Value::Funcref(_)),
            Vim9Type::List(inner) => match value {
                Value::List(items) => items.borrow().iter().all(|v| inner.accepts_value(v)),
                _ => false,
            },
            Vim9Type::Dict(inner) => match value {
                Value::Dict(dict) => dict.borrow().iter().all(|(_, v)| inner.accepts_value(v)),
                _ => false,
            },
            Vim9Type::Tuple(items) => match value {
                Value::List(list) => {
                    let list = list.borrow();
                    list.len() == items.len()
                        && items
                            .iter()
                            .zip(list.iter())
                            .all(|(ty, v)| ty.accepts_value(v))
                }
                _ => false,
            },
            // Named types are checked by their runtime value type name.
            Vim9Type::Named(name) => match name.as_str() {
                "dict" => matches!(value, Value::Dict(_)),
                "list" => matches!(value, Value::List(_)),
                _ => true,
            },
        }
    }
}

impl fmt::Display for Vim9Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// Least common type: equal types stay, 'any' absorbs, otherwise 'any'.
pub fn join(a: &Vim9Type, b: &Vim9Type) -> Vim9Type {
    if a == b {
        return a.clone();
    }
    match (a, b) {
        (Vim9Type::Any, _) | (_, Vim9Type::Any) => Vim9Type::Any,
        (Vim9Type::List(x), Vim9Type::List(y)) => Vim9Type::List(Box::new(join(x, y))),
        (Vim9Type::Dict(x), Vim9Type::Dict(y)) => Vim9Type::Dict(Box::new(join(x, y))),
        (Vim9Type::Bool, Vim9Type::Number) | (Vim9Type::Number, Vim9Type::Bool) => Vim9Type::Number,
        _ => Vim9Type::Any,
    }
}

/// Split on commas that are not nested inside '<>' / '(...)' / '[]'.
fn split_top_level(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(text[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < text.len() {
        out.push(text[start..].trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_types() {
        assert_eq!(
            Vim9Type::parse("list<dict<any>>").unwrap(),
            Vim9Type::List(Box::new(Vim9Type::Dict(Box::new(Vim9Type::Any))))
        );
        assert_eq!(
            Vim9Type::parse("tuple<number, string>").unwrap(),
            Vim9Type::Tuple(vec![Vim9Type::Number, Vim9Type::String])
        );
        assert_eq!(Vim9Type::parse("number").unwrap(), Vim9Type::Number);
        assert_eq!(
            Vim9Type::parse("list<dict<any>>").unwrap().name(),
            "list<dict<any>>"
        );
    }

    #[test]
    fn assignment_rules() {
        assert!(Vim9Type::Number.assignable_to(&Vim9Type::Any));
        assert!(Vim9Type::Any.assignable_to(&Vim9Type::String));
        assert!(Vim9Type::Bool.assignable_to(&Vim9Type::Number));
        assert!(!Vim9Type::String.assignable_to(&Vim9Type::Number));
        assert!(
            Vim9Type::List(Box::new(Vim9Type::Number))
                .assignable_to(&Vim9Type::List(Box::new(Vim9Type::Any)))
        );
        assert!(
            !Vim9Type::List(Box::new(Vim9Type::String))
                .assignable_to(&Vim9Type::List(Box::new(Vim9Type::Number)))
        );
    }

    #[test]
    fn values_have_inferable_types() {
        assert_eq!(Vim9Type::of_value(&Value::Number(3)), Vim9Type::Number);
        assert_eq!(
            Vim9Type::of_value(&Value::list(vec![Value::Number(1), Value::Number(2)])),
            Vim9Type::List(Box::new(Vim9Type::Number))
        );
        assert!(Vim9Type::String.accepts_value(&Value::string("x")));
        assert!(!Vim9Type::Number.accepts_value(&Value::string("x")));
    }
}
