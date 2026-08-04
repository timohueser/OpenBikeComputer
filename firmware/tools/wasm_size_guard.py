#!/usr/bin/env python3
"""Bundle-size budgets for the hosted builder's wasm bridges.

Three modules, same argument. `apps/obc-web-convert` (#896) is what a visitor downloads the moment
they drop a route; `apps/obc-web-assemble` (#1034) is what turns downloaded map cells into a map. Both are
reached through a dynamic import. `apps/obc-skin-preview` (#1045) is the firmware reader + renderer
opened only by the skin editor. Each is its own chunk rather than part of the initial page, and a
silent size regression is a product regression. CI runs this right after each `wasm-pack build`,
on the very bytes it then hands to the frontend job.

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
# --- obc-web-assemble ------------------------------------------------------------------------
#
# Measured 2026-07-31 on the initial P4b artifact (wasm-pack 0.15.0 / wasm-opt -Oz): 434,476 B raw
# wasm + 20,474 B glue -> 184,441 B gzipped. Three times the other two, and it should be: this one
# links the whole OBCA assembly engine (`obcm-assemble`'s graft, POI merge, nav rewrite, shard
# planner and §4.8 verify pass) plus `obc-reader`'s decoder, SHA-256, and serde/serde_json for the
# schema and skin documents. It is a *program*, not a converter.
#
# The budget is set differently from the other two on purpose. Those are latency budgets — they
# guard the moment a visitor drops a file or hovers a preset card, where tens of KB are felt. This
# module is fetched when someone has already chosen to assemble a map they are about to download
# hundreds of MB of cells for, so 180 KB is not the cost that matters. What the budget is here for
# is the *structural* regression: linking `obc-pack` (libGEOS), a second renderer, or the whole app.
# Hence ~10 % headroom over the measurement, same as the others: enough that a toolchain bump never
# turns a green PR red, far too little to absorb another crate.
#
# Re-measured 2026-07-31 after the review round (the §4.8 progress/abort wrapper, the double-take
# refusal, the budget override, the warn-once console binding): 435,990 B raw + 24,452 B glue ->
# 186,601 B gzipped. +1,514 B raw / +2,160 B gzipped, which is the shape a fix round should have —
# error prose and a handful of branches, no new crate. Budgets unchanged (89 % / 91 %).
#
# Re-measured 2026-08-03 for EL4 (#1072, the terrain shard): 481,473 B raw + 27,118 B glue ->
# 205,343 B gzipped. +45,483 B raw / +18,742 B gzipped — the largest single jump this module has
# taken, and the one case where a *bigger* number is the right answer rather than a regression to
# hunt. Two crates joined the graph, both deliberately and both small: `obc-elevation`'s
# `TerrainReader` (the §4.8 read-back of the raster runs through the same parser the firmware does,
# not a second opinion about the bytes) and `obc-dem`'s `container::ShardWriter` behind
# `default-features = false` — which is the *whole point* of that feature gate, because the
# alternative was a second OBCT container writer living in the assembler. Neither brings a
# dependency of its own; the growth is object code for one format's reader and writer.
#
# What the budget still guards is unchanged: `obc-pack`/libGEOS, a renderer, or the app itself would
# each be an order of magnitude more than this. Budgets raised to keep the same ~10 % headroom over
# the new measurement (94 % / 92 %).
#
# Re-measured 2026-08-04 for #1116 phase D (the external merge: the scratch/extsort machinery, the
# hierarchical prune, the sort-merge id joins, the banded verify): 546,661 B raw -> 231,537 B
# gzipped at D3. Engine object code again — the sorts and the join walks are real passes with real
# code, and no new crate arrived (`cargo tree` diff is clean: the same dependency set as EL4).
# Budgets re-based to the same ~10 % headroom (91 % / 91 %), sized so the remaining phase-D stage
# (D4's streaming emission) fits without another bump while `obc-pack`/GEOS would still blow
# straight through.
# --- obc-skin-preview ------------------------------------------------------------------------
#
# Measured 2026-08-01 on #1045 (wasm-pack 0.15.0 / wasm-opt -Oz): 240,227 B raw wasm + 11,859 B
# glue -> 112,275 B gzipped. It intentionally links `obc-reader`, `obc-render`, and just enough of
# `obcm-assemble` to resolve and stamp a skin; it does not link obc-pack, GEOS, or the cell assembly
# driver. The module is lazy-loaded only when the editor opens, and its map/frame object is released
# when it closes.
# Budgets leave ~14 % headroom while still catching a second engine or accidental packer link.
BUDGETS = {
    "convert": {"gzipped": 62 * 1024, "raw_wasm": 112 * 1024},
    "assemble": {"gzipped": 248 * 1024, "raw_wasm": 588 * 1024},
    "preview": {"gzipped": 128 * 1024, "raw_wasm": 272 * 1024},
}

# What to *do* about an over-budget module, which is not the same advice for all three — and a guard
# that gives the wrong advice gets obeyed anyway. Convert is a latency budget: the number is
# what a visitor waits for, so "make it smaller" is the literal fix. Assemble is a structural guard
# on a module nobody is waiting on, so the fix is almost never "shrink it" — it is to find out what
# got linked in that should not have been.
ADVICE = {
    "convert": (
        "This is the moment a visitor drops a route, and it ships to every one of them:"
        " shrink it, or raise the budget in firmware/tools/wasm_size_guard.py with the reason in the PR body."
    ),
    "assemble": (
        "This budget is a structural guard, not a latency one — nobody waits on this module, so the question is"
        " not 'how do I make it smaller' but 'what got linked in'. Diff the dependency graph"
        " (`cargo tree -p obc-web-assemble --target wasm32-unknown-unknown`) against the base branch and look for a"
        " new crate: obc-pack (libGEOS), a renderer, or the app itself would each land in this range. If the growth"
        " really is the engine getting bigger for a good reason, raise the budget in"
        " firmware/tools/wasm_size_guard.py with the reason in the PR body."
    ),
    "preview": (
        "This module should contain the reader, renderer, and skin resolver only. Diff the dependency graph"
        " (`cargo tree -p obc-skin-preview --target wasm32-unknown-unknown`) and make sure obc-pack, GEOS,"
        " or the full assembly driver did not get linked. If renderer growth is intentional, raise the budget"
        " in firmware/tools/wasm_size_guard.py with the reason in the PR body."
    ),
}

#: Where each module's wasm-pack output lands in the frontend.
PKG_DIRS = {
    "convert": Path("builder/app/src/lib/convert/pkg"),
    "assemble": Path("builder/app/src/lib/assemble/pkg"),
    "preview": Path("builder/app/src/lib/skin/pkg"),
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
                f" {ADVICE[args.module]}"
            )
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
