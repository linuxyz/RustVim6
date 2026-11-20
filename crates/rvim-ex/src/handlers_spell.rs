//! :spellgood, :mkspell, :spelldump and the spell command family
//! (reference: spellfile.c).
//!
//! The good/bad/rare word sets live in rvim_editor::spell::SpellState. The
//! commands without ! also append the word to the first 'spellfile' as a plain
//! word list; ! keeps the word in the session's internal word list only,
//! mirroring Vim's internal word list.

use std::io::Write;
use std::path::{Path, PathBuf};

use rvim_core::{VimError, VimResult};
use rvim_editor::Editor;
use rvim_editor::spell::{self, WordKind};
use rvim_viml::Env;

use crate::exec::ExOutcome;
use crate::parser::ParsedEx;
use crate::script::ScriptState;

/// Dispatch one spell command.
pub fn cmd_spell(
    ed: &mut Editor,
    _env: &mut Env,
    _state: &mut ScriptState,
    p: &ParsedEx,
    name: &str,
) -> VimResult<ExOutcome> {
    match name {
        "spellgood" | "spellwrong" | "spellrare" | "spellundo" => add_word(ed, p, name),
        "spellinfo" => spell_info(ed),
        "spelldump" => spell_dump(ed),
        // Vim reports E752 for this; the M6 task specifies E753. The message
        // text is Vim's.
        "spellrepall" => Err(VimError::new("E753", "No previous spell replacement")),
        "mkspell" => mkspell(ed, p),
        other => Err(VimError::new(
            "E492",
            format!("Not an editor command: {other}"),
        )),
    }
}

/// :spellgood, :spellwrong, :spellrare and :spellundo.
fn add_word(ed: &mut Editor, p: &ParsedEx, name: &str) -> VimResult<ExOutcome> {
    let word = p.args.trim();
    if word.is_empty() {
        return Err(VimError::new("E471", "Argument required"));
    }
    ed.spell.ensure_loaded(ed);
    if name == "spellundo" {
        ed.spell.remove_word(word, p.bang);
        ed.messages.info(if p.bang {
            format!("Word '{word}' removed from internal word list")
        } else {
            format!("Word '{word}' removed")
        });
        return Ok(ExOutcome::Continue);
    }
    let kind = match name {
        "spellwrong" => WordKind::Bad,
        "spellrare" => WordKind::Rare,
        _ => WordKind::Good,
    };
    if p.bang {
        ed.spell.add_word(word, kind, true);
        ed.messages
            .info(format!("Word '{word}' added to internal word list"));
        return Ok(ExOutcome::Continue);
    }
    let target = spell::spellfile_paths(ed)
        .into_iter()
        .next()
        .ok_or_else(|| VimError::new("E756", "No spell file name"))?;
    append_word(&target, word, kind)?;
    ed.spell.add_word(word, kind, false);
    ed.messages
        .info(format!("Word '{word}' added to {}", target.display()));
    Ok(ExOutcome::Continue)
}

/// Append one word to a plain word list, creating the file (and its directory)
/// when needed.
fn append_word(path: &Path, word: &str, kind: WordKind) -> VimResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|_| {
                VimError::new("E484", format!("Can't open file {}", path.display()))
            })?;
        }
    }
    let suffix = match kind {
        WordKind::Good => "",
        WordKind::Bad => "/!",
        WordKind::Rare => "/?",
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| VimError::new("E484", format!("Can't open file {}", path.display())))?;
    writeln!(file, "{word}{suffix}")
        .map_err(|_| VimError::new("E484", format!("Can't write to file {}", path.display())))
}

/// :spellinfo: a one line summary plus the files that were loaded.
fn spell_info(ed: &mut Editor) -> VimResult<ExOutcome> {
    ed.spell.ensure_loaded(ed);
    let summary = ed.spell.summary();
    let language = ed.options.str_or("spelllang", "");
    ed.messages.info(format!(
        "spell: spelllang={language} words={} rare={} bad={} internal={}",
        summary.words, summary.rare, summary.bad, summary.internal
    ));
    for source in &summary.sources {
        ed.messages.info(format!("  {source}"));
    }
    for warning in &summary.warnings {
        ed.messages.info(format!("  {warning}"));
    }
    Ok(ExOutcome::Continue)
}

/// :spelldump: report how many words are loaded plus a sorted sample.
fn spell_dump(ed: &mut Editor) -> VimResult<ExOutcome> {
    ed.spell.ensure_loaded(ed);
    let summary = ed.spell.summary();
    let sample = ed.spell.sample_words(10);
    ed.messages
        .info(format!("spelldump: {} words", summary.words));
    for word in sample {
        ed.messages.info(word);
    }
    Ok(ExOutcome::Continue)
}

/// :mkspell[!] {outfile} {infile...}.
///
/// Writes a UTF-8 plain word list (one word per line, sorted, duplicates
/// removed). Banned (/!) and rare (/? ) entries keep their flag so the output
/// stays a valid Vim word list; a real binary .spl writer is out of scope.
/// Without infile arguments the current buffer is used.
fn mkspell(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let mut arguments = p.args.split_whitespace();
    let Some(outfile) = arguments.next() else {
        return Err(VimError::new("E471", "Argument required"));
    };
    let outfile = spell::expand_path(outfile);
    let infiles: Vec<PathBuf> = arguments.map(spell::expand_path).collect();
    let mut entries: Vec<spell::WordEntry> = Vec::new();
    if infiles.is_empty() {
        let buffer = ed.current_buffer();
        for line in 0..buffer.line_count() {
            let text = buffer.line_str(line).unwrap_or("");
            for (_, _, word) in spell::split_words(text) {
                entries.push(spell::WordEntry {
                    word,
                    kind: WordKind::Good,
                });
            }
        }
    } else {
        for path in &infiles {
            let words = spell::read_words_from_file(path).map_err(|_| {
                VimError::new("E484", format!("Can't open file {}", path.display()))
            })?;
            entries.extend(words);
        }
    }
    let mut lines: Vec<String> = entries
        .into_iter()
        .map(|entry| match entry.kind {
            WordKind::Good => entry.word,
            WordKind::Bad => format!("{}/!", entry.word),
            WordKind::Rare => format!("{}/?", entry.word),
        })
        .collect();
    lines.sort_unstable();
    lines.dedup();
    if let Some(parent) = outfile.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|_| {
                VimError::new("E484", format!("Can't open file {}", outfile.display()))
            })?;
        }
    }
    let mut file = std::fs::File::create(&outfile)
        .map_err(|_| VimError::new("E484", format!("Can't open file {}", outfile.display())))?;
    for line in &lines {
        writeln!(file, "{line}").map_err(|_| {
            VimError::new("E484", format!("Can't write to file {}", outfile.display()))
        })?;
    }
    ed.messages.info(format!(
        "mkspell: wrote {} words to {}",
        lines.len(),
        outfile.display()
    ));
    Ok(ExOutcome::Continue)
}
