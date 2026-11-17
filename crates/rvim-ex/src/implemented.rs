//! Implemented-command bookkeeping.
//!
//! The authoritative list lives next to the dispatch in [`crate::exec`] so the
//! two cannot drift; this module re-exports it for tooling and provides the
//! consistency tests.

pub use crate::exec::IMPLEMENTED;

/// True when `name` (a canonical command name) has a handler.
pub fn is_implemented(name: &str) -> bool {
    IMPLEMENTED.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::run_command;
    use crate::parser::parse_line;
    use crate::table::{COMMANDS, lookup};
    use rvim_editor::Editor;

    #[test]
    fn implemented_names_are_canonical_table_names() {
        for name in IMPLEMENTED {
            assert_eq!(
                lookup(name).map(|c| c.name),
                Some(*name),
                "{name} must resolve to itself through the prefix lookup"
            );
            assert_eq!(*name, lookup(name).unwrap().name);
        }
        // No duplicates, and every entry exists in the generated table.
        let mut seen = std::collections::BTreeSet::new();
        for name in IMPLEMENTED {
            assert!(seen.insert(*name), "duplicate entry in IMPLEMENTED: {name}");
            assert!(
                COMMANDS.iter().any(|c| c.name == *name),
                "{name} is not in the baseline table"
            );
        }
    }

    #[test]
    fn implemented_list_matches_dispatch() {
        // Probe every implemented command with empty arguments on a scratch
        // editor. The dispatch's fallback arm reports ERV001 for any command
        // that is listed as implemented but has no arm; anything else (ok,
        // quit or a normal Vim error) is fine here.
        for name in IMPLEMENTED {
            let mut ed = Editor::new();
            let parsed = parse_line(name, 1).expect("parse");
            let result = run_command(&mut ed, &parsed);
            if let Err(e) = result {
                assert_ne!(
                    e.code, "ERV001",
                    "command {name} is listed as implemented but dispatch has no arm: {e}"
                );
            }
        }
    }
}
