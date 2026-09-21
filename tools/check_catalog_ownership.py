#!/usr/bin/env python3
"""Fail when an executor grows a catalog-refresh policy of its own.

Every re-read of the object store is ordered by `CatalogMachine`: three events arm one owed bit, and
`CatalogState::next_effect` is the only place any of them becomes a `ReadCatalog`.

Two spellings can bring a second policy back, and this guard blocks both. `rescan_owed` is a private
retry: a failed read is answered `Unreadable`, and the domain re-offers it once per pass. A feeder
call inside the host's `remove_object` is a re-feed composed by a removal; the function takes no
`&mut App` so that it cannot.

This is a blocklist of names and nothing more. A policy rebuilt under other spellings passes it, and
what catches that is the conformance gate's own executor, where a removal re-feeds nothing.
"""

from __future__ import annotations

GOVERNS = ['**/*.rs', '**/*.py']
RULE = 'Only CatalogMachine orders a re-read of the object store.'

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SKIP_PARTS = {".git", ".claude", ".codex", ".venv", "dist", "node_modules", "target"}
# Rust and the Python contract tests that read it: the retry can come back in either.
SUFFIXES = ("*.rs", "*.py")

RETIRED = re.compile(r"\brescan_" + r"owed\b")
# This file is the one place the banned name is written out in full, because naming what may not
# come back is what the guard is for.
EXEMPT = {Path("tools/check_catalog_ownership.py")}

DISPATCH = Path("host/obc-host-core/src/dispatch.rs")
# The store operation the domain orders around. A feeder here is an executor deciding when a
# refresh happens.
FEEDERS = re.compile(r"\bfeed_routes\b|\bfeed_rides\b|\brefeed\b|\brescan\b|&mut App\b")


def removal_body(source: str) -> str:
    """The text of `remove_object`, from its signature to the next top-level item."""
    start = source.index("fn remove_object(")
    end = source.index("\n}\n", start)
    return source[start:end]


def main() -> int:
    failures: list[str] = []
    for path in sorted(q for suffix in SUFFIXES for q in ROOT.rglob(suffix)):
        rel = path.relative_to(ROOT)
        if set(rel.parts) & SKIP_PARTS or rel in EXEMPT:
            continue
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            match = RETIRED.search(line)
            if match:
                failures.append(f"{rel}:{line_no}: `{match.group(0)}` — an executor's own refresh retry")

    dispatch = (ROOT / DISPATCH).read_text(encoding="utf-8")
    try:
        body = removal_body(dispatch)
    except ValueError:
        failures.append(f"{DISPATCH}: no `fn remove_object(` — this guard is stale, not the code")
    else:
        for match in FEEDERS.finditer(body):
            failures.append(f"{DISPATCH}: `{match.group(0)}` inside `remove_object`")

    if failures:
        print("An executor is deciding when the catalog is re-read (#1541):")
        print("\n".join(failures))
        print(
            "\nThe re-read belongs to `CatalogMachine`. A completed removal and a failed read both\n"
            "arm `CatalogState::refresh_owed` (firmware/obc-app/src/catalog_state.rs), and\n"
            "`next_effect` turns it into one `ReadCatalog` when no deletion is pending. An executor\n"
            "that re-feeds or retries beside that gives the device two refresh policies again."
        )
        return 1
    print("the catalog re-read is the domain's: no executor retry, no feeder in the removal")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
