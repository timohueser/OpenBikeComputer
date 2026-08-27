#!/usr/bin/env python3
"""Fail when a setting has two homes — one in a drawer row and one in a central settings screen.

#1515's whole reason for the contextual drawer is that a screen-specific setting should have **one
obvious home**, not also live deep inside a settings tree. The D4 slices move each editor into its
context and delete the central row in the same push; nothing about the language stops the deleted
row growing back, so this guard states the rule the slices are executing:

1. **No `Settings` field is written by both a drawer and a settings screen.** The write is the
   home — a row that draws a value it cannot change is a readout, not a second home.
2. **No catalog key on a context row's label is drawn by a settings screen.** The label is what the
   rider searches for, so the same word appearing in both places is the duplication they would see.

Deliberate exceptions are listed below, each with the decision that made it one. An exception is a
recorded deviation, not a way to keep a duplicate quiet.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCREEN = ROOT / "firmware/obc-app/src/screen"
SETTINGS = SCREEN / "settings"
DRAWERS = [SCREEN / "context_drawer.rs", SCREEN / "quick_drawer.rs"]

# `Settings` fields a drawer and a settings screen may both write, and why.
#
# `ble_enabled` — #1515 rules the quick drawer owns the radio switch while "detailed Bluetooth
# pairing/device management can remain a settings screen". That screen is the pairing surface and
# keeps its own switch beside the bond it manages (#1515 D2). The rule holds for every other field.
ALLOWED_SHARED_FIELDS = {"ble_enabled"}

# `cx.settings.<field> = …` — the one production write path a screen has into the persisted record.
FIELD_WRITE = re.compile(r"\bcx\.settings\.([a-z_][a-z0-9_]*)\s*=")
# `ContextRow { label: Msg::<Key>` — a context row's own catalog label.
ROW_LABEL = re.compile(r"ContextRow\s*\{\s*label:\s*Msg::([A-Za-z0-9_]+)")
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

    # A field a settings screen writes but the drawer only reads is fine, and so is the reverse —
    # only the *pair* is a second home, which is what the loop above checks.

    row_labels: dict[str, str] = {}
    for path in DRAWERS:
        for key in ROW_LABEL.findall(path.read_text()):
            row_labels[key] = str(path.relative_to(ROOT))

    if not row_labels:
        failures.append("one-home guard: no context row labels found — has `ContextRow` been renamed?")

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
        f"one-home guard: {len(drawer_fields)} drawer-written setting(s), "
        f"{len(row_labels)} context row label(s) — no setting has two homes"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
