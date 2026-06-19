#!/usr/bin/env python3
"""Pixel-diff two PNGs rendered by `obc-sim --png` (handover §6.3, the render-diff
backstop). Where relation assembly diverges in vertex *set* (osmium's AreaManager
vs GEOS build_area/node repair a broken relation differently), the multiset gate
can't reconcile them — but the filled areas are the same, so the renders must be
pixel-equivalent. This asserts that.

Reports the fraction of differing pixels and the max per-channel difference, and
exits non-zero if the differing fraction exceeds --threshold (default 0: exact).

Usage:  compare_png.py <a.png> <b.png> [--threshold FRAC] [--diff OUT.png]
"""
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageChops


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    threshold = 0.0
    if "--threshold" in sys.argv:
        threshold = float(sys.argv[sys.argv.index("--threshold") + 1])
    diff_out = None
    if "--diff" in sys.argv:
        diff_out = sys.argv[sys.argv.index("--diff") + 1]
    a_path, b_path = args[0], args[1]

    a = Image.open(a_path).convert("RGB")
    b = Image.open(b_path).convert("RGB")
    if a.size != b.size:
        print(f"SIZE MISMATCH: {a_path}={a.size} {b_path}={b.size}")
        sys.exit(1)

    diff = ImageChops.difference(a, b)
    bbox = diff.getbbox()
    total = a.size[0] * a.size[1]
    if bbox is None:
        print(f"  IDENTICAL {Path(a_path).name} == {Path(b_path).name} ({a.size[0]}x{a.size[1]})")
        sys.exit(0)

    # Count pixels that differ at all, and the worst channel delta.
    arr = np.asarray(diff)
    max_delta = int(arr.max())
    differing = int(np.count_nonzero(arr.any(axis=2)))
    frac = differing / total

    if diff_out:
        diff.save(diff_out)
    status = "OK   " if frac <= threshold else "DIFF "
    print(
        f"  {status} {Path(a_path).name} vs {Path(b_path).name}: "
        f"{differing}/{total} px differ ({frac*100:.4f}%), max channel delta {max_delta}"
        + (f", diff -> {diff_out}" if diff_out else "")
    )
    sys.exit(0 if frac <= threshold else 1)


if __name__ == "__main__":
    main()
