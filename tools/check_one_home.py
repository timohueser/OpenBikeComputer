#!/usr/bin/env python3
"""Fail when a setting has two homes — one in a drawer row and one in a central settings screen.

The contextual drawer exists so a screen-specific setting has one obvious home, and nothing about
the language stops a deleted central row growing back. Two rules follow.

No `Settings` field is written by both a drawer and a settings screen: the write is the home, and a
row that draws a value it cannot change is a readout. No catalog key on a context row's label is
drawn by a settings screen: the label is what the rider searches for.

Deliberate exceptions are listed below with the decision that made each one. A text guard fails by
going blind, so the census is pinned: every `ContextRow` must yield a parsed label, and the totals
must clear a floor that matches the declared controls.
"""

from __future__ import annotations

GOVERNS = ['firmware/obc-app/src/screen/**/*.rs']
RULE = 'A setting has one home: a drawer row or a settings screen, never both.'

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCREEN = ROOT / "firmware/obc-app/src/screen"
SETTINGS = SCREEN / "settings"
DRAWERS = [SCREEN / "context_drawer.rs", SCREEN / "quick_drawer.rs"]

# The universal Bluetooth on/off shortcut also stays with connection setup in Settings.
ALLOWED_SHARED_FIELDS: set[str] = {"ble_enabled"}

# Update these floors when controls are added or removed. A parser change must not lower them.
MIN_ROW_LABELS = 11
MIN_DRAWER_FIELDS = 7

# `cx.settings.<field> = …` — the one production write path a screen has into the persisted record.
# `=(?!=)` so an equality test is not read as a write.
FIELD_WRITE = re.compile(r"\bcx\.settings\.([a-z_][a-z0-9_]*)\s*=(?!=)")
# A `ContextRow { … }` literal, and the `label: Msg::<Key>` somewhere inside it — the label is not
# required to be the first field, and a row whose body yields no label is a parse failure rather
# than a row that quietly leaves the census. The lookbehind skips the `struct ContextRow` shape
# declaration, which is the one brace pair that legitimately holds no key.
CONTEXT_ROW = re.compile(r"(?<!struct )ContextRow\s*\{([^{}]*)\}")
ROW_LABEL = re.compile(r"\blabel\s*:\s*Msg::([A-Za-z0-9_]+)")
MSG_KEY = re.compile(r"\bMsg::([A-Za-z0-9_]+)")


def rust_sources(root: Path) -> list[Path]:
    return sorted(p for p in root.rglob("*.rs"))


def fields_written(paths: list[Path]) -> dict[str, list[str]]:
    """Map each `Settings` field written under `paths` to the files that write it."""
    out: dict[str, list[str]] = {}
    for path in paths:
        for field in set(FIELD_WRITE.findall(path.read_text())):
            out.setdefault(field, []).append(str(path.relative_to(ROOT)))
    return out


def main() -> int:
    for path in [SETTINGS, *DRAWERS]:
        if not path.exists():
            print(f"one-home guard: {path} is missing — did a slice move it?", file=sys.stderr)
            return 1

    failures: list[str] = []

    settings_files = rust_sources(SETTINGS)
    settings_fields = fields_written(settings_files)
    drawer_fields = fields_written(DRAWERS)

    for field, drawer_paths in sorted(drawer_fields.items()):
        if field not in settings_fields or field in ALLOWED_SHARED_FIELDS:
            continue
        failures.append(
            f"`Settings::{field}` has two homes: written by {', '.join(sorted(drawer_paths))} "
            f"and by {', '.join(sorted(settings_fields[field]))}. Delete the central row in the "
            f"same push that moves the editor, or record the exception in ALLOWED_SHARED_FIELDS."
        )

    # A field a settings screen writes but the drawer only reads is fine, and so is the reverse.
    # Only the pair is a second home.

    row_labels: dict[str, str] = {}
    rows_seen = 0
    for path in DRAWERS:
        where = str(path.relative_to(ROOT))
        for body in CONTEXT_ROW.findall(path.read_text()):
            rows_seen += 1
            label = ROW_LABEL.search(body)
            if label is None:
                failures.append(
                    f"a `ContextRow` in {where} has no `label: Msg::…` the guard can read "
                    f"({' '.join(body.split())!r}). Spell the label inline, or teach the guard the "
                    f"new shape — a row it cannot parse is a home it cannot check."
                )
                continue
            row_labels[label.group(1)] = where

    # The census floors catch an incomplete parser or a changed set of controls.
    if len(row_labels) < MIN_ROW_LABELS:
        failures.append(
            f"only {len(row_labels)} context row label(s) parsed, below the pinned floor of "
            f"{MIN_ROW_LABELS} ({rows_seen} `ContextRow` literal(s) seen). Update MIN_ROW_LABELS "
            f"deliberately when controls change; never lower it to hide a parser failure."
        )
    if len(drawer_fields) < MIN_DRAWER_FIELDS:
        failures.append(
            f"only {len(drawer_fields)} drawer-written setting(s) found, below the pinned floor of "
            f"{MIN_DRAWER_FIELDS}. Either a write moved out of a drawer, or `FIELD_WRITE` no longer "
            f"matches the write path."
        )

    for path in settings_files:
        drawn = set(MSG_KEY.findall(path.read_text()))
        for key in sorted(drawn & set(row_labels)):
            failures.append(
                f"`Msg::{key}` is a context row label ({row_labels[key]}) and is also drawn by "
                f"{path.relative_to(ROOT)} — one home per setting."
            )

    if failures:
        print("one-home guard failed:\n", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print(
        f"one-home guard: {len(drawer_fields)} drawer-written setting(s) (floor {MIN_DRAWER_FIELDS}), "
        f"{len(row_labels)} context row label(s) from {rows_seen} row literal(s) "
        f"(floor {MIN_ROW_LABELS}) — no unapproved shared setting"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
