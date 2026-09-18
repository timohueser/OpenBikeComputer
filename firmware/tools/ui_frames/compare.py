"""Count the pixels two rendered frames disagree about.

The simulator writes 8-bit RGB PNGs, so a reader for that one shape is all `--vs` needs, and it
keeps the sweep free of an image dependency.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

SIGNATURE = b"\x89PNG\r\n\x1a\n"


def _paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    return a if pa <= pb and pa <= pc else (b if pb <= pc else c)


def _unfilter(raw: bytes, stride: int, height: int) -> bytes:
    out = bytearray()
    previous = bytearray(stride)
    at = 0
    for _ in range(height):
        kind, line = raw[at], bytearray(raw[at + 1 : at + 1 + stride])
        at += 1 + stride
        if kind:
            for i in range(stride):
                a = line[i - 3] if i >= 3 else 0
                b = previous[i]
                c = previous[i - 3] if i >= 3 else 0
                if kind == 1:
                    line[i] = (line[i] + a) & 0xFF
                elif kind == 2:
                    line[i] = (line[i] + b) & 0xFF
                elif kind == 3:
                    line[i] = (line[i] + (a + b) // 2) & 0xFF
                elif kind == 4:
                    line[i] = (line[i] + _paeth(a, b, c)) & 0xFF
                else:
                    raise ValueError(f"unknown PNG filter {kind}")
        out += line
        previous = line
    return bytes(out)


def pixels(path: Path) -> tuple[int, int, bytes]:
    """``(width, height, RGB bytes)`` of one rendered frame."""
    data = Path(path).read_bytes()
    if data[:8] != SIGNATURE:
        raise ValueError(f"{path} is not a PNG")
    at, body, header = 8, bytearray(), None
    while at < len(data):
        length = int.from_bytes(data[at : at + 4], "big")
        kind, chunk = data[at + 4 : at + 8], data[at + 8 : at + 8 + length]
        at += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", chunk)
        elif kind == b"IDAT":
            body += chunk
        elif kind == b"IEND":
            break
    if header is None:
        raise ValueError(f"{path} holds no header")
    width, height, depth, colour, _, _, interlace = header
    if (depth, colour, interlace) != (8, 2, 0):
        raise ValueError(f"{path}: expected a plain 8-bit RGB PNG")
    return width, height, _unfilter(zlib.decompress(bytes(body)), width * 3, height)


def changed(first: Path, second: Path) -> tuple[int, int]:
    """``(changed pixels, total pixels)`` between two frames of the same size."""
    width, height, one = pixels(first)
    other_width, other_height, other = pixels(second)
    if (width, height) != (other_width, other_height):
        raise ValueError(f"{first} is {width}x{height}, {second} is {other_width}x{other_height}")
    count = sum(1 for at in range(0, len(one), 3) if one[at : at + 3] != other[at : at + 3])
    return count, width * height
