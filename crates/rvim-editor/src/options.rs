//! Options (reference: optiondefs.h, 429 entries in the baseline).
//!
//! RustVim deliberately has **no vi-compatible mode**: `'compatible'` is
//! recognised (so runtime scripts containing `set nocompatible` keep working)
//! but permanently locked off. Attempting to enable it is reported rather than
//! silently accepted.

use rvim_core::{VimError, VimResult};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum OptValue {
    Bool(bool),
    Num(i64),
    Str(String),
}

impl OptValue {
    pub fn as_bool(&self) -> Option<bool> {
        if let OptValue::Bool(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_num(&self) -> Option<i64> {
        if let OptValue::Num(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let OptValue::Str(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OptKind {
    Bool,
    Num,
    Str,
}

#[derive(Clone, Debug)]
struct OptDef {
    name: &'static str,
    short: &'static str,
    kind: OptKind,
    default: OptValue,
}

/// The M1 option subset. Names match the baseline option table exactly so the
/// full 429-entry table can replace this array without touching call sites.
fn defs() -> Vec<OptDef> {
    vec![
        OptDef {
            name: "compatible",
            short: "cp",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "number",
            short: "nu",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "relativenumber",
            short: "rnu",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "expandtab",
            short: "et",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "autoindent",
            short: "ai",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "ignorecase",
            short: "ic",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "smartcase",
            short: "scs",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "list",
            short: "list",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "wrap",
            short: "wrap",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "readonly",
            short: "ro",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "modified",
            short: "mod",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "hidden",
            short: "hid",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "incsearch",
            short: "is",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "hlsearch",
            short: "hls",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "ruler",
            short: "ru",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "showcmd",
            short: "sc",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "tabstop",
            short: "ts",
            kind: OptKind::Num,
            default: OptValue::Num(8),
        },
        OptDef {
            name: "softtabstop",
            short: "sts",
            kind: OptKind::Num,
            default: OptValue::Num(0),
        },
        OptDef {
            name: "shiftwidth",
            short: "sw",
            kind: OptKind::Num,
            default: OptValue::Num(8),
        },
        OptDef {
            name: "shiftround",
            short: "sr",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "scrolloff",
            short: "so",
            kind: OptKind::Num,
            default: OptValue::Num(0),
        },
        OptDef {
            name: "textwidth",
            short: "tw",
            kind: OptKind::Num,
            default: OptValue::Num(0),
        },
        OptDef {
            name: "undolevels",
            short: "ul",
            kind: OptKind::Num,
            default: OptValue::Num(1000),
        },
        OptDef {
            name: "history",
            short: "hi",
            kind: OptKind::Num,
            default: OptValue::Num(50),
        },
        OptDef {
            name: "fileformat",
            short: "ff",
            kind: OptKind::Str,
            // Vim's DFLT_FF (option.h): "dos" on Windows, "unix" elsewhere.
            default: OptValue::Str(if cfg!(windows) { "dos" } else { "unix" }.into()),
        },
        OptDef {
            name: "fileencoding",
            short: "fenc",
            kind: OptKind::Str,
            default: OptValue::Str("utf-8".into()),
        },
        OptDef {
            name: "filetype",
            short: "ft",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "iskeyword",
            short: "isk",
            kind: OptKind::Str,
            default: OptValue::Str("@,48-57,_,192-255".into()),
        },
        OptDef {
            name: "wrapscan",
            short: "ws",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "runtimepath",
            short: "rtp",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "joinspaces",
            short: "js",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "report",
            short: "rep",
            kind: OptKind::Num,
            default: OptValue::Num(2),
        },
        OptDef {
            name: "cpo",
            short: "cpo",
            kind: OptKind::Str,
            default: OptValue::Str("aABceFs".into()),
        },
        OptDef {
            name: "verbose",
            short: "vbs",
            kind: OptKind::Num,
            default: OptValue::Num(0),
        },
        OptDef {
            name: "backspace",
            short: "bs",
            kind: OptKind::Str,
            default: OptValue::Str("indent,eol,start".into()),
        },
        OptDef {
            name: "wildmenu",
            short: "wmnu",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "timeout",
            short: "to",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "timeoutlen",
            short: "tm",
            kind: OptKind::Num,
            default: OptValue::Num(1000),
        },
        OptDef {
            name: "ttimeout",
            short: "ttimeout",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "ttimeoutlen",
            short: "ttm",
            kind: OptKind::Num,
            default: OptValue::Num(-1),
        },
        OptDef {
            name: "display",
            short: "dy",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "mouse",
            short: "mouse",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "guioptions",
            short: "go",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "term",
            short: "term",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "langremap",
            short: "lrm",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "nolangremap",
            short: "nlrm",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "nrformats",
            short: "nf",
            kind: OptKind::Str,
            default: OptValue::Str("bin,hex".into()),
        },
        OptDef {
            name: "background",
            short: "bg",
            kind: OptKind::Str,
            default: OptValue::Str("dark".into()),
        },
        OptDef {
            name: "syntax",
            short: "syn",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "more",
            short: "more",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "shell",
            short: "sh",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "shellcmdflag",
            short: "shcf",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "shellslash",
            short: "ssl",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "encoding",
            short: "enc",
            kind: OptKind::Str,
            default: OptValue::Str("utf-8".into()),
        },
        OptDef {
            name: "virtualedit",
            short: "ve",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "foldenable",
            short: "fen",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
    ]
}

/// Enforce Vim's per-option value constraints, keeping the old value on error.
///
/// * `tabstop` must be positive (option.c:3730 `b_p_ts <= 0` → E487).
/// * `shiftwidth` must not be negative (option.c:3329 `b_p_sw < 0` → E487).
/// * `fileformat` must be unix/dos/mac (did_set_string_option PV_FF → E474).
fn validate(canon: &str, value: OptValue) -> VimResult<OptValue> {
    match (canon, &value) {
        ("tabstop", OptValue::Num(n)) if *n < 1 => {
            Err(VimError::argument_must_be_positive(canon, *n))
        }
        ("shiftwidth", OptValue::Num(n)) if *n < 0 => {
            Err(VimError::argument_must_be_positive(canon, *n))
        }
        ("fileformat", OptValue::Str(s)) => {
            rvim_buffer::FileFormat::parse(s)
                .map_err(|_| VimError::invalid_option_argument(&format!("fileformat={s}")))?;
            Ok(value)
        }
        _ => Ok(value),
    }
}

#[derive(Debug, Default)]
pub struct Options {
    values: HashMap<String, OptValue>,
}

impl Options {
    pub fn new() -> Self {
        let mut values = HashMap::new();
        for d in defs() {
            values.insert(d.name.to_string(), d.default.clone());
        }
        Self { values }
    }

    fn def_of(name: &str) -> Option<OptDef> {
        let lower = name.to_ascii_lowercase();
        defs()
            .into_iter()
            .find(|d| d.name == lower || d.short == lower)
    }

    /// Normalise a user-typed name (long or short) to the canonical long name.
    pub fn canonical(name: &str) -> VimResult<String> {
        Self::def_of(name)
            .map(|d| d.name.to_string())
            .ok_or_else(|| VimError::unknown_option(name))
    }

    pub fn get(&self, name: &str) -> VimResult<&OptValue> {
        let canon = Self::canonical(name)?;
        self.values
            .get(&canon)
            .ok_or_else(|| VimError::unknown_option(name))
    }

    pub fn bool_or(&self, name: &str, default: bool) -> bool {
        self.get(name)
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    pub fn num_or(&self, name: &str, default: i64) -> i64 {
        self.get(name)
            .ok()
            .and_then(|v| v.as_num())
            .unwrap_or(default)
    }

    pub fn str_or(&self, name: &str, default: &str) -> String {
        self.get(name)
            .ok()
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| default.to_string())
    }

    /// Set by parsed value. Accepts both `opt`/`noopt` names for booleans
    /// (the same spelling Vim's `:set` accepts). The `compatible` lock is
    /// enforced here.
    pub fn set(&mut self, name: &str, value: OptValue) -> VimResult<()> {
        // Handle the "no" inversion prefix for boolean options.
        if let Some(rest) = name.strip_prefix("no") {
            if Self::def_of(rest).is_some() {
                let inverted = match value {
                    OptValue::Bool(b) => OptValue::Bool(!b),
                    other => other,
                };
                return self.set(rest, inverted);
            }
        }
        let canon = Self::canonical(name)?;
        if canon == "compatible" {
            if value.as_bool() == Some(true) {
                return Err(VimError::new(
                    "ERV010",
                    "RustVim has no vi-compatible mode; 'compatible' is permanently off",
                ));
            }
            // `set nocompatible` is the expected no-op for runtime scripts.
            return Ok(());
        }
        let kind = Self::def_of(&canon).unwrap().kind;
        let checked = match (kind, value) {
            (OptKind::Bool, v @ OptValue::Bool(_)) => v,
            (OptKind::Num, OptValue::Num(n)) => OptValue::Num(n),
            (OptKind::Str, OptValue::Str(s)) => OptValue::Str(s),
            (OptKind::Num, OptValue::Str(s)) => OptValue::Num(
                s.trim()
                    .parse()
                    .map_err(|_| VimError::invalid_argument(&format!("number required: {s}")))?,
            ),
            (OptKind::Bool, OptValue::Str(s)) => match s.as_str() {
                "" => OptValue::Bool(true),
                other => OptValue::Bool(other.parse().map_err(|_| {
                    VimError::invalid_argument(&format!("boolean required: {other}"))
                })?),
            },
            (_, v) => {
                return Err(VimError::invalid_argument(&format!(
                    "wrong type for {canon}: {v:?}"
                )));
            }
        };
        // Value validation (option.c: check_num_option / did_set_string_option):
        // on a bad value Vim reports the error and keeps the old value.
        let checked = validate(&canon, checked)?;
        self.values.insert(canon, checked);
        Ok(())
    }

    /// Reset an option to its default (`:set opt&`).
    pub fn reset_default(&mut self, name: &str) -> VimResult<()> {
        let d = Self::def_of(name).ok_or_else(|| VimError::unknown_option(name))?;
        if d.name == "compatible" {
            return Ok(()); // stays off
        }
        self.values.insert(d.name.to_string(), d.default);
        Ok(())
    }

    /// Render `:set` output: bool options appear as name / noname, values as name=...
    pub fn render_all(&self) -> Vec<String> {
        let mut defs = defs();
        defs.sort_by(|a, b| a.name.cmp(b.name));
        defs.iter()
            .map(|d| {
                let v = self.values.get(d.name).unwrap_or(&d.default);
                match v {
                    OptValue::Bool(true) => d.name.to_string(),
                    OptValue::Bool(false) => format!("no{}", d.name),
                    OptValue::Num(n) => format!("{}={}", d.name, n),
                    OptValue::Str(s) => format!("{}={:?}", d.name, s),
                }
            })
            .collect()
    }

    pub fn render_one(&self, name: &str) -> VimResult<String> {
        let canon = Self::canonical(name)?;
        let v = self.get(&canon)?;
        Ok(match v {
            OptValue::Bool(true) => canon,
            OptValue::Bool(false) => format!("no{canon}"),
            OptValue::Num(n) => format!("{canon}={n}"),
            OptValue::Str(s) => format!("{canon}={s:?}"),
        })
    }

    /// Number of options in the M1 table (coverage reporting hook).
    pub fn table_size() -> usize {
        defs().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_short_names() {
        let o = Options::new();
        assert!(!o.bool_or("expandtab", true));
        assert!(o.get("et").unwrap().as_bool() == Some(false));
        assert_eq!(o.num_or("tabstop", 0), 8);
        assert_eq!(Options::canonical("ts").unwrap(), "tabstop");
    }

    #[test]
    fn compatible_is_locked_off() {
        let mut o = Options::new();
        assert!(o.set("compatible", OptValue::Bool(true)).is_err());
        assert!(o.set("cp", OptValue::Bool(true)).is_err());
        // "set nocompatible" parses as the no-prefix with truthy value,
        // inverting to compatible=false (the locked-off default).
        assert!(o.set("nocompatible", OptValue::Bool(true)).is_ok());
        assert!(o.set("noet", OptValue::Bool(true)).is_ok());
        assert!(!o.bool_or("expandtab", true));
        assert!(!o.bool_or("compatible", true));
    }

    #[test]
    fn set_forms_and_reset() {
        let mut o = Options::new();
        o.set("tabstop", OptValue::Num(4)).unwrap();
        o.set("number", OptValue::Bool(true)).unwrap();
        assert_eq!(o.num_or("ts", 0), 4);
        assert!(o.bool_or("nu", false));
        o.reset_default("tabstop").unwrap();
        assert_eq!(o.num_or("tabstop", 0), 8);
        assert!(
            o.set("nonsense", OptValue::Bool(true))
                .is_err_and(|e| e.code == "E518")
        );
    }

    #[test]
    fn render_set_output() {
        let o = Options::new();
        assert_eq!(o.render_one("number").unwrap(), "nonumber");
        assert_eq!(o.render_one("tabstop").unwrap(), "tabstop=8");
        assert_eq!(o.render_one("compatible").unwrap(), "nocompatible");
    }

    #[test]
    fn fileformat_defaults_to_the_platform_value() {
        let o = Options::new();
        let expected = if cfg!(windows) { "dos" } else { "unix" };
        assert_eq!(o.str_or("fileformat", ""), expected, "DFLT_FF");
        assert_eq!(o.str_or("ff", ""), expected);
    }

    #[test]
    fn invalid_numeric_values_are_rejected_and_keep_the_old_value() {
        let mut o = Options::new();
        let err = o.set("tabstop", OptValue::Num(0)).unwrap_err();
        assert_eq!(err.code, "E487", "tabstop min is 1 (option.c:3730)");
        assert_eq!(o.num_or("tabstop", 0), 8);
        let err = o.set("tabstop", OptValue::Num(-3)).unwrap_err();
        assert_eq!(err.code, "E487");
        assert_eq!(o.num_or("tabstop", 0), 8);
        let err = o.set("shiftwidth", OptValue::Num(-1)).unwrap_err();
        assert_eq!(err.code, "E487", "shiftwidth min is 0 (option.c:3329)");
        assert_eq!(o.num_or("shiftwidth", 0), 8);
        // shiftwidth=0 means "use 'tabstop'" and must stay accepted.
        o.set("shiftwidth", OptValue::Num(0)).unwrap();
        assert_eq!(o.num_or("shiftwidth", 8), 0);
    }

    #[test]
    fn invalid_fileformat_is_rejected_and_keeps_the_old_value() {
        let mut o = Options::new();
        let before = o.str_or("fileformat", "");
        let err = o
            .set("fileformat", OptValue::Str("bogus".into()))
            .unwrap_err();
        assert_eq!(err.code, "E474");
        assert_eq!(o.str_or("fileformat", ""), before);
        o.set("fileformat", OptValue::Str("mac".into())).unwrap();
        assert_eq!(o.str_or("ff", ""), "mac");
    }
}
