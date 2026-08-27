#!/usr/bin/env python3
"""Track human-owned prose in the public documentation source."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
CONTENT = ROOT / "docs" / "content"
COPY_STATES = ("ai", "mixed", "human")
HUMAN_START = "<!-- human-copy:start -->"
HUMAN_END = "<!-- human-copy:end -->"
REVIEW_RE = re.compile(r"<!--\s*copy-review:\s*(.*?)-->", re.DOTALL)
FRONT_MATTER_RE = re.compile(r"^---\s*\n(.*?)\n---\s*\n", re.DOTALL)
HEADING_RE = re.compile(r"^#{1,6}\s+(.+?)\s*$")


@dataclass(frozen=True)
class Review:
    line: int
    heading: str
    note: str


@dataclass(frozen=True)
class Page:
    path: str
    copy: str
    human_blocks: int
    reviews: tuple[Review, ...]


def front_matter(text: str) -> tuple[dict[str, str], int]:
    match = FRONT_MATTER_RE.match(text)
    if not match:
        return {}, 0
    fields = {}
    for line in match.group(1).splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            fields[key.strip()] = value.strip().strip('"')
    return fields, match.end()


def nearest_heading(lines: list[str], line: int) -> str:
    for candidate in reversed(lines[: line - 1]):
        match = HEADING_RE.match(candidate)
        if match:
            return match.group(1)
    return "Page introduction"


def parse_page(path: Path, content_root: Path) -> tuple[Page | None, list[str]]:
    text = path.read_text(encoding="utf-8")
    relative = path.relative_to(content_root).as_posix()
    fields, body_start = front_matter(text)
    state = fields.get("copy", "")
    errors = []
    if state not in COPY_STATES:
        expected = "|".join(COPY_STATES)
        errors.append(f"{relative}: front matter needs copy: {expected}")
        return None, errors

    lines = text.splitlines()
    open_line = None
    human_blocks = 0
    for line_number, line in enumerate(lines, 1):
        marker = line.strip()
        if marker == HUMAN_START:
            if open_line is not None:
                errors.append(f"{relative}:{line_number}: nested human-copy block")
            else:
                open_line = line_number
        elif marker == HUMAN_END:
            if open_line is None:
                errors.append(f"{relative}:{line_number}: human-copy:end has no start")
            else:
                protected = "\n".join(lines[open_line: line_number - 1]).strip()
                if not protected:
                    errors.append(f"{relative}:{open_line}: human-copy block is empty")
                human_blocks += 1
                open_line = None
    if open_line is not None:
        errors.append(f"{relative}:{open_line}: human-copy block has no end")

    if state == "mixed" and human_blocks == 0:
        errors.append(f"{relative}: copy: mixed needs at least one human-copy block")
    if state != "mixed" and human_blocks:
        errors.append(f"{relative}: human-copy blocks require copy: mixed")

    reviews = []
    body = text[body_start:]
    body_line = text[:body_start].count("\n") + 1
    for match in REVIEW_RE.finditer(body):
        note = " ".join(part.strip() for part in match.group(1).splitlines() if part.strip())
        line = body_line + body[: match.start()].count("\n")
        if not note:
            errors.append(f"{relative}:{line}: copy-review note is empty")
            continue
        reviews.append(Review(line, nearest_heading(lines, line), note))
    if state == "ai" and reviews:
        errors.append(f"{relative}: copy-review notes are only valid for mixed or human copy")

    return Page(relative, state, human_blocks, tuple(reviews)), errors


def scan(content_root: Path = CONTENT) -> tuple[list[Page], list[str]]:
    pages = []
    errors = []
    for path in sorted(content_root.rglob("*.md")):
        page, page_errors = parse_page(path, content_root)
        if page is not None:
            pages.append(page)
        errors.extend(page_errors)
    if not pages and not errors:
        errors.append(f"{content_root}: no Markdown pages found")
    return pages, errors


def print_errors(errors: list[str]) -> None:
    for error in errors:
        print(f"error: {error}", file=sys.stderr)


def print_status(pages: list[Page]) -> None:
    human_blocks = sum(page.human_blocks for page in pages)
    reviews = [(page, review) for page in pages for review in page.reviews]
    print("Documentation copy status")
    print(f"  Human:   {sum(page.copy == 'human' for page in pages)}")
    print(f"  Mixed:   {sum(page.copy == 'mixed' for page in pages)} ({human_blocks} human-owned blocks)")
    print(f"  AI draft: {sum(page.copy == 'ai' for page in pages)}")
    print(f"  Review:  {len(reviews)}")

    for state, title in (("human", "Human"), ("mixed", "Mixed"), ("ai", "AI draft")):
        matches = [page.path for page in pages if page.copy == state]
        if matches:
            print(f"\n{title}")
            for path in matches:
                print(f"  - {path}")

    print("\nNeeds review")
    if not reviews:
        print("  None")
    for page, review in reviews:
        print(f"  - {page.path}:{review.line} — {review.heading}")
        print(f"    {review.note}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=("status", "check"), default="status")
    parser.add_argument("--content", type=Path, default=CONTENT, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)

    pages, errors = scan(args.content)
    if errors:
        print_errors(errors)
        return 1
    if args.command == "status":
        print_status(pages)
    else:
        review_count = sum(len(page.reviews) for page in pages)
        print(f"docs copy: {len(pages)} pages valid, {review_count} need review")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
