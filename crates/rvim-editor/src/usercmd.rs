//! User-defined commands (reference: `:command` in ex_docmd.c, usercmd.c level).
//!
//! M2 stores and validates the declaration, resolves the command through the
//! same lookup path as builtins, and expands the `<args>` style placeholders
//! that do not need expression evaluation. `<expr>`/`<mods>` need M3's
//! evaluator and are rejected explicitly.

use rvim_core::{VimError, VimResult};

/// `-nargs=` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Nargs {
    /// `-nargs=0` (default): no arguments allowed.
    #[default]
    Zero,
    /// `-nargs=1`
    One,
    /// `-nargs=*`
    Star,
    /// `-nargs=?`
    Question,
    /// `-nargs=+`
    Plus,
}

impl Nargs {
    pub fn parse(text: &str) -> VimResult<Self> {
        Ok(match text {
            "0" => Self::Zero,
            "1" => Self::One,
            "*" => Self::Star,
            "?" => Self::Question,
            "+" => Self::Plus,
            other => {
                return Err(VimError::new(
                    "E176",
                    format!("Invalid argument for -nargs: {other}"),
                ));
            }
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "0",
            Self::One => "1",
            Self::Star => "*",
            Self::Question => "?",
            Self::Plus => "+",
        }
    }

    /// Validate an invocation's argument text against this setting.
    pub fn check(self, args: &str) -> VimResult<()> {
        let n = args.split_whitespace().count();
        let ok = match self {
            Self::Zero => n == 0,
            Self::One => n == 1,
            Self::Star => true,
            Self::Question => n <= 1,
            Self::Plus => n >= 1,
        };
        if ok {
            Ok(())
        } else {
            Err(VimError::new(
                "E488",
                format!("Trailing characters: {args}"),
            ))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserCommand {
    pub name: String,
    /// `-bang`
    pub bang: bool,
    /// `-bar`
    pub bar: bool,
    /// `-register`
    pub register: bool,
    /// `-range` / `-range=%` (max 1) / `-range=N`
    pub range: bool,
    /// `-count=N`
    pub count: Option<u32>,
    pub nargs: Nargs,
    /// `-complete=` value, kept verbatim.
    pub complete: String,
    /// `-expr`: the replacement text is a VimL expression whose result is
    /// executed as an Ex command when the user command runs.
    pub expr: bool,
    /// Replacement text; placeholders such as `<args>` are expanded on invoke.
    pub command: String,
}

impl UserCommand {
    /// `:command` listing line, e.g.
    /// `    Name   -nargs=*  ...`
    pub fn listing(&self) -> String {
        let mut flags = String::new();
        if self.bang {
            flags.push_str(" -bang");
        }
        if self.bar {
            flags.push_str(" -bar");
        }
        if self.register {
            flags.push_str(" -register");
        }
        if self.range {
            flags.push_str(" -range");
        }
        if let Some(c) = self.count {
            flags.push_str(&format!(" -count={c}"));
        }
        flags.push_str(&format!(" -nargs={}", self.nargs.as_str()));
        if !self.complete.is_empty() {
            flags.push_str(&format!(" -complete={}", self.complete));
        }
        if self.expr {
            flags.push_str(" -expr");
        }
        format!("    {}  {}\t{}", self.name, flags.trim(), self.command)
    }
}

#[derive(Debug, Default)]
pub struct UserCommandStore {
    entries: Vec<UserCommand>,
}

impl UserCommandStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[UserCommand] {
        &self.entries
    }

    /// Define (or redefine, with `:command!`) a user command.
    pub fn define(&mut self, cmd: UserCommand, force: bool) -> VimResult<()> {
        if !is_valid_name(&cmd.name) {
            return Err(VimError::new(
                "E182",
                format!("E182: Invalid command name: {}", cmd.name),
            ));
        }
        let existing = self.entries.iter().position(|e| e.name == cmd.name);
        match (existing, force) {
            (Some(_), false) => Err(VimError::new(
                "E174",
                format!("Command already exists: add ! to replace it: {}", cmd.name),
            )),
            (Some(idx), true) => {
                self.entries[idx] = cmd;
                Ok(())
            }
            (None, _) => {
                self.entries.push(cmd);
                Ok(())
            }
        }
    }

    pub fn delete(&mut self, name: &str) -> VimResult<()> {
        let before = self.entries.len();
        self.entries.retain(|e| e.name != name);
        if self.entries.len() == before {
            return Err(VimError::new("E184", format!("No such command: {name}")));
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&UserCommand> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Commands whose name starts with `prefix` (for completion and listing).
    pub fn with_prefix(&self, prefix: &str) -> Vec<&UserCommand> {
        self.entries
            .iter()
            .filter(|e| e.name.starts_with(prefix))
            .collect()
    }
}

/// User command names start with an upper-case letter and continue with
/// alphanumerics (so they cannot shadow builtin Ex commands).
fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// Quote argument text the way vim's `<q-args>` does: one double-quoted
/// string, with embedded quotes doubled.
fn quote_args(args: &str) -> String {
    format!("\"{}\"", args.replace('"', "\"\""))
}

/// Split argument text on unquoted whitespace, each word double-quoted, as
/// vim's `<f-args>` does.
fn quote_each_arg(args: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in args.chars() {
        match quote {
            Some(q) => {
                current.push(ch);
                if ch == q {
                    quote = None;
                }
            }
            None if ch == '"' || ch == '\'' => {
                current.push(ch);
                quote = Some(ch);
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
        .into_iter()
        .map(|w| quote_args(&w))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Expand the placeholders a user command may use in its replacement text.
///
/// Supported (no evaluator needed): `<args>`, `<q-args>`, `<f-args>`, `<bang>`,
/// `<count>`, `<line1>`, `<line2>`, `<range>`, `<lt>` and `<mods>`.
/// `<expr>`-style placeholders are rejected explicitly because they need the
/// M3 evaluator.
pub fn expand_placeholders(
    cmd: &UserCommand,
    args: &str,
    bang: bool,
    range: Option<(usize, usize)>,
) -> VimResult<String> {
    let mut out = cmd.command.clone();
    let replacements = [
        ("<q-args>", quote_args(args)),
        ("<f-args>", quote_each_arg(args)),
        ("<args>", args.to_string()),
        ("<bang>", if bang { "!".to_string() } else { String::new() }),
        (
            "<count>",
            cmd.count
                .map(|c| c.to_string())
                .unwrap_or_else(|| "0".to_string()),
        ),
        (
            "<line1>",
            range
                .map(|(s, _)| (s + 1).to_string())
                .unwrap_or_else(|| "0".to_string()),
        ),
        (
            "<line2>",
            range
                .map(|(_, e)| e.to_string())
                .unwrap_or_else(|| "0".to_string()),
        ),
        (
            "<range>",
            range
                .map(|(s, e)| format!("{},{}", s + 1, e))
                .unwrap_or_default(),
        ),
        ("<lt>", "<".to_string()),
        ("<mods>", String::new()),
    ];
    for (needle, value) in replacements {
        if out.contains(needle) {
            out = out.replace(needle, &value);
        }
    }
    if out.contains("<expr>") || out.contains("<q-bang>") {
        return Err(VimError::new(
            "E15",
            "user command placeholder needs the VimL evaluator (M3)".to_string(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, nargs: Nargs) -> UserCommand {
        UserCommand {
            name: name.to_string(),
            bang: false,
            bar: false,
            register: false,
            range: false,
            count: None,
            nargs,
            complete: String::new(),
            expr: false,
            command: "set number".to_string(),
        }
    }

    #[test]
    fn names_must_be_capitalised_and_alphanumeric() {
        let mut store = UserCommandStore::new();
        assert!(store.define(sample("MyCmd", Nargs::Zero), false).is_ok());
        assert_eq!(
            store
                .define(sample("lower", Nargs::Zero), false)
                .unwrap_err()
                .code,
            "E182"
        );
        assert_eq!(
            store
                .define(sample("My_Cmd", Nargs::Zero), false)
                .unwrap_err()
                .code,
            "E182"
        );
    }

    #[test]
    fn redefine_needs_bang() {
        let mut store = UserCommandStore::new();
        store.define(sample("Foo", Nargs::Zero), false).unwrap();
        assert_eq!(
            store
                .define(sample("Foo", Nargs::Zero), false)
                .unwrap_err()
                .code,
            "E174"
        );
        assert!(store.define(sample("Foo", Nargs::Zero), true).is_ok());
        assert_eq!(store.entries().len(), 1);
    }

    #[test]
    fn nargs_validation() {
        assert!(Nargs::Zero.check("").is_ok());
        assert!(Nargs::Zero.check("x").is_err());
        assert!(Nargs::One.check("x").is_ok());
        assert!(Nargs::One.check("x y").is_err());
        assert!(Nargs::Plus.check("x").is_ok());
        assert!(Nargs::Plus.check("").is_err());
        assert!(Nargs::Question.check("").is_ok());
        assert!(Nargs::Question.check("x y").is_err());
        assert!(Nargs::Star.check("x y z").is_ok());
        assert_eq!(
            Nargs::parse("!"),
            Err(VimError::new("E176", "Invalid argument for -nargs: !"))
        );
    }

    #[test]
    fn placeholder_expansion() {
        let mut cmd = sample("Foo", Nargs::Star);
        cmd.command = "echo '<args>' <bang> <line1>,<line2>".to_string();
        let out = expand_placeholders(&cmd, "a b", true, Some((0, 2))).unwrap();
        assert_eq!(out, "echo 'a b' ! 1,2");
    }

    #[test]
    fn q_args_are_quoted_and_f_args_are_split() {
        // `<q-args>` is the argument text as one quoted string (vim doubles
        // embedded quotes); `<f-args>` splits on whitespace and quotes each word
        // so it can be passed to a function call.
        let mut cmd = sample("Foo", Nargs::Star);
        cmd.command = "echo <q-args>".to_string();
        assert_eq!(
            expand_placeholders(&cmd, "hello world", false, None).unwrap(),
            "echo \"hello world\""
        );
        assert_eq!(
            expand_placeholders(&cmd, "say \"hi\"", false, None).unwrap(),
            "echo \"say \"\"hi\"\"\""
        );

        let mut cmd = sample("Foo", Nargs::Star);
        cmd.command = "call Bar(<f-args>)".to_string();
        assert_eq!(
            expand_placeholders(&cmd, "a b", false, None).unwrap(),
            "call Bar(\"a\", \"b\")"
        );
    }

    #[test]
    fn unsupported_placeholders_are_explicit_errors() {
        let mut cmd = sample("Foo", Nargs::Star);
        cmd.command = "echo <expr>".to_string();
        assert_eq!(
            expand_placeholders(&cmd, "", false, None).unwrap_err().code,
            "E15"
        );
    }

    #[test]
    fn delete_and_prefix_lookup() {
        let mut store = UserCommandStore::new();
        store.define(sample("Foo", Nargs::Zero), false).unwrap();
        store.define(sample("FooBar", Nargs::Zero), false).unwrap();
        assert_eq!(store.with_prefix("Foo").len(), 2);
        store.delete("Foo").unwrap();
        assert_eq!(store.with_prefix("Foo").len(), 1);
        assert_eq!(store.delete("Nope").unwrap_err().code, "E184");
    }
}
