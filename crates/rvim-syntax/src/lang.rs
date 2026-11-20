//! Built-in syntax definitions for the filetypes RustVim knows.
//!
//! vim reads these from `$VIMRUNTIME/syntax/<ft>.vim`; RustVim ships a
//! compact standard set so highlighting works without the runtime present.
//! Each definition is plain `syntax keyword`/`match`/`region` data, so a user
//! can override any of it with `:syntax` commands.

use crate::{Item, ItemKind, SyntaxDef};

/// Builder that assigns the ids `synID()` reports.
struct DefBuilder {
    def: SyntaxDef,
    next_id: i64,
}

impl DefBuilder {
    fn new(name: &str) -> Self {
        Self {
            def: SyntaxDef {
                name: name.to_string(),
                ..SyntaxDef::default()
            },
            next_id: 1,
        }
    }

    fn push(&mut self, group: &str, kind: ItemKind) {
        let id = self.next_id;
        self.next_id += 1;
        self.def.items.push(Item {
            id,
            group: group.to_string(),
            kind,
        });
    }

    fn keyword(&mut self, group: &str, words: &[&str]) {
        self.push(
            group,
            ItemKind::Keyword {
                words: words.iter().map(|w| w.to_string()).collect(),
            },
        );
    }

    fn matches(&mut self, group: &str, pattern: &str) {
        self.push(
            group,
            ItemKind::Match {
                pattern: pattern.to_string(),
                contains: Vec::new(),
            },
        );
    }

    fn region(&mut self, group: &str, start: &str, end: &str) {
        self.push(
            group,
            ItemKind::Region {
                start: start.to_string(),
                end: end.to_string(),
                contains: Vec::new(),
            },
        );
    }

    fn ignore_case(&mut self) {
        self.def.ignore_case = true;
    }

    fn build(self) -> SyntaxDef {
        self.def
    }
}

/// Every syntax name [builtin] accepts.
pub const NAMES: &[&str] = &[
    "c",
    "cpp",
    "css",
    "go",
    "html",
    "java",
    "javascript",
    "json",
    "lua",
    "make",
    "markdown",
    "python",
    "ruby",
    "rust",
    "sh",
    "sql",
    "toml",
    "typescript",
    "vim",
    "yaml",
];

