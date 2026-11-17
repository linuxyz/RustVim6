//! `:map` / `:abbreviate` command handlers.

use rvim_core::{VimError, VimResult};
use rvim_editor::{AbbrevMode, Editor, MapMode, MapOptions};

use crate::exec::ExOutcome;
use crate::parser::ParsedEx;

/// Which mapping family a command name belongs to.
struct MapFamily {
    modes: Vec<MapMode>,
    recursive: bool,
}

fn map_family(name: &str) -> Option<MapFamily> {
    use MapMode::*;
    let all_except_insert_cmdline = vec![Normal, Visual, VisualOnly, Select, Operator];
    let recursive = !name.contains("noremap");
    let modes = match name {
        "map" | "noremap" => all_except_insert_cmdline,
        "nmap" | "nnoremap" => vec![Normal],
        "vmap" | "vnoremap" => vec![Visual, VisualOnly, Select],
        "xmap" | "xnoremap" => vec![VisualOnly],
        "smap" | "snoremap" => vec![Select],
        "omap" | "onoremap" => vec![Operator],
        "imap" | "inoremap" => vec![Insert],
        "lmap" | "lnoremap" => vec![Insert, Cmdline, Language],
        "cmap" | "cnoremap" => vec![Cmdline],
        _ => return None,
    };
    Some(MapFamily { modes, recursive })
}

/// `:map!`, `:noremap!`, `:unmap!` and `:mapclear!` are the insert+cmdline
/// variants of the un-suffixed commands (vim models this with the bang, not
/// with separate table entries).
fn bang_expands_to_insert_cmdline(name: &str) -> bool {
    matches!(name, "map" | "noremap" | "unmap" | "mapclear")
}

/// Modes an `:unmap`-style command clears / removes from.
fn unmap_modes(name: &str) -> Option<Vec<MapMode>> {
    use MapMode::*;
    Some(match name {
        "unmap" => vec![
            Normal, Visual, VisualOnly, Select, Operator, Insert, Cmdline,
        ],
        "nunmap" => vec![Normal],
        "vunmap" => vec![Visual, VisualOnly, Select],
        "xunmap" => vec![VisualOnly],
        "sunmap" => vec![Select],
        "ounmap" => vec![Operator],
        "iunmap" => vec![Insert],
        "lunmap" => vec![Insert, Cmdline, Language],
        "cunmap" => vec![Cmdline],
        _ => return None,
    })
}

fn mapclear_modes(name: &str) -> Option<Vec<MapMode>> {
    use MapMode::*;
    Some(match name {
        "mapclear" => vec![Normal, Visual, VisualOnly, Select, Operator],
        "nmapclear" => vec![Normal],
        "vmapclear" => vec![Visual, VisualOnly, Select],
        "xmapclear" => vec![VisualOnly],
        "smapclear" => vec![Select],
        "omapclear" => vec![Operator],
        "imapclear" => vec![Insert],
        "lmapclear" => vec![Insert, Cmdline, Language],
        "cmapclear" => vec![Cmdline],
        _ => return None,
    })
}

/// Parse leading `<buffer> <nowait> <silent> <special> <script> <expr> <unique>`
/// option words off a mapping argument list.
fn take_map_options(args: &str) -> (MapOptions, String) {
    let mut opts = MapOptions::default();
    let mut rest = args.trim_start().to_string();
    loop {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('<') {
            break;
        }
        let Some(close) = trimmed.find('>') else {
            break;
        };
        let token = &trimmed[..=close];
        match token.to_ascii_lowercase().as_str() {
            "<buffer>" => opts.buffer = true,
            "<nowait>" => opts.nowait = true,
            "<silent>" => opts.silent = true,
            "<special>" => opts.special = true,
            "<script>" => opts.script = true,
            "<expr>" => opts.expr = true,
            "<unique>" => opts.unique = true,
            _ => break,
        }
        rest = trimmed[close + 1..].trim_start().to_string();
    }
    (opts, rest)
}

