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
  cell skipped by both the terminal renderer and Direct2D). Wide characters
  prefer `guifontwide`; other characters prefer `guifont`.
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

Normal-mode `j`/`k` and Up/Down preserve the desired display-cell column
across Latin, CJK and tab-indented lines. Short or empty lines do not reset the
goal. If the target falls inside a wide character or tab, the cursor rests on
that character's first cell and keeps the original goal for subsequent moves.

## Fonts

```vim
set guifont=Consolas:h12
set guifontwide=MS\ Gothic:h14
set guifont?
```

`hN` is the point size (fractional sizes such as `h12.5` are supported).
Use `:b` for bold, `:W600` for a numeric Windows weight (400 normal, 700 bold),
and `:i` for italic. Font-family spaces can be escaped or written as underscores:

```vim
set guifont=Consolas:h12:W400
set guifontwide=Microsoft_YaHei:h14:W600
set guifont=Maple_Mono_NF_CN:h12:W300  " Light
```

`guifontwide` is independent of `guifont`: an explicit wide point size is
preserved. If `guifontwide` omits `hN`, it inherits the `guifont` point size.
The primary font defines cell width using the average advance of the 52 Latin
letters. Row height accommodates both configured fonts; text is vertically
centred. A wide glyph is centred in two cells and compressed horizontally only
if its natural advance exceeds those cells, preserving its requested height.

Double-width characters (CJK, full-width punctuation, emoji) prefer
`guifontwide` whenever it is configured, even when `guifont` also covers them.
Other characters use `guifont`, with coverage fallback to `guifontwide`.
DirectWrite's system fallback handles glyphs missing from the selected font.
The grid cell count comes from Unicode display width (`unicode-width`).

In the GUI, a bare `:set guifont`, `:set guifontwide`, `:set guifont=*` or
`:set guifont=` opens the native Windows **Font** dialog (ChooseFont). The
selection preserves fractional point size, weight and italic style in the
option string. Legacy face names returned by the dialog (such as
`Maple Mono NF CN Light`) are resolved through GDI interop to the DirectWrite
family, with size and weight kept separate, so they do not silently select a
proportional fallback font. Cancelling the dialog leaves the option unchanged.

## IME and clipboard

- IME uses IMM32 with the native IME composition and candidate windows.
  `WM_IME_STARTCOMPOSITION` positions both windows at the caret with
  `ImmSetCompositionWindow` and `ImmSetCandidateWindow`;
  start, unfinished composition, cancellation and end messages reach the
  default IME window so composition text and the candidate list are displayed.
  `WM_IME_COMPOSITION` consumes a successfully retrieved `GCS_RESULTSTR` once;
  any simultaneous new composition is forwarded without the result flags.
  Results that cannot be read use the default handler and `WM_IME_CHAR` path.
  `WM_CHAR` and `WM_IME_CHAR` decode UTF-16 surrogate pairs, including rare CJK
  characters and emoji; `WM_UNICHAR` also supports direct Unicode input.
- Register `*` is synchronised with the Windows clipboard (CF_UNICODETEXT),
  converting between LF and CRLF. Yanking to `*` updates the clipboard; an
  external clipboard change is loaded into `*` before the next key.
- In Insert mode, **Shift+Insert** inserts the clipboard text literally;
  **Ctrl+Insert** copies the visual selection (or the current line) to `*` and
  the clipboard.

## Tabs

The current GUI uses the `guioptions` **e** flag to control its text grid
tabline. The default is `egmrLtT`; use `:set guioptions-=e` to hide the tabline,
or `:set guioptions+=e` to show it when more than one tab page exists. Every
redraw applies the updated value and adjusts the available buffer rows.

`guioptions` is a character flag string: `+=` appends, `^=` prepends, and `-=`
removes the supplied substring (remove individual flags when their order is
irrelevant). Duplicates are removed, and illegal characters report E539 without
changing the previous value. Completion after `go+=` / `go^=` offers absent
flags; `go-=` offers currently enabled flags.