/// The syntax for a filetype name, or `None` when there is no built-in.
pub fn builtin(name: &str) -> Option<SyntaxDef> {
    let mut b = DefBuilder::new(name);
    match name {
        "rust" => {
            b.keyword(
                "Keyword",
                &[
                    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                    "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop",
                    "match", "mod", "move", "mut", "pub", "ref", "return", "self", "static",
                    "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
                ],
            );
            b.keyword(
                "Type",
                &[
                    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize",
                    "str", "u8", "u16", "u32", "u64", "u128", "usize", "String", "Vec", "Option",
                    "Result", "Box", "Self",
                ],
            );
            common_c(&mut b);
            b.matches("PreProc", r"#\[[a-zA-Z_][^\]]*\]");
            b.matches("PreProc", r"[a-zA-Z_][a-zA-Z0-9_]*!");
        }
        "c" | "cpp" | "java" => {
            b.keyword(
                "Keyword",
                &[
                    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum",
                    "extern", "for", "goto", "if", "inline", "register", "return", "sizeof",
                    "static", "struct", "switch", "typedef", "union", "volatile", "while",
                ],
            );
            b.keyword(
                "Type",
                &[
                    "bool", "char", "double", "float", "int", "long", "short", "signed",
                    "unsigned", "void", "size_t",
                ],
            );
            if name != "c" {
                b.keyword(
                    "Keyword",
                    &[
                        "class",
                        "namespace",
                        "template",
                        "public",
                        "private",
                        "protected",
                        "virtual",
                        "override",
                        "new",
                        "delete",
                        "using",
                        "try",
                        "catch",
                        "throw",
                        "constexpr",
                        "noexcept",
                        "nullptr",
                        "interface",
                        "extends",
                        "implements",
                        "package",
                        "import",
                        "final",
                        "this",
                        "super",
                        "abstract",
                    ],
                );
                b.keyword("Type", &["String", "Object", "Integer", "Boolean"]);
            }
            common_c(&mut b);
            b.matches("PreProc", r"^[ \t]*#[ \t]*[a-zA-Z_]*");
        }
        "python" => {
            b.keyword(
                "Keyword",
                &[
                    "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
                    "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
                    "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
                    "return", "try", "while", "with", "yield",
                ],
            );
            b.keyword("Constant", &["None", "True", "False", "self"]);
            b.region("String", "\"\"\"", "\"\"\"");
            b.region("String", "'''", "'''");
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
            b.matches("Comment", "#.*");
            b.matches("Number", r"\d\+");
            b.matches("Number", r"0[xX][0-9a-fA-F]\+");
        }
        "vim" => {
            b.keyword(
                "Keyword",
                &[
                    "let",
                    "unlet",
                    "const",
                    "if",
                    "elseif",
                    "else",
                    "endif",
                    "while",
                    "endwhile",
                    "for",
                    "endfor",
                    "function",
                    "endfunction",
                    "return",
                    "call",
                    "execute",
                    "normal",
                    "source",
                    "runtime",
                    "map",
                    "nmap",
                    "nnoremap",
                    "inoremap",
                    "vnoremap",
                    "command",
                    "augroup",
                    "autocmd",
                    "au",
                    "syntax",
                    "highlight",
                    "echo",
                    "echom",
                    "echon",
                    "try",
                    "catch",
                    "finally",
                    "endtry",
                    "throw",
                    "set",
                    "setlocal",
                    "setglobal",
                    "silent",
                    "delfunction",
                ],
            );
            b.matches("Comment", "\".*");
            b.region("String", "'", "'");
            b.matches("Special", r"&[a-zA-Z]+");
            b.matches("Number", r"\d\+");
        }
        "javascript" | "typescript" => {
            b.keyword(
                "Keyword",
                &[
                    "async",
                    "await",
                    "break",
                    "case",
                    "catch",
                    "class",
                    "const",
                    "continue",
                    "default",
                    "delete",
                    "do",
                    "else",
                    "export",
                    "extends",
                    "finally",
                    "for",
                    "from",
                    "function",
                    "if",
                    "import",
                    "in",
                    "instanceof",
                    "let",
                    "new",
                    "of",
                    "return",
                    "super",
                    "switch",
                    "this",
                    "throw",
                    "try",
                    "typeof",
                    "var",
                    "void",
                    "while",
                    "yield",
                ],
            );
            b.keyword("Constant", &["true", "false", "null", "undefined", "NaN"]);
            if name == "typescript" {
                b.keyword(
                    "Keyword",
                    &[
                        "interface",
                        "type",
                        "enum",
                        "implements",
                        "private",
                        "protected",
                        "public",
                        "readonly",
                        "namespace",
                        "declare",
                        "abstract",
                    ],
                );
                b.keyword(
                    "Type",
                    &[
                        "string", "number", "boolean", "any", "unknown", "never", "void",
                    ],
                );
            }
            b.matches("Comment", "//.*");
            b.region("Comment", r"/\*", r"\*/");
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
            b.region("String", "`", "`");
            b.matches("Number", r"\d\+");
        }
        "json" => {
            b.keyword("Constant", &["true", "false", "null"]);
            b.region("String", "\"", "\"");
            b.matches("Number", r"-\?\d\+");
        }
        "yaml" => {
            b.keyword(
                "Constant",
                &["true", "false", "null", "yes", "no", "on", "off"],
            );
            b.matches("Comment", "#.*");
            b.matches("Identifier", r"^[ \t-]*[A-Za-z0-9_.-]\+:");
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
            b.matches("Number", r"\d\+");
        }
        "toml" => {
            b.matches("Comment", "#.*");
            b.matches("Title", r"^[ \t]*\[[^\]]*\]");
            b.keyword("Constant", &["true", "false"]);
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
            b.matches("Number", r"\d\+");
        }
        "html" => {
            b.region("Comment", "<!--", "-->");
            b.matches("Statement", r"<[/!]\?[a-zA-Z][a-zA-Z0-9-]*");
            b.matches("Statement", r">");
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
        }
        "css" => {
            b.region("Comment", r"/\*", r"\*/");
            b.matches("Identifier", r"[a-z-]\+[ \t]*:");
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
            b.matches("Number", r"\d\+");
            b.matches("Statement", r"[.#][a-zA-Z][a-zA-Z0-9_-]*");
        }
        "sh" => {
            b.keyword(
                "Keyword",
                &[
                    "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while",
                    "until", "case", "esac", "function", "return", "local", "export", "echo",
                    "read", "set", "exit", "shift", "source",
                ],
            );
            b.matches("Comment", "#.*");
            b.matches("Identifier", r"\$\w\+");
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
            b.matches("Number", r"\d\+");
        }
        "make" => {
            b.matches("Comment", "#.*");
            b.matches("Function", r"^[a-zA-Z0-9_.-]\+:");
            b.matches("Identifier", r"\$\(\w\+");
            b.matches("Identifier", r"\$\w\+");
            b.keyword(
                "Keyword",
                &[
                    "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "include", "export",
                ],
            );
        }
        "markdown" => {
            b.matches("Title", r"^#\+ .*");
            b.matches("Comment", r"^>.*");
            b.matches("String", "`[^`]*`");
            b.matches("Statement", r"\*\*[^\*]\*\*");
            b.matches("Identifier", r"\[[^\]]*\]\([^)]*\)");
        }
        "lua" => {
            b.keyword(
                "Keyword",
                &[
                    "and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if",
                    "in", "local", "not", "or", "repeat", "return", "then", "until", "while",
                ],
            );
            b.keyword("Constant", &["true", "false", "nil"]);
            b.matches("Comment", "--.*");
            b.region("Comment", r"--\[\[", r"]]");
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
            b.matches("Number", r"\d\+");
        }
        "go" => {
            b.keyword(
                "Keyword",
                &[
                    "break",
                    "case",
                    "chan",
                    "const",
                    "continue",
                    "default",
                    "defer",
                    "else",
                    "fallthrough",
                    "for",
                    "func",
                    "go",
                    "goto",
                    "if",
                    "import",
                    "interface",
                    "map",
                    "package",
                    "range",
                    "return",
                    "select",
                    "struct",
                    "switch",
                    "type",
                    "var",
                ],
            );
            b.keyword("Constant", &["nil", "true", "false", "iota"]);
            b.keyword(
                "Type",
                &[
                    "bool", "byte", "error", "float32", "float64", "int", "int64", "rune", "string",
                ],
            );
            b.matches("Comment", "//.*");
            b.region("Comment", r"/\*", r"\*/");
            b.region("String", "\"", "\"");
            b.region("String", "`", "`");
            b.matches("Number", r"\d\+");
        }
        "ruby" => {
            b.keyword(
                "Keyword",
                &[
                    "begin", "break", "case", "class", "def", "do", "else", "elsif", "end",
                    "ensure", "for", "if", "in", "module", "next", "rescue", "return", "self",
                    "then", "unless", "until", "when", "while", "yield",
                ],
            );
            b.keyword("Constant", &["nil", "true", "false"]);
            b.matches("Comment", "#.*");
            b.region("String", "\"", "\"");
            b.region("String", "'", "'");
            b.matches("Constant", r":\w\+");
            b.matches("Number", r"\d\+");
        }
        "sql" => {
            b.ignore_case();
            b.keyword(
                "Keyword",
                &[
                    "select", "from", "where", "insert", "into", "values", "update", "set",
                    "delete", "create", "table", "drop", "alter", "join", "left", "right", "inner",
                    "outer", "on", "group", "by", "order", "having", "limit", "offset", "union",
                    "all", "distinct", "as", "and", "or", "not", "null", "is", "in", "exists",
                    "case", "when", "then", "else", "end",
                ],
            );
            b.matches("Comment", "--.*");
            b.region("Comment", r"/\*", r"\*/");
            b.region("String", "'", "'");
            b.matches("Number", r"\d\+");
        }
        _ => return None,
    }
    Some(b.build())
}

