//! Declarative Ex command table (baseline: `src/ex_cmds.h`, 589 EXCMD entries).
//!
//! The table itself is *generated* into [`crate::cmd_table`] from the vendored
//! reference source, preserved in the reference's curated order. This module
//! adds the EX_* flag bits, the address kinds and Vim's lookup rule.
//!
//! Vim sorts `cmdnames[]` on the **shortest working abbreviation** and resolves
//! an abbreviation to the *first* entry it prefixes, which is why `:r` is
//! `:read` (not `:rewind`), `:w` is `:write` (not `:wq`) and `:p` is `:print`
//! (not `:previous`). See reference `ex_docmd.c` `find_ex_command()`.

pub use crate::cmd_table::{COMMAND_COUNT, COMMANDS, Cmd};

/// Address type, aligned with `cmd_addr_T` in ex_cmds.h lines 71-84.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrKind {
    /// Buffer line numbers.
    Lines,
    /// Window number.
    Windows,
    /// Argument number.
    Arguments,
    /// Buffer number of a loaded buffer.
    LoadedBuffers,
    /// Buffer number.
    Buffers,
    /// Tab page number.
    Tabs,
    /// Tab page that only allows relative addressing.
    TabsRelative,
    /// Quickfix list valid entry number.
    QuickfixValid,
    /// Quickfix list entry number.
    Quickfix,
    /// Positive count or zero, defaults to 1.
    Unsigned,
    /// Something else; use the line number for `$`, `%` etc.
    Other,
    /// No range used.
    None,
}

/// EX_* flag bits from ex_cmds.h lines 30-61.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ExFlags(pub u32);

impl ExFlags {
    pub const RANGE: Self = Self(0x001);
    pub const BANG: Self = Self(0x002);
    pub const EXTRA: Self = Self(0x004);
    pub const XFILE: Self = Self(0x008);
    pub const NOSPC: Self = Self(0x010);
    pub const DFLALL: Self = Self(0x020);
    pub const WHOLEFOLD: Self = Self(0x040);
    pub const NEEDARG: Self = Self(0x080);
    pub const TRLBAR: Self = Self(0x100);
    pub const REGSTR: Self = Self(0x200);
    pub const COUNT: Self = Self(0x400);
    pub const NOTRLCOM: Self = Self(0x800);
    pub const ZEROR: Self = Self(0x1000);
    pub const CTRLV: Self = Self(0x2000);
    pub const CMDARG: Self = Self(0x4000);
    pub const BUFNAME: Self = Self(0x8000);
    pub const BUFUNL: Self = Self(0x10000);
    pub const ARGOPT: Self = Self(0x20000);
    pub const SBOXOK: Self = Self(0x40000);
    pub const CMDWIN: Self = Self(0x80000);
    pub const MODIFY: Self = Self(0x100000);
    pub const FLAGS: Self = Self(0x200000);
    pub const RESTRICT: Self = Self(0x400000);
    pub const EXPAND: Self = Self(0x800000);
    pub const LOCK_OK: Self = Self(0x1000000);
    pub const NONWHITE_OK: Self = Self(0x2000000);
    pub const KEEPSCRIPT: Self = Self(0x4000000);
    pub const EXPR_ARG: Self = Self(0x8000000);
    pub const WHOLE: Self = Self(0x10000000);

    // Composite macros from ex_cmds.h lines 63-65.
    pub const FILES: Self = Self(0x008 | 0x004);
    pub const FILE1: Self = Self(0x008 | 0x004 | 0x010);
    pub const WORD1: Self = Self(0x004 | 0x010);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    /// True when no flag bits are set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl Cmd {
    pub const fn flags(&self) -> ExFlags {
        ExFlags(self.flags)
    }

    pub const fn has(&self, flag: ExFlags) -> bool {
        self.flags().contains(flag)
    }
}

/// Table index of the first command with this exact name.
pub fn command_index(name: &str) -> Option<usize> {
    COMMANDS.iter().position(|c| c.name == name)
}

