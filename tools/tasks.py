#!/usr/bin/env python3
"""List tasks from native just metadata and show a recipe without executing it."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple

AGENT = "agent"
# Groups print in this order; a group not named here prints after them, sorted.
ORDER = ("run", "device", "ios", "maps", "build", "test", "docs", AGENT)

class Task(NamedTuple):
    group: str
    doc: str


def load(justfile: Path) -> dict[str, Task]:
    document = json.loads(subprocess.check_output(
        ["just", "--justfile", str(justfile), "--dump", "--dump-format", "json"], text=True,
    ))
    return {
        name: Task(
            next((attribute["group"] for attribute in recipe["attributes"]
                  if isinstance(attribute, dict) and "group" in attribute), ""),
            recipe.get("doc") or "",
        )
        for name, recipe in document["recipes"].items()
        if not recipe["private"]
    }


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
            lines.append(f"  {name.ljust(width)}  {tasks[name].doc.splitlines()[0] if tasks[name].doc else ''}".rstrip())
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
    parser.add_argument("--task", metavar="NAME", help="describe one task")
    args = parser.parse_args()

    if args.task:
        return subprocess.run(["just", "--justfile", str(args.justfile), "--show", args.task]).returncode
    try:
        tasks = load(args.justfile)
    except subprocess.CalledProcessError as error:
        return error.returncode
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
