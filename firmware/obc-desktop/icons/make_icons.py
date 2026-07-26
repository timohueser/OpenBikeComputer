#!/usr/bin/env python3
"""Generate the app icons from one description, with no image library.

The mark is the field-guide signature the docs site and the builder already use:
parchment ground, forest contour lines. Drawn here rather than checked in as an
opaque blob so a colour change is a one-line edit and every size stays in step —
`python3 firmware/obc-desktop/icons/make_icons.py` rewrites all of them.

This is a *placeholder mark*, deliberately simple. D3 (#908) owns what an
installed app looks like on a dock and in a store listing; this exists because
`tauri::generate_context!` will not compile without an icon, and shipping a
default Tauri logo would be worse than shipping our own colours.

stdlib only (zlib + struct): the repo's Python is stdlib-only for docs, and one
icon script is not a reason to grow a dependency.
"""
import math
import os
import struct
import zlib

PARCHMENT = (0xEC, 0xE8, 0xCF)
FOREST = (0x3C, 0x6B, 0x39)
FOREST_DEEP = (0x2C, 0x52, 0x30)

# Contour lines as (y at x=0, amplitude, wavelength) in unit coordinates.
CONTOURS = [(0.24, 0.055, 1.05), (0.44, 0.048, 0.95), (0.64, 0.052, 1.15), (0.82, 0.042, 0.9)]
LINE_HALF_WIDTH = 0.021  # unit radius of a contour stroke
CORNER = 0.20  # rounded-corner radius, unit


def sample(u, v):
    """Colour at unit (u, v), supersampled by the caller."""
    # Outside the rounded square: transparent.
    dx = max(abs(u - 0.5) - (0.5 - CORNER), 0.0)
    dy = max(abs(v - 0.5) - (0.5 - CORNER), 0.0)
    if math.hypot(dx, dy) > CORNER:
        return (0, 0, 0, 0)
    for i, (y0, amp, wave) in enumerate(CONTOURS):
        y = y0 + amp * math.sin((u / wave) * 2 * math.pi + i * 1.7)
        if abs(v - y) < LINE_HALF_WIDTH:
            return (*(FOREST if i % 2 else FOREST_DEEP), 255)
    return (*PARCHMENT, 255)


def render(size, ss=3):
    """RGBA rows, supersampled ss×ss per pixel."""
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            r = g = b = a = 0
            for sy in range(ss):
                for sx in range(ss):
                    cr, cg, cb, ca = sample((px + (sx + 0.5) / ss) / size, (py + (sy + 0.5) / ss) / size)
                    r += cr * ca
                    g += cg * ca
                    b += cb * ca
                    a += ca
            n = ss * ss
            row += bytes((r // a if a else 0, g // a if a else 0, b // a if a else 0, a // n))
        rows.append(bytes(row))
    return rows


def write_png(path, size):
    raw = b"".join(b"\x00" + row for row in render(size))

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)
    print(f"{path}: {size}x{size}, {len(png)} bytes")


def write_ico(path, sizes=(16, 32, 48, 256)):
    """A PNG-in-ICO container — what Windows has read since Vista, and what
    tauri.conf.json's `icon` list wants for the MSI/NSIS bundle."""
    images = []
    for size in sizes:
        raw = b"".join(b"\x00" + row for row in render(size))

        def chunk(tag, data):
            body = tag + data
            return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

        png = b"\x89PNG\r\n\x1a\n"
        png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        png += chunk(b"IDAT", zlib.compress(raw, 9))
        png += chunk(b"IEND", b"")
        images.append((size, png))

    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries, blobs = b"", b""
    for size, png in images:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
        blobs += png
    with open(path, "wb") as f:
        f.write(header + entries + blobs)
    print(f"{path}: {[s for s, _ in images]}, {len(header + entries + blobs)} bytes")


if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    # The names tauri.conf.json lists, plus the two the macOS/Windows bundlers
    # look for by convention.
    for name, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)]:
        write_png(os.path.join(here, name), size)
    write_ico(os.path.join(here, "icon.ico"))
