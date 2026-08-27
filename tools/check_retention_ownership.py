#!/usr/bin/env python3
"""Fail when a retention decision grows a second home outside `RetentionMachine` (#1548).

The domain owns the whole retention policy: when a sweep is due, which candidate class goes first,
whether the clock may be trusted, and when a delete candidate has been satisfied. Two spellings took
that back once each, and this guard is a blocklist of exactly those two:

- **a prefixed `*_delete_inflight` field** — the per-kind in-flight slots. `CatalogState` holds one
  intent and one in-flight operation, so at most one removal is ever outstanding; a second slot can
  only describe a concurrency the catalog does not have. One `delete_inflight` is the whole of it.
- **a `clock_trusted()` call guarding a `retention.` call** — invariant 1 stated at a call site. The
  gate lives inside the domain entry point, which is why `App::with_retention` hands the machine a
  `RetentionView` whose `now_utc` is already `None` on an untrusted boot. A caller that re-derives
  the rule is a second implementation of it, and the two drift.

**This is a blocklist of names, and that is all it is.** A retention policy rebuilt under other
spellings passes it. What catches that is the conformance corpus
(`host/obc-host-core/tests/device_core_corpus`), where the expiry scenario settles only because the
domain retired its own candidate.
"""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SKIP_PARTS = {".git", ".claude", ".codex", ".venv", "dist", "node_modules", "target"}

# A prefixed spelling of the single in-flight slot: `route_delete_inflight`, `ride_delete_inflight`.
PER_KIND_SLOT = re.compile(r"\b[a-z0-9]+_delete_" + r"inflight\b")
# The trusted-clock gate, and the domain call a guarded branch would reach.
CLOCK_GATE = re.compile(r"clock_trusted\(\)")
RETENTION_CALL = re.compile(r"\bretention\.\w")
# How far past the gate a guarded call still counts as inside its branch.
GUARD_WINDOW = 4

# This file is the one place both retired shapes are written out in full, because naming what may
# not come back is what the guard is for.
EXEMPT = {Path("tools/check_retention_ownership.py")}


def offences(rel: Path, text: str) -> list[str]:
    lines = text.splitlines()
    found = []
    for line_no, line in enumerate(lines, 1):
        match = PER_KIND_SLOT.search(line)
        if match:
            found.append(f"{rel}:{line_no}: `{match.group(0)}` — one removal is in flight, not one per class")
        if not CLOCK_GATE.search(line):
            continue
        for offset, following in enumerate(lines[line_no - 1 : line_no - 1 + GUARD_WINDOW]):
            if RETENTION_CALL.search(following):
                where = line_no + offset
                found.append(f"{rel}:{where}: a `clock_trusted()` branch reaches `retention.` — invariant 1 twice")
                break
    return found


def main() -> int:
    failures: list[str] = []
    for path in sorted(ROOT.rglob("*.rs")):
        rel = path.relative_to(ROOT)
        if set(rel.parts) & SKIP_PARTS or rel in EXEMPT:
            continue
        failures.extend(offences(rel, path.read_text(encoding="utf-8")))

    if failures:
        print("A retention decision has a second home outside RetentionMachine (#1548):")
        print("\n".join(failures))
        print(
            "\nThe policy belongs to `RetentionMachine` (firmware/obc-app/src/retention.rs). One\n"
            "`delete_inflight` slot paces every removal, because `CatalogState` runs one operation\n"
            "at a time; and the trusted-clock gate is applied inside the domain entry point, which\n"
            "reads it from the `RetentionView` `App::with_retention` assembles."
        )
        return 1
    print("retention decisions have one home: one in-flight slot, one trusted-clock gate")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
