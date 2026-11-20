//! Tag files and the tag stack (reference: tag.c).
//!
//! A ctags file holds one tag per line:
//!
//! ```text
//! name<TAB>filename<TAB>cmd;<TAB>extended fields
//! ```
//!
//! This module implements the file-format half of Vim's tag support:
//!
//! * pseudo tags (`!_TAG_...` header lines) are skipped (tag.c:558);
//! * extended fields after `;"` are parsed: `kind:`, a bare kind (ctags writes
//!   it right after `;"`), `file:` (which marks a *static* tag,
//!   `test_for_static()`) and arbitrary `key:value` pairs;
//! * a tag's file name is resolved the way `expand_tag_fname()` does when
//!   'tagrelative' is set: a relative name is looked up next to the tags file
//!   that contains it.  As an extension (and to keep old tags files usable
//!   after a `:cd`), a name that does not exist there is also tried relative to
//!   the editor's current directory.
//!
//! Matching itself is split in two so that the caller can choose the matcher:
//! [`find_tags`] does Vim's ordinary exact match (honouring 'ignorecase'), while
//! [`find_tags_where`] lets `handlers_tags.rs` plug in the regexp engine for
//! `:tag /pattern` — rvim-editor does not depend on rvim-viml.

use std::path::{Path, PathBuf};

use crate::Editor;

/// One tag file entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagEntry {
    /// Tag name as written in the tags file.
    pub name: String,
    /// File name as written in the tags file, possibly relative.
    pub filename: String,
    /// The Ex command field, with the `;"` extended-field marker removed.
    pub cmd: String,
    /// `kind:` extended field, or the bare kind field ctags writes.
    pub kind: Option<String>,
    /// True for a static tag (a `file:` extended field is present).
    pub static_tag: bool,
    /// The tags file this entry was read from.
    pub tagfile: PathBuf,
}

impl TagEntry {
    /// Resolve the tag's file name against the tags file's own directory first
    /// ('tagrelative' on, Vim's default) and the current directory second.
    pub fn path(&self, cwd: &Path) -> PathBuf {
        let raw = PathBuf::from(&self.filename);
        if raw.is_absolute() {
            return raw;
        }
        let relative_to_tags = self
            .tagfile
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&raw);
        if relative_to_tags.is_file() {
            return relative_to_tags;
        }
        let relative_to_cwd = cwd.join(&raw);
        if relative_to_cwd.is_file() {
            return relative_to_cwd;
        }
        relative_to_tags
    }
}

/// Split the 'tags' option into entries the way Vim's `copy_option_part()`
/// does: commas and spaces separate file names.
fn option_parts(value: &str) -> Vec<String> {
    value
        .split([',', ' '])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// The tags files that exist, in search order: (name as written, absolute path).
///
/// Files reached through more than one entry are reported once, like
/// `get_tagfname()` does.
pub fn tag_files(ed: &Editor) -> Vec<(String, PathBuf)> {
    let option = ed.options.str_or("tags", "tags");
    // Vim's compiled-in default is "tags"; keep it when the option is empty.
    let value = if option.trim().is_empty() {
        "tags".to_string()
    } else {
        option
    };
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for part in option_parts(&value) {
        let raw = PathBuf::from(&part);
        let path = if raw.is_absolute() {
            raw
        } else {
            ed.cwd.join(&raw)
        };
        if !path.is_file() {
            continue;
        }
        let identity = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen.contains(&identity) {
            continue;
        }
        seen.push(identity);
        found.push((part, path));
    }
    found
}

/// Parse one ctags line; `None` for pseudo tags and malformed lines.
pub fn parse_tag_line(line: &str, tagfile: &Path) -> Option<TagEntry> {
    let line = line.trim_end_matches(['\r']);
    if line.is_empty() || line.starts_with("!_TAG_") {
        return None;
    }
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() < 3 {
        return None;
    }
    let name = fields[0];
    let filename = fields[1];
    if name.is_empty() || filename.is_empty() {
        return None;
    }
    // The third field is the Ex command.  Extended fields usually follow the
    // `;"` marker in the same field, and any further ones in their own fields.
    let command_field = fields[2];
    let (cmd, inline_extra) = match command_field.find(";\"") {
        Some(index) => (&command_field[..index], Some(&command_field[index + 2..])),
        None => (command_field, None),
    };
    let mut extras: Vec<&str> = Vec::new();
    if let Some(rest) = inline_extra {
        extras.extend(rest.split('\t').filter(|part| !part.is_empty()));
    }
    extras.extend(fields[3..].iter().copied());

    let mut kind: Option<String> = None;
    let mut static_tag = false;
    for extra in extras {
        if extra.is_empty() {
            continue;
        }
        if extra.starts_with("file:") {
            // `file:` (usually without a value) marks a static tag.
            static_tag = true;
            continue;
        }
        if let Some(value) = extra.strip_prefix("kind:") {
            if !value.is_empty() {
                kind = Some(value.to_string());
            }
            continue;
        }
        if extra.contains(':') {
            // Any other key:value extended field (class:, signature:, line:, ...).
            continue;
        }
        // A bare field is the tag kind (ctags writes it right after `;"`).
        if kind.is_none() {
            kind = Some(extra.to_string());
        }
    }
    Some(TagEntry {
        name: name.to_string(),
        filename: filename.to_string(),
        cmd: cmd.to_string(),
        kind,
        static_tag,
        tagfile: tagfile.to_path_buf(),
    })
}

/// Every tag matching `predicate`, in tags-file order and line order.
pub fn find_tags_where(ed: &Editor, predicate: impl Fn(&str) -> bool) -> Vec<TagEntry> {
    let mut found = Vec::new();
    for (_, path) in tag_files(ed) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let Some(entry) = parse_tag_line(line, &path) else {
                continue;
            };
            if predicate(&entry.name) {
                found.push(entry);
            }
        }
    }
    found
}

