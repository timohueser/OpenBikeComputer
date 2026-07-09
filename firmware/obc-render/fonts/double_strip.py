#!/usr/bin/env python3
"""Integer-double a converted Terminus strip (the `.raw` produced by `convert_bdf.py`).

Terminus' largest native cut is 16×32; the Home clock wants a bigger one. Rather than
scale from the BDF, this nearest-neighbour-doubles an already-converted strip: each glyph
pixel becomes a 2×2 block, so the layout (16 glyphs/row, 1bpp MSB-first) is preserved and
the cell doubles (e.g. 16×32 → 32×64). The chunky doubled edges read as deliberate at the
oversized clock size.

    python3 double_strip.py ter_u32b.raw ter_u64b.raw 16 32 [ROWS]

The clock only draws digits and the colon, so only the first `ROWS` glyph-rows of the source
are doubled (default 6 = printable ASCII 0x20..0x7F). This means a `latin`-charset source (20
rows) yields the same ASCII-only Huge strip as an `ascii`-charset one — its ASCII glyphs sit in
those first 6 rows unchanged — so the doubled font stays small and the `Huge` tier keeps eg's
`mapping::ASCII` (issue #489).

Args: SRC.raw DST.raw CELL_W CELL_H [ROWS]  (the source cell size; the strip is 16*CELL_W wide).
"""
import sys


def main():
    src_path, dst_path, cw, ch = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
    rows = int(sys.argv[5]) if len(sys.argv) > 5 else 6
    sw, sh = 16 * cw, rows * ch  # 16 glyphs/row; double only the first `rows` glyph-rows
    src = open(src_path, "rb").read()
    sstride = (sw + 7) // 8
    assert len(src) >= sstride * sh, (len(src), sstride * sh)

    def get(x, y):
        return (src[y * sstride + (x >> 3)] >> (7 - (x & 7))) & 1

    dw, dh = sw * 2, sh * 2
    dstride = (dw + 7) // 8
    out = bytearray(dstride * dh)
    for y in range(sh):
        for x in range(sw):
            if get(x, y):
                for dy in range(2):
                    for dx in range(2):
                        px, py = x * 2 + dx, y * 2 + dy
                        out[py * dstride + (px >> 3)] |= 1 << (7 - (px & 7))
    open(dst_path, "wb").write(bytes(out))
    print(f"{dst_path}: cell {cw * 2}x{ch * 2}  strip {dw}x{dh}  {len(out)}B")


if __name__ == "__main__":
    main()
