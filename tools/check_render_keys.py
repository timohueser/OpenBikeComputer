#!/usr/bin/env python3
"""Fail when a hand-written repaint mirror grows back beside the declared render keys.

A screen's content is declared once, in its `screens!` row, as a `RenderKeyKind`. The pass compares
the visible stack's key across its own stages and dirties the map when it moves. A fact a screen
draws belongs in its declared key, and a repaint edge a key cannot see belongs in one of the
documented explicit classes on `Dirty`, never in a private copy of the value.

This is a blocklist of names and nothing more. A mirror rebuilt under another spelling passes it.
What catches that is the differential replay in `apps/obc-sim/tests/dirty_parity.rs`, which compares
frames rather than identifiers.
"""

from __future__ import annotations

GOVERNS = ['**/*.rs']
RULE = 'A fact a screen draws belongs in its declared render key, never in a private repaint mirror.'

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SKIP_PARTS = {".git", ".claude", ".codex", ".venv", "dist", "node_modules", "target"}

# Spellings are split so this guard's own source, and the prose above, do not trip it.
RETIRED = [
    re.compile(r"\bstate_" + r"before\b"),
    re.compile(r"\bprev_no_" + r"fix\b"),
    re.compile(r"\bprev_live_" + r"sensors\b"),
    re.compile(r"\boverlay_was_" + r"active\b"),
    re.compile(r"\boverlay_" + r"edge\b"),
    re.compile(r"\bengaged_" + r"shown\b"),
    re.compile(r"\btake_engaged_" + r"edge\b"),
    re.compile(r"\btake_overlay_" + r"dirty\b"),
    # The base-screen gate the per-quantity guards hung off. Its replacement is per screen and not
    # per screen class: the Map and the Statistics grid draw different live data, and lumping them
    # together spends a whole map render on a heart-rate notification.
    re.compile(r"\bshows_live_" + r"data\b"),
]

# One file is exempt: `firmware/tools/resource_baseline.json` is a measurement log whose notes
# record what past slices moved, by name. Rewriting them to dodge a grep would falsify the record.
EXEMPT = {Path("firmware/tools/resource_baseline.json")}

def main() -> int:
    failures: list[str] = []
    for path in sorted(ROOT.rglob("*.rs")):
        rel = path.relative_to(ROOT)
        if set(rel.parts) & SKIP_PARTS or rel in EXEMPT:
            continue
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for retired in RETIRED:
                match = retired.search(line)
                if match:
                    failures.append(f"{rel}:{line_no}: `{match.group(0)}`")

    if failures:
        print("A repaint mirror the declared render keys replaced has grown back:")
        print("\n".join(failures))
        print(
            "\nDeclare the fact in the screen's `RenderKeyKind` instead. If no key can see the\n"
            "mutation — a host seam between two passes, a screen's own state, the card sweep, a\n"
            "planner landing, or resident data no row declares — ask for the repaint there and say\n"
            "so: those are the five explicit classes documented on `Dirty`\n"
            "(firmware/obc-app/src/dirty.rs)."
        )
        return 1
    print("no hand-written repaint mirrors — the screens declare what they draw")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