/// Find tags matching name exactly (:tag, CTRL-]).
///
/// 'ignorecase' is honoured, matching `find_tags()` when TAG_NOIC is clear.
pub fn find_tags(ed: &Editor, name: &str) -> Vec<TagEntry> {
    if name.is_empty() {
        return Vec::new();
    }
    let ignore_case = ed.options.bool_or("ignorecase", false);
    find_tags_where(ed, |candidate| {
        if ignore_case {
            candidate.eq_ignore_ascii_case(name)
        } else {
            candidate == name
        }
    })
}

/// True for a character 'iskeyword' accepts in its default value
/// `@,48-57,_,192-255`: letters (`@`), ASCII digits, `_` and bytes 192-255.
pub fn is_keyword_char(ch: char) -> bool {
    ch.is_alphabetic() || ch.is_ascii_digit() || ch == '_' || (ch as u32) >= 192
}

/// The keyword at or after byte column `col` (Vim's `<cword>` for CTRL-] and
/// `:tag` without an argument): non-keyword characters from the cursor on are
/// skipped, then the following run of keyword characters is returned.
pub fn keyword_at(text: &str, col: usize) -> String {
    let mut word = String::new();
    let mut started = false;
    for (index, ch) in text.char_indices() {
        if index < col {
            continue;
        }
        if is_keyword_char(ch) {
            started = true;
            word.push(ch);
        } else if started {
            break;
        }
    }
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kind_and_static_tags() {
        let entry = parse_tag_line(
            "alpha\tsrc/a.c\t/^int alpha;/;\"\tf\tfile:\tline:12",
            Path::new("tags"),
        )
        .expect("parsed");
        assert_eq!(entry.name, "alpha");
        assert_eq!(entry.filename, "src/a.c");
        assert_eq!(entry.cmd, "/^int alpha;/");
        assert_eq!(entry.kind.as_deref(), Some("f"));
        assert!(entry.static_tag);
        assert_eq!(entry.tagfile, PathBuf::from("tags"));
    }

    #[test]
    fn pseudo_tags_and_short_lines_are_skipped() {
        assert!(parse_tag_line("!_TAG_FILE_FORMAT\t2", Path::new("tags")).is_none());
        assert!(parse_tag_line("only\tone", Path::new("tags")).is_none());
        assert!(parse_tag_line("", Path::new("tags")).is_none());
    }

    #[test]
    fn keyword_at_skips_to_the_next_word() {
        assert_eq!(keyword_at("alpha beta", 0), "alpha");
        assert_eq!(keyword_at("alpha beta", 6), "beta");
        // Vim's <cword> starts at the cursor: it does not back up into the word.
        assert_eq!(keyword_at("alpha beta", 8), "ta");
        assert_eq!(keyword_at("a (b)", 3), "b");
        assert_eq!(keyword_at("...", 0), "");
        assert_eq!(keyword_at("", 0), "");
        assert_eq!(keyword_at("snake_case9", 0), "snake_case9");
    }

    #[test]
    fn relative_names_prefer_the_tags_directory() {
        let dir = std::env::temp_dir().join(format!("rvim_tags_unit_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("target.c"), "x\n").unwrap();
        let tagfile = dir.join("tags");
        let found = TagEntry {
            name: "x".to_string(),
            filename: "target.c".to_string(),
            cmd: "1".to_string(),
            kind: None,
            static_tag: false,
            tagfile: tagfile.clone(),
        };
        assert_eq!(found.path(Path::new("nowhere")), dir.join("target.c"));
        // A name that is missing next to the tags file falls back to the cwd.
        let cwd = dir.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("from_cwd.c"), "x\n").unwrap();
        let from_cwd = TagEntry {
            filename: "from_cwd.c".to_string(),
            ..found.clone()
        };
        assert_eq!(from_cwd.path(&cwd), cwd.join("from_cwd.c"));
        // A file that exists nowhere keeps the tags-relative name.
        let missing = TagEntry {
            filename: "missing.c".to_string(),
            ..found.clone()
        };
        assert_eq!(missing.path(Path::new("nowhere")), dir.join("missing.c"));
        // Absolute names are returned unchanged.
        let absolute = TagEntry {
            filename: dir.join("target.c").display().to_string(),
            ..found
        };
        assert_eq!(absolute.path(Path::new("nowhere")), dir.join("target.c"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
