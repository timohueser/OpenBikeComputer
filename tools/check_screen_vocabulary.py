#!/usr/bin/env python3
"""Fail when a shared screen-drawing helper is defined outside `screen/vocab/`.

`firmware/obc-app/src/screen/mod.rs` is the navigation engine: the `screens!` table, `Caps`, the
contexts, `Transition`, and the ride-session entry points. The drawing vocabulary every screen
composes its page from lives one module per concept under `firmware/obc-app/src/screen/vocab/`.
Nothing enforces that split at compile time — a helper re-grown next to the table, or a screen
quietly re-declaring one it could have imported, would build fine — so this guard keeps it
honest: each landmark definition below exists exactly once in `screen/vocab/`, and nowhere else
under `screen/`.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCREEN = ROOT / "firmware/obc-app/src/screen"
VOCAB = SCREEN / "vocab"

# One landmark per vocabulary module, spelled as its definition site.
LANDMARKS = ["title_frame", "card_triangle", "ledger_row", "draw_guarded_rows", "tile", "waypoint_panel"]


def definitions(name: str, paths: list[Path]) -> list[str]:
    pattern = re.compile(r"\bfn " + re.escape(name) + r"\s*[(<]")
    hits = []
    for path in paths:
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if pattern.search(line):
                hits.append(f"{path.relative_to(ROOT)}:{line_no}")
    return hits


def main() -> int:
    failures: list[str] = []
    vocab_files = sorted(VOCAB.rglob("*.rs"))
    screen_files = [p for p in sorted(SCREEN.rglob("*.rs")) if VOCAB not in p.parents]
    for name in LANDMARKS:
        outside = definitions(name, screen_files)
        if outside:
            failures.append(f"`fn {name}` is defined outside vocab/ ({', '.join(outside)}) — import it instead")
        in_vocab = definitions(name, vocab_files)
        if len(in_vocab) != 1:
            where = ", ".join(in_vocab) or "nowhere"
            failures.append(f"`fn {name}` must be defined exactly once under screen/vocab/, found {where}")

    if failures:
        print("The shared screen vocabulary has drifted out of `screen/vocab/`:")
        print("\n".join(failures))
        return 1
    print(f"screen vocabulary intact: {len(LANDMARKS)} landmark helpers, each defined once under screen/vocab/")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
