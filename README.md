# RustVim

A Vim-compatible editor written from scratch in Rust, with **no third-party crates**
(`std` only) and an offline build. Behaviour is anchored to Vim `v8.2.5172`
(`src/` + `runtime/` as the only reference). The sole dependency exception is the
M8 encryption/compression work (see [docs/PLAN.md](docs/PLAN.md)).

## Status: M6 (syntax highlighting)

| Milestone | Delivered |
|---|---|
| M0 | workspace, CI gates, golden-test harness `rvim --version` / `-es` |
| M1 | headless editing core (buffers, undo tree, marks, registers, options) |
| M2 | 589-entry Ex command table, 247 implemented, command line + completion, maps/abbrevs |
| M3 | legacy VimL interpreter, 393/543 builtins, autocmd dispatch, `:source` of real runtime scripts |
| M4 | terminal TUI (grid, keys, mappings, search, message paging, resize) |
| M5 | windows/tabpages, folds, diff (`rvim -d`), quickfix/location lists, visual mode + text objects, `.` repeat, jumps/changes, viminfo, sign/textprop/popup APIs |
| M6 | syntax highlighting: `rvim-syntax` (keyword/match/region, clusters, cross-line state), 20 built-in filetypes, `:syntax`/`:highlight`, coloured TUI cells |

Coverage tables are generated, never hand-edited: `cargo run -p rvim-tools -- coverage`.
Known deviations are enumerated in [docs/compat_deviations.md](docs/compat_deviations.md).

## Quick start

```powershell
pwsh -File scripts/fetch-reference.ps1   # fetch the Vim baseline (not versioned, ~57 MB)
cargo build --offline
cargo test  --offline

target\debug\rvim.exe file.txt          # interactive TUI
target\debug\rvim.exe --keys ':split<CR>Vjzf<CR>' --cols 60 --rows 16 file.txt   # scripted frame dump
target\debug\rvim.exe -es -c '%s/a/b/g' -c 'wq' file.txt                          # batch Ex mode
target\debug\rvim.exe -d old.txt new.txt                                          # vimdiff
```

## Documentation

- [docs/ARCH.md](docs/ARCH.md) - architecture, crate map, subsystem design
- [docs/PLAN.md](docs/PLAN.md) - milestone plan and exit criteria
- [docs/COVERAGE.md](docs/COVERAGE.md) - generated Ex/options/builtin coverage
- [docs/RUNTIME_COMPAT.md](docs/RUNTIME_COMPAT.md) - runtime compatibility matrix
- [docs/compat_deviations.md](docs/compat_deviations.md) - every known deviation, with the Vim source reference

## Constraints

- `std` only: no crates.io, no `unsafe` outside the platform layer (`ARCH` §3).
- Deterministic headless core: buffered text + keys in, document + messages out.
- Compatibility is a continuous process: each milestone updates the three coverage tables.