The `e`, `r` and `R` flags have a visual effect in this implementation.
Other recognised Vim flags are stored for script compatibility; native menus
(`m`), toolbars (`T`), left/horizontal scrollbars (`l`/`L`/`b`), automatic
selection and other flag behaviours are not implemented. This differs from Vim: Vim's `e` selects a native tab
control, and removing it can fall back to a text tabline. RustVim currently
has neither the native tab control nor a `showtabline` option.

## Overlay vertical scrollbars

Each scrollable pane has a slim, rounded, translucent scrollbar at its right
edge, enabled by `guioptions` `r` or `R` (the default contains `r`). The thumb
represents the visible fraction of the buffer, has a minimum grab size, widens
and brightens on hover/drag, and fades out after interaction. Scrollbars overlay
the grid, so showing them does not shift text or change the column count.
Short files that fit in the pane have no scrollbar.

- Drag the thumb to scroll, including beyond the window edge while captured.
- Click the track to jump to that position, with the thumb centred on the click.
- The mouse wheel scrolls the pane under the pointer, three lines per notch;
  partial wheel deltas accumulate instead of forcing a full step.
- Pointer scrolling preserves the edit cursor and active split, even when the
  cursor goes off screen. Keyboard input resumes following the active cursor.
- Geometry updates with pane resizing, tabline changes, font size and DPI.
  Positions use buffer line numbers; targets inside a closed fold snap to its
  header.

Use `:set go-=r go-=R` to hide the right scrollbars, or `:set go+=r` to enable
them. This implementation provides overlay vertical scrollbars; it does not
provide a minimap or pixel-based smooth scrolling.

## Emoji and ligatures

- Emoji use DirectWrite font fallback with Direct2D's
  `DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT`; they occupy two Unicode display cells
  and prefer `guifontwide` when configured.
- Text runs are shaped by DirectWrite as a unit. A font with programming
  ligatures (for example `Cascadia Code` or `Fira Code`) can be selected with
  `set guifont=Cascadia\ Code:h12`; standard ligatures such as `->` are
  applied within a style run.
- The simple cell model does not join ZWJ emoji sequences (for example
  family/skin-tone sequences) into one grapheme; individual emoji glyphs still
  render correctly.

### Which fonts are used

The renderer uses three sources:

1. `guifont` is the primary font and defines cell width (average Latin advance);
2. `guifontwide` is preferred for double-width characters and supplies coverage
   fallback for other characters missing from the primary font;
3. DirectWrite's system font fallback handles glyphs missing from the selected
   font (rare CJK, emoji, ...).

Font choice uses Unicode display width and glyph coverage, independent of
UTF-8 byte length. A wide character occupies two cells, and a Latin character one.

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

## Normal-mode split windows

`Ctrl-W s` / `Ctrl-W S` split horizontally and `Ctrl-W v` splits vertically.
Holding Ctrl for the second key also works (`Ctrl-W Ctrl-S`, `Ctrl-W Ctrl-V`).
Mixed splits work from either the original or the new pane.

Use `Ctrl-W h/j/k/l` or arrows to move between panes, and `Ctrl-W Ctrl-W` to
cycle. `Ctrl-W n` opens an empty buffer in a new pane. Counts can precede or
follow the prefix: `8 Ctrl-W s` and `Ctrl-W 8 s` create an eight-line pane;
`Ctrl-W 25 v` creates a 25-column pane. Counts on both sides multiply.

`Ctrl-W +` / `-` change height and `Ctrl-W >` / `<` change width; counts specify
the change. `Ctrl-W _` / `|` maximise height/width without a count, or set the
requested size with one. `Ctrl-W =` equalises pane sizes. `Ctrl-W c` closes the
current pane; `Ctrl-W q` can also quit the last unmodified window.

## Input

- `WM_CHAR` feeds printable characters; `WM_KEYDOWN` maps Enter, Backspace,
  Escape, Tab, arrows, Home/End, Insert/Delete, PageUp/PageDown, F1-F12 and
  Ctrl/Shift navigation.
- The mouse wheel scrolls the pane under the pointer independently of the edit
  cursor; dragging and track clicks use the overlay vertical scrollbars.
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
