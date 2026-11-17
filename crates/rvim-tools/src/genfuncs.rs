//! Generate the builtin-function table from the vendored reference source.
//!
//! Source: `reference/vim/src/evalfunc.c` `global_functions[]` (v8.2.5172), the
//! user-callable builtin set: 543 entries with `(name, minargs, maxargs)`.

use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug)]
struct Entry {
    name: String,
    min: u32,
    max: u32,
}

/// Extract `{"name", min, max, ...}` entries from the `global_functions[]`
/// array (the only `funcentry_T` table in evalfunc.c for this baseline).
fn parse_entries(src: &str) -> Result<Vec<Entry>, String> {
    let start = src
        .find("static funcentry_T global_functions[]")
        .ok_or("global_functions[] not found")?;
    let rest = &src[start..];
    let end = rest
        .find("\n};")
        .ok_or("end of global_functions not found")?;
    let table = &rest[..end];
    let mut out = Vec::new();
    for line in table.lines() {
        let line = line.trim_start();
        let Some(after_brace) = line.strip_prefix('{') else {
            continue;
        };
        let after_brace = after_brace.trim_start();
        let Some(rest) = after_brace.strip_prefix('"') else {
            continue;
        };
        let Some(quote_end) = rest.find('"') else {
            continue;
        };
        let name = &rest[..quote_end];
        if name.is_empty() {
            continue;
        }
        // After the closing quote: ", min, max, <rest>"
        let tail = &rest[quote_end + 1..];
        let tail = tail.trim_start().trim_start_matches(',').trim_start();
        let mut parts = tail.split(',');
        let min: u32 = parts
            .next()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| format!("bad minargs for {name}"))?;
        let max: u32 = parts
            .next()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| format!("bad maxargs for {name}"))?;
        out.push(Entry {
            name: name.to_string(),
            min,
            max,
        });
    }
    Ok(out)
}

fn render(entries: &[Entry]) -> String {
    let mut out = String::new();
    out.push_str(
        "//! GENERATED FILE - do not edit by hand.\n\
         //!\n\
         //! Builtin-function table mirrored from the baseline `reference/vim/src/evalfunc.c`\n\
         //! `global_functions[]` (v8.2.5172): the user-callable builtin set with arity\n\
         //! bounds. Coverage is measured against this fixed denominator.\n\
         //!\n\
         //! Regenerate with: `cargo run -p rvim-tools -- gen-funcs`\n\n\
         /// One builtin function declaration.\n\
         #[derive(Clone, Copy, Debug)]\n\
         pub struct FuncDef {\n\
         \x20   pub name: &'static str,\n\
         \x20   /// Minimum accepted argument count.\n\
         \x20   pub min_args: u32,\n\
         \x20   /// Maximum accepted argument count.\n\
         \x20   pub max_args: u32,\n\
         }\n\n",
    );
    let _ = writeln!(
        out,
        "/// Number of builtin functions in the baseline table.\npub const FUNC_COUNT: usize = {};\n",
        entries.len()
    );
    let _ = writeln!(out, "pub static FUNCTIONS: [FuncDef; FUNC_COUNT] = [");
    for e in entries {
        let _ = writeln!(
            out,
            "    FuncDef {{ name: {:?}, min_args: {}, max_args: {} }},",
            e.name, e.min, e.max
        );
    }
    out.push_str("];\n");
    out
}

pub fn run(root: &Path) -> Result<(), String> {
    let path = root.join("reference/vim/src/evalfunc.c");
    let src = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let entries = parse_entries(&src)?;
    if entries.len() != 543 {
        return Err(format!(
            "expected 543 builtin functions in the baseline, parsed {}",
            entries.len()
        ));
    }
    let out_path = root.join("crates/rvim-viml/src/func_table.rs");
    std::fs::write(&out_path, render(&entries))
        .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
    println!(
        "wrote {} ({} builtin functions)",
        out_path.display(),
        entries.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entry_names_and_arity() {
        let src = r#"
static funcentry_T global_functions[] =
{
    {"abs",		1, 1, FEARG_1,	    arg1_float_or_nr,
			ret_any,	    FLOAT_FUNC(f_abs)},
    {"add",		2, 2, FEARG_1,	    arg2_listblob_item,
			ret_first_arg,	    f_add},
    {"and",		2, 3, FEARG_1,	    arg2_number,
			ret_number,	    f_and},
};
"#;
        let entries = parse_entries(src).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "abs");
        assert_eq!((entries[0].min, entries[0].max), (1, 1));
        assert_eq!((entries[2].min, entries[2].max), (2, 3));
    }
}
