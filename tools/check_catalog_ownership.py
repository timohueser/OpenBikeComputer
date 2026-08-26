#!/usr/bin/env python3
"""Fail when an executor grows a catalog-refresh policy of its own back (#1541).

Every re-read of the object store is ordered by `CatalogMachine`: three events arm one owed bit —
the store moved underneath us, a removal completed, a read failed — and
`CatalogState::next_effect` is the only place any of them becomes a `ReadCatalog`. Before this,
three executors disagreed about it: the host composed a re-feed inside each deletion, the board kept
a private `rescan_owed` retry the host could not even produce, and neither of them was the domain.

Two spellings can bring that back, and this guard is a blocklist of exactly those two:

- **`rescan_owed`** — the board's own retry. A failed read is answered `Unreadable`, and the domain
  re-offers it once per pass, which is the cadence that field existed to give.
- **a feeder call inside the host's `remove_object`** — the re-feed a removal used to compose. The
  function is free-standing and takes no `&mut App` precisely so it *cannot*; this guard says so out
  loud for a reader who is about to hand it one.

**This is a blocklist of names, and that is all it is.** A refresh policy rebuilt under other
spellings passes it. What catches that is the conformance gate's own executor
(`host/obc-host-core/tests/device_core_conformance.rs`), where a removal re-feeds nothing and the
delete scenarios settle only because the domain ordered the read.
"""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SKIP_PARTS = {".git", ".claude", ".codex", ".venv", "dist", "node_modules", "target"}
# Rust and the Python contract tests that read it: both are places the retry came back from.
SUFFIXES = ("*.rs", "*.py")

RETIRED = re.compile(r"\brescan_" + r"owed\b")
# This file is the one place the retired name is written out in full, because naming what may not
# come back is what the guard is for. Rewriting its own prose to dodge its own grep would make the
# rule unreadable.
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
