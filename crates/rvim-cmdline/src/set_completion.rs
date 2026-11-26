//! Completion of :set option names and values, independent of the frontend.

use crate::{CompletionKind, completion_kind};
use rvim_editor::{Editor, OptValue, Options};

#[derive(Debug, PartialEq, Eq)]
pub struct SetCompletion {
    /// Byte offset at which to replace the token before the command-line cursor.
    pub start: usize,
    pub matches: Vec<String>,
}

/// Complete the text before the cursor. Candidates replace only the active
/// option token; previous settings and text after the cursor stay untouched.
pub fn complete_set(line: &str, editor: &Editor, max: usize) -> Option<SetCompletion> {
    if completion_kind(line) != CompletionKind::Option {
        return None;
    }
    let start = line
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    let token = &line[start..];
    let mut matches: Vec<String> = if let Some(equal) = token.find('=') {
        let operator_start = if token[..equal].ends_with(['+', '-', '^']) {
            equal - 1
        } else {
            equal
        };
        let Ok(name) = Options::canonical(&token[..operator_start]) else {
            return Some(SetCompletion {
                start,
                matches: Vec::new(),
            });
        };
        let operator = &token[operator_start..=equal];
        let command = line.trim_start_matches(':').split_whitespace().next()?;
        let global = rvim_ex::table::lookup(command).is_some_and(|cmd| cmd.name == "setglobal");
        let current = if global {
            editor.options.get(&name).ok()?.clone()
        } else {
            editor.wopt(&name).ok()?
        };
        value_candidates(&name, operator, &token[equal + 1..], &current)
            .into_iter()
            .map(|value| format!("{}{value}", &token[..=equal]))
            .collect()
    } else {
        let (prefix, name) = if let Some(name) = token.strip_prefix("inv") {
            ("inv", name)
        } else if let Some(name) = token.strip_prefix("no") {
            ("no", name)
        } else {
            ("", token)
        };
        let suffix_at = name.find(['?', '!', '&', '<']).unwrap_or(name.len());
        let (name, suffix) = name.split_at(suffix_at);
        Options::completion_names(name, !prefix.is_empty() || suffix == "!")
            .into_iter()
            .map(|name| format!("{prefix}{name}{suffix}"))
            .collect()
    };
    matches.sort();
    matches.dedup();
    matches.truncate(max);
    Some(SetCompletion { start, matches })
}

/// Finite values and comma-separated sets supported by the option registry.
fn choices(name: &str) -> (&'static [&'static str], bool) {
    match name {
        "fileformat" => (&["unix", "dos", "mac"], false),
        "background" => (&["dark", "light"], false),
        "encoding" | "fileencoding" => (
            &[
                "utf-8", "latin1", "utf-16", "utf-16le", "utf-16be", "ucs-2", "ucs-2le", "ucs-2be",
                "ucs-4", "ucs-4le", "ucs-4be", "cp1252",
            ],
            false,
        ),
        "cryptmethod" => (&["zip", "blowfish", "blowfish2"], false),
        "foldmethod" => (
            &["manual", "indent", "expr", "marker", "syntax", "diff"],
            false,
        ),
        "foldcolumn" => (
            &[
                "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "auto",
            ],
            false,
        ),
        "signcolumn" => (
            &[
                "auto", "yes", "no", "number", "auto:1", "auto:2", "auto:3", "auto:4", "auto:5",
                "auto:6", "auto:7", "auto:8", "auto:9",
            ],
            false,
        ),
        "selection" => (&["old", "inclusive", "exclusive"], false),
        "buftype" => (
            &[
                "nofile", "nowrite", "acwrite", "quickfix", "help", "terminal", "prompt", "popup",
            ],
            false,
        ),
        "backspace" => (&["indent", "eol", "start"], true),
        "display" => (&["lastline", "truncate", "uhex", "msgsep"], true),
        "nrformats" => (&["bin", "octal", "hex", "alpha", "unsigned"], true),
        "virtualedit" => (&["block", "insert", "all", "onemore", "none"], true),
        "clipboard" => (
            &[
                "unnamed",
                "unnamedplus",
                "autoselect",
                "autoselectml",
                "autoselectplus",
                "exclude:",
            ],
            true,
        ),
        "diffopt" => (
            &[
                "filler",
                "context:",
                "iblank",
                "icase",
                "iwhite",
                "iwhiteall",
                "iwhiteeol",
                "horizontal",
                "vertical",
                "closeoff",
                "hiddenoff",
                "followwrap",
                "internal",
                "indent-heuristic",
                "algorithm:myers",
                "algorithm:minimal",
                "algorithm:patience",
                "algorithm:histogram",
                "foldcolumn:",
            ],
            true,
        ),
        "sessionoptions" => (
            &[
                "blank",
                "buffers",
                "curdir",
                "folds",
                "globals",
                "help",
                "localoptions",
                "options",
                "resize",
                "sesdir",
                "skiprtp",
                "slash",
                "tabpages",
                "terminal",
                "unix",
                "winpos",
                "winsize",
            ],
            true,
        ),
        "viewoptions" => (
            &[
                "cursor",
                "curdir",
                "folds",
                "options",
                "localoptions",
                "slash",
                "unix",
            ],
            true,
        ),
        "spelloptions" => (&["camel", "noplainbuffer"], true),
        "spellsuggest" => (&["best", "fast", "double", "file:"], true),
        _ => (&[], false),
    }
}

