#!/usr/bin/env python3
"""List the `obc` tasks by group.

`obc` shows every group but `agent`, so the everyday list stays short. Nothing becomes
unreachable: `obc --agent` shows the agent tasks and `obc --all` shows all of them.
A task belongs to `agent` when an automation is its main user.

The justfile is read as text, not through `just`. The CI runners have no `just`, and the
listing needs three line shapes: a comment, an attribute and a recipe header.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

AGENT = "agent"
# Groups print in this order; a group not named here prints after them, sorted.
ORDER = ("run", "device", "ios", "maps", "build", "test", "docs", AGENT)

# A recipe header at column 0: a name, optional parameters and dependencies, then a colon.
# `x := y` and `set shell := [...]` do not match, because `=` is not whitespace or a comment.
RECIPE = re.compile(r"^([a-z][a-z0-9-]*)(?:[ \t]+[^:=\n]*?)?:[ \t]*(?:#.*)?$")
ATTRIBUTE = re.compile(r"^\[(.+)\][ \t]*$")
GROUP = re.compile(r"""group\([ \t]*['"]([^'"]+)['"][ \t]*\)""")


def load(justfile: Path) -> dict[str, tuple[str, str]]:
    """Map each public task name to its (group, doc).

    `just` documents a recipe with the comment line directly above it, and the last line of a
    block of comments wins. An empty line or any other statement breaks that association.
    """
    tasks: dict[str, tuple[str, str]] = {}
    doc, attributes = "", ""
    for line in justfile.read_text(encoding="utf-8").splitlines():
        recipe = RECIPE.match(line)
        if recipe:
            if "private" not in attributes:
                group = GROUP.search(attributes)
                tasks[recipe.group(1)] = (group.group(1) if group else "", doc)
            doc, attributes = "", ""
            continue
        attribute = ATTRIBUTE.match(line)
        if attribute:
            attributes += attribute.group(1) + " "
        elif line.startswith("#"):
            doc = line.lstrip("#").strip()
        elif not line[:1].isspace() or not line.strip():
            doc, attributes = "", ""
    if not tasks:
        raise SystemExit(f"obc: no task found in {justfile}")
    return tasks


def grouped(tasks: dict[str, tuple[str, str]], keep) -> list[tuple[str, list[str]]]:
    names: dict[str, list[str]] = {}
    for name, (group, _) in sorted(tasks.items()):
        if keep(group):
            names.setdefault(group, []).append(name)
    rank = {group: index for index, group in enumerate(ORDER)}
    return sorted(names.items(), key=lambda row: (rank.get(row[0], len(ORDER)), row[0]))


def render(tasks, keep, pointer: str = "") -> str:
    lines = []
    width = max((len(name) for name, (group, _) in tasks.items() if keep(group)), default=0)
    for group, names in grouped(tasks, keep):
        lines.append(f"\n{group or 'other'}")
        for name in names:
            lines.append(f"  {name.ljust(width)}  {tasks[name][1]}".rstrip())
    if pointer:
        lines.append(f"\n{pointer}")
    return "\n".join(lines).lstrip("\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--justfile", type=Path, default=Path(__file__).parent / "justfile")
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument("--agent", action="store_true", help="the agent tasks only")
    scope.add_argument("--all", action="store_true", help="every task")
    parser.add_argument("--names", action="store_true", help="bare names, for completion")
    args = parser.parse_args()

    tasks = load(args.justfile)
    if args.all:
        keep, pointer = (lambda _: True), ""
    elif args.agent:
        keep, pointer = (lambda group: group == AGENT), ""
    else:
        keep, pointer = (lambda group: group != AGENT), "agent tasks: obc --agent"

    if args.names:
        print(" ".join(sorted(name for name, (group, _) in tasks.items() if keep(group))))
    else:
        print(render(tasks, keep, pointer))
    return 0


if __name__ == "__main__":
    sys.exit(main())
