#!/usr/bin/env python3
"""PROTOTYPE, never merges (#3000 workstream 1): object count and largest pack of z9 packs
under a size cap, from the file sizes of a release. A cell over the cap splits into its 4 children.

    curl -s https://maps.openbikecomputer.com/releases/ID/release.json -o release.json
    python3 tools/prototype_pack_cap.py release.json
"""
import json, collections, re, statistics, sys
f = json.load(open(sys.argv[1]))['files']
packs = []  # (layer, z, x, y, bytes)
for p, v in f.items():
    m = re.match(r'maps/tiles/(.+)/(\d+)-(\d+)-(\d+)\.pmtiles$', p)
    if m:
        layer = 'trailview-snow' if m[1].startswith('trailview-snow-') else m[1]
        packs.append((layer, int(m[2]), int(m[3]), int(m[4]), v['bytes']))

def layout(cell, low):
    out = collections.Counter()
    for layer, z, x, y, b in packs:
        if z < low:
            out[(layer, 'low')] += b
        else:
            zz = min(z, cell)
            out[(layer, zz, x >> (z - zz), y >> (z - zz))] += b
    return out

for cell in (11, 10, 9):
    for low in (7, 9):
        o = layout(cell, low); s = sorted(o.values())
        print(f'cell z{cell} low<z{low}: {len(o)} objects, median {statistics.median(s)/1e6:.2f} MB, largest {s[-1]/1e6:.1f} MB')

print()
z9 = layout(9, 7)
big = sorted(((b, k) for k, b in z9.items()), reverse=True)[:12]
for b, k in big: print(f'  {b/1e6:5.1f} MB {k}')
# Bytes per (layer, z, x, y) of today's packs, z>=7.
own = {(l, z, x, y): b for l, z, x, y, b in packs if z >= 7}

def size(layer, z, x, y):
    """Bytes of all today's packs inside cell (z, x, y), at zoom z or deeper."""
    return sum(b for (l, zz, xx, yy), b in own.items() if l == layer and zz >= z and (xx >> (zz - z), yy >> (zz - z)) == (x, y))

def split(layer, z, x, y, cap):
    """Packs for one cell under `cap`: the cell itself, or its own tiles plus 4 split children."""
    total = size(layer, z, x, y)
    if total <= cap or z == 11:
        return [total]
    kids = [s for dx in (0, 1) for dy in (0, 1) for s in split(layer, z + 1, 2 * x + dx, 2 * y + dy, cap) if s]
    mine = own.get((layer, z, x, y), 0)
    return kids + ([mine] if mine else [])

cells = {k for k in z9 if k[1] != 'low' and k[1] == 9}
fixed = len(z9) - len(cells)
print(f'\n{fixed} objects outside z9 cells (low and z7/z8 packs)')
print(' cap MB  objects  +vs no cap  largest MB  worst cold s  over-cap z11')
base = None
for cap in (2, 4, 6, 8, 10, 12, 15, 20, 25, 30, 40, 60):
    out = [s for (l, z, x, y) in cells for s in split(l, z, x, y, cap * 1e6)]
    n = fixed + len(out); base = base or None
    print(f'{cap:>7} {n:>8} {"":>11} {max(out)/1e6:>10.1f} {0.25 + max(out)/20e6:>12.2f}  {sum(s > cap*1e6 for s in out):>5}')
