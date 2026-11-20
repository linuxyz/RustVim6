# RustVim

A Vim-compatible editor written from scratch in Rust, with **no third-party crates**
(`std` only) and an offline build. Behaviour is anchored to Vim `v8.2.5172`
(`src/` + `runtime/` as the only reference). The sole dependency exception is the
M8 encryption/compression work (see [docs/PLAN.md](docs/PLAN.md)).

## Status: M7 + M8 / 1.0 (jobs/channels/timers/terminal, encryption, packaging)

| Milestone | Delivered |
|---|---|
| M0 | workspace, CI gates, golden-test harness `rvim --version` / `-es` |
| M1 | headless editing core (buffers, undo tree, marks, registers, options) |
| M2 | 589-entry Ex command table, 294 implemented, command line + completion, maps/abbrevs |
| M3 | legacy VimL interpreter, 441/543 builtins, autocmd dispatch, `:source` of real runtime scripts |
| M4 | terminal TUI (grid, keys, mappings, search, message paging, resize) |
| M5 | windows/tabpages, folds, diff (`rvim -d`), quickfix/location lists, visual mode + text objects, `.` repeat, jumps/changes, viminfo, sign/textprop/popup APIs |
| M6 | syntax highlighting: `rvim-syntax` (keyword/match/region, contained/transparent/skip/nextgroup, sync, runtime `syntax/*.vim`), 20 built-in filetypes, `:syntax`/`:highlight`, `synID()` family, coloured TUI cells; regex closures (`\zs`, backrefs, lookaround, `\_.`, magic levels); `:s` replacement specials; spell (`.spl`/word lists, suggestions, Spell* highlighting); tags (`:tag` family, tag stack, `<C-]>`); `cindent`/`smartindent`; `:vimgrep` |
| M7 | jobs (`job_start`/callbacks), TCP channels + JSON-RPC (`ch_*`), timers (`timer_*`), minimal terminal buffers (`term_start`, `term_getline`), `:sleep` event pumping |
| M8 | compatibility closure: encryption/compression (`zip`, `blowfish`, `blowfish2`), `has()` feature list, `:redir`, session/view files, packaging script (Windows zip / Unix tar.gz, runtime merge), installed `$VIMRUNTIME` lookup, `rvim 1.0` |

Next: **M9** Vim9 script (`rvim 1.1`). M7 (jobs/channels/timers/minimal terminal) and M8 (1.0) are delivered above.

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

## Packaging (M8)

```powershell
pwsh -File scripts/package.ps1                 # Windows zip + merged runtime
pwsh -File scripts/package.ps1 -Target unix    # Linux tar.gz (run on Linux)
makensis packaging\rvim.nsi                    # optional NSIS installer
```

The installer/staged tree ships `rvim`/`rvim.exe`, docs and a merged `runtime/`.
`default_runtimepath()` finds `runtime` next to the executable (or in
`../runtime`), then falls back to `$RVIM_RUNTIME` and the development baseline.

## Encryption (M8)

```vim
set key=secret
set cryptmethod=blowfish2   " or blowfish / zip
write
```

Files use Vim's `VimCrypt~01/02/03!` headers and are mutually readable with
real Vim; see [docs/compat_deviations.md](docs/compat_deviations.md) §12.

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
