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
            // optiondefs.h: P_BOOL|P_VIM without P_VI_DEF, so the VIM_DEFAULT
            // (on) applies: 'showcmd' is enabled by default in a nocompatible Vim.
            default: OptValue::Bool(true),
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
            name: "key",
            short: "key",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "cryptmethod",
            short: "cm",
            kind: OptKind::Str,
            default: OptValue::Str("blowfish2".into()),
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
        // ---- M5 window-local options (optiondefs.h P_WIN) -------------------
        OptDef {
            name: "foldmethod",
            short: "fdm",
            kind: OptKind::Str,
            default: OptValue::Str("manual".into()),
        },
        OptDef {
            name: "foldlevel",
            short: "fdl",
            kind: OptKind::Num,
            default: OptValue::Num(0),
        },
        OptDef {
            name: "foldcolumn",
            short: "fdc",
            kind: OptKind::Str,
            default: OptValue::Str("0".into()),
        },
        OptDef {
            name: "foldminlines",
            short: "fml",
            kind: OptKind::Num,
            default: OptValue::Num(1),
        },
        OptDef {
            name: "foldtext",
            short: "fdt",
            kind: OptKind::Str,
            default: OptValue::Str("foldtext()".into()),
        },
        OptDef {
            name: "diff",
            short: "diff",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "cursorline",
            short: "cul",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "cursorcolumn",
            short: "cuc",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "sidescrolloff",
            short: "siso",
            kind: OptKind::Num,
            default: OptValue::Num(0),
        },
        OptDef {
            name: "signcolumn",
            short: "scl",
            kind: OptKind::Str,
            default: OptValue::Str("auto".into()),
        },
        OptDef {
            name: "colorcolumn",
            short: "cc",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "spell",
            short: "spell",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "previewwindow",
            short: "pvw",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "linebreak",
            short: "lbr",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "breakindent",
            short: "bri",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "numberwidth",
            short: "nuw",
            kind: OptKind::Num,
            default: OptValue::Num(4),
        },
        OptDef {
            name: "winfixheight",
            short: "wfh",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "winfixwidth",
            short: "wfw",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "winminheight",
            short: "wmh",
            kind: OptKind::Num,
            default: OptValue::Num(1),
        },
        OptDef {
            name: "winminwidth",
            short: "wmw",
            kind: OptKind::Num,
            default: OptValue::Num(1),
        },
        OptDef {
            name: "scrollbind",
            short: "scb",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "cursorbind",
            short: "crb",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        // ---- options the runtime scripts read before M5 (packpath/maxfuncdepth)
        OptDef {
            name: "packpath",
            short: "pp",
            kind: OptKind::Str,
            // Vim defaults it to `'runtimepath'`; the empty string makes
            // `:runtime` fall back to 'runtimepath' (handlers_script.rs).
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "maxfuncdepth",
            short: "mfd",
            kind: OptKind::Num,
            default: OptValue::Num(100),
        },
        // ---- M6: tags, C indenting and spell checking -----------------------
        OptDef {
            name: "tags",
            short: "tags",
            kind: OptKind::Str,
            default: OptValue::Str("tags".into()),
        },
        OptDef {
            name: "taglength",
            short: "tl",
            kind: OptKind::Num,
            default: OptValue::Num(0),
        },
        OptDef {
            name: "showfulltag",
            short: "sft",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "cindent",
            short: "cin",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "smartindent",
            short: "si",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "cinoptions",
            short: "cino",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "cinwords",
            short: "cinw",
            kind: OptKind::Str,
            default: OptValue::Str("if,else,while,do,for,switch".into()),
        },
        OptDef {
            name: "cinkeys",
            short: "cink",
            kind: OptKind::Str,
            default: OptValue::Str("0{,0},0),0],:,0#,!^F,o,O,e".into()),
        },
        OptDef {
            name: "indentexpr",
            short: "inde",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "indentkeys",
            short: "indk",
            kind: OptKind::Str,
            default: OptValue::Str("0{,0},0),0],:,0#,!^F,o,O,e".into()),
        },
        OptDef {
            name: "spelllang",
            short: "spl",
            kind: OptKind::Str,
            default: OptValue::Str("en".into()),
        },
        OptDef {
            name: "spellfile",
            short: "spf",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "spellcapcheck",
            short: "spc",
            kind: OptKind::Str,
            default: OptValue::Str("[.?!]\\s*[\"')\\]]*\\s*$".into()),
        },
        OptDef {
            name: "spellsuggest",
            short: "sps",
            kind: OptKind::Str,
            default: OptValue::Str("best".into()),
        },
        OptDef {
            name: "spelloptions",
            short: "spo",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        // ---- M8 closure: commonly queried remaining options -----------------
        OptDef {
            name: "termguicolors",
            short: "tgc",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "guifont",
            short: "gfn",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "guifontwide",
            short: "gfw",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "sessionoptions",
            short: "ssop",
            kind: OptKind::Str,
            default: OptValue::Str(
                "blank,buffers,curdir,folds,help,tabpages,winsize,terminal".into(),
            ),
        },
        OptDef {
            name: "viewoptions",
            short: "vop",
            kind: OptKind::Str,
            default: OptValue::Str("folds,options,cursor,curdir".into()),
        },
        OptDef {
            name: "foldexpr",
            short: "fde",
            kind: OptKind::Str,
            default: OptValue::Str("0".into()),
        },
        OptDef {
            name: "foldignore",
            short: "fdi",
            kind: OptKind::Str,
            default: OptValue::Str("#".into()),
        },
        OptDef {
            name: "foldnestmax",
            short: "fdn",
            kind: OptKind::Num,
            default: OptValue::Num(20),
        },
        OptDef {
            name: "foldmarker",
            short: "fmr",
            kind: OptKind::Str,
            default: OptValue::Str("{{{,}}}".into()),
        },
        OptDef {
            name: "diffopt",
            short: "dip",
            kind: OptKind::Str,
            default: OptValue::Str("internal,filler,closeoff".into()),
        },
        OptDef {
            name: "commentstring",
            short: "cms",
            kind: OptKind::Str,
            default: OptValue::Str("/*%s*/".into()),
        },
        OptDef {
            name: "formatoptions",
            short: "fo",
            kind: OptKind::Str,
            default: OptValue::Str("tcq".into()),
        },
        OptDef {
            name: "selection",
            short: "sel",
            kind: OptKind::Str,
            default: OptValue::Str("inclusive".into()),
        },
        OptDef {
            name: "clipboard",
            short: "cb",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "paste",
            short: "paste",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "modifiable",
            short: "ma",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "buftype",
            short: "bt",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
        OptDef {
            name: "swapfile",
            short: "swf",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "undofile",
            short: "udf",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "backup",
            short: "bk",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "writebackup",
            short: "wb",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "binary",
            short: "bin",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "endofline",
            short: "eol",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "fixendofline",
            short: "fixeol",
            kind: OptKind::Bool,
            default: OptValue::Bool(true),
        },
        OptDef {
            name: "bomb",
            short: "bomb",
            kind: OptKind::Bool,
            default: OptValue::Bool(false),
        },
        OptDef {
            name: "listchars",
            short: "lcs",
            kind: OptKind::Str,
            default: OptValue::Str("eol:$".into()),
        },
        OptDef {
            name: "fillchars",
            short: "fcs",
            kind: OptKind::Str,
            default: OptValue::Str("vert:|,fold:-".into()),
        },
        OptDef {
            name: "showbreak",
            short: "sbr",
            kind: OptKind::Str,
            default: OptValue::Str("".into()),
        },
    ]
}

/// Options with window scope (`optiondefs.h` P_WIN): `:setlocal` stores them on
/// the current window, and every window keeps its own value (M5).
pub const WINDOW_LOCAL: &[&str] = &[
    "breakindent",
    "colorcolumn",
    "cursorbind",
    "cursorcolumn",
    "cursorline",
    "diff",
    "binary",
    "bomb",
    "buftype",
    "endofline",
    "fixendofline",
    "foldcolumn",
    "foldenable",
    "foldexpr",
    "foldignore",
    "foldlevel",
    "foldmarker",
    "foldmethod",
    "foldminlines",
    "foldtext",
    "linebreak",
    "list",
    "number",
    "numberwidth",
    "previewwindow",
    "relativenumber",
    "scrollbind",
    "scrolloff",
    "sidescrolloff",
    "signcolumn",
    "cindent",
    "cinkeys",
    "cryptmethod",
    "key",
    "cinoptions",
    "cinwords",
    "indentexpr",
    "indentkeys",
    "smartindent",
    "spell",
    "spellfile",
    "spelllang",
    "modifiable",
    "spelloptions",
    "spellsuggest",
    "swapfile",
    "tags",
    "undofile",
    "winfixheight",
    "winfixwidth",
    "winminheight",
    "winminwidth",
    "wrap",
];

/// True when `name` (canonical or short) names a window-local option.
pub fn is_window_local(name: &str) -> bool {
    match Options::canonical(name) {
        Ok(canon) => WINDOW_LOCAL.contains(&canon.as_str()),
        Err(_) => false,
    }
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
        ("cryptmethod", OptValue::Str(s)) => {
            rvim_buffer::crypt::CryptMethod::from_name(s)?;
            Ok(value)
        }
        // option.c: 'foldmethod' only accepts the documented methods.
        ("foldmethod", OptValue::Str(s)) => match s.as_str() {
            "manual" | "indent" | "marker" | "syntax" | "expr" | "diff" => Ok(value),
            other => Err(VimError::new("E474", format!("Invalid argument: {other}"))),
        },
        // 'foldcolumn' is a number 0-12 or "auto" (optionstr.c).
        ("foldcolumn", OptValue::Str(s)) => {
            let ok = s == "auto"
                || s.parse::<i64>()
                    .map(|n| (0..=12).contains(&n))
                    .unwrap_or(false);
            if ok {
                Ok(value)
            } else {
                Err(VimError::new("E474", format!("Invalid argument: {s}")))
            }
        }
        // 'signcolumn' accepts auto/yes/no/number/auto:N (optionstr.c).
        ("signcolumn", OptValue::Str(s)) => {
            let ok = matches!(s.as_str(), "auto" | "yes" | "no" | "number")
                || s.strip_prefix("auto:")
                    .and_then(|n| n.parse::<i64>().ok())
                    .is_some_and(|n| (1..=9).contains(&n));
            if ok {
                Ok(value)
            } else {
                Err(VimError::new("E474", format!("Invalid argument: {s}")))
            }
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
            // `:set fdc=1` parses the value as a number; string options take it
            // as text (option.c checks the type but accepts either spelling for
            // numeric-looking strings).
            (OptKind::Str, OptValue::Num(n)) => OptValue::Str(n.to_string()),
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
                    OptValue::Str(s) if d.name == "key" && !s.is_empty() => "key=*****".to_string(),
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
            OptValue::Str(s) if canon == "key" && !s.is_empty() => "key=*****".to_string(),
            OptValue::Str(s) => format!("{canon}={s:?}"),
        })
    }

    /// Validate and coerce a value for `name` without storing it.
    ///
    /// Window-local options live on [`crate::window::Window`] and are validated
    /// through the same path as global ones, so `:setlocal fdm=indent` reports
    /// E474 for a bad method exactly like `:set` does.
    pub fn validate_value(name: &str, value: OptValue) -> VimResult<OptValue> {
        let mut scratch = Options::new();
        let canon = Options::canonical(name)?;
        scratch.set(&canon, value)?;
        Ok(scratch.get(&canon)?.clone())
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
