#!/usr/bin/env python3
"""Fail when a hand-written repaint mirror grows back beside the declared render keys.

A screen's content is declared once, in its `screens!` row, as a `RenderKeyKind`
(firmware/obc-app/src/render_key.rs). The pass compares the visible stack's key across its own
stages and dirties the map when it moves. What that replaced was seven private copies of the values
being watched — `state_before`, `RideEngine::prev_no_fix`, `RideEngine::prev_live_sensors`, and the
three overlay level-to-edge converters `InputPlane::overlay_was_active`, `UiRuntime::overlay_edge`
and `CoreMode::engaged_shown`.

Each one was added for a real missed redraw, and each is easy to re-add for the next one. That is
what this guard is for: a fact a screen draws belongs in its declared key, and a repaint edge that a
key cannot see belongs in one of the five documented explicit classes on `Dirty` — never in a new
private copy of the value.

**This is a blocklist of names, and that is all it is.** It catches the seven mirrors coming back
under their own spellings, which is the likely shape of the mistake, because a reader who reaches
for one reaches for it by name. It does **not** catch the same mirror renamed: a fresh
`prev_battery` field beside a fresh `map_dirty = true` passes this guard. What catches *that* is the
differential replay in `apps/obc-sim/tests/dirty_parity.rs`, which compares frames rather than
identifiers, and the review question this file exists to prompt: why is a value being remembered
next to the state that already holds it?
"""

from __future__ import annotations

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
    # The base-screen gate the per-quantity guards hung off. Its replacement is per screen, not per
    # screen *class*: the Map and the Statistics grid draw different live data, and lumping them
    # together is what spent a ~97 ms map render on a heart-rate notification.
    re.compile(r"\bshows_live_" + r"data\b"),
    # The seventh, and the last: a between-pass seam that sniffed the top screen and dirtied the map
    # because a resample is invisible to a stack-local key. What replaced it is a monotone revision
    # the weather domain holds, named by `WeatherKey` (#1549).
    re.compile(r"\bweather_feed_" + r"changed\b"),
]

# One file is exempt, and the reason is what the file is: `firmware/tools/resource_baseline.json` is
# a measurement log whose `_resident_note_*` entries record what past slices moved, by name.
# Rewriting those notes to dodge a grep would falsify the record this guard has no business in.
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
        print("A repaint mirror deleted by #1447 has grown back:")
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
