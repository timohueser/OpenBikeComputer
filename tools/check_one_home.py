#!/usr/bin/env python3
"""Fail when a setting has two homes — a row on a screen's sheet and a row on a settings page.

The contextual drawer exists so a screen-specific setting has one obvious home, and nothing stops
a deleted central row growing back. Pages and sheets bind their values through one table of
bindings (`ContextValue`, `ContextToggle`), so a home is where a binding is placed on a row.

No binding sits on both a sheet row and a page row. No `Settings` field is written directly by
both a drawer and a settings screen. No catalog key on a sheet row's label is drawn by a settings
screen: the label is what the rider searches for.

Exceptions are listed below. A text guard fails by going blind, so the census is pinned: every
`ContextRow` must yield a parsed label, and the totals must clear floors that match the controls.
"""

from __future__ import annotations

GOVERNS = ['firmware/obc-app/src/screen/**/*.rs']
RULE = 'A setting has one home: a sheet row or a settings page, never both.'

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCREEN = ROOT / "firmware/obc-app/src/screen"
SETTINGS = SCREEN / "settings"
CONTEXT = SCREEN / "context_drawer.rs"
QUICK = SCREEN / "quick_drawer.rs"
PAGES = SETTINGS / "page.rs"

# The quick drawer's two shortcuts also stay on their settings pages: brightness on Display, the
# Bluetooth radio on Connections.
ALLOWED_SHARED_FIELDS: set[str] = {"brightness", "ble_enabled"}
# The bike type is one current value the rider sets before a ride, from the route-plan sheet, the
# start card or the Ride settings page (owner decision). All three open the same editor.
ALLOWED_SHARED_BINDINGS: set[str] = {"BikeProfile"}

# The census, pinned at the declared controls. Update these when controls are added or removed;
# a parser change must not lower them.
ROW_LABELS = 26
SHEET_BINDINGS = 12
PAGE_BINDINGS = 11

