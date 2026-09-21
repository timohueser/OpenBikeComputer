#!/usr/bin/env python3
"""Hold written prose to a budget, the way every other resource here is held to one.

RAM, flash, stack, coverage and the storage line count all have a ceiling. Prose had none,
which is the whole reason a gate file grew an 80-entry changelog and the guide grew pages no
one can read in a sitting. The budgets below are ratchets: a file over its cap must not grow,
a file under it must not cross. `--update` records a file that shrank and never one that grew.

Three structural rules travel with the budgets, each covering a gap nothing else checks:

- every page under `docs/content/` is published, meaning `nav.json` lists it or it is a blog post;
- every `src:` target in the guide exists, which `build_docs.py` cannot see because it expands
  them to external GitHub URLs;
- every relative file link in *tracked* Markdown resolves, including under `docs/assets/`, which
  the documentation gate never walks.

Usage: python3 tools/prose.py [--check | --update | --report]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / "testing" / "prose-baseline.json"

# `policy` is capped hardest because an agent loads those files on every turn.
CAPS = {"guide": 2500, "readme": 800, "guard": 150, "policy": 2000}

POLICY = ("CLAUDE.md", "CONTRIBUTING.md", "docs/testing.md", "docs/README.md")

SVG = re.compile(r"<svg.*?</svg>", re.S)
FENCE = re.compile(r"```.*?```", re.S)
COMMENT = re.compile(r"<!--.*?-->", re.S)
FRONT = re.compile(r"\A---\n.*?\n---\n", re.S)
LINK = re.compile(r"\]\((?:<([^>]+)>|([^\s)]+))(?:\s+[^)]*)?\)")
CODE = re.compile(r"`[^`\n]*`")
DOCSTRING = re.compile(r'\A(?:#![^\n]*\n)?\s*"""(.*?)"""', re.S)


def tracked() -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files"], capture_output=True, text=True, check=True
    ).stdout.split("\n")
    return [f for f in out if f and (ROOT / f).exists()]


def words(text: str) -> int:
    """Count prose words: diagrams, code blocks, front matter and comments are not prose."""
    for pattern in (SVG, FENCE, COMMENT, FRONT):
        text = pattern.sub("", text)
    return len(text.split())


def published() -> set[str]:
    nav = json.loads((ROOT / "docs/content/nav.json").read_text())
    return {p["path"] for section in nav["sections"] for p in section["pages"]}


def corpus() -> dict[str, tuple[str, int]]:
    """Every budgeted file, as path -> (kind, word count)."""
    found: dict[str, tuple[str, int]] = {}
    nav = published()
    for f in tracked():
        path = ROOT / f
        if f in POLICY:
            found[f] = ("policy", words(path.read_text(encoding="utf-8", errors="ignore")))
        elif f.startswith("docs/content/") and f.endswith(".md"):
            stem = f[len("docs/content/") : -len(".md")]
            if stem in nav:
                found[f] = ("guide", words(path.read_text(encoding="utf-8", errors="ignore")))
        elif os.path.basename(f) == "README.md" and "vendor/" not in f:
            found[f] = ("readme", words(path.read_text(encoding="utf-8", errors="ignore")))
        elif re.fullmatch(r"tools/check_[a-z_]+\.py", f):
            m = DOCSTRING.match(path.read_text(encoding="utf-8", errors="ignore"))
            found[f] = ("guard", words(m.group(1)) if m else 0)
    return found


def structure() -> list[str]:
    """The three rules that no other gate covers."""
    problems: list[str] = []
    files = set(tracked())
    nav = published()

    for f in files:
        if not (f.startswith("docs/content/") and f.endswith(".md")):
            continue
        stem = f[len("docs/content/") : -len(".md")]
        if stem not in nav and not re.fullmatch(r"blog/[^/]+/index", stem):
            problems.append(f"{f}: under docs/content/ but nav.json does not publish it")

    for f in sorted(files):
        if not f.endswith(".md") or f.startswith("firmware/obc-fw-nrf54l/vendor/"):
            continue
        text = (ROOT / f).read_text(encoding="utf-8", errors="ignore")
        # A link inside code is an example of syntax, not a link.
        text = CODE.sub("", FENCE.sub("", text))
        for m in re.finditer(r"\]\((src|spec):([^\s)]+)\)", text):
            scheme, target = m.group(1), m.group(2)
            if scheme == "spec":
                problems.append(f"{f}: `spec:` is not a link scheme; use `src:specs/{target}`")
            elif not (ROOT / target).exists():
                problems.append(f"{f}: src:{target} does not exist")
        if f.startswith("docs/content/"):
            # build_docs.py --check-links owns the rendered guide's own links.
            continue
        for m in LINK.finditer(text):
            target = (m.group(1) or m.group(2) or "").split("#")[0]
            if not target or target.endswith("/"):
                continue
            if target.startswith(("http", "mailto:", "/", "src:", "spec:")):
                continue
            resolved = os.path.normpath(os.path.join(os.path.dirname(f), target))
            if not (ROOT / resolved).exists():
                problems.append(f"{f}: link to {target} does not resolve")
    return problems


def budgets(found: dict[str, tuple[str, int]], base: dict[str, int]) -> list[str]:
    over: list[str] = []
    for f, (kind, count) in sorted(found.items()):
        allowed = max(CAPS[kind], base.get(f, 0))
        if count > allowed:
            reason = "over cap" if allowed == CAPS[kind] else "grew past its recorded size"
            over.append(f"{f}: {count:,} words, {reason} ({allowed:,})")
    return over


def report(found: dict[str, tuple[str, int]], base: dict[str, int]) -> None:
    for kind, cap in CAPS.items():
        rows = sorted(
            ((c, f) for f, (k, c) in found.items() if k == kind), reverse=True
        )
        total = sum(c for c, _ in rows)
        print(f"\n{kind} (cap {cap:,} words) — {len(rows)} files, {total:,} words")
        for count, f in rows[:8]:
            flag = "  OVER" if count > max(cap, base.get(f, 0)) else ""
            print(f"  {count:>6,}  {f}{flag}")
        if len(rows) > 8:
            print(f"  … {len(rows) - 8} more")

    # Specs are reported, never gated: capping words on a byte-layout contract would push out
    # tables, not rationale. Words are the signal — the line ratio barely moves when prose is cut
    # and every table stays.
    print("\nspec prose words (reported, not gated)")
    for spec in sorted((ROOT / "specs").glob("*.md")):
        text = spec.read_text(encoding="utf-8")
        body = FENCE.sub("", text)
        prose = " ".join(
            line for line in body.splitlines()
            if not line.strip().startswith(("|", "#"))
        )
        flag = "  ← worth a pass" if len(prose.split()) > 6000 else ""
        print(f"  {len(prose.split()):>6,}  {spec.name}{flag}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--check", action="store_true", help="fail on a violation (the CI gate)")
    group.add_argument(
        "--update", nargs="*", metavar="PATH",
        help="with no path, record every file that shrank; with a path, re-record that file "
             "at its current size even if it grew, which makes raising a budget a visible diff",
    )
    group.add_argument("--report", action="store_true", help="print the current sizes")
    args = parser.parse_args()

    found = corpus()
    base = json.loads(BASELINE.read_text()) if BASELINE.exists() else {}

    if args.update is not None:
        deliberate = set(args.update)
        unknown = deliberate - found.keys()
        if unknown:
            print("prose: not a budgeted file: " + ", ".join(sorted(unknown)))
            return 1
        merged = {
            f: c if f in deliberate else min(c, base.get(f, c))
            for f, (_, c) in found.items()
        }
        BASELINE.write_text(json.dumps(dict(sorted(merged.items())), indent=2) + "\n")
        print(f"prose baseline: {len(merged)} files recorded")
        return 0

    if args.report or not args.check:
        report(found, base)
        return 0

    problems = structure() + budgets(found, base)
    if problems:
        print("prose: %d problem(s)" % len(problems))
        for p in problems:
            print(f"  {p}")
        return 1
    total = sum(c for _, c in found.values())
    print(f"prose: {len(found)} files, {total:,} words, every budget and link holds")
    return 0


if __name__ == "__main__":
    sys.exit(main())
