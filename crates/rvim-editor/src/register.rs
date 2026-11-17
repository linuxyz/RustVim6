//! Registers (reference: register.c, ops.c).
//!
//! M1 covers the unnamed `"`, yank `0`, numbered `1-9`, small-delete `-`,
//! last-insert `.`, last-ex `:`, search `/`, black-hole `_` and the
//! named `a-z`/`A-Z` registers. The read-only `%` register is computed
//! dynamically by the editor.
//!
//! Vim has no separate unnamed register: `"` is an alias for the register
//! written last (`y_previous` in register.c:29, used by `get_yank_register()`
//! at register.c:223-226). Writing a named register therefore makes it the
//! unnamed one, and an uppercase name appends to the lowercase register and
//! becomes the unnamed register too (register.c:216-263, ops.c:685-731).

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RegType {
    #[default]
    CharWise,
    LineWise,
    BlockWise,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Register {
    pub text: String,
    pub reg_type: RegType,
}

impl Register {
    pub fn linewise(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            reg_type: RegType::LineWise,
        }
    }

    pub fn charwise(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            reg_type: RegType::CharWise,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RegisterStore {
    map: HashMap<char, Register>,
    /// Last explicitly selected register (the `"x` prefix), defaulting to '"'.
    pub last_selected: char,
}

impl RegisterStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a register by name. As in Vim's `get_yank_register()` (used for
    /// reading), an uppercase name refers to the lowercase register
    /// (register.c:216-263), so `"Ap` and `getreg('A')` read `"a`.
    pub fn get(&self, name: char) -> Option<&Register> {
        let slot = name.to_ascii_lowercase();
        self.map.get(&slot)
    }

    /// Raw write used by `:let @x = ...` / `setreg()`: it does *not* change
    /// the unnamed register (`finish_write_reg()` restores `y_previous` for
    /// names other than '"', register.c:2742-2745).
    pub fn put(&mut self, name: char, reg: Register) {
        self.map.insert(name, reg);
    }

    /// Append `reg` to the lowercase register `name` (the `"A` form) and
    /// return the merged register (register.c:1180-1243 op_yank()).
    fn append_to(&mut self, name: char, reg: Register) -> Register {
        let merged = match self.map.get(&name) {
            Some(old) if !old.text.is_empty() => {
                let mut text = old.text.clone();
                text.push_str(&reg.text);
                let reg_type = if reg.reg_type == RegType::LineWise {
                    RegType::LineWise
                } else {
                    old.reg_type
                };
                Register { text, reg_type }
            }
            _ => reg,
        };
        self.map.insert(name, merged.clone());
        merged
    }

    /// Write `reg` into `target` (uppercase names append to the lowercase
    /// register) and make the result the unnamed register.
    fn write_target(&mut self, target: char, reg: Register) -> Register {
        let written = if target.is_ascii_uppercase() {
            self.append_to(target.to_ascii_lowercase(), reg)
        } else {
            self.map.insert(target, reg.clone());
            reg
        };
        self.map.insert('"', written.clone());
        self.last_selected = target;
        written
    }

    /// Write a yank: the named target (or the unnamed register) receives the
    /// text, and the unnamed register follows the write. Only an *unnamed* yank
    /// fills register `0`; yanking into a named register leaves `"0` alone
    /// (ops.c:685-731 op_yank / register.c:2742-2745). The black-hole `_`
    /// discards everything.
    pub fn yank(&mut self, target: Option<char>, reg: Register) {
        let name = target.unwrap_or('"');
        if name == '_' {
            return;
        }
        if name == '"' {
            self.map.insert('0', reg.clone());
        }
        self.write_target(name, reg);
    }

    /// Record a deletion (ops.c:685-731). Line-wise deletes shift `1..9` and
    /// land in register `1`; a small delete without a named target goes to
    /// `-`. The unnamed register always follows the write.
    pub fn delete(&mut self, target: Option<char>, reg: Register) {
        // `""d` is an explicit unnamed delete: it behaves like no target at all
        // (small deletes land in '-', line-wise ones shift 1..9).
        let target = match target {
            Some('"') => None,
            other => other,
        };
        let name = target.unwrap_or('"');
        if name == '_' {
            // Black hole: nothing is recorded and the unnamed register keeps
            // its previous contents.
            return;
        }
        // A named target is written first; the unnamed yank (`y_previous`) is
        // set by `get_yank_register(..., TRUE)`.
        let written = match target {
            None => reg.clone(),
            Some(t) => self.write_target(t, reg.clone()),
        };
        if reg.reg_type == RegType::LineWise {
            // shift_delete_registers(): "9 is dropped, "8 becomes "9, ...,
            // and "1 receives the new text (register.c:982-994).
            for i in (2..=9).rev() {
                if let Some(r) = self.map.get(&char::from_digit(i - 1, 10).unwrap()).cloned() {
                    self.map.insert(char::from_digit(i, 10).unwrap(), r);
                }
            }
            self.map.insert('1', reg.clone());
            // After the shift y_previous points at "1, except while appending
            // with an uppercase register, where it stays the lowercase one.
            let unnamed = if target.is_some() && name.is_ascii_uppercase() {
                written
            } else {
                reg
            };
            self.map.insert('"', unnamed);
        } else if target.is_none() {
            // Small delete without a named register goes to "-" and becomes
            // the unnamed register.
            self.map.insert('-', reg.clone());
            self.map.insert('"', reg);
            self.last_selected = '-';
        } else {
            // Named small delete: the named register is the unnamed one.
            self.map.insert('"', written);
        }
    }

    pub fn set_last_ex(&mut self, cmdline: impl Into<String>) {
        self.map.insert(':', Register::linewise(cmdline));
    }

    pub fn set_last_insert(&mut self, text: impl Into<String>) {
        self.map.insert('.', Register::charwise(text));
    }

    pub fn set_search(&mut self, pattern: impl Into<String>) {
        self.map.insert('/', Register::charwise(pattern));
    }

    pub fn unnamed(&self) -> Option<&Register> {
        self.map.get(&'"')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_registers_shift_on_line_delete() {
        let mut s = RegisterStore::new();
        s.delete(None, Register::linewise("one"));
        s.delete(None, Register::linewise("two"));
        assert_eq!(s.get('1').unwrap().text, "two");
        assert_eq!(s.get('2').unwrap().text, "one");
        assert_eq!(s.unnamed().unwrap().text, "two");
    }

    #[test]
    fn small_delete_uses_dash_and_yank_uses_zero() {
        let mut s = RegisterStore::new();
        s.delete(None, Register::charwise("x"));
        assert_eq!(s.get('-').unwrap().text, "x");
        assert_eq!(s.unnamed().unwrap().text, "x");
        s.yank(None, Register::linewise("yy"));
        assert_eq!(s.get('0').unwrap().text, "yy");
        assert_eq!(s.unnamed().unwrap().text, "yy");
    }

    #[test]
    fn black_hole_discards() {
        let mut s = RegisterStore::new();
        s.yank(Some('_'), Register::charwise("gone"));
        assert!(s.get('_').is_none());
        assert!(s.unnamed().is_none());
        assert!(s.get('0').is_none());

        s.yank(None, Register::charwise("keep"));
        s.delete(Some('_'), Register::charwise("gone"));
        assert_eq!(s.unnamed().unwrap().text, "keep");
        assert!(s.get('_').is_none());
    }

    #[test]
    fn named_yank_sets_unnamed_but_leaves_register_zero() {
        let mut s = RegisterStore::new();
        s.yank(Some('a'), Register::linewise("one\n"));
        assert_eq!(s.get('a').unwrap().text, "one\n");
        assert_eq!(s.unnamed().unwrap().text, "one\n");
        // Only an unnamed yank fills "0 (register.c get_yank_register(0,TRUE)).
        assert!(s.get('0').is_none(), "a named yank leaves \"0 alone");
    }

    #[test]
    fn uppercase_yank_appends_and_becomes_unnamed() {
        let mut s = RegisterStore::new();
        s.yank(Some('a'), Register::linewise("one\n"));
        s.yank(Some('A'), Register::linewise("two\n"));
        assert_eq!(s.get('a').unwrap().text, "one\ntwo\n");
        assert_eq!(s.unnamed().unwrap().text, "one\ntwo\n");
        // Reading an uppercase name resolves to the lowercase register
        // (getreg('A') / `"Ap` in vim).
        assert_eq!(s.get('A').unwrap().text, "one\ntwo\n");
    }

    #[test]
    fn append_to_the_unnamed_register() {
        let mut s = RegisterStore::new();
        s.yank(Some('"'), Register::charwise("ab"));
        s.yank(Some('"'), Register::charwise("cd"));
        assert_eq!(s.get('0').unwrap().text, "cd");
        assert_eq!(s.unnamed().unwrap().text, "cd");
        // Explicitly appending to "" appends to register 0's target too.
        s.yank(Some('A'), Register::charwise("X"));
        s.yank(Some('"'), Register::charwise("Y"));
        assert_eq!(s.unnamed().unwrap().text, "Y");
    }

    #[test]
    fn named_linewise_delete_updates_unnamed_with_register_one() {
        let mut s = RegisterStore::new();
        s.delete(Some('a'), Register::linewise("gone\n"));
        assert_eq!(s.get('a').unwrap().text, "gone\n");
        assert_eq!(s.get('1').unwrap().text, "gone\n");
        assert_eq!(s.unnamed().unwrap().text, "gone\n");
    }
}
