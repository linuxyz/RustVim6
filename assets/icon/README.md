# RustVim Windows icon

Source: [rvim.svg](rvim.svg). The generated assets are committed so a normal
build does not need Python or Pillow.

| File | Use |
|---|---|
| `rvim.ico` | Windows application/installer icon: **16, 20, 24, 32, 40, 48, 64, 96, 128, 256** px |
| `rvim-48.png` | Small documentation/UI preview |
| `rvim-256.png` | Documentation/Store preview |
| `rvim-512.png`, `rvim-1024.png` | Source-resolution previews |

The ICO stores 256 px as PNG (Windows Vista+) and 16-128 px as 32-bit
bottom-up DIBs with an AND mask, so Explorer, taskbar, Alt-Tab, file dialogs and
older shell surfaces all pick a native size instead of scaling a single bitmap.

## Regenerate

```powershell
python scripts/gen-icons.py
```

Requires Pillow (`pip install pillow`) for development only. The script is
deterministic and rewrites every generated file above.

## Build integration

- `bin/rvim/build.rs` compiles the ICO with the Windows SDK `rc.exe` and links
  it into `rvim.exe` (falls back to `windres` for GNU targets; a missing
  resource compiler is a warning, not a build failure).
- `packaging/rvim.nsi` uses the same ICO for the installer and uninstaller.
- Windows selects the closest size from the multi-image ICO for each DPI:
  16/20/24 px at 100-150%, 32/40/48 px at 150-300%, 64-256 px for large icons
  and high-DPI previews.
