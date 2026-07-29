#!/usr/bin/env python3
"""Bundle-size budgets for the hosted builder's wasm bridges.

Two modules, same argument. `apps/obc-web-convert` (#896) is what a visitor downloads the moment
they drop a route; `apps/obc-web-preview` (#899) is what renders the preset cards. Both are
reached through a dynamic import, so each is its own chunk rather than part of the initial page —
and a silent size regression in either is a product regression: the thing stops feeling instant.
CI runs this right after each `wasm-pack build`, on the very bytes it then hands to the frontend
job.

What is measured, and why:

* **gzipped `.wasm` + `.js`** — the number that matters. Every static host serves these compressed
  (a CDN will do better still with brotli), so this is what a visitor actually pays. The JS glue is
  counted because it ships with the module and is not optional.
* **raw `.wasm`** — a second, looser gate so a change that compresses well cannot hide behind gzip.
  Raw size is also what the browser has to compile and keep resident.

The budgets are round numbers a comfortable margin above the measured artifact, not high-water
marks: they are meant to catch "a dependency crept in" (which moves this by tens of KB), not to
force a bump on every refactor. Raising one is a deliberate edit with a reason in the PR body.
"""

from __future__ import annotations

import argparse
import gzip
import sys
from pathlib import Path


# Budgets in bytes, per module. See the module docstring before changing any of them.
#
# --- obc-web-convert --------------------------------------------------------------------------
#
# Measured 2026-07-26 on the initial A2 artifact (wasm-pack 0.15.0 / wasm-bindgen 0.2.125 /
# wasm-opt -Oz, rustc stable 1.96): 84,108 B raw wasm + 11,293 B glue → 47,235 B gzipped. Of the
# raw wasm, ~60 KB is code (the GPX scanner, the decimator's geometry, the OBCR emitter, the track
# exporter, plus `f64` parsing, `core::fmt` and the allocator) and ~22 KB is data — the panic and
# format strings `std` brings, and this bridge's own error prose.
#
# Both budgets sit ~20 % above that. Chosen so a toolchain bump (rustc/wasm-bindgen move this by a
# few KB in either direction) never turns a green PR red, while anything structural — one more
# shared crate linked in, a serializer, a second format — blows straight through it and has to be
# argued for rather than absorbed.
#
# Re-measured 2026-07-29 while adding the waypoint read-back (`obc_convert_obcr_to_waypoints`,
# chart-room preview): 104,201 B raw + 14,219 B glue → 57,363 B gzipped. The route read-back
# directions added since the A2 measurement (`obcr_to_track`, then the waypoint table) pull in
# `RouteIndex`/`RouteReader`/`for_each_waypoint`, which had eaten the old headroom (101 KB raw at
# the previous commit — 99 % of the 100 KB budget before this addition's ~2.8 KB). Budgets
# re-based to ~10 % above the new measurement, same philosophy as before.
# --- obc-web-preview -------------------------------------------------------------------------
#
# Measured 2026-07-29 on the initial B2 artifact (same toolchain as above): 124,857 B raw wasm +
# 13,798 B glue -> 59,556 B gzipped. Bigger than the conversion bridge and unsurprisingly so: this
# one links the whole render path (`obc-render`'s painter, scanline fill and stroker, plus
# `obc-reader`'s OBCM decoder and quadtree walk) rather than a file converter. It stays in the
# same order of magnitude because it links no *app* — no screens, no replay, no planner.
#
# Same ~10 % headroom, same philosophy: a toolchain bump must not turn a green PR red, but linking
# `obc-app` or a second renderer has to be argued for.
BUDGETS = {
    "convert": {"gzipped": 62 * 1024, "raw_wasm": 112 * 1024},
    "preview": {"gzipped": 66 * 1024, "raw_wasm": 138 * 1024},
}

#: Where each module's wasm-pack output lands in the frontend.
PKG_DIRS = {
    "convert": Path("builder/app/src/lib/convert/pkg"),
    "preview": Path("builder/app/src/lib/preview/pkg"),
}


def gzipped_len(data: bytes) -> int:
    """Length after gzip -9 with no filename/mtime header (deterministic across runs)."""
    return len(gzip.compress(data, compresslevel=9, mtime=0))


def measure(pkg: Path) -> tuple[list[tuple[str, int, int]], int, int]:
    """Return per-file (name, raw, gzipped) rows plus the two totals the budgets gate."""
    wasm = sorted(pkg.glob("*_bg.wasm"))
    glue = sorted(p for p in pkg.glob("*.js") if not p.name.endswith(".d.ts"))
    if len(wasm) != 1:
        raise SystemExit(f"expected exactly one *_bg.wasm in {pkg}, found {[p.name for p in wasm]}")
    if not glue:
        raise SystemExit(f"no JS glue in {pkg} — was this built with `wasm-pack --target web`?")

    rows = []
    for path in wasm + glue:
        data = path.read_bytes()
        rows.append((path.name, len(data), gzipped_len(data)))
    raw_wasm = rows[0][1]
    total_gzipped = sum(row[2] for row in rows)
    return rows, raw_wasm, total_gzipped


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--module",
        choices=sorted(BUDGETS),
        default="convert",
        help="which wasm bridge to measure (default: convert)",
    )
    parser.add_argument(
        "--pkg",
        type=Path,
        default=None,
        help="the wasm-pack output directory (default: the module's checked-out location)",
    )
    args = parser.parse_args()

    pkg = args.pkg or PKG_DIRS[args.module]
    if not pkg.is_dir():
        raise SystemExit(f"{pkg} does not exist — run `wasm-pack build` first (see firmware/README.md)")

    rows, raw_wasm, total_gzipped = measure(pkg)
    width = max(len(name) for name, _, _ in rows)
    print(f"{'file'.ljust(width)}  {'raw':>9}  {'gzipped':>9}")
    for name, raw, gz in rows:
        print(f"{name.ljust(width)}  {raw:>9,}  {gz:>9,}")
    print(f"{'total'.ljust(width)}  {sum(r for _, r, _ in rows):>9,}  {total_gzipped:>9,}")

    failed = False
    budgets = BUDGETS[args.module]
    for label, measured, budget in (
        ("gzipped wasm + glue", total_gzipped, budgets["gzipped"]),
        ("raw wasm", raw_wasm, budgets["raw_wasm"]),
    ):
        pct = 100 * measured / budget
        status = "over" if measured > budget else "ok"
        print(f"{label}: {measured:,} B / {budget:,} B budget ({pct:.0f}%) — {status}")
        if measured > budget:
            print(
                f"::error::obc-web-{args.module} {label} is {measured:,} B, over the {budget:,} B budget."
                " This ships to every visitor: shrink it, or raise the budget in"
                " firmware/tools/wasm_size_guard.py with the reason in the PR body."
            )
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
