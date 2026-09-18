#!/usr/bin/env python3
"""List the `obc` tasks by group.

`obc` shows every group but `agent`, so the everyday list stays short. Nothing becomes
unreachable: `obc --agent` shows the agent tasks and `obc --all` shows all of them.
A task belongs to `agent` when an automation is its main user.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

AGENT = "agent"
# Groups print in this order; a group not named here prints after them, sorted.
ORDER = ("run", "device", "ios", "maps", "build", "test", "docs", AGENT)


def load(justfile: Path) -> dict[str, tuple[str, str]]:
    """Map each public task name to its (group, doc)."""
    dump = subprocess.run(
        ["just", "--justfile", str(justfile), "--dump", "--dump-format", "json"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    tasks = {}
    for name, recipe in json.loads(dump)["recipes"].items():
        if recipe["private"]:
            continue
        group = ""
        for attribute in recipe["attributes"]:
            if isinstance(attribute, dict) and "group" in attribute:
                group = attribute["group"]
        tasks[name] = (group, recipe["doc"] or "")
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
            doc = tasks[name][1]
            lines.append(f"  {name.ljust(width)}  {doc}".rstrip())
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
