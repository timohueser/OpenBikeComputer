#!/usr/bin/env python3
"""Locate polygons that differ between two ingest dumps (oracle vs rust), so the
render-diff (`run_stage4.sh`) can aim a camera at each divergence and prove it's
render-equivalent. A "divergence" is a polygon whose canonical key (style + rings
up to rotation/winding, `compare_ingest.canon_ring`) appears a different number of
times on the two sides — i.e. the handful of broken relations osmium's AreaManager
and GEOS build_area/node repair into different vertex sets.

Prints one `lon_microdeg,lat_microdeg,zoom_mul` line per divergent polygon (deduped
to a grid so re-tessellations of the *same* feature on both sides collapse to one
camera target). The camera targets a **boundary extreme** of the feature (the
exterior vertex farthest from its centroid — a lobe tip, where assembly differences
concentrate), and `zoom_mul` is clamped so the view always lands in the **finest,
no-simplify LOD**. That isolates the relation-assembly difference from the coarse-
LOD GEOS-version simplify skew (the Stage-3 residual): a large forest must NOT be
framed whole at low zoom, or the render-diff would measure simplify skew, not
assembly. Empty output ⇒ multiset-identical, nothing to render-check.

Usage:  find_divergences.py <oracle.json> <rust.json> [--grid MICRODEG] [--finest-mpp M]
"""
import json
import sys
from collections import Counter
from pathlib import Path


def canon_ring(verts):
    pts = [tuple(p) for p in verts]
    if len(pts) >= 2 and pts[0] == pts[-1]:
        pts = pts[:-1]
    n = len(pts)
    if n == 0:
        return ()
    best = None
    for seq in (pts, pts[::-1]):
        m = min(seq)
        for i in range(n):
            if seq[i] == m:
                cand = tuple(seq[i:] + seq[:i])
                if best is None or cand < best:
                    best = cand
    return best


def key(feat):
    return (feat["style_id"], tuple(sorted(canon_ring(r) for r in feat["rings"])))


def bbox(feat):
    ext = feat["rings"][0]
    xs = [p[0] for p in ext]
    ys = [p[1] for p in ext]
    return min(xs), min(ys), max(xs), max(ys)


def boundary_tip(feat, cx, cy):
    """Exterior vertex farthest from the centroid — a lobe tip on the boundary,
    where assembly (and simplify) differences are most pronounced."""
    ext = feat["rings"][0]
    return max(ext, key=lambda p: (p[0] - cx) ** 2 + (p[1] - cy) ** 2)


def map_span(*dumps):
    """Largest dimension (microdeg) of the bbox over all features + coastlines —
    the obc-sim zoom-1 (whole-map) extent, used to scale per-feature zoom."""
    lo_x = lo_y = float("inf")
    hi_x = hi_y = float("-inf")
    for d in dumps:
        rings = [r for f in d["features"] for r in f["rings"]] + d.get("coastlines", [])
        for ring in rings:
            for x, y in ring:
                lo_x, hi_x = min(lo_x, x), max(hi_x, x)
                lo_y, hi_y = min(lo_y, y), max(hi_y, y)
    return max(hi_x - lo_x, hi_y - lo_y, 1)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    grid = 2000  # microdegrees (~0.2 km); collapse near-coincident divergences
    if "--grid" in sys.argv:
        grid = int(sys.argv[sys.argv.index("--grid") + 1])
    finest_mpp = 18.0  # smallest finite max_mpp in config = the no-simplify LOD
    if "--finest-mpp" in sys.argv:
        finest_mpp = float(sys.argv[sys.argv.index("--finest-mpp") + 1])
    oracle = json.loads(Path(args[0]).read_text())
    rust = json.loads(Path(args[1]).read_text())

    o_polys = [f for f in oracle["features"] if f["kind"] == "polygon"]
    r_polys = [f for f in rust["features"] if f["kind"] == "polygon"]
    o_keys = Counter(key(f) for f in o_polys)
    r_keys = Counter(key(f) for f in r_polys)
    span = map_span(oracle, rust)

    # Min zoom that lands the camera in the finest (no-simplify) LOD: at zoom Z the
    # view is span/Z wide over a 480px frame, so mpp ≈ (span_deg*111320)/(480*Z).
    # Require mpp <= finest_mpp with margin → Z >= base_mpp/(finest_mpp*0.8).
    base_mpp = (span / 1e6) * 111320.0 / 480.0
    min_finest_zoom = base_mpp / (finest_mpp * 0.8)

    # A polygon is "divergent" if its canonical key is surplus on its own side.
    # Keep the largest feature span seen per grid cell to frame the camera.
    targets = {}
    for polys, mine, theirs in ((o_polys, o_keys, r_keys), (r_polys, r_keys, o_keys)):
        for f in polys:
            k = key(f)
            if mine[k] > theirs.get(k, 0):
                minx, miny, maxx, maxy = bbox(f)
                cx, cy = (minx + maxx) // 2, (miny + maxy) // 2
                feat_span = max(maxx - minx, maxy - miny, 1)
                cell = (cx // grid, cy // grid)
                prev = targets.get(cell)
                if prev is None or feat_span > prev[2]:
                    tx, ty = boundary_tip(f, cx, cy)
                    targets[cell] = (tx, ty, feat_span)

    for _cell, (tx, ty, feat_span) in sorted(targets.items()):
        # Frame the feature at ~half the viewport (zoom = span/(2*feat_span)), but
        # never coarser than the finest LOD — so large forests get a finest-LOD
        # boundary tile, not a simplified whole-feature view.
        framing = span / (2.0 * feat_span)
        zoom = max(min_finest_zoom, min(framing, 4000.0))
        print(f"{tx},{ty},{zoom:.1f}")


if __name__ == "__main__":
    main()
