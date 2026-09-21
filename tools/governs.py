#!/usr/bin/env python3
"""Answer one question: what governs this file?

An index tells you what rules exist. That is not the question anyone has. The question,
before changing a file, is which of them reach *it* — and the answer already exists,
scattered across `testing/suites.toml`, `testing/coverage-policy.toml`,
`firmware/tools/dependency_rules.json`, `firmware/ui-frames.toml` and the guards
themselves. Nothing had joined them.

Nothing here is authored. Every line is read from the file that enforces it, so this
cannot drift from what CI actually does.

Usage: python3 tools/governs.py PATH [PATH ...]
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


sys.path.insert(0, str(ROOT / "tools"))
from test_plan import glob_matches as matches  # noqa: E402 — the selector's own glob rule

GUARDS = ("tools/check_*.py", "firmware/tools/check_*.py", "firmware/tools/*_guard.py")


def cargo_package(path: str) -> tuple[str, str] | None:
    """The nearest Cargo.toml above the path, as (name, directory)."""
    here = (ROOT / path).parent if (ROOT / path).is_file() else ROOT / path
    while here != ROOT and ROOT in here.parents:
        manifest = here / "Cargo.toml"
        if manifest.exists():
            m = re.search(r'^\s*name\s*=\s*"([^"]+)"', manifest.read_text(), re.M)
            if m:
                return m.group(1), str(here.relative_to(ROOT))
        here = here.parent
    return None


def guards(path: str) -> list[tuple[str, str, bool]]:
    found = []
    for script in sorted(p for g in GUARDS for p in ROOT.glob(g)):
        text = script.read_text(encoding="utf-8")
        scope = re.search(r"^GOVERNS = (\[.*?\])$", text, re.M | re.S)
        rule = re.search(r"^RULE = (['\"])(.*?)\1$", text, re.M)
        if not scope or not rule:
            continue
        patterns = json.loads(scope.group(1).replace("'", '"'))
        if any(matches(path, p) for p in patterns):
            wide = all(p.startswith("**") for p in patterns)
            found.append((script.name, rule.group(2), wide))
    return found


def suites(path: str, package: str | None) -> list[tuple[str, str]]:
    data = tomllib.loads((ROOT / "testing/suites.toml").read_text())
    found = []
    declared = False
    for entry in data.get("package", []):
        if entry.get("name") == package:
            declared = True
            if entry.get("route"):
                found.append((f"package {entry['name']}", f"route = {entry['route']}"))
            for trigger in entry.get("triggers", []):
                found.append(
                    (f"package {entry['name']}", f"also re-runs when {trigger} changes")
                )
        for trigger in entry.get("triggers", []):
            if matches(path, trigger) and entry.get("name") != package:
                found.append((f"package {entry['name']}", f"triggered by {trigger}"))
    if package and not declared:
        found.append(
            (f"package {package}", "selected by Cargo's own graph; suites.toml adds nothing")
        )
    for entry in data.get("suite", []):
        for trigger in entry.get("triggers", []):
            if matches(path, trigger):
                found.append((entry.get("id", "?"), f"triggered by {trigger}"))
                break
    return found


def coverage(path: str) -> list[str]:
    data = tomllib.loads((ROOT / "testing/coverage-policy.toml").read_text())
    lines = []
    for rule in data.get("exclude", []):
        if matches(path, rule["path"]):
            lines.append(f"excluded from coverage — {rule['evidence']}")
    for component in data.get("component", []):
        if not any(matches(path, g) for g in component.get("include", [])):
            continue
        skipped = next(
            (r for r in component.get("exclude", []) if matches(path, r["path"])), None
        )
        if skipped:
            lines.append(f"component {component['id']}: excluded — {skipped['evidence']}")
        else:
            lines.append(
                f"component {component['id']} ({component.get('enforcement', '?')})"
            )
    return lines


def layering(package: str | None) -> list[str]:
    if not package:
        return []
    data = json.loads((ROOT / "firmware/tools/dependency_rules.json").read_text())
    return [
        f"dependency group '{group}' — direction enforced by firmware/tools/check_dependencies.py"
        for group, members in data.get("groups", {}).items()
        if package in members
    ]


def frames(path: str) -> list[str]:
    """UI frames whose expected screen matches this screen module."""
    if "/screen/" not in path or not path.endswith(".rs"):
        return []
    stem = Path(path).stem
    variant = stem.replace("_", "")
    data = tomllib.loads((ROOT / "firmware/ui-frames.toml").read_text())
    hits = sorted({
        f["name"]
        for f in data.get("frame", [])
        if f.get("expect", "").lower().replace("_", "") == variant
    })
    if not hits:
        return []
    shown = ", ".join(hits[:6]) + (f", … {len(hits) - 6} more" if len(hits) > 6 else "")
    return [f"{len(hits)} snapshot frame(s), digests pinned in ui-snapshots.sha256: {shown}"]


def prose(path: str) -> list[str]:
    baseline = ROOT / "testing/prose-baseline.json"
    if not path.endswith(".md") or not baseline.exists():
        return []
    recorded = json.loads(baseline.read_text()).get(path)
    if recorded is None:
        return []
    return [f"prose budget — recorded at {recorded:,} words; `obc prose --check` holds it there"]


def specs_for(package: str | None, path: str) -> list[str]:
    if path.startswith("specs/"):
        return ["this file is itself a normative contract; specs/vectors pins its bytes"]
    data = tomllib.loads((ROOT / "testing/suites.toml").read_text())
    for entry in data.get("package", []):
        if entry.get("name") != package:
            continue
        pinned = [t for t in entry.get("triggers", []) if t.startswith("specs/")]
        if pinned:
            return [f"pinned to {t}" for t in pinned]
    return []


def describe(path: str) -> None:
    package = cargo_package(path)
    name = package[0] if package else None

    print(f"\n{path}")
    if not (ROOT / path).exists():
        print("  (not in the repository)")
        return
    if package:
        print(f"  package        {package[0]}  ({package[1]})")

    sections = [
        ("contracts", specs_for(name, path)),
        ("layering", layering(name)),
        ("coverage", coverage(path)),
        ("ui frames", frames(path)),
        ("prose", prose(path)),
    ]
    reached = guards(path)
    scoped = [(g, r) for g, r, w in reached if not w]
    wide = [(g, r) for g, r, w in reached if w]
    if scoped:
        sections.append(("guards", [f"{g} — {r}" for g, r in scoped]))
    for title, lines in sections:
        for i, line in enumerate(lines):
            print(f"  {title if i == 0 else '':<14} {line}")

    selected = suites(path, name)
    for i, (unit, why) in enumerate(selected):
        print(f"  {'tests' if i == 0 else '':<14} {unit} — {why}")

    if wide:
        print("  repository-wide rules that also apply:")
        for g, r in wide:
            print(f"                 {g} — {r}")
    if not any(lines for _, lines in sections) and not selected and not wide:
        print("  nothing declared reaches this path")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("paths", nargs="+", help="repository-relative paths")
    parser.add_argument("--changed", metavar="BASE", help="also describe files changed since BASE")
    args = parser.parse_args()

    paths = list(args.paths)
    if args.changed:
        diff = subprocess.run(
            ["git", "-C", str(ROOT), "diff", "--name-only", args.changed],
            capture_output=True, text=True, check=True,
        ).stdout.split()
        paths += [p for p in diff if p not in paths]

    for path in paths:
        describe(path.lstrip("./"))
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
