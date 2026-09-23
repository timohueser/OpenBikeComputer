#!/usr/bin/env python3
"""Fail when a shared screen-drawing helper is defined outside `screen/vocab/`.

`screen/mod.rs` is the navigation engine, and the drawing vocabulary lives one module per concept
under `screen/vocab/`. Nothing enforces that split at compile time, so each landmark definition
below must exist once in `screen/vocab/` and nowhere else under `screen/`, and what the
vocabulary owns must not grow again beside a screen's draw code.
"""

from __future__ import annotations

GOVERNS = ['firmware/obc-app/src/screen/**/*.rs', 'firmware/obc-app/src/stat_fields.rs']
RULE = 'A shared screen-drawing helper is defined once, under screen/vocab/.'

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCREEN = ROOT / "firmware/obc-app/src/screen"
VOCAB = SCREEN / "vocab"

# `stat_fields.rs` sits beside `screen/` rather than under it, but its tiles print the same
# quantities the screens do — it grew the first six formatters, and is scanned with them.
NEIGHBOURS = [ROOT / "firmware/obc-app/src/stat_fields.rs"]

# One landmark per vocabulary module, spelled as its definition site.
LANDMARKS = [
    "title_frame",
    "card_triangle",
    "recalculating_banner",
    "ledger_row",
    "draw_guarded_rows",
    "tile",
    "waypoint_panel",
    "toggle_slider",
    "needle_region",
    "distance_short",
    "duration_hms",
    "elevation_short",
]

# Constants that tune a shared mechanism. A re-declaration is how the drift comes back.
CONSTANTS = ["SPIN_DPS", "SPIN_FRAME_MS", "PAGE_FLIP_MS"]

# Spellings that appear only when a screen has re-grown something the vocabulary owns. `prev_top` is
# the elevation band's connected top stroke, which `vocab/band.rs` owns. `fit_name` and
# `fit_caption` are the display fitters `vocab/marquee.rs` replaced: a screen that cuts a name with
# its own dots is how the second ellipsis convention comes back.
RETIRED = ["prev_top", "fit_name", "fit_caption"]


def matches(pattern: re.Pattern[str], paths: list[Path]) -> list[str]:
    hits = []
    for path in paths:
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if pattern.search(line):
                hits.append(f"{path.relative_to(ROOT)}:{line_no}")
    return hits


def check_once(kind: str, name: str, pattern: re.Pattern[str], vocab: list[Path], screens: list[Path]) -> list[str]:
    """`name` must be defined exactly once under vocab/ and nowhere else under screen/."""
    failures = []
    outside = matches(pattern, screens)
    if outside:
        failures.append(f"`{kind} {name}` is defined outside vocab/ ({', '.join(outside)}) — import it instead")
    in_vocab = matches(pattern, vocab)
    if len(in_vocab) != 1:
        where = ", ".join(in_vocab) or "nowhere"
        failures.append(f"`{kind} {name}` must be defined exactly once under screen/vocab/, found {where}")
    return failures


def main() -> int:
    failures: list[str] = []
    vocab_files = sorted(VOCAB.rglob("*.rs"))
    screen_files = [p for p in sorted(SCREEN.rglob("*.rs")) if VOCAB not in p.parents] + NEIGHBOURS
    for name in LANDMARKS:
        pattern = re.compile(r"\bfn " + re.escape(name) + r"\s*[(<]")
        failures += check_once("fn", name, pattern, vocab_files, screen_files)
    for name in CONSTANTS:
        pattern = re.compile(r"\bconst " + re.escape(name) + r"\s*:")
        failures += check_once("const", name, pattern, vocab_files, screen_files)
    for name in RETIRED:
        hits = matches(re.compile(r"\b" + re.escape(name) + r"\b"), screen_files)
        if hits:
            failures.append(f"`{name}` is back outside vocab/ ({', '.join(hits)}) — draw through the vocabulary")

    if failures:
        print("The shared screen vocabulary has drifted out of `screen/vocab/`:")
        print("\n".join(failures))
        return 1
    pinned = len(LANDMARKS) + len(CONSTANTS)
    print(
        f"screen vocabulary intact: {pinned} landmark definitions, each exactly once under screen/vocab/, "
        "and no retired spelling is back beside a screen's draw code"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
