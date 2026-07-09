#!/usr/bin/env python3
"""Convert a BDF bitmap font into an embedded-graphics `ImageRaw<BinaryColor>` glyph strip.

The on-device text path (`obc-render`) draws through embedded-graphics' `MonoFont`, a 1bpp
bitmap *strip*: glyphs laid out 16 per row, packed MSB-first with each strip row byte-aligned.
A glyph's slot in the strip is its index in the font's `glyph_mapping`, so all we have to do is
lay glyphs out in that same index order — no custom glyph table beyond the mapping string.
Strip width is `16 * cell_w`, always a multiple of 8, so the stride is exact and rows never
need padding.

Two charsets, selected by `--charset` (default `ascii`), each matching an
embedded-graphics `StrGlyphMapping` exactly so the strip and the mapping agree:

  * `ascii` — printable ASCII 0x20..=0x7F (96 glyphs, 6 rows); eg's `mapping::ASCII`.
  * `latin` — ASCII + Latin-1 Supplement (0xA0..=0xFF) + Latin Extended-A (0x100..=0x17F),
    320 glyphs over 20 rows. Covers umlauts/accents for European route & POI names
    (ä ö ü ß é è à č š ž ł ő ű …). Matches the custom `LATIN` mapping in `font_data.rs`;
    its ASCII glyphs occupy the first 6 rows unchanged, so an `ascii` strip is a prefix
    of the `latin` one (issue #489).

Unlike a font whose every glyph fills the cell, a general BDF positions each glyph by its
`BBX` offset relative to the baseline (`FONT_ASCENT` from the cell top). This compositor
honours those offsets, so it handles Terminus (per-glyph offsets) as well as Spleen
(full-cell glyphs). `--scale N` integer-scales the cell + bitmap N× (nearest-neighbour), to
reach sizes a font has no native cut for while keeping crisp pixel edges.

`--deslash-zero` replaces the `0` glyph with the (identical-outline, slash-free) capital
`O`, for fonts like Terminus that draw a slashed/dotted zero.

Usage:
    python3 convert_bdf.py SRC.bdf OUT.raw [--charset ascii|latin] [--scale N] [--deslash-zero]
"""
import sys

GLYPHS_PER_ROW = 16

# Character sets as ordered inclusive codepoint ranges. The strip index of a codepoint is its
# position when the ranges are walked in order — the exact scheme embedded-graphics'
# `StrGlyphMapping` uses (see `font_data.rs`), so strip layout and glyph_mapping stay in lockstep.
CHARSETS = {
    "ascii": [(0x20, 0x7F)],
    "latin": [(0x20, 0x7F), (0xA0, 0xFF), (0x100, 0x17F)],
}


def charset_index(ranges):
    """Map each codepoint in `ranges` to its consecutive strip index; return (index, count)."""
    index, n = {}, 0
    for lo, hi in ranges:
        for cp in range(lo, hi + 1):
            index[cp] = n
            n += 1
    return index, n


def parse_bdf(path, wanted):
    """Return (cell_w, cell_h, ascent, {codepoint: (bbx, rows)}) for codepoints in `wanted`.

    `bbx` is `(w, h, xoff, yoff)`; `rows[y]` a list of 0/1 of length `w`.
    """
    cell_w = cell_h = box_yoff = ascent = None
    glyphs = {}
    lines = open(path).read().splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith("FONTBOUNDINGBOX"):
            _, w, h, _x, y = line.split()
            cell_w, cell_h, box_yoff = int(w), int(h), int(y)
        elif line.startswith("FONT_ASCENT"):
            ascent = int(line.split()[1])
        elif line.startswith("STARTCHAR"):
            enc = bbx = None
            i += 1
            while i < len(lines) and not lines[i].startswith("BITMAP"):
                if lines[i].startswith("ENCODING"):
                    enc = int(lines[i].split()[1])
                elif lines[i].startswith("BBX"):
                    _, bw, bh, bx, by = lines[i].split()
                    bbx = (int(bw), int(bh), int(bx), int(by))
                i += 1
            bw, bh, _bx, _by = bbx
            rows = []
            for r in range(bh):
                hexstr = lines[i + 1 + r].strip()
                val = int(hexstr, 16)
                nbits = len(hexstr) * 4  # hex is byte-padded; MSB is the left pixel
                rows.append([(val >> (nbits - 1 - x)) & 1 for x in range(bw)])
            i += 1 + bh
            if enc in wanted:
                glyphs[enc] = (bbx, rows)
        i += 1
    if ascent is None:  # fall back to the bounding box: top of box above the baseline
        ascent = cell_h + box_yoff
    return cell_w, cell_h, ascent, glyphs


