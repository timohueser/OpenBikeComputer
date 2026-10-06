#!/usr/bin/env python3
"""Gate the production builder core on raw WASM and gzipped WASM plus glue.

The test-device variant is measured but does not ship. Raising a cap needs a reason in the PR.
"""

from __future__ import annotations

GOVERNS = ['builder/wasm/**', 'host/obcm-assemble/**']
RULE = 'A wasm bridge stays inside its recorded gzip and raw size budgets.'

import argparse
import gzip
import sys
from pathlib import Path


# Preserve the sum of the conversion, assembly and preview caps. Consolidation does not
# increase the permitted production download or raw WASM size.
BUDGETS = {
    "builder": {"gzipped": (62 + 352 + 128) * 1024, "raw_wasm": (112 + 944 + 272) * 1024},
}

ADVICE = {
    "builder": (
        "Check the production dependency graph with cargo tree -p obc-builder-bridge"
        " --target wasm32-unknown-unknown. GEOS, obc-pack and the test device must stay out."
    ),
}

PKG_DIRS = {
    "builder": Path("builder/app/src/lib/core/pkg"),
    "flat-device": Path("builder/app/test-support/flat-device/pkg"),
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
    for label, measured, budget in (
        ("gzipped wasm + glue", total_gzipped, budgets["gzipped"]),
        ("raw wasm", raw_wasm, budgets["raw_wasm"]),
    ):
        pct = 100 * measured / budget
        status = "over" if measured > budget else "ok"
        print(f"{label}: {measured:,} B / {budget:,} B budget ({pct:.0f}%) — {status}")
        if measured > budget:
            print(
                f"::error::obc-{args.module}-bridge {label} is {measured:,} B, over the {budget:,} B budget."
                f" {ADVICE[args.module]}"
            )
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
