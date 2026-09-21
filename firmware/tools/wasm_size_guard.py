#!/usr/bin/env python3
"""Bundle-size budgets for the hosted builder's wasm bridges.

Three modules are budgeted and one is measured and never gated. CI runs this right after each
`wasm-pack build`, on the bytes it then hands to the frontend job.

Each module is gated on its gzipped `.wasm` plus `.js`, which is what a visitor pays, and on its raw
`.wasm`, so a change that compresses well cannot hide behind gzip. The budgets sit a margin above
the measured artifact, so they catch a dependency creeping in rather than force a bump on every
refactor. Raising one needs a reason in the pull request body.
"""

from __future__ import annotations

GOVERNS = ['apps/obc-web-convert/**', 'apps/obc-web-assemble/**', 'apps/obc-skin-preview/**', 'host/obcm-assemble/**']
RULE = 'A wasm bridge stays inside its recorded gzip and raw size budgets.'

import argparse
import gzip
import sys
from pathlib import Path


# Budgets in bytes, per module, each about 10 % above the measured artifact.
#
# `convert` is a latency budget: a visitor downloads it the moment they drop a route, so its size is
# a wait. `assemble` links the whole assembly engine and is fetched only by someone who has already
# chosen to download hundreds of megabytes of cells, so its budget guards against a crate being
# linked in and not against the engine's own size. `preview` links the reader, the renderer and
# enough of the assembler to stamp a skin, and nothing else.
BUDGETS = {
    "convert": {"gzipped": 62 * 1024, "raw_wasm": 112 * 1024},
    "assemble": {"gzipped": 320 * 1024, "raw_wasm": 832 * 1024},
    "preview": {"gzipped": 128 * 1024, "raw_wasm": 272 * 1024},
}

# The advice differs per module, because the fix does. Convert is a latency budget, so the literal
# fix is to make it smaller. Assemble is a structural guard on a module nobody waits for, so the fix
# is to find what got linked in.
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
    "flat-device": Path("builder/app/test-support/flat-device/pkg"),
}

# The test device has no budget: every module above ships to a visitor, and this one is downloaded
# only by the test runner and the dev harness. Its size is worth printing and worth nothing as a
# gate.
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
                f"::error::obc-web-{args.module} {label} is {measured:,} B, over the {budget:,} B budget."
                f" {ADVICE[args.module]}"
            )
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
