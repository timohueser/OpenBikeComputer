#!/usr/bin/env python3
"""Fail when a record's signature appears outside the changelog.

An issue or pull-request number, or a date, marks a record of what happened. Records live in
the pull request and in the generated changelog. Everywhere else the text states what is,
without its history.

Checked: every tracked Markdown file outside code fences and SVG, and every comment in Rust,
Swift, TypeScript, Svelte, JavaScript and Python (docstrings included). Not checked: the
changelog, the blog, licences and legal pages, vendored code, links to another project's
issues, dates before 2025, and dates inside code comments (test data and examples use them).
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

GOVERNS = ['**/*.md', '**/*.rs', '**/*.swift', '**/*.ts', '**/*.svelte', '**/*.js', '**/*.py']
RULE = 'An issue number or a date belongs in the pull request and the changelog, not in a document or a comment.'

ROOT = Path(__file__).resolve().parents[1]

EXEMPT = (
    "CHANGELOG.md", "THIRD-PARTY.md", "LICENSE", "LICENSE.hardware", "docs/BLOG.md",
    "docs/content/blog/", "docs/content/impressum.md", "docs/content/datenschutz.md",
    "vendor/", "node_modules/", "tools/check_records.py",
)

# `#1234` as an issue reference: not a colour (`fill:#000`, `stroke="#3d3427"`), not an HTML
# entity (`&#9654;`), not an attribute (`#[cfg]`), not an assembly immediate (`sub sp, #128`),
# not a selector or a path segment.
ISSUE = re.compile(r'(?<![:"\'=&#/\w])(?<!, )#\d{3,4}\b(?![\w-])(?!\.\w)')
DATE = re.compile(r'\b20(2[5-9]|[3-9]\d)-\d\d-\d\d\b')
FOREIGN_ISSUE = re.compile(r'https://github\.com/(?!timohueser/OpenBikeComputer)\S+/(issues|pull)/\d+')
FENCE = re.compile(r'^\s*(```|~~~)')
DOC_OPENER = re.compile(r'^\s*(async def |def |class )')
COMMENT_START = re.compile(r'^\s*(//|#(?!!)|\*|/\*)')


def tracked() -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files"], capture_output=True, text=True, check=True
    ).stdout.split("\n")
    return [f for f in out if f and not any(f == e or f.startswith(e) or f"/{e}" in f for e in EXEMPT)]


def comment_lines(path: str, text: str):
    """(line number, text) for every comment line, including Python docstrings."""
    in_doc = False
    previous = ""
    for n, line in enumerate(text.splitlines(), 1):
        if path.endswith(".py"):
            quotes = line.count(chr(34) * 3) + line.count(chr(39) * 3)
            opens = quotes and not in_doc and (n == 1 or DOC_OPENER.match(previous))
            if in_doc or opens:
                yield n, line
                if quotes % 2:
                    in_doc = not in_doc
                if line.strip():
                    previous = line
                continue
            if line.strip():
                previous = line
        if COMMENT_START.match(line) or (path.endswith((".svelte", ".ts", ".js")) and "<!--" in line):
            yield n, line


def main() -> int:
    problems: list[str] = []
    for f in tracked():
        path = ROOT / f
        if not path.is_file():
            continue
        if f.endswith(".md"):
            text = path.read_text(encoding="utf-8", errors="ignore")
            in_svg = in_fence = False
            for n, line in enumerate(text.splitlines(), 1):
                if FENCE.match(line):
                    in_fence = not in_fence
                if "<svg" in line:
                    in_svg = True
                skip = in_svg or in_fence or FOREIGN_ISSUE.search(line)
                # A date in a table cell is a field; a date in prose is a story.
                dated = DATE.search(line) and not line.lstrip().startswith("|")
                if not skip and (ISSUE.search(line) or dated):
                    problems.append(f"{f}:{n}: {line.strip()[:100]}")
                if "</svg>" in line:
                    in_svg = False
        elif f.endswith((".rs", ".swift", ".ts", ".svelte", ".js", ".py")):
            text = path.read_text(encoding="utf-8", errors="ignore")
            for n, line in comment_lines(f, text):
                if ISSUE.search(line):
                    problems.append(f"{f}:{n}: {line.strip()[:100]}")
    if problems:
        print(f"records: {len(problems)} line(s) carry an issue number or a date; move the history to the pull request")
        for p in problems:
            print(f"  {p}")
        return 1
    print("records: no issue numbers or dates outside the changelog")
    return 0


if __name__ == "__main__":
    sys.exit(main())
