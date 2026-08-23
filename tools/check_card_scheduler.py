#!/usr/bin/env python3
"""Fail when a scheduler-owned card is constructed outside the card scheduler.

Six modal card families are the [`CardScheduler`](firmware/obc-app/src/card_scheduler.rs)'s: the
BLE passkey card, the map-transfer card, the route/trip upload popups, the advisory warning card,
the post-update toast, and the terminal DFU answers. Their delivery rules — never land mid-hold,
the passkey card outranks, replace instead of stacking, timeout dismisses, revalidate the durable
identity at delivery — hold only if nothing else builds one of those screens and pushes it. That
is what this guard keeps true: production code reaches those cards through the scheduler's slots,
never through their constructors.
"""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

# The card constructors the scheduler owns. Spellings are split so this guard's own source does not
# trip a `grep` for them.
OWNED = [
    re.compile(r"\bPasskeyScreen::" + r"new\b"),
    re.compile(r"\bMapTransferScreen::" + r"new\b"),
    re.compile(r"\bRouteReceivedScreen::" + r"new\b"),
    re.compile(r"\bRouteUpdatedScreen::" + r"new\b"),
    re.compile(r"\bTripReceivedScreen::" + r"new\b"),
    re.compile(r"\bRouteSwapScreen::" + r"received\b"),
    re.compile(r"\bWarningScreen::" + r"new\b"),
    re.compile(r"\bDfuUpdatedScreen::" + r"new\b"),
    re.compile(r"\bDfuFailedScreen::" + r"new\b"),
    re.compile(r"\bDfuConfirmScreen::" + r"new\b"),
    re.compile(r"\bDfuErrorScreen::" + r"new"),
    re.compile(r"\bDfuInstallingScreen::" + r"new\b"),
]

# Where building one is legitimate:
#   * the scheduler itself — it is the one that lands them;
#   * test harnesses and integration tests, which stage a stack to exercise something else.
#
# `firmware/obc-app/src/screen/` is deliberately **not** exempt, even though it defines these cards:
# it is where every `Transition::Push` in the codebase is authored, so a screen's `handle()` pushing
# a scheduler-owned card is the single most plausible way this invariant breaks. The cards' own
# constructor uses there all sit below their file's `#[cfg(test)]` gate, which the scan already
# honours, so covering the directory costs nothing.
#
# Deliberately not on the banned list at all: `RouteSwapScreen::new` (the rider's own menu-opened
# swap prompt) is a rider-opened screen, so any screen may open it.
ALLOWED_PATHS = (
    Path("firmware/obc-app/src/card_scheduler.rs"),
    Path("firmware/obc-app/src/harness"),
    Path("firmware/obc-app/tests"),
)

# A `#[cfg(test)]` module is the crate's own staging ground: a test that pushes a passkey card to
# check some *other* rule is not a second delivery path. Every such module in this repository sits
# at the end of its file, so the first `#[cfg(test)]` line is where production code stops.
TEST_GATE = "#[cfg(test)]"


def allowed(rel: Path) -> bool:
    return any(rel == p or p in rel.parents for p in ALLOWED_PATHS)


def main() -> int:
    failures: list[str] = []
    for path in sorted(ROOT.rglob("*.rs")):
        rel = path.relative_to(ROOT)
        if "target" in rel.parts or allowed(rel):
            continue
        lines = path.read_text(encoding="utf-8").splitlines()
        gate = next((i for i, line in enumerate(lines) if line.strip() == TEST_GATE), len(lines))
        for line_no, line in enumerate(lines[:gate], 1):
            # A comment is prose, not a construction: intra-doc links such as
            # `[`received`](RouteSwapScreen::received)` name a constructor without calling it.
            if line.lstrip().startswith("//"):
                continue
            for owned in OWNED:
                match = owned.search(line)
                if match:
                    failures.append(f"{rel}:{line_no}: `{match.group(0)}` outside the card scheduler")

    if failures:
        print("Scheduler-owned cards are built outside `card_scheduler.rs`:")
        print("\n".join(failures))
        print("\nPost the fact into the scheduler's slot instead; the sweep owns when the card lands.")
        return 1
    print("scheduler-owned cards are built only by the card scheduler")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