/// Split mapping arguments into `(lhs, rhs)`. A lhs may be a single special key
/// (`<CR>`); otherwise it is the first whitespace-delimited word.
fn split_lhs_rhs(rest: &str) -> (String, String) {
    let trimmed = rest.trim_start();
    if trimmed.is_empty() {
        return (String::new(), String::new());
    }
    if trimmed.starts_with('<') {
        if let Some(close) = trimmed.find('>') {
            let head = &trimmed[..=close];
            // `<C-x>` style lhs is a single token; anything with a space inside
            // the brackets cannot be a key name, so fall back to word split.
            if !head.contains(' ') {
                return (
                    head.to_string(),
                    trimmed[close + 1..].trim_start().to_string(),
                );
            }
        }
    }
    match trimmed.find(char::is_whitespace) {
        Some(idx) => (
            trimmed[..idx].to_string(),
            trimmed[idx..].trim_start().to_string(),
        ),
        None => (trimmed.to_string(), String::new()),
    }
}

/// True when `name` is handled by [`map_command`].
pub fn is_map_command(name: &str) -> bool {
    map_family(name).is_some() || unmap_modes(name).is_some() || mapclear_modes(name).is_some()
}

/// True when `name` is handled by [`abbrev_command`].
pub fn is_abbrev_command(name: &str) -> bool {
    abbrev_family(name).is_some() || unabbrev_modes(name).is_some() || abclear_modes(name).is_some()
}

fn render_maps(entries: &[&rvim_editor::MapEntry], modes: &[MapMode]) -> Vec<String> {
    let mut out = Vec::new();
    for e in entries {
        let mode_char = e
            .modes
            .iter()
            .find(|m| modes.contains(m))
            .map(|m| m.to_char())
            .unwrap_or(' ');
        let mut flags = String::new();
        if e.opts.expr {
            flags.push('*');
        }
        if e.opts.silent {
            flags.push('&');
        }
        if e.opts.nowait {
            flags.push('!');
        }
        let rhs = if e.opts.expr || e.opts.silent {
            // Vim shows the raw rhs when special characters are involved.
            e.rhs_raw.clone()
        } else {
            e.rhs_notation()
        };
        out.push(format!("{mode_char}{flags}\t{}\t{}", e.lhs_notation(), rhs));
    }
    out
}

/// Handle `:map`, `:nmap`, `:inoremap`, `:unmap`, `:mapclear`, ...
pub fn map_command(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let name = p.command.as_str();
    if let Some(family) = map_family(name) {
        let modes = if p.bang && bang_expands_to_insert_cmdline(name) {
            vec![MapMode::Insert, MapMode::Cmdline]
        } else {
            family.modes
        };
        let (opts, rest) = take_map_options(&p.args);
        let opts = MapOptions {
            recursive: family.recursive,
            ..opts
        };
        let (lhs, rhs) = split_lhs_rhs(&rest);
        if lhs.is_empty() {
            // No lhs: list the mappings for these modes.
            let mut entries: Vec<&rvim_editor::MapEntry> = Vec::new();
            for m in &modes {
                for e in ed.maps.entries_for(*m) {
                    if !entries.iter().any(|x| std::ptr::eq(*x, e)) {
                        entries.push(e);
                    }
                }
            }
            for line in render_maps(&entries, &modes) {
                ed.messages.info(line);
            }
            return Ok(ExOutcome::Continue);
        }
        if rhs.is_empty() && !p.bang {
            // Query a single mapping: print it if it exists (vim prints "No
            // mapping found" with E31 otherwise).
            let keys = rvim_editor::maps::parse_keys(&lhs);
            let found = modes
                .iter()
                .find_map(|m| ed.maps.entries_for(*m).into_iter().find(|e| e.lhs == keys));
            match found {
                Some(e) => {
                    ed.messages
                        .info(format!("{}\t{}", e.lhs_notation(), e.rhs_notation()));
                    return Ok(ExOutcome::Continue);
                }
                None => {
                    return Err(VimError::new("E31", format!("No mapping found: {lhs}")));
                }
            }
        }
        let rhs_text = if rhs.is_empty() {
            "<Nop>".to_string()
        } else {
            rhs
        };
        ed.maps
            .map(&lhs, &rhs_text, &modes, opts)
            .map(|_| ExOutcome::Continue)
    } else if let Some(modes) = unmap_modes(name) {
        let modes = if p.bang && bang_expands_to_insert_cmdline(name) {
            vec![MapMode::Insert, MapMode::Cmdline]
        } else {
            modes
        };
        let (lhs, _rhs) = split_lhs_rhs(&p.args);
        if lhs.is_empty() {
            // `:unmap` with no arguments removes everything for those modes.
            ed.maps.clear(&modes);
            return Ok(ExOutcome::Continue);
        }
        ed.maps.unmap(&lhs, &modes).map(|_| ExOutcome::Continue)
    } else if let Some(modes) = mapclear_modes(name) {
        let modes = if p.bang && bang_expands_to_insert_cmdline(name) {
            vec![MapMode::Insert, MapMode::Cmdline]
        } else {
            modes
        };
        let (lhs, _rhs) = split_lhs_rhs(&p.args);
        if !lhs.is_empty() {
            ed.maps.unmap(&lhs, &modes)?;
        } else {
            ed.maps.clear(&modes);
        }
        Ok(ExOutcome::Continue)
    } else {
        Err(VimError::new(
            "E492",
            format!("Not an editor command: {name}"),
        ))
    }
}