# `cx.settings.<field> = …`, or `s.<field> = …` after `let s = &mut *cx.settings` — the production
# write paths a screen has into the persisted record. `=(?!=)` so an equality test is not read as
# a write.
FIELD_WRITE = re.compile(r"\b(?:cx\.settings|s)\.([a-z_][a-z0-9_]*)\s*=(?!=)")
# A `ContextRow { … }` literal, and the `label: Msg::<Key>` somewhere inside it — the label is not
# required to be the first field, and a row whose body yields no label is a parse failure rather
# than a row that quietly leaves the census. The lookbehind skips the `struct ContextRow` shape
# declaration, which is the one brace pair that legitimately holds no key.
CONTEXT_ROW = re.compile(r"(?<!struct )ContextRow\s*\{([^{}]*)\}")
ROW_LABEL = re.compile(r"\blabel\s*:\s*Msg::([A-Za-z0-9_]+)")
MSG_KEY = re.compile(r"\bMsg::([A-Za-z0-9_]+)")
# A binding placed on a row: `ContextValue::X` or `ContextToggle::X` in a row table.
BINDING = re.compile(r"\bContext(?:Value|Toggle)::([A-Za-z0-9_]+)")
# The page tables: `static NAME: Menu = Menu { … };`.
PAGE_TABLE = re.compile(r"static [A-Z_]+: Menu = Menu \{.*?\n\};", re.S)
# A binding's commit or flip arm and the first settings field it writes:
# `ContextValue::X => s.field = …` or `ContextToggle::X => { s.field = …`.
BINDING_FIELD = re.compile(r"Context(?:Value|Toggle)::([A-Za-z0-9_]+)(?:\([^)]*\))?\s*=>\s*\{?\s*s\.([a-z_][a-z0-9_]*)\s*[\^!]?=")


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
    for path in [SETTINGS, CONTEXT, QUICK, PAGES]:
        if not path.exists():
            print(f"one-home guard: {path} is missing — did a slice move it?", file=sys.stderr)
            return 1

    failures: list[str] = []

    context_text = CONTEXT.read_text()
    sheet_bindings: set[str] = set()
    row_labels: dict[str, str] = {}
    rows_seen = 0
    for path in [CONTEXT, QUICK]:
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
            sheet_bindings.update(BINDING.findall(body))

    page_bindings: set[str] = set()
    for table in PAGE_TABLE.findall(PAGES.read_text()):
        page_bindings.update(BINDING.findall(table))

    for binding in sorted(ALLOWED_SHARED_BINDINGS - (sheet_bindings & page_bindings)):
        failures.append(
            f"ALLOWED_SHARED_BINDINGS names `{binding}`, but it is not on both a sheet row and a page "
            f"row — a vacuous exception hides a blind parser; drop it or fix the parser."
        )
    for binding in sorted((sheet_bindings & page_bindings) - ALLOWED_SHARED_BINDINGS):
        failures.append(
            f"`{binding}` has two homes: a sheet row in {CONTEXT.relative_to(ROOT)} and a page row in "
            f"{PAGES.relative_to(ROOT)}. Delete the page row in the same push that moves the editor."
        )

    # Every field a page binding writes, from the bindings' own commit and flip arms, plus the
    # settings screens' direct edits; against the quick drawer's direct writes. A page binding
    # whose write the guard cannot read is a home it cannot check.
    binding_field = dict(BINDING_FIELD.findall(context_text))
    page_fields: dict[str, str] = {}
    for binding in sorted(page_bindings):
        field = binding_field.get(binding)
        if field is None:
            failures.append(
                f"`{binding}` sits on a page row but the guard finds no `s.<field> =` in its commit or "
                f"flip arm in {CONTEXT.relative_to(ROOT)} — teach `BINDING_FIELD` the new shape."
            )
            continue
        page_fields[field] = f"{PAGES.relative_to(ROOT)} ({binding})"
    settings_files = [p for p in rust_sources(SETTINGS) if p != PAGES]
    settings_fields = {f: ", ".join(sorted(paths)) for f, paths in fields_written(settings_files).items()}
    settings_fields.update(page_fields)
    drawer_fields = fields_written([QUICK])
    shared = {f for f in drawer_fields if f in settings_fields}
    for field in sorted(shared - ALLOWED_SHARED_FIELDS):
        failures.append(
            f"`Settings::{field}` has two homes: written by {', '.join(sorted(drawer_fields[field]))} "
            f"and by {settings_fields[field]}. Delete the central row in the same push that moves the "
            f"editor, or record the exception in ALLOWED_SHARED_FIELDS."
        )
    for field in sorted(ALLOWED_SHARED_FIELDS - shared):
        failures.append(
            f"ALLOWED_SHARED_FIELDS names `{field}`, but it is not written by both the quick drawer and "
            f"a settings page — a vacuous exception hides a blind parser; drop it or fix the parser."
        )

    # The census is pinned, so an incomplete parser or a changed set of controls is a finding.
    for what, found, pinned in [
        ("context row label", len(row_labels), ROW_LABELS),
        ("sheet binding", len(sheet_bindings), SHEET_BINDINGS),
        ("page binding", len(page_bindings), PAGE_BINDINGS),
    ]:
        if found != pinned:
            failures.append(
                f"{found} {what}(s) found, {pinned} pinned ({rows_seen} `ContextRow` literal(s) seen). "
                f"Re-pin deliberately when controls change; never to hide a parser failure."
            )

    for path in rust_sources(SETTINGS):
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
        f"one-home guard: {len(sheet_bindings)} sheet binding(s), {len(page_bindings)} page binding(s) "
        f"over {len(page_fields)} field(s), {len(row_labels)} context row label(s) from {rows_seen} row "
        f"literal(s), shared with the quick drawer: {', '.join(sorted(shared)) or 'none'} — one home each"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
