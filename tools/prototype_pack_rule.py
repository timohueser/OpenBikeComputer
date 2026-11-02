#!/usr/bin/env python3
"""PROTOTYPE, never merges (#3069): objects, largest pack and largest block index of the pack
rule of #3069 (base levels 0, 7, 9, a 15 MiB limit, block index per base square), from the file
sizes of a release. `merge` merges the 46 TrailView snow layers into one.

    curl -s https://maps.openbikecomputer.com/releases/ID/release.json -o release.json
    python3 tools/prototype_pack_rule.py release.json bands|merge
"""
import json, collections, re, sys
f = json.load(open(sys.argv[1]))['files']
merge = sys.argv[2] == 'merge'
CAP = 15 * 1024 * 1024
tiles = collections.defaultdict(dict)   # layer -> {(z,x,y) of today's pack: bytes}
for p, v in f.items():
    m = re.match(r'maps/tiles/(.+)/(\d+)-(\d+)-(\d+)\.pmtiles$', p)
    if m:
        layer = 'trailview-snow' if merge and m[1].startswith('trailview-snow-') else m[1]
        k = (int(m[2]), int(m[3]), int(m[4])); tiles[layer][k] = tiles[layer].get(k, 0) + v['bytes']
def base(z): return 0 if z < 7 else 7 if z < 9 else 9
def end(b): return 6 if b == 0 else 8 if b == 7 else 99
today_blocks = set(); packs_today = 0
packs = []  # (layer, z, x, y, bytes)
for layer, t in tiles.items():
    for (z, x, y) in t:
        packs_today += 1; bz = max(z - 4, 0); today_blocks.add((layer, bz, x >> (z - bz), y >> (z - bz)))
    # today's packs are at most z11; group into base squares, split by size.
    def size(z, x, y, b):
        return sum(s for (zz, xx, yy), s in t.items() if base(zz) == b and zz >= z and (xx >> (zz - z), yy >> (zz - z)) == (x, y))
    def own(z, x, y, b):
        return sum(s for (zz, xx, yy), s in t.items() if base(zz) == b and zz == z and (xx, yy) == (x, y)) if z < 11 else size(z, x, y, b)
    def cut(z, x, y, b):
        s = size(z, x, y, b)
        if s == 0: return
        if s <= CAP or z >= min(end(b), 11): packs.append((layer, z, x, y, s)); return
        if own(z, x, y, b): packs.append((layer, z, x, y, own(z, x, y, b)))
        for dx in (0, 1):
            for dy in (0, 1): cut(z + 1, 2 * x + dx, 2 * y + dy, b)
    roots = {(b, x >> (z - b), y >> (z - b)) for (z, x, y) in t for b in [base(z)]}
    for b, x, y in roots: cut(b, x, y, b)
blocks = collections.Counter(); bbytes = collections.Counter()
for layer, z, x, y, s in packs:
    b = base(z); bz = max(b - 2, 0); key = (layer, bz, x >> (z - bz), y >> (z - bz))
    blocks[key] += 1; bbytes[key] += len(f'"{z}-{x}-{y}.pmtiles":') + 110
print(f"{'merged' if merge else '46 bands'}: today {packs_today} packs + {len(today_blocks)} block indexes = {packs_today + len(today_blocks)}")
print(f"  new: {len(packs)} packs + {len(blocks)} block indexes = {len(packs) + len(blocks)}; largest pack {max(p[4] for p in packs)/2**20:.1f} MiB; largest index {max(bbytes.values())/1024:.1f} KiB, {max(blocks.values())} entries")