/// Abbreviation families: command name -> modes and recursive flag.
fn abbrev_family(name: &str) -> Option<(Vec<AbbrevMode>, bool)> {
    use AbbrevMode::*;
    Some(match name {
        "abbreviate" => (vec![Insert, Cmdline], true),
        "noreabbrev" => (vec![Insert, Cmdline], false),
        "iabbrev" => (vec![Insert], true),
        "inoreabbrev" => (vec![Insert], false),
        "cabbrev" => (vec![Cmdline], true),
        "cnoreabbrev" => (vec![Cmdline], false),
        _ => return None,
    })
}

fn unabbrev_modes(name: &str) -> Option<Vec<AbbrevMode>> {
    use AbbrevMode::*;
    Some(match name {
        "unabbreviate" => vec![Insert, Cmdline],
        "iunabbrev" => vec![Insert],
        "cunabbrev" => vec![Cmdline],
        _ => return None,
    })
}

fn abclear_modes(name: &str) -> Option<Vec<AbbrevMode>> {
    use AbbrevMode::*;
    Some(match name {
        "abclear" => vec![Insert, Cmdline],
        "iabclear" => vec![Insert],
        "cabclear" => vec![Cmdline],
        _ => return None,
    })
}

/// Handle `:abbreviate` and friends.
pub fn abbrev_command(ed: &mut Editor, p: &ParsedEx) -> VimResult<ExOutcome> {
    let name = p.command.as_str();
    if let Some((modes, recursive)) = abbrev_family(name) {
        let (opts, rest) = take_map_options(&p.args);
        let opts = MapOptions { recursive, ..opts };
        let (lhs, rhs) = split_lhs_rhs(&rest);
        if lhs.is_empty() {
            for mode in &modes {
                for e in ed.abbrevs.entries_for(*mode) {
                    let m = match mode {
                        AbbrevMode::Insert => 'i',
                        AbbrevMode::Cmdline => 'c',
                    };
                    ed.messages.info(format!("{m}\t{} -> {}", e.lhs, e.rhs));
                }
            }
            return Ok(ExOutcome::Continue);
        }
        ed.abbrevs.abbreviate(&lhs, &rhs, &modes, opts)?;
        Ok(ExOutcome::Continue)
    } else if let Some(modes) = unabbrev_modes(name) {
        let (lhs, _) = split_lhs_rhs(&p.args);
        if lhs.is_empty() {
            ed.abbrevs.clear(&modes);
            return Ok(ExOutcome::Continue);
        }
        ed.abbrevs.unabbreviate(&lhs, &modes)?;
        Ok(ExOutcome::Continue)
    } else if let Some(modes) = abclear_modes(name) {
        ed.abbrevs.clear(&modes);
        Ok(ExOutcome::Continue)
    } else {
        Err(VimError::new(
            "E492",
            format!("Not an editor command: {name}"),
        ))
    }
}