def cell_grid(cell_w, cell_h, ascent, bbx, rows):
    """Composite one glyph's BBX bitmap into a `cell_h × cell_w` 0/1 grid.

    The baseline sits `ascent` px below the cell top; the glyph's bottom-left is `yoff`
    above the baseline and `xoff` from the left, so its top row lands at
    `ascent - (yoff + bh)`. Pixels outside the cell are clipped.
    """
    bw, bh, xoff, yoff = bbx
    top = ascent - (yoff + bh)
    grid = [[0] * cell_w for _ in range(cell_h)]
    for ry, row in enumerate(rows):
        cy = top + ry
        if 0 <= cy < cell_h:
            for rx, bit in enumerate(row):
                cx = xoff + rx
                if bit and 0 <= cx < cell_w:
                    grid[cy][cx] = 1
    return grid


def scale_grid(grid, s):
    """Integer nearest-neighbour scale a 0/1 grid by `s` (each pixel → s×s block)."""
    if s == 1:
        return grid
    return [[grid[y // s][x // s] for x in range(len(grid[0]) * s)] for y in range(len(grid) * s)]


def build_strip(cell_w, cell_h, ascent, glyphs, index, count, scale):
    rows_of_glyphs = (count + GLYPHS_PER_ROW - 1) // GLYPHS_PER_ROW
    cw, ch = cell_w * scale, cell_h * scale
    strip_w, strip_h = GLYPHS_PER_ROW * cw, rows_of_glyphs * ch
    assert strip_w % 8 == 0, f"strip width {strip_w} must be a byte multiple"
    grid = [[0] * strip_w for _ in range(strip_h)]
    for code, (bbx, rows) in glyphs.items():
        g = scale_grid(cell_grid(cell_w, cell_h, ascent, bbx, rows), scale)
        gx0 = (index[code] % GLYPHS_PER_ROW) * cw
        gy0 = (index[code] // GLYPHS_PER_ROW) * ch
        for y, gr in enumerate(g):
            for x, bit in enumerate(gr):
                if bit:
                    grid[gy0 + y][gx0 + x] = 1
    stride = strip_w // 8
    out = bytearray(stride * strip_h)
    for y in range(strip_h):
        for x in range(strip_w):
            if grid[y][x]:
                out[y * stride + (x >> 3)] |= 0x80 >> (x & 7)
    return bytes(out), (cw, ch), (strip_w, strip_h)


def cap_height(cell_w, cell_h, ascent, glyphs):
    """Ink height of 'A' in the composited cell — the cap height, for the mm annotation."""
    if ord("A") not in glyphs:
        return 0
    g = cell_grid(cell_w, cell_h, ascent, *glyphs[ord("A")])
    inked = [y for y, r in enumerate(g) if any(r)]
    return (inked[-1] - inked[0] + 1) if inked else 0


def main():
    args = sys.argv[1:]
    scale = 1
    if "--scale" in args:
        k = args.index("--scale")
        scale = int(args[k + 1])
        args = args[:k] + args[k + 2 :]
    charset = "ascii"
    if "--charset" in args:
        k = args.index("--charset")
        charset = args[k + 1]
        args = args[:k] + args[k + 2 :]
    deslash = "--deslash-zero" in args
    args = [a for a in args if a != "--deslash-zero"]
    if len(args) != 2 or charset not in CHARSETS:
        sys.exit(__doc__)
    src, dst = args
    index, count = charset_index(CHARSETS[charset])
    cell_w, cell_h, ascent, glyphs = parse_bdf(src, set(index))
    if deslash and ord("O") in glyphs:
        glyphs[ord("0")] = glyphs[ord("O")]  # slash-free zero = the capital-O ring
    data, (cw, ch), (sw, sh) = build_strip(cell_w, cell_h, ascent, glyphs, index, count, scale)
    open(dst, "wb").write(data)
    cap = cap_height(cell_w, cell_h, ascent, glyphs) * scale
    print(f"{dst}: {charset} {len(glyphs)}/{count} glyphs  cell {cw}x{ch} (x{scale})  "
          f"strip {sw}x{sh}  {len(data)}B  cap={cap}px ({cap / 7.39:.2f}mm)  ascent={ascent * scale}")


if __name__ == "__main__":
    main()
