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
  is a green cell outline.
- 256-colour ANSI values and 24-bit RGB values are both mapped to Direct2D
  colours. The font is Consolas at 16 DIPs, scaled by the window DPI.
- The executable manifest requests PerMonitorV2 DPI awareness and the icon is
  embedded from `assets/icon/rvim.ico`.

## Input

- `WM_CHAR` feeds printable characters; `WM_KEYDOWN` maps Enter, Backspace,
  Escape, Tab, arrows, Home/End, Insert/Delete, PageUp/PageDown, F1-F12 and
  Ctrl/Shift navigation.
- The mouse wheel feeds Ctrl-Y / Ctrl-E.
- `WM_SIZE`, `WM_DPICHANGED` and device-loss (`D2DERR_RECREATE_TARGET`) rebuild
  the render target; a 100 ms timer pumps jobs/channels/timers.

## Limitations

- No IME composition, native clipboard bridge, mouse click-to-position,
  drag selection, or native menus/dialogs yet.
- No cursor blink; the cursor is drawn as a static cell outline.
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
