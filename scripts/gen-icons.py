#!/usr/bin/env python3
"""Generate the RustVim Windows icon set.

Requires Pillow (development-time only; the committed assets are what ship).
Outputs:
  assets/icon/rvim.ico        16..256 px, 32-bit DIB + PNG-compressed 256
  assets/icon/rvim-48.png
  assets/icon/rvim-256.png
  assets/icon/rvim-512.png
  assets/icon/rvim-1024.png

The 256px entry is PNG-compressed (Vista+); smaller entries are 32-bit DIBs
with an AND mask so they render in older shell surfaces too.
"""

from io import BytesIO
from pathlib import Path
import struct

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "icon"
OUT.mkdir(parents=True, exist_ok=True)

SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
SCALE = 4
BASE = 1024
BG = (13, 17, 23, 255)
BORDER = (48, 54, 61, 255)
GREEN = (63, 185, 80, 255)
CURSOR = (240, 246, 252, 255)


def render_master() -> Image.Image:
    size = BASE * SCALE
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    s = SCALE
    draw.rounded_rectangle(
        [32 * s, 32 * s, (BASE - 32) * s, (BASE - 32) * s],
        radius=220 * s,
        fill=BG,
        outline=BORDER,
        width=10 * s,
    )
    points = [(300 * s, 300 * s), (512 * s, 724 * s), (724 * s, 300 * s)]
    draw.line(points, fill=GREEN, width=132 * s, joint="curve")
    # Round the two upper tips (PIL's line caps are square).
    for x, y in (points[0], points[2]):
        r = 66 * s
        draw.ellipse([x - r, y - r, x + r, y + r], fill=GREEN)
    draw.rounded_rectangle(
        [650 * s, 720 * s, 840 * s, 830 * s],
        radius=24 * s,
        fill=CURSOR,
    )
    return img.resize((BASE, BASE), Image.Resampling.LANCZOS)


def dib(img: Image.Image) -> bytes:
    """32-bit bottom-up DIB (XOR bitmap) plus 1-bit AND mask."""
    w, h = img.size
    px = img.load()
    header = struct.pack(
        "<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0
    )
    xor = bytearray()
    for y in range(h - 1, -1, -1):
        for x in range(w):
            r, g, b, a = px[x, y]
            xor += bytes((b, g, r, a))
    row_bytes = ((w + 31) // 32) * 4
    and_mask = bytearray(row_bytes * h)
    for y in range(h):
        for x in range(w):
            if px[x, h - 1 - y][3] == 0:
                and_mask[y * row_bytes + (x // 8)] |= 0x80 >> (x % 8)
    return header + xor + and_mask


def png(img: Image.Image) -> bytes:
    buf = BytesIO()
    img.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def write_ico(master: Image.Image) -> None:
    entries = []
    for size in SIZES:
        image = master.resize((size, size), Image.Resampling.LANCZOS)
        data = png(image) if size == 256 else dib(image)
        entries.append((size, data))
    offset = 6 + 16 * len(entries)
    out = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    for size, data in entries:
        dim = 0 if size == 256 else size
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _size, data in entries:
        out += data
    (OUT / "rvim.ico").write_bytes(out)


def main() -> None:
    master = render_master()
    master.save(OUT / "rvim-1024.png", optimize=True)
    for size in (512, 256, 48):
        master.resize((size, size), Image.Resampling.LANCZOS).save(
            OUT / f"rvim-{size}.png", optimize=True
        )
    write_ico(master)
    print(f"wrote {OUT / 'rvim.ico'} ({len(SIZES)} sizes)")


if __name__ == "__main__":
    main()
