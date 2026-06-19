#!/usr/bin/env python3
"""Compare the Stage-3 expected ingest set (`dump_ingest.py`, the oracle) to the
Rust ingest (`ingest_dump` bin) as a multiset of `(style_id, kind, vertices)`.

Lines compare **exactly** — both sides walk the way's nodes in the same order, so
the microdegree vertex lists are identical. Polygons are canonicalized up to ring
**rotation + reflection** first: osmium normalizes a closed way's ring start
vertex and winding, while the Rust port keeps the raw way order, so geometrically
identical polygons would otherwise look different (handover §3.4 / §6).

Exits non-zero on any multiset difference.

Usage:  compare_ingest.py <oracle.json> <rust.json> [--max-examples N]
"""
import json
import sys
from collections import Counter
from pathlib import Path


def canon_ring(verts):
    """Canonical form of a closed ring, invariant to start vertex + winding:
    strip the closing duplicate, then take the lexicographically-smallest
    sequence over all rotations of the ring and its reversal."""
    pts = [tuple(p) for p in verts]
    if len(pts) >= 2 and pts[0] == pts[-1]:
        pts = pts[:-1]
    n = len(pts)
    if n == 0:
        return ()
    best = None
    for seq in (pts, pts[::-1]):
        m = min(seq)
        for i in range(n):  # only rotations starting at an occurrence of the min vertex
            if seq[i] == m:
                cand = tuple(seq[i:] + seq[:i])
                if best is None or cand < best:
                    best = cand
    return best


def feature_key(feat):
    if feat["kind"] == "polygon":
        rings = tuple(sorted(canon_ring(r) for r in feat["rings"]))
        return ("polygon", feat["style_id"], rings)
    # Line: exact vertex order matters (and matches both sides).
    pts = tuple(tuple(p) for p in (feat["rings"][0] if feat["rings"] else []))
    return ("line", feat["style_id"], pts)


def load(path):
    d = json.loads(Path(path).read_text())
    feats = Counter(feature_key(f) for f in d["features"])
    # Coastlines are lines; compare exactly.
    coasts = Counter(tuple(tuple(p) for p in c) for c in d["coastlines"])
    return feats, coasts


def report(name, a, b, max_examples):
    """a = oracle (expected), b = rust (candidate). Returns True if equal."""
    only_a = a - b  # in oracle, missing/short in rust
    only_b = b - a  # extra in rust
    na, nb = sum(a.values()), sum(b.values())
    if not only_a and not only_b:
        print(f"  MATCH {name}: {na} == {nb}")
        return True
    print(f"  DIFF  {name}: oracle={na} rust={nb}; only-in-oracle={sum(only_a.values())} only-in-rust={sum(only_b.values())}")
    for label, c in (("only-in-oracle", only_a), ("only-in-rust", only_b)):
        shown = 0
        for k, cnt in c.items():
            if shown >= max_examples:
                break
            kind, style = k[0], k[1]
            npts = len(k[2]) if kind == "line" else sum(len(r) for r in k[2])
            print(f"    {label} x{cnt}: kind={kind} style={style} pts={npts}")
            shown += 1
    return False


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    max_examples = 5
    if "--max-examples" in sys.argv:
        max_examples = int(sys.argv[sys.argv.index("--max-examples") + 1])
    oracle_path, rust_path = args[0], args[1]

    a_feats, a_coasts = load(oracle_path)
    b_feats, b_coasts = load(rust_path)

    ok = True
    ok &= report("features", a_feats, b_feats, max_examples)
    ok &= report("coastlines", a_coasts, b_coasts, max_examples)

    # Machine-readable residual, split by kind, so a caller (run_stage4_ingest.sh)
    # can distinguish a benign relation-assembly re-tessellation (balanced polygon
    # residual, lines exact — render-verified by run_stage4.sh) from a real bug.
    def split(counter):
        line = sum(c for k, c in counter.items() if k[0] == "line")
        poly = sum(c for k, c in counter.items() if k[0] == "polygon")
        return line, poly

    fl, fp = split(a_feats - b_feats)
    bl, bp = split(b_feats - a_feats)
    cl = sum((a_coasts - b_coasts).values()) + sum((b_coasts - a_coasts).values())
    print(
        f"SUMMARY line_only_oracle={fl} line_only_rust={bl} "
        f"poly_only_oracle={fp} poly_only_rust={bp} coast_diff={cl}"
    )

    if ok:
        print("OK — ingest multiset identical")
        sys.exit(0)
    print("FAILED — see diffs above")
    sys.exit(1)


if __name__ == "__main__":
    main()
