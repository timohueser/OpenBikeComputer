#!/usr/bin/env python3
"""List the `obc` tasks by group, or describe one task.

`obc` shows every group but `agent`, so the everyday list stays short. Nothing becomes
unreachable: `obc --agent` shows the agent tasks and `obc --all` shows all of them.
A task belongs to `agent` when an automation is its main user. `obc help TASK` prints the
whole comment block above the recipe, whose last line is the summary the listing shows.

The justfile is read as text, not through `just`. The CI runners have no `just`, and the
listing needs three line shapes: a comment, an attribute and a recipe header.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import NamedTuple

AGENT = "agent"
# Groups print in this order; a group not named here prints after them, sorted.
ORDER = ("run", "device", "ios", "maps", "build", "test", "docs", AGENT)

# A recipe header at column 0: a name, optional parameters and dependencies, then a colon.
# `x := y` and `set shell := [...]` do not match, because `=` is not whitespace or a comment.
RECIPE = re.compile(r"^([a-z][a-z0-9-]*)(?:[ \t]+[^:=\n]*?)?:[ \t]*(?:#.*)?$")
ATTRIBUTE = re.compile(r"^\[(.+)\][ \t]*$")
GROUP = re.compile(r"""group\([ \t]*['"]([^'"]+)['"][ \t]*\)""")


class Task(NamedTuple):
    group: str
    block: tuple[str, ...]

    @property
    def doc(self) -> str:
        """The listing summary: the last comment line, as `just --list` shows it."""
        return self.block[-1].strip() if self.block else ""


def load(justfile: Path) -> dict[str, Task]:
    """Map each public task name to its group and comment block.

    `just` documents a recipe with the block of comment lines directly above it. An empty
    line or any other statement breaks that association.
    """
    tasks: dict[str, Task] = {}
    block: list[str] = []
    attributes = ""
    for line in justfile.read_text(encoding="utf-8").splitlines():
        recipe = RECIPE.match(line)
        if recipe:
            if "private" not in attributes:
                group = GROUP.search(attributes)
                tasks[recipe.group(1)] = Task(group.group(1) if group else "", tuple(block))
            block, attributes = [], ""
            continue
        attribute = ATTRIBUTE.match(line)
        if attribute:
            attributes += attribute.group(1) + " "
        elif line.startswith("#"):
            block.append(line[1:].removeprefix(" ").rstrip())
        elif not line[:1].isspace() or not line.strip():
            block, attributes = [], ""
    if not tasks:
        raise SystemExit(f"obc: no task found in {justfile}")
    return tasks


def grouped(tasks: dict[str, Task], keep) -> list[tuple[str, list[str]]]:
    names: dict[str, list[str]] = {}
    for name, task in sorted(tasks.items()):
        if keep(task.group):
            names.setdefault(task.group, []).append(name)
    rank = {group: index for index, group in enumerate(ORDER)}
    return sorted(names.items(), key=lambda row: (rank.get(row[0], len(ORDER)), row[0]))


def render(tasks, keep, pointer: str = "") -> str:
    lines = []
    width = max((len(name) for name, task in tasks.items() if keep(task.group)), default=0)
    for group, names in grouped(tasks, keep):
        lines.append(f"\n{group or 'other'}")
        for name in names:
            lines.append(f"  {name.ljust(width)}  {tasks[name].doc}".rstrip())
    if pointer:
        lines.append(f"\n{pointer}")
    return "\n".join(lines).lstrip("\n")


def describe(name: str, task: Task) -> str:
    """The summary as a heading, then the rest of the comment block."""
    lines = [f"obc {name}  {task.doc}".rstrip()]
    body = [f"  {line}".rstrip() for line in task.block[:-1]]
    if body:
        lines += ["", *body]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--justfile", type=Path, default=Path(__file__).parent / "justfile")
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument("--agent", action="store_true", help="the agent tasks only")
    scope.add_argument("--all", action="store_true", help="every task")
    parser.add_argument("--names", action="store_true", help="bare names, for completion")
    parser.add_argument("--task", metavar="NAME", help="describe one task")
    args = parser.parse_args()

    tasks = load(args.justfile)
    if args.task:
        if args.task not in tasks:
            print(f"obc: no task named {args.task}; `obc --all` lists them", file=sys.stderr)
            return 1
        print(describe(args.task, tasks[args.task]))
        return 0
    if args.all:
        keep, pointer = (lambda _: True), ""
    elif args.agent:
        keep, pointer = (lambda group: group == AGENT), ""
    else:
        keep, pointer = (lambda group: group != AGENT), "agent tasks: obc --agent"

    if args.names:
        print(" ".join(sorted(name for name, task in tasks.items() if keep(task.group))))
    else:
        print(render(tasks, keep, pointer))
    return 0


if __name__ == "__main__":
    sys.exit(main())