/// `//` + `/* */` comments, `"..."`/`'...'` strings and decimal/hex numbers.
fn common_c(b: &mut DefBuilder) {
    b.matches("Comment", "//.*");
    b.region("Comment", r"/\*", r"\*/");
    b.region("String", "\"", "\"");
    b.region("String", "'", "'");
    b.matches("Number", r"\d\+");
    b.matches("Number", r"0[xX][0-9a-fA-F]\+");
}

/// Guess a syntax name from a file name (`filetype.vim` detection subset).
pub fn from_filename(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    if lower == "makefile" || lower == "gnumakefile" {
        return Some("make");
    }
    let ext = lower.rsplit_once('.')?.1;
    Some(match ext {
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "css" => "css",
        "go" => "go",
        "htm" | "html" => "html",
        "java" => "java",
        "js" | "mjs" | "cjs" => "javascript",
        "json" => "json",
        "lua" => "lua",
        "mk" => "make",
        "md" | "markdown" => "markdown",
        "py" | "pyw" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "sh" | "bash" | "zsh" => "sh",
        "sql" => "sql",
        "toml" => "toml",
        "ts" | "tsx" => "typescript",
        "vim" => "vim",
        "yaml" | "yml" => "yaml",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, State};

    fn groups(engine: &Engine, text: &str) -> Vec<(String, String)> {
        let (spans, _) = engine.line(&State::default(), text);
        spans
            .iter()
            .map(|s| (text[s.start..s.end].to_string(), s.group.clone()))
            .collect()
    }

    #[test]
    fn every_advertised_language_exists() {
        for name in NAMES {
            let def = builtin(name).unwrap_or_else(|| panic!("{name} has no syntax"));
            assert!(!def.items.is_empty(), "{name} has no items");
            assert_eq!(def.name, *name);
        }
    }

    #[test]
    fn twenty_filetypes_highlight_their_signature_constructs() {
        // (syntax name, sample line, [(text, group)])
        type Case = (
            &'static str,
            &'static str,
            &'static [(&'static str, &'static str)],
        );
        let cases: &[Case] = &[
            (
                "rust",
                "fn main() { let x = 1; }",
                &[("fn", "Keyword"), ("1", "Number")],
            ),
            (
                "c",
                "int main(void) { return 0; }",
                &[("int", "Type"), ("return", "Keyword")],
            ),
            (
                "cpp",
                "class A { public: int x; };",
                &[("class", "Keyword"), ("int", "Type")],
            ),
            (
                "python",
                "def f():\n    return None",
                &[("def", "Keyword"), ("None", "Constant")],
            ),
            ("vim", "let g:x = 1", &[("let", "Keyword")]),
            (
                "javascript",
                "const x = true;",
                &[("const", "Keyword"), ("true", "Constant")],
            ),
            (
                "typescript",
                "interface A { x: string }",
                &[("interface", "Keyword")],
            ),
            ("json", "{\"a\": true}", &[("true", "Constant")]),
            ("yaml", "key: value", &[("value", "Normal")]),
            ("toml", "[table]", &[("[table]", "Title")]),
            ("html", "<div>hi</div>", &[("<div", "Statement")]),
            ("css", "a { color: red; }", &[("color:", "Identifier")]),
            ("sh", "if true; then echo x; fi", &[("if", "Keyword")]),
            ("make", "all: build", &[("all:", "Function")]),
            ("markdown", "# Title", &[("# Title", "Title")]),
            ("lua", "local x = nil", &[("local", "Keyword")]),
            ("java", "public class A {}", &[("public", "Keyword")]),
            ("go", "func main() {}", &[("func", "Keyword")]),
            ("ruby", "def f; end", &[("def", "Keyword")]),
            ("sql", "SELECT * FROM t", &[("SELECT", "Keyword")]),
        ];
        assert_eq!(cases.len(), 20);
        for (name, text, expected) in cases {
            let engine = Engine::new(builtin(name).unwrap());
            let found = groups(&engine, text);
            for (needle, group) in *expected {
                if *group == "Normal" {
                    // "value" is only checked to be unhighlighted.
                    assert!(
                        !found.iter().any(|(t, _)| t == needle),
                        "{name}: {needle} should not be highlighted: {found:?}"
                    );
                    continue;
                }
                assert!(
                    found.iter().any(|(t, g)| t == needle && g == group),
                    "{name}: expected {needle} as {group}, got {found:?}"
                );
            }
        }
    }

    #[test]
    fn filenames_map_to_syntax_names() {
        assert_eq!(from_filename("/tmp/a.rs"), Some("rust"));
        assert_eq!(from_filename("C:\\src\\main.cpp"), Some("cpp"));
        assert_eq!(from_filename("Makefile"), Some("make"));
        assert_eq!(from_filename("notes.md"), Some("markdown"));
        assert_eq!(from_filename("x.unknown"), None);
    }
}