/// Resolve a typed command word exactly like `find_ex_command()`:
///
/// * lowercase first character: scan the whole table from the start;
/// * uppercase first character: start at `Next` (so `:N` is `:Next`);
/// * anything else (non-alphabetic commands): start at `!`.
///
/// The first entry whose name starts with `word` wins. That single rule
/// reproduces Vim's abbreviation behaviour, including its famous quirks,
/// because the table order is curated on the shortest working abbreviation.
pub fn lookup(word: &str) -> Option<&'static Cmd> {
    // Accept the common spelling while preserving Vim's canonical command table.
    let word = if word == "colorschema" {
        "colorscheme"
    } else {
        word
    };
    // Vim's one_letter_cmd(): ":k" may be followed directly by the mark name
    // without whitespace (":ka" == ":k a"), before the command table is
    // consulted (ex_docmd.c:3513-3535, 3799-3802). Restrict to a two-character
    // word so the ":keep*" command modifiers still resolve through the table.
    let mut chars = word.chars();
    if word.starts_with('k') {
        if let (Some('k'), Some(mark), None) = (chars.next(), chars.next(), chars.next()) {
            if mark.is_ascii_alphabetic() {
                let index = command_index("k")?;
                return COMMANDS.get(index);
            }
        }
    }
    let first = word.chars().next()?;
    let start = if first.is_ascii_lowercase() {
        0
    } else if first.is_ascii_uppercase() {
        command_index("Next")?
    } else {
        command_index("!")?
    };
    COMMANDS[start..].iter().find(|c| c.name.starts_with(word))
}

/// Number of commands declared in the baseline table.
pub const fn table_size() -> usize {
    COMMAND_COUNT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_table_has_589_entries() {
        assert_eq!(COMMAND_COUNT, 589);
        assert_eq!(COMMANDS.len(), 589);
    }

    #[test]
    fn table_is_in_curated_abbreviation_order() {
        // Documented invariant: ex_cmds.h is sorted on the shortest working
        // abbreviation, so these relative orders decide what ":w", ":r", ":p"
        // and ":q" mean. Regenerating the table must preserve them.
        let before = |a: &str, b: &str| {
            assert!(
                command_index(a).unwrap() < command_index(b).unwrap(),
                "{a} must precede {b}"
            );
        };
        before("write", "wq");
        before("write", "wall");
        before("print", "previous");
        before("read", "rewind");
        before("quit", "qall");
        before("substitute", "sunhide");
    }

    #[test]
    fn abbreviation_resolution_matches_vim() {
        let cases: &[(&str, &str)] = &[
            ("q", "quit"),
            ("qa", "qall"),
            ("w", "write"),
            ("wq", "wq"),
            ("wa", "wall"),
            ("r", "read"),
            ("p", "print"),
            ("P", "Print"),
            ("N", "Next"),
            ("x", "xit"),
            ("e", "edit"),
            ("s", "substitute"),
            ("d", "delete"),
            ("del", "delete"),
            ("b", "buffer"),
            ("bn", "bnext"),
            ("ls", "ls"),
            ("files", "files"),
            ("buffers", "buffers"),
            ("reg", "registers"),
            ("noh", "nohlsearch"),
            ("u", "undo"),
            ("ve", "version"),
            ("so", "source"),
            ("sort", "sort"),
            ("g", "global"),
            ("v", "vglobal"),
            ("m", "move"),
            ("co", "copy"),
            ("t", "t"),
            ("pu", "put"),
            ("norm", "normal"),
            ("!", "!"),
            ("=", "="),
            ("#", "#"),
            ("&", "&"),
            ("~", "~"),
            ("<", "<"),
            (">", ">"),
            ("@", "@"),
        ];
        for (typed, expected) in cases {
            assert_eq!(
                lookup(typed).map(|c| c.name),
                Some(*expected),
                "lookup({typed:?})"
            );
        }
    }

    #[test]
    fn unknown_words_do_not_resolve() {
        assert!(lookup("zzz").is_none());
        assert!(lookup("qqq").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn flags_are_parsed_from_the_reference_table() {
        // Sample of well-known flag combinations.
        let w = lookup("write").unwrap();
        assert!(w.has(ExFlags::BANG));
        assert!(w.has(ExFlags::DFLALL));
        let d = lookup("delete").unwrap();
        assert!(d.has(ExFlags::RANGE));
        assert!(d.has(ExFlags::REGSTR));
        assert!(d.has(ExFlags::COUNT));
        let s = lookup("substitute").unwrap();
        assert!(s.has(ExFlags::RANGE));
        assert!(s.has(ExFlags::EXTRA));
        // :quit accepts a window range (:1q quits window 1) and no line range.
        let q = lookup("quit").unwrap();
        assert!(q.has(ExFlags::RANGE));
        assert!(q.has(ExFlags::BANG));
        assert_eq!(q.addr, AddrKind::Windows);
    }

    #[test]
    fn address_kinds_are_parsed() {
        assert_eq!(lookup("write").unwrap().addr, AddrKind::Lines);
        assert_eq!(lookup("buffer").unwrap().addr, AddrKind::Buffers);
        assert_eq!(lookup("abbreviate").unwrap().addr, AddrKind::None);
        assert_eq!(lookup("argument").unwrap().addr, AddrKind::Arguments);
    }
}
