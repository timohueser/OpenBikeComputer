#!/usr/bin/env python3
"""Fail when a shared screen-drawing helper is defined outside `screen/vocab/`.

`firmware/obc-app/src/screen/mod.rs` is the navigation engine: the `screens!` table, `Caps`, the
contexts, `Transition`, and the ride-session entry points. The drawing vocabulary every screen
composes its page from lives one module per concept under `firmware/obc-app/src/screen/vocab/`.
Nothing enforces that split at compile time — a helper re-grown next to the table, or a screen
quietly re-declaring one it could have imported, would build fine — so this guard keeps it
honest: each landmark definition below exists exactly once in `screen/vocab/`, and nowhere else
under `screen/`, and each retired helper name stays retired.
"""

from __future__ import annotations

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
    "needle_region",
    "distance_short",
    "duration_hms",
    "elevation_short",
]

# Quantity formatters that existed twice, or under a name that said nothing about the quantity.
# Each one is a shape a screen must import from `vocab/fmt.rs`, never re-declare next to its draw
# code — that is how two screens came to round the same number differently.
RETIRED_FORMATTERS = [
    "fmt_km",
    "fmt_dist_short",
    "fmt_speed",
    "fmt_int",
    "fmt_int_opt",
    "fmt_elev",
    "fmt_hms",
    "fmt_pct",
    "fmt_climb_delta",
    "fmt_remaining",
    "fmt_date",
    "fmt_offset",
    "fmt_bytes",
    "fmt_addr",
    "fmt_temp",
    "write_off_route",
    "write_away",
    "write_short_date",
    "write_computed_distance",
    "write_climb",
    "write_distance",
]

# Constants that tune a shared mechanism. Each was copied verbatim into a second screen before the
# mechanism was unified; a re-declaration is how that drift comes back.
CONSTANTS = ["SPIN_DPS", "SPIN_FRAME_MS", "PAGE_FLIP_MS"]

# Spellings that only appear when a screen has re-grown a raster the vocabulary owns. `prev_top` is
# the state of the elevation band's connected top stroke, which `vocab/band.rs` owns as
# `TopStroke`. The received card's mini sparkline builds its own columns but strokes through that
# same rule, so no screen under `screen/` keeps this state and the ban needs no exemptions.
RETIRED = ["prev_top"]


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
    for name in RETIRED_FORMATTERS:
        pattern = re.compile(r"\bfn " + re.escape(name) + r"\s*[(<]")
        hits = matches(pattern, screen_files) + matches(pattern, vocab_files)
        if hits:
            failures.append(f"`fn {name}` is back ({', '.join(hits)}) — format through `vocab::fmt` instead")

    if failures:
        print("The shared screen vocabulary has drifted out of `screen/vocab/`:")
        print("\n".join(failures))
        return 1
    pinned = len(LANDMARKS) + len(CONSTANTS)
    print(
        f"screen vocabulary intact: {pinned} landmark definitions, each exactly once under screen/vocab/; "
        f"{len(RETIRED_FORMATTERS)} retired formatter names stay retired"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
