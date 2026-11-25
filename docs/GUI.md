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

`hN` is the point size. `guifontwide` is used for East Asian wide-character
runs. In the GUI, a bare `:set guifont`, `:set guifontwide`, `:set guifont=*`
or `:set guifont=` opens the native Windows **Font** dialog (ChooseFont); the
selection is written back as `Family:hN`. Cancelling the dialog leaves the
option unchanged.

## Input

- `WM_CHAR` feeds printable characters; `WM_KEYDOWN` maps Enter, Backspace,
  Escape, Tab, arrows, Home/End, Insert/Delete, PageUp/PageDown, F1-F12 and
  Ctrl/Shift navigation.
- The mouse wheel feeds Ctrl-Y / Ctrl-E.
- `WM_SIZE`, `WM_DPICHANGED` and device-loss (`D2DERR_RECREATE_TARGET`) rebuild
  the render target; a 100 ms timer pumps jobs/channels/timers.

## Limitations

- No IME composition, native clipboard bridge, mouse click-to-position,
  drag selection, native menus, or cursor shape customization through
  `guicursor` yet.
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
