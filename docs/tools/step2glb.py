#!/usr/bin/env python3
"""STEP -> GLB converter for the blog's ```model viewer.

    python3 docs/tools/step2glb.py part.step docs/content/blog/<slug>/part.glb

Uses `cascadio` (a thin OpenCascade binding with prebuilt wheels):

    pip install cascadio

This is an *authoring* tool, run once per model on the host — the committed .glb is
what the site serves, so the site build itself stays stdlib-only. Commit the .step
next to it if you want the post's "Download STEP" button.

Tolerances: --linear (default 0.05, model units — smaller = smoother + bigger file)
and --angular (default 0.4 rad) control the tessellation.
"""

import argparse
import sys
from pathlib import Path


def main():
    ap = argparse.ArgumentParser(description="Convert a STEP file to GLB for the blog viewer.")
    ap.add_argument("step", help="input .step / .stp file")
    ap.add_argument("glb", help="output .glb path (put it next to the post's index.md)")
    ap.add_argument("--linear", type=float, default=0.05, help="linear tessellation tolerance")
    ap.add_argument("--angular", type=float, default=0.4, help="angular tolerance (radians)")
    args = ap.parse_args()

    try:
        import cascadio
    except ImportError:
        sys.exit(
            "step2glb: the 'cascadio' package is missing.\n"
            "Install it with:  pip install cascadio\n"
            "(prebuilt wheels; no system OpenCascade needed)"
        )

    src, dst = Path(args.step), Path(args.glb)
    if not src.exists():
        sys.exit("step2glb: no such file: %s" % src)
    dst.parent.mkdir(parents=True, exist_ok=True)

    cascadio.step_to_glb(str(src), str(dst),
                         tol_linear=args.linear, tol_angular=args.angular)

    size = dst.stat().st_size
    print("wrote %s (%.1f KB)" % (dst, size / 1024))
    if size > 4 * 1024 * 1024:
        print("note: >4 MB — consider a coarser --linear tolerance for the web")


if __name__ == "__main__":
    main()
