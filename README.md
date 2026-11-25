# RustVim

A Vim-compatible editor written from scratch in Rust. The editor core is
**`std`-only** apart from the small `unicode-width` table used for CJK display
width; the Windows GUI platform layer uses the `winapi` 0.3 bindings.
The editor baseline is Vim `v8.2.5172`;
**Vim9 script** is anchored to Vim `v9.2.1398`
(`https://github.com/vim/vim/tree/v9.2.1398`), including class/interface/enum/type
alias syntax. The only third-party binding is `winapi` for the Windows GUI;
M8 encryption/compression is self-contained (see [docs/PLAN.md](docs/PLAN.md)).

## Status: M9 / 1.1 (Vim9 script, Windows GUI, jobs/channels/timers/terminal, encryption, packaging)

| Milestone | Delivered |
|---|---|
| M0 | workspace, CI gates, golden-test harness `rvim --version` / `-es` |
| M1 | headless editing core (buffers, undo tree, marks, registers, options) |
| M2 | 589-entry Ex command table, 311 implemented (after M9), command line + completion, maps/abbrevs |
| M3 | legacy VimL interpreter, **543/543 builtins wired** (subsystem-less ones are documented stubs), autocmd dispatch, `:source` of real runtime scripts |
| M4 | terminal TUI (grid, keys, mappings, search, message paging, resize), CJK/wide-glyph cell layout |
| M5 | windows/tabpages, folds, diff (`rvim -d`), quickfix/location lists, visual mode + text objects, `.` repeat, jumps/changes, viminfo, sign/textprop/popup APIs |
| M6 | syntax highlighting: `rvim-syntax` (keyword/match/region, contained/transparent/skip/nextgroup, sync, runtime `syntax/*.vim`), 20 built-in filetypes, `:syntax`/`:highlight`, `synID()` family, coloured TUI cells; regex closures (`\zs`, backrefs, lookaround, `\_.`, magic levels); `:s` replacement specials; spell (`.spl`/word lists, suggestions, Spell* highlighting); tags (`:tag` family, tag stack, `<C-]>`); `cindent`/`smartindent`; `:vimgrep` |
| M7 | jobs (`job_start`/callbacks), TCP channels + JSON-RPC (`ch_*`), timers (`timer_*`), minimal terminal buffers (`term_start`, `term_getline`), `:sleep` event pumping |
| M8 | compatibility closure: encryption/compression (`zip`, `blowfish`, `blowfish2`), `has()` feature list, `:redir`, session/view files, packaging script (Windows zip / Unix tar.gz, runtime merge), **multi-size HiDPI Windows icon (16-256 px, embedded in exe + installer)**, installed `$VIMRUNTIME` lookup, `rvim 1.0` |
| M9 | Vim9 script (`rvim 1.1`, reference **v9.2.1398**): `vim9script`, typed `def`/`var`/`const`/`final`, `import`/`export`, lambdas, method calls, interpolation, exceptions, **classes/objects/static members**, **enums**, interfaces, type aliases; 7/8 Vim 9.2 Vim9 autoloads source; 9.2 conformance subset 26/322 pass (8.1%), 123 run to completion |
| GUI | Windows **Direct2D/DirectWrite** front-end (`rvim --gui` or no console): native window, `Tui` grid rendering, keyboard/mouse-wheel input, IME, `*` clipboard, Vim cursors, CJK/wide-glyph and colour-emoji rendering, ligature fonts, `guifont`/`guifontwide` with 40-cell wide-font auto-fit, guioptions tabline, resize and PerMonitorV2 DPI; see [docs/GUI.md](docs/GUI.md) |

All nine milestones are delivered. See [docs/VIM9.md](docs/VIM9.md) for the Vim9 9.2
feature matrix and the published conformance numbers, and
[docs/PLAN.md](docs/PLAN.md) for the remaining known deviations.

Coverage tables are generated, never hand-edited: `cargo run -p rvim-tools -- coverage`.
Known deviations are enumerated in [docs/compat_deviations.md](docs/compat_deviations.md).

## Quick start

```powershell
pwsh -File scripts/fetch-reference.ps1   # fetch the Vim baseline (not versioned, ~57 MB)
cargo build          # fetches winapi the first time
cargo test

target\debug\rvim.exe file.txt          # interactive TUI
target\debug\rvim.exe --keys ':split<CR>Vjzf<CR>' --cols 60 --rows 16 file.txt   # scripted frame dump
target\debug\rvim.exe -es -c '%s/a/b/g' -c 'wq' file.txt                          # batch Ex mode
target\debug\rvim.exe -d old.txt new.txt                                          # vimdiff
```

## Vim9 script (M9)

Vim9 is implemented against **Vim 9.2.1398** (the editor's compatibility baseline
stays 8.2.5172). Supported: typed `def`, `var/const/final`, `import/export`,
lambdas, method calls, interpolation, exceptions, classes/objects with
`new()`/`this`/static members, enums, interfaces and type aliases.

```vim
vim9script
class Counter
  var count: number = 0
  static var total: number = 0
  def new()
    total += 1
  enddef
  def Inc(): number
    this.count += 1
    return this.count
  enddef
endclass
var c = Counter.new()
g:result = c.Inc()
```

`:source` compiles and runs the file; `:vim9cmd` runs one Vim9 command line.
Fetch the 9.2 reference and reproduce the conformance run with:

```powershell
pwsh -File scripts/fetch-vim9-reference.ps1
pwsh -File scripts/vim9_conformance.ps1
```

See [docs/VIM9.md](docs/VIM9.md) for the matrix and known gaps.

## Windows GUI (Direct2D / DirectWrite)

The Windows front-end renders the same editor grid with Direct2D and
DirectWrite:

```powershell
target\release\rvim.exe --gui file.txt
```

Starting `rvim.exe` without a console also opens the GUI. It supports the
existing TUI layouts, syntax colours, statusline, popups, keyboard input,
mouse-wheel scrolling, window resizing, PerMonitorV2 DPI, Vim-style
block/insert/replace cursors, `guifont`/`guifontwide` with the native Font
dialog, IME composition, `*` clipboard sync, Shift+Insert/Ctrl+Insert, and a
guioptions-controlled tabline. See [docs/GUI.md](docs/GUI.md) for the
rendering/input details and current limitations (mouse click/drag selection and
native menus are not implemented yet).

## Packaging (M8)

```powershell
pwsh -File scripts/package.ps1                 # Windows zip + merged runtime
pwsh -File scripts/package.ps1 -Target unix    # Linux tar.gz (run on Linux)
makensis packaging\rvim.nsi                    # optional NSIS installer
```

The installer/staged tree ships `rvim`/`rvim.exe`, docs and a merged `runtime/`.
`rvim.exe` and the NSIS installer embed the 10-size icon from [assets/icon](assets/icon); Windows
picks the native 16/20/24/32/40/48/64/96/128/256 px image for the current DPI.
`default_runtimepath()` finds `runtime` next to the executable (or in
`../runtime`), then falls back to `$RVIM_RUNTIME` and the development baseline.

### Prebuilt binaries

[`.github/workflows/build.yml`](.github/workflows/build.yml) builds release binaries on
Windows x64 and Linux amd64 for every push/PR and uploads them as workflow
artifacts. Pushing a `v*` tag also publishes both archives to a GitHub Release:

- `rvim-<version>-windows-x64.zip` (`rvim.exe`, docs, merged `runtime/`)
- `rvim-<version>-linux-x64.tar.gz` (`rvim`, docs, merged `runtime/`)

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
