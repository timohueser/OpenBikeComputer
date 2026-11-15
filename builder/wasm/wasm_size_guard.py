#!/usr/bin/env python3
"""Gate each production wasm bridge on raw WASM and gzipped WASM plus glue.

The test-device variant is measured but does not ship. Raising a cap needs a reason in the PR.
The ground-plan budget is its recorded CI size plus the owner's margin; both parts print.
"""

from __future__ import annotations

GOVERNS = ['builder/wasm/**', 'host/obcm-assemble/**', 'builder/ground-plan-wasm/**', 'host/obc-ground-plan/**']
RULE = 'A wasm bridge stays inside its recorded gzip and raw size budgets.'

import argparse
import gzip
import sys
from pathlib import Path


# Measured in CI on the build with the outline's turns, the levels that the tiles carry, the faces' tile-line
# vertices, the decks' ceilings and end faces, the shared water level and curve of the feature bake, line water
# and its culverts with their mouths, the classifier's ford rule, and the storeys over the ground with their open
# junction ends, their ends on the stubs' points, what each deck is made of, the junctions that lie on the
# ground, and a building passage of an older bake on the ground (the crate's feature `storeys` on, stable 1.100.0);
# then the sidewalks' slabs with their ends, rings and kerbs: that CI size plus their growth in one local build
# against its base (26,567 B gzipped, 60,228 B raw), until CI records the build's own size.
# The owner allows about 60 kB of code growth for the crossing block, stated in its pull request and recorded here.
GROUND_PLAN_MEASURED = {"gzipped": 834_666, "raw_wasm": 2_151_164}
# The owner's margin: compiler and dependency updates move the size by a few kilobytes and must not fail
# unrelated pull requests; real growth past the margin still fails. Raw gets the same fraction as gzipped.
# A pull request that changes the bridge's code records its own CI size here; the margin is only for toolchain
# and dependency noise.
GROUND_PLAN_MARGIN_GZIPPED = 8 * 1024
GROUND_PLAN_MARGIN = {
    "gzipped": GROUND_PLAN_MARGIN_GZIPPED,
    "raw_wasm": round(GROUND_PLAN_MARGIN_GZIPPED * GROUND_PLAN_MEASURED["raw_wasm"] / GROUND_PLAN_MEASURED["gzipped"]),
}

BUDGETS = {
    # Preserve the sum of the conversion, assembly and preview caps. Consolidation does not
    # increase the permitted production download or raw WASM size.
    "builder": {"gzipped": (62 + 352 + 128) * 1024, "raw_wasm": (112 + 944 + 272) * 1024},
    "ground-plan": {key: GROUND_PLAN_MEASURED[key] + GROUND_PLAN_MARGIN[key] for key in GROUND_PLAN_MEASURED},
}
# A budget made of a measured size and a margin prints both, so a reviewer sees growth inside the margin.
BUDGET_PARTS = {"ground-plan": (GROUND_PLAN_MEASURED, GROUND_PLAN_MARGIN)}

ADVICE = {
    "builder": (
        "Check the production dependency graph with cargo tree -p obc-builder-bridge"
        " --target wasm32-unknown-unknown. GEOS, obc-pack and the test device must stay out."
    ),
    "ground-plan": "Check the dependency graph with cargo tree -p obc-ground-plan-bridge --target wasm32-unknown-unknown.",
}

PKG_DIRS = {
    "builder": Path("builder/web/src/lib/core/pkg"),
    "flat-device": Path("builder/web/test-support/flat-device/pkg"),
    "ground-plan": Path("builder/web/src/lib/trailview/plan/pkg"),
}
UNBUDGETED = {"flat-device"}


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
        choices=sorted(PKG_DIRS),
        default="builder",
        help="which wasm bridge to measure (default: builder)",
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
        raise SystemExit(f"{pkg} does not exist — run `wasm-pack build` first (see builder/wasm/README.md)")

    rows, raw_wasm, total_gzipped = measure(pkg)
    width = max(len(name) for name, _, _ in rows)
    print(f"{'file'.ljust(width)}  {'raw':>9}  {'gzipped':>9}")
    for name, raw, gz in rows:
        print(f"{name.ljust(width)}  {raw:>9,}  {gz:>9,}")
    print(f"{'total'.ljust(width)}  {sum(r for _, r, _ in rows):>9,}  {total_gzipped:>9,}")

    if args.module in UNBUDGETED:
        print(f"{args.module}: no budget - a test-only module, measured for information.")
        return 0

    failed = False
    budgets = BUDGETS[args.module]
    parts = BUDGET_PARTS.get(args.module)
    for key, label, measured in (
        ("gzipped", "gzipped wasm + glue", total_gzipped),
        ("raw_wasm", "raw wasm", raw_wasm),
    ):
        budget = budgets[key]
        pct = 100 * measured / budget
        status = "over" if measured > budget else "ok"
        print(f"{label}: {measured:,} B / {budget:,} B budget ({pct:.0f}%) — {status}")
        if parts:
            recorded, margin = parts
            print(
                f"  budget = {recorded[key]:,} B recorded + {margin[key]:,} B margin;"
                f" {measured - recorded[key]:+,} B against the recorded size"
            )
        if measured > budget:
            print(
                f"::error::obc-{args.module}-bridge {label} is {measured:,} B, over the {budget:,} B budget."
                f" {ADVICE[args.module]}"
            )
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
