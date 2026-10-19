#!/usr/bin/env python3
"""Generate the Solaris launcher icon (stdlib only, no Pillow).

Pixel-art sun over a night horizon: chunky 16px cells on a 256x256 RGBA
canvas, blocky sun disc with warm glow, dark terrain silhouette, a few
blocky stars. Deterministic output.

Writes:
  launcher/assets/icon.png      256x256 RGBA
  launcher/assets/icon.ico      16/32/48/256 entries (PNG-compressed)
"""
import os
import struct
import zlib

SIZE = 256
CELL = 16  # pixel-art cell in px
N = SIZE // CELL  # 16 cells per side

# Palette.
SKY_TOP = (13, 17, 48)
SKY_MID = (24, 30, 74)
SKY_LOW = (58, 44, 96)
GLOW = (255, 150, 60)
SUN_OUTER = (255, 190, 70)
SUN_INNER = (255, 225, 140)
SUN_CORE = (255, 246, 205)
HILL_FAR = (26, 34, 58)
HILL_NEAR = (14, 20, 38)
STAR = (235, 240, 255)

SUN_C = (7.5, 8.5)  # sun centre in cell coords
SUN_R = 4.2


def lerp(a, b, t):
    return tuple(int(x + (y - x) * t) for x, y in zip(a, b))


def sky(row):
    t = row / (N - 1)
    if t < 0.5:
        return lerp(SKY_TOP, SKY_MID, t * 2)
    return lerp(SKY_MID, SKY_LOW, (t - 0.5) * 2)


STARS = {(2, 1), (5, 2), (12, 1), (14, 3), (1, 4), (10, 3), (13, 5), (4, 5)}


def hill_height(col):
    # Blocky two-layer horizon (cells from top).
    far = 11 + (1 if 3 <= col <= 9 else 0) - (1 if col in (0, 15) else 0)
    near = 13 + (1 if 5 <= col <= 11 else 0)
    return far, near


def cell_color(cx, cy):
    if (cx, cy) in STARS and cy < 7:
        return STAR
    dx, dy = cx - SUN_C[0], cy - SUN_C[1]
    d = (dx * dx + dy * dy) ** 0.5
    base = sky(cy)
    if d <= SUN_R + 1.2:
        # Warm glow bleeding into the sky.
        t = max(0.0, min(1.0, (SUN_R + 1.2 - d) / 1.2))
        base = lerp(base, GLOW, 0.55 * t)
    if d <= SUN_R:
        base = SUN_OUTER
    if d <= SUN_R - 1.0:
        base = SUN_INNER
    if d <= SUN_R - 2.2:
        base = SUN_CORE
    far, near = hill_height(cx)
    if cy >= far:
        base = HILL_FAR
    if cy >= near:
        base = HILL_NEAR
    return base


def render(size):
    cell = size // N
    px = bytearray()
    for y in range(size):
        cy = min(y // cell, N - 1)
        for x in range(size):
            cx = min(x // cell, N - 1)
            r, g, b = cell_color(cx, cy)
            px += bytes((r, g, b, 255))
    return bytes(px)


def png_bytes(size):
    raw = render(size)
    stride = size * 4
    scan = b"".join(b"\x00" + raw[i:i + stride] for i in range(0, len(raw), stride))
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(scan, 9)) + chunk(b"IEND", b""))


def write_png(path):
    with open(path, "wb") as f:
        f.write(png_bytes(SIZE))


def write_ico(path):
    images = [(s, png_bytes(s)) for s in (16, 32, 48, 256)]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    for size, data in images:
        w = 0 if size == 256 else size
        entries += struct.pack("<BBBBHHII", w, w, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    with open(path, "wb") as f:
        f.write(header + entries)
        for _, data in images:
            f.write(data)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    assets = os.path.join(root, "launcher", "assets")
    os.makedirs(assets, exist_ok=True)
    png_path = os.path.join(assets, "icon.png")
    ico_path = os.path.join(assets, "icon.ico")
    write_png(png_path)
    write_ico(ico_path)
    print(f"wrote {png_path} ({os.path.getsize(png_path)} bytes)")
    print(f"wrote {ico_path} ({os.path.getsize(ico_path)} bytes)")


if __name__ == "__main__":
    main()
