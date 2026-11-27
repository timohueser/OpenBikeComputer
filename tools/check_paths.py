#!/usr/bin/env python3
"""Fail when a page points at something that is not there.

`AGENTS.md` and the README of each top-level directory are the memory of the agents that work
here. A page that names a moved file or a removed task misleads its reader, so this guard holds
what a page states about the tree:

- root `AGENTS.md` has at most 150 lines, and each top-level directory has a `README.md` of at
  most 250 lines;
- in each `AGENTS.md` and `README.md`, a backticked repository path exists, from the root or
  from the directory of the file, and a named `obc` task exists;
- in each tracked Markdown file, a relative link resolves;
- each page under `docs/content/` is in `nav.json` or is a blog post, and each `src:` target
  exists. `build_docs.py` cannot see either: it renders only the pages of `nav.json`, and it
  expands `src:` to a GitHub address.

Usage: python3 tools/check_paths.py
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The owner chose both caps: `AGENTS.md` loads into every session, a page is read on demand.
INDEX_LINES = 150
PAGE_LINES = 250

# Third-party text: its paths and commands describe another repository.
THIRD_PARTY = ("/vendor/", "/node_modules/", ".agents/ui/")

FENCE = re.compile(r"^```.*?^```", re.S | re.M)
SPAN = re.compile(r"`([^`\n]+)`")
LINK = re.compile(r"\]\((?:<([^>]+)>|([^\s)]+))(?:\s+[^)]*)?\)")
# Two or more segments of plain path characters. One segment (`fetch.url`, `bom.py`) says too
# little to tell a file from a field.
PATH = re.compile(r"\.?[\w.*-]+(?:/[\w.*-]+)+/?")
TASK = re.compile(r"(?:^|[\s;&|(])(?:\./)?(?:tools/)?obc +([a-z][a-z0-9-]*)", re.M)
RECIPE = re.compile(r"^@?([a-z][a-z0-9-]*)\b[^:=\n]*:(?!=)", re.M)
# Handled by `tools/obc` before `just` sees them.
WRAPPER_TASKS = {"help"}
# `planner/routing` and `device/cells` name a layer or a step of `obc data`, not a path.
LAYER = re.compile(r"(?:device|planner)(?:/[a-z-]+)+")


def git(*args: str, stdin: str | None = None) -> str:
    return subprocess.run(
        ["git", "-C", str(ROOT), *args], input=stdin, capture_output=True, text=True
    ).stdout


def tracked() -> list[str]:
    return [f for f in git("ls-files").split("\n") if f and (ROOT / f).exists()]


def tasks() -> set[str]:
    return set(RECIPE.findall((ROOT / "tools/justfile").read_text())) | WRAPPER_TASKS


def is_page(f: str) -> bool:
    return os.path.basename(f) in ("AGENTS.md", "README.md") and not any(
        part in f"/{f}" for part in THIRD_PARTY
    )


def areas(files: list[str]) -> list[str]:
    """The top-level directories: each has an area page."""
    return sorted({f.split("/")[0] for f in files if "/" in f and not f.startswith(".")})


def caps(files: list[str]) -> list[str]:
    problems = []
    for page, cap in [("AGENTS.md", INDEX_LINES)] + [(f"{a}/README.md", PAGE_LINES) for a in areas(files)]:
        if not (ROOT / page).exists():
            problems.append(f"{page}: each top-level directory needs this page")
            continue
        lines = len((ROOT / page).read_text(encoding="utf-8").splitlines())
        if lines > cap:
            problems.append(f"{page}: {lines} lines, the cap is {cap}")
    return problems


def page_paths(f: str, text: str, top: set[str]) -> list[str]:
    """Backticked paths that start at a real directory and end nowhere."""
    here = os.path.dirname(f)
    missing = []
    for span in SPAN.findall(FENCE.sub("", text)):
        if not PATH.fullmatch(span) or LAYER.fullmatch(span):
            continue
        first = span.split("/")[0]
        bases = [b for b in ("", here) if first in top or (ROOT / b / first).exists()]
        if not bases:
            continue  # Not anchored in the tree: a URL path, a layer name, a path inside a package.
        if not any(any(True for _ in (ROOT / b).glob(span.rstrip("/"))) for b in bases):
            missing.append(span)
    # A build output is named on purpose and is not in the tree. An ignore rule for a directory
    # matches a missing path only with its slash.
    spellings = {
        m: [p + end for b in ("", here) for end in ("", "/")
            if not (p := os.path.normpath(os.path.join(b, m))).startswith("..")]
        for m in missing
    }
    asked = sorted({p for group in spellings.values() for p in group})
    ignored = set(git("check-ignore", "--stdin", stdin="\n".join(asked)).split("\n")) if asked else set()
    return [f"{f}: `{m}` does not exist" for m in missing if not ignored.intersection(spellings[m])]


def page_tasks(f: str, text: str, known: set[str]) -> list[str]:
    code = "\n".join(SPAN.findall(FENCE.sub("", text)) + FENCE.findall(text))
    return [
        f"{f}: `obc {name}` is not a task of tools/justfile"
        for name in sorted(set(TASK.findall(code)) - known)
    ]


def links(f: str, text: str) -> list[str]:
    problems = []
    # A link inside code is an example of syntax, not a link.
    text = SPAN.sub("", FENCE.sub("", text))
    for m in re.finditer(r"\[src:[^\]]+\](?!\()", text):
        problems.append(f"{f}: {m.group(0)} is not a link; write [text](src:path)")
    for m in re.finditer(r"\]\((src|spec):([^\s)#]+)", text):
        scheme, target = m.groups()
        if scheme == "spec":
            problems.append(f"{f}: `spec:` is not a link scheme; use `src:specs/{target}`")
        elif not (ROOT / target).exists():
            problems.append(f"{f}: src:{target} does not exist")
    if f.startswith("docs/content/"):
        return problems  # `build_docs.py --check-links` owns the links of the rendered guide.
    for m in LINK.finditer(text):
        target = (m.group(1) or m.group(2) or "").split("#")[0]
        if not target or target.endswith("/") or target.startswith(("http", "mailto:", "/", "src:", "spec:")):
            continue
        if not (ROOT / os.path.normpath(os.path.join(os.path.dirname(f), target))).exists():
            problems.append(f"{f}: link to {target} does not resolve")
    return problems


def unpublished(files: list[str]) -> list[str]:
    nav = json.loads((ROOT / "docs/content/nav.json").read_text())
    published = {p["path"] for section in nav["sections"] for p in section["pages"]}
    problems = []
    for f in files:
        if f.startswith("docs/content/") and f.endswith(".md"):
            stem = f[len("docs/content/"):-len(".md")]
            if stem not in published and not re.fullmatch(r"blog/[^/]+/index", stem):
                problems.append(f"{f}: under docs/content/ but nav.json does not publish it")
    return problems


def check() -> list[str]:
    files = tracked()
    top = {f.split("/")[0] for f in files}
    known = tasks()
    problems = caps(files) + unpublished(files)
    for f in sorted(files):
        if not f.endswith(".md") or any(part in f"/{f}" for part in THIRD_PARTY):
            continue
        text = (ROOT / f).read_text(encoding="utf-8", errors="ignore")
        problems += links(f, text)
        if is_page(f):
            problems += page_paths(f, text, top) + page_tasks(f, text, known)
    return problems


def main() -> int:
    problems = check()
    if problems:
        print(f"paths: {len(problems)} problem(s)")
        for p in problems:
            print(f"  {p}")
        return 1
    print("paths: every page, link and named task resolves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
