#!/usr/bin/env python3
"""Compare two land dumps — Rust `land_probe` vs oracle `dump_land.py` — by polygon
count, total vertices, and total area (shoelace, exterior minus holes). The two
differ in ring winding + sub-microdegree reproject/clip vertices (GEOS 3.14 vs
shapely 3.13), so the gate is **area agreement** (within `--tol`, relative), not
vertex identity. The polygon count can differ slightly because a clip MultiPolygon
counts as one face in the oracle's `len(...)` but is flattened both sides here;
counts are reported, area is the assertion.

Usage:  compare_land.py <rust.json> <oracle.json> [--tol 1e-4]
"""
import json
import sys
from pathlib import Path


def ring_area(r):
    a = 0.0
    n = len(r)
    for i in range(n):
        x1, y1 = r[i]
        x2, y2 = r[(i + 1) % n]
        a += x1 * y2 - x2 * y1
    return abs(a) / 2.0


def load(p):
    polys = json.loads(Path(p).read_text())["polygons"]
    nvert = sum(len(p["ext"]) + sum(len(h) for h in p["holes"]) for p in polys)
    area = sum(ring_area(p["ext"]) - sum(ring_area(h) for h in p["holes"]) for p in polys)
    return len(polys), nvert, area


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    tol = 1e-4
    if "--tol" in sys.argv:
        tol = float(sys.argv[sys.argv.index("--tol") + 1])
    na, va, aa = load(args[0])  # rust
    nb, vb, ab = load(args[1])  # oracle
    rel = abs(aa - ab) / ab if ab else (0.0 if aa == 0 else 1.0)
    print(f"  rust:   polys={na} verts={va} area={aa:.8e} deg^2")
    print(f"  oracle: polys={nb} verts={vb} area={ab:.8e} deg^2")
    print(f"  area rel diff = {rel:.3e} (tol {tol:.0e})")
    if rel <= tol:
        print("  OK — land area matches")
        sys.exit(0)
    print("  DIFF — land area mismatch")
    sys.exit(1)


if __name__ == "__main__":
    main()
