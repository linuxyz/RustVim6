# Windows GUI (Direct2D / DirectWrite)

`rvim --gui [file ...]` opens the native Windows front-end. When `rvim.exe` is
started without a console (for example from Explorer), it also defaults to the
GUI. The console/TUI remains the default when a terminal is attached.

## Requirements

- Windows 10 or newer (Direct2D, DirectWrite, `GetDpiForWindow`).
- The GUI platform layer uses the `winapi` 0.3 crate. The editor core and the
  Linux build remain std-only; the dependency is target-gated in
  `crates/rvim-gui/Cargo.toml`.

## Rendering

- The existing `rvim-app::tui::Tui` computes the logical `Grid` (buffer,
  statusline, command line, messages, popups, syntax styles).
- `crates/rvim-gui/src/win.rs` creates an `ID2D1HwndRenderTarget` and an
  `IDWriteFactory`/`IDWriteTextFormat` set (normal, bold, italic, bold-italic).
- Each row is split into same-style/same-font runs; backgrounds are filled with
  `ID2D1SolidColorBrush`, text is drawn with `DrawTextLayout`, and the cursor
  uses the Vim block/vertical/horizontal shapes described below.
- East Asian wide glyphs occupy two grid cells (the second is a continuation
  cell skipped by both the terminal renderer and Direct2D). Font choice is by
  Unicode glyph coverage, not by character width.
- 256-colour ANSI values and 24-bit RGB values are both mapped to Direct2D
  colours. The default font is Consolas at 12 pt (16 DIPs at 96 DPI), scaled by
  the window DPI and configurable with `guifont` / `guifontwide`.
- The executable manifest requests PerMonitorV2 DPI awareness and the icon is
  embedded from `assets/icon/rvim.ico`.

## Cursor

The cursor follows Vim's GUI shapes:

| Mode | Shape |
|---|---|
| Normal, Visual, Select, Command-line | block |
| Insert | vertical bar (25% cell width) |
| Replace | horizontal bar (20% cell height) |
| Operator-pending | horizontal bar (50% cell height) |

The block is filled with the `Cursor` highlight group (or the inverse of
`Normal` when `Cursor` is undefined), and the character under it is redrawn in
the cursor foreground. Blinking follows Vim's default timings (700 ms wait,
400 ms off, 250 ms on) and resets to visible on every key press.

## Fonts

```vim
set guifont=Consolas:h12
set guifontwide=MS\ Gothic:h14
set guifont?
```

`hN` is the point size. `guifontwide` is independent of `guifont`:
its size is not copied to the normal font and it is not auto-fitted to the
grid. If `guifontwide` omits `hN`, it inherits the `guifont` point size.

The normal font defines the cell width and height, using the average advance
of the 52 Latin letters (Vim's `GetAverageFontSize()`). `guifont` and
`guifontwide` form one coverage-based font stack: a character uses the first
configured font whose font face has a glyph for it, independently of whether
the character is Latin, CJK or emoji. The grid cell count comes from Unicode
display width (`unicode-width`), not from the font. DirectWrite's system
fallback handles characters neither configured font covers.

In the GUI, a bare `:set guifont`, `:set guifontwide`, `:set guifont=*` or
`:set guifont=` opens the native Windows **Font** dialog (ChooseFont); the
selection is written back as `Family:hN`. Cancelling the dialog leaves the
option unchanged.

## IME and clipboard

- IME uses IMM32. `WM_IME_STARTCOMPOSITION` positions the composition window at
  the cursor, `WM_IME_COMPOSITION` collects `GCS_COMPSTR`/`GCS_RESULTSTR`, and the
  composition string is drawn inline at the cursor until it is committed.
- Register `*` is synchronised with the Windows clipboard (CF_UNICODETEXT),
  converting between LF and CRLF. Yanking to `*` updates the clipboard; an
  external clipboard change is loaded into `*` before the next key.
- In Insert mode, **Shift+Insert** inserts the clipboard text literally;
  **Ctrl+Insert** copies the visual selection (or the current line) to `*` and
  the clipboard.

## Tabs

The GUI follows Vim's `guioptions` **e** flag for the tabline. The default is
`egmrLtT`; use `:set guioptions-=e` to hide the tabline, or
`:set guioptions+=e` to show it when more than one tab page exists.

## Emoji and ligatures

- Emoji use DirectWrite font fallback with Direct2D's
  `DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT`; they occupy two Unicode display cells
  and go through the same coverage-based font selection as every other
  character.
- Text runs are shaped by DirectWrite as a unit. A font with programming
  ligatures (for example `Cascadia Code` or `Fira Code`) can be selected with
  `set guifont=Cascadia\ Code:h12`; standard ligatures such as `->` are
  applied within a style run.
- The simple cell model does not join ZWJ emoji sequences (for example
  family/skin-tone sequences) into one grapheme; individual emoji glyphs still
  render correctly.

### Which fonts are used

The renderer uses three sources:

1. `guifont` is the primary font and defines the cell width/height (the
   average Latin advance, matching Vim's `GetAverageFontSize()`);
2. `guifontwide` is the coverage fallback when the primary face has no glyph;
3. DirectWrite's system font fallback handles characters neither configured
   font covers (rare CJK, emoji, ...).

Font choice is by Unicode scalar coverage. It is deliberately **not** based on
UTF-8 byte length or on "CJK vs Latin" ranges. A wide character still occupies
two Unicode display cells, and a Latin character one.

Cursor positions and the statusline ruler use character display width, not the
font or UTF-8 byte offset: a Latin cell moves one column, a CJK/emoji cell moves
two. DirectWrite may use more fallback faces inside a run; the cell grid stays
monospaced.

## DPI changes

The manifest declares PerMonitorV2. On `WM_DPICHANGED` the suggested window
rectangle is applied, then the text formats and cell metrics are rebuilt from
the DPI in `wParam` (not only `GetDpiForWindow`, which can still report the old
monitor during the transition). Font sizes are specified in points, so moving
between 100%, 150% and 200% monitors changes physical cell size and the number
of columns/rows while keeping the same logical font size.

## Input

- `WM_CHAR` feeds printable characters; `WM_KEYDOWN` maps Enter, Backspace,
  Escape, Tab, arrows, Home/End, Insert/Delete, PageUp/PageDown, F1-F12 and
  Ctrl/Shift navigation.
- The mouse wheel feeds Ctrl-Y / Ctrl-E.
- `WM_SIZE`, `WM_DPICHANGED` and device-loss (`D2DERR_RECREATE_TARGET`) rebuild
  the render target; a 100 ms timer pumps jobs/channels/timers.

## Limitations

- No mouse click-to-position or drag selection, native menus, or cursor shape
  customization through `guicursor` yet. Combining marks are not attached to
  the preceding glyph by the simple cell model (wide CJK glyphs are supported).
- The GUI still runs the console-subsystem `rvim.exe` and detaches its console
  with `FreeConsole`; a dedicated GUI-subsystem launcher is not shipped.
- Windows-only. `rvim --gui` on other platforms returns an error (the Linux
  release is the TUI).

## Diagnostics

- `RVIM_GUI_AUTOCLOSE_MS=2000` closes the window after the given delay; useful
  for smoke tests.
- `RVIM_GUI_LOG=<file>` appends lifecycle diagnostics.

## Smoke test

```powershell
$env:RVIM_GUI_AUTOCLOSE_MS='2000'
target\debug\rvim.exe --gui --clean README.md
```

Typing `ihello<Esc>:wq<CR>` in the GUI writes `hello` to the opened file.
