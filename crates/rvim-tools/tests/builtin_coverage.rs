//! Coverage gate: the M3 exit criterion requires >= 70% of the baseline
//! builtin-function table (543 entries from `evalfunc.c`) to be implemented.
//!
//! The implemented set is the union of the pure builtins in `rvim-viml` and the
//! editor-dependent host builtins spread across `rvim-ex`.

fn implemented_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = rvim_viml::builtins::implemented_funcs();
    names.extend_from_slice(rvim_ex::script::HOST_BUILTINS);
    names.extend_from_slice(rvim_ex::host_win::FUNCS);
    names.extend_from_slice(rvim_ex::host_meta::FUNCS);
    names.extend_from_slice(rvim_ex::host_more::FUNCS);
    names.sort_unstable();
    names.dedup();
    names
}

#[test]
fn builtin_coverage_is_at_least_70_percent() {
    let total = rvim_viml::FUNC_COUNT;
    let names = implemented_names();
    let implemented = names.len();
    let percent = implemented * 100 / total;
    assert!(
        percent >= 70,
        "builtin coverage regressed: {implemented}/{total} = {percent}% (need >= 70%)"
    );
}

#[test]
fn every_implemented_name_is_in_the_baseline_table() {
    let names = implemented_names();
    for name in &names {
        assert!(
            rvim_viml::FUNCTIONS.iter().any(|f| f.name == *name),
            "{name} is implemented but missing from the 543-entry baseline table"
        );
    }
}

#[test]
fn no_function_is_implemented_twice_across_layers() {
    // Overlaps between the pure layer and host layers are only allowed for the
    // names the host deliberately shadows (they are deduped for coverage but
    // should stay a short, reviewed list).
    let pure: Vec<&str> = rvim_viml::builtins::implemented_funcs();
    let host_groups: [&[&str]; 4] = [
        rvim_ex::script::HOST_BUILTINS,
        rvim_ex::host_win::FUNCS,
        rvim_ex::host_meta::FUNCS,
        rvim_ex::host_more::FUNCS,
    ];
    let mut overlaps = Vec::new();
    for group in host_groups {
        for name in group {
            if pure.contains(name) {
                overlaps.push(*name);
            }
        }
    }
    overlaps.sort_unstable();
    overlaps.dedup();
    let allowed = [
        "balloon_split",
        "byte2line",
        "chdir",
        "getpid",
        "getpos",
        "getreg",
        "getregtype",
        "hostname",
    ];
    for name in overlaps {
        assert!(
            allowed.contains(&name),
            "unexpected duplicate implementation across layers: {name}"
        );
    }
}
