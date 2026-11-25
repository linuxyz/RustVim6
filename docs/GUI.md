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
- Each row is split into same-style runs; backgrounds are filled with
  `ID2D1SolidColorBrush`, text is drawn with `DrawTextLayout`, and the cursor
  uses the Vim block/vertical/horizontal shapes described below.
- East Asian wide glyphs occupy two grid cells (the second is a continuation
  cell skipped by both the terminal renderer and Direct2D); `guifontwide` is
  used for wide runs.
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

`hN` is the point size. If `guifontwide` specifies `hN`, that size also sets
the normal `guifont` size. The wide format is then measured with the 20 Han
characters `一二三人口手千千万万大大反方上海重启还在` (40 cells) and its
point size is adjusted automatically so those glyphs fit exactly 40 normal
cells; the fitted format is what gets used. If `guifontwide` omits `hN`, it
inherits the `guifont` size.

The normal font defines the cell width and height, using Vim's average-width
measurement over the Latin alphabet; the fitted wide font does not widen Latin
cells. Changing either size updates the layout on the next frame.

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

- Emoji runs use **Segoe UI Emoji** with Direct2D's
  `DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT`, so colour emoji are rendered instead
  of missing-glyph boxes. Emoji occupy two grid cells.
- Text runs are shaped by DirectWrite as a unit. A font with programming
  ligatures (for example `Cascadia Code` or `Fira Code`) can be selected with
  `set guifont=Cascadia\ Code:h12`; standard ligatures such as `->` are
  applied within a style run.
- The simple cell model does not join ZWJ emoji sequences (for example
  family/skin-tone sequences) into one grapheme; individual emoji glyphs still
  render correctly.

### Which fonts are used

The renderer uses up to four sources:

1. `guifont` for ordinary cells (the cell width is the average Latin advance,
   matching Vim's `GetAverageFontSize()`; this is why choosing a CJK font does
   not double Latin cursor movement);
2. `guifontwide` for double-width runs when configured;
3. **Segoe UI Emoji** for emoji runs, with colour-glyph rendering;
4. DirectWrite's system font fallback for characters missing from the selected
   family (for example Latin in a CJK-only font or a rare CJK glyph).

Cursor positions and the statusline ruler use character display width, not the
font or UTF-8 byte offset: a Latin cell moves one column, a CJK/emoji cell moves
two. DirectWrite may use more fallback faces inside a run; the cell grid stays
monospaced.

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