fn value_candidates(name: &str, operator: &str, value: &str, current: &OptValue) -> Vec<String> {
    // Never reveal the encryption key through the completion menu.
    if name == "key" || matches!(current, OptValue::Bool(_)) {
        return Vec::new();
    }
    if name == "guioptions" {
        let flags = rvim_editor::options::GUIOPTION_FLAGS;
        if !value.chars().all(|ch| flags.contains(ch)) {
            return Vec::new();
        }
        let current = current.as_str().unwrap_or("");
        if operator == "-=" {
            return current
                .chars()
                .map(|ch| ch.to_string())
                .filter(|flag| flag.starts_with(value))
                .collect();
        }
        if operator != "=" && !value.is_empty() {
            return if value.chars().all(|ch| !current.contains(ch)) {
                vec![value.to_string()]
            } else {
                Vec::new()
            };
        }
        return flags
            .chars()
            .filter(|ch| !value.contains(*ch))
            .filter(|ch| operator == "=" || !current.contains(*ch))
            .map(|ch| format!("{value}{ch}"))
            .collect();
    }
    let flags = match name {
        "mouse" => Some("anvichr"),
        "formatoptions" => Some("tcro/q2vlb1mMBn,aw]jp"),
        _ => None,
    };
    if let Some(flags) = flags {
        if operator != "=" || !value.chars().all(|ch| flags.contains(ch)) {
            return Vec::new();
        }
        return flags
            .chars()
            .filter(|ch| !value.contains(*ch))
            .map(|ch| format!("{value}{ch}"))
            .collect();
    }
    let (known, list) = choices(name);
    if operator != "=" && !list {
        return Vec::new();
    }
    let (head, prefix) = if list {
        value
            .rsplit_once(',')
            .map(|(head, tail)| (&value[..head.len() + 1], tail))
            .unwrap_or(("", value))
    } else {
        ("", value)
    };
    let existing: Vec<&str> = head.split(',').filter(|item| !item.is_empty()).collect();
    let current_text = match current {
        OptValue::Str(text) => text.clone(),
        OptValue::Num(number) => number.to_string(),
        _ => String::new(),
    };
    let candidates = if operator == "-=" && list {
        current_text
            .split(',')
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect()
    } else if !known.is_empty() {
        known
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
    } else if operator == "="
        && !current_text.is_empty()
        && !current_text.chars().any(char::is_whitespace)
    {
        vec![current_text.clone()]
    } else {
        Vec::new()
    };
    candidates
        .into_iter()
        .filter(|candidate| candidate.starts_with(prefix))
        .filter(|candidate| !list || !existing.contains(&candidate.as_str()))
        .filter(|candidate| {
            !list
                || !matches!(operator, "+=" | "^=")
                || !current_text.split(',').any(|item| item == candidate)
        })
        .map(|candidate| format!("{head}{candidate}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvim_editor::SetScope;

    fn candidates(line: &str) -> Vec<String> {
        complete_set(line, &Editor::new(), usize::MAX)
            .unwrap()
            .matches
    }

    #[test]
    fn all_registered_options_are_available() {
        assert_eq!(candidates("set "), Options::completion_names("", false));
        assert_eq!(candidates("set ").len(), Options::table_size());
        assert_eq!(candidates("set ts"), vec!["tabstop"]);
        assert!(candidates("set gfn").contains(&"guifont".to_string()));
    }

    #[test]
    fn boolean_prefixes_only_offer_boolean_options() {
        for prefix in ["no", "inv"] {
            let matches = candidates(&format!("set {prefix}"));
            assert!(!matches.is_empty());
            for item in matches {
                assert!(matches!(
                    Options::new().get(&item[prefix.len()..]),
                    Ok(OptValue::Bool(_))
                ));
            }
        }
        assert_eq!(candidates("set nonu"), vec!["nonumber"]);
        assert_eq!(candidates("set invnu"), vec!["invnumber"]);
    }

    #[test]
    fn abbreviations_and_multiple_settings_preserve_the_replacement_span() {
        for command in ["se", "setl", "setg"] {
            let line = format!("{command} number backg");
            let result = complete_set(&line, &Editor::new(), 10).unwrap();
            assert_eq!(result.start, line.rfind(' ').unwrap() + 1);
            assert_eq!(result.matches, vec!["background"]);
        }
        assert!(complete_set("setfiletype ru", &Editor::new(), 10).is_none());
        assert_eq!(candidates("set nu?"), vec!["number?", "numberwidth?"]);
        assert_eq!(
            candidates("set nu&vim"),
            vec!["number&vim", "numberwidth&vim"]
        );
    }

    #[test]
    fn enum_values_complete_long_names_and_aliases() {
        assert_eq!(candidates("set ff=d"), vec!["ff=dos"]);
        assert_eq!(candidates("setlocal fdm=i"), vec!["fdm=indent"]);
        assert_eq!(candidates("setglobal bg=l"), vec!["bg=light"]);
        assert_eq!(candidates("set selection=e"), vec!["selection=exclusive"]);
        assert!(candidates("set unknown=x").is_empty());
    }

    #[test]
    fn comma_lists_preserve_previous_values_and_skip_duplicates() {
        assert_eq!(candidates("set bs=indent,e"), vec!["bs=indent,eol"]);
        assert!(!candidates("set bs=indent,").contains(&"bs=indent,indent".to_string()));
        assert_eq!(
            candidates("set clipboard=unnamed,u"),
            vec!["clipboard=unnamed,unnamedplus"]
        );
    }

    #[test]
    fn compound_list_operations_complete_additions_and_removals() {
        assert_eq!(candidates("set bs-=i"), vec!["bs-=indent"]);
        assert!(candidates("set bs+=i").is_empty());
        assert!(candidates("set bs^=i").is_empty());
        assert_eq!(candidates("set nf+=o"), vec!["nf+=octal"]);
    }

    #[test]
    fn current_values_respect_local_and_global_scope() {
        let mut editor = Editor::new();
        editor
            .set_option("scrolloff", OptValue::Num(3), SetScope::Global)
            .unwrap();
        editor
            .set_option("scrolloff", OptValue::Num(9), SetScope::Local)
            .unwrap();
        for (line, expected) in [
            ("setlocal so=", "so=9"),
            ("setglobal so=", "so=3"),
            ("set so=", "so=9"),
        ] {
            assert_eq!(
                complete_set(line, &editor, 10).unwrap().matches,
                vec![expected]
            );
        }
    }

    #[test]
    fn invalid_values_and_boolean_assignments_have_no_candidates() {
        assert!(candidates("set ff=invalid").is_empty());
        assert!(candidates("set number=").is_empty());
        assert!(candidates("set nonu=").is_empty());
        assert!(candidates("set tabstop?=").is_empty());
    }

    #[test]
    fn sensitive_values_are_never_suggested() {
        let mut editor = Editor::new();
        editor
            .options
            .set("key", OptValue::Str("secret".to_string()))
            .unwrap();
        assert!(
            complete_set("set key=", &editor, 10)
                .unwrap()
                .matches
                .is_empty()
        );
    }

    #[test]
    fn flag_values_append_available_flags_without_duplicates() {
        assert!(candidates("set mouse=").contains(&"mouse=a".to_string()));
        let matches = candidates("set mouse=n");
        assert!(matches.contains(&"mouse=nv".to_string()));
        assert!(!matches.contains(&"mouse=nn".to_string()));
        assert!(candidates("set guioptions=").contains(&"guioptions=e".to_string()));
        assert!(candidates("set formatoptions=t").contains(&"formatoptions=tc".to_string()));
    }

    #[test]
    fn guioptions_add_prepend_and_remove_complete_character_flags() {
        let removals = candidates("set go-=");
        assert!(removals.contains(&"go-=e".to_string()));
        assert!(!removals.contains(&"go-=b".to_string()));
        assert_eq!(candidates("set go-=e"), vec!["go-=e"]);
        for op in ["+=", "^="] {
            let additions = candidates(&format!("set go{op}"));
            assert!(additions.contains(&format!("go{op}b")));
            assert!(!additions.contains(&format!("go{op}e")));
            assert!(additions.contains(&format!("go{op}P")));
            assert!(additions.contains(&format!("go{op}R")));
        }
    }

    #[test]
    fn completing_an_addition_keeps_the_requested_flag() {
        let mut editor = Editor::new();
        editor
            .options
            .set("go", OptValue::Str("mr".to_string()))
            .unwrap();
        for op in ["+=", "^="] {
            let result = complete_set(&format!("set go{op}e"), &editor, 50).unwrap();
            assert_eq!(result.matches, vec![format!("go{op}e")]);
            assert!(
                complete_set(&format!("set go{op}m"), &editor, 50)
                    .unwrap()
                    .matches
                    .is_empty()
            );
        }
    }

    #[test]
    fn guioption_completion_and_validation_share_the_same_flags() {
        for item in candidates("set go=") {
            let value = item.strip_prefix("go=").unwrap();
            Options::validate_value("guioptions", OptValue::Str(value.to_string())).unwrap();
        }
        assert!(candidates("set go=Z").is_empty());
        assert!(candidates("set go+=,").is_empty());
    }

    #[test]
    fn candidates_are_sorted_limited_and_deduplicated() {
        let limited = complete_set("set ", &Editor::new(), 3).unwrap();
        assert_eq!(limited.matches.len(), 3);
        assert!(limited.matches.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
