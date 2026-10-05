#!/usr/bin/env python3
"""Fail when `host/obc-data` reaches a GPL crate.

`host/obc-data` is MIT OR Apache-2.0, so everything it links must allow that. Every other crate
in this repository is GPL-3.0-only, so a path dependency fails, whatever its manifest says. A
crates.io dependency fails when each alternative of its licence expression is a GPL family
licence (GPL, LGPL or AGPL), or when it declares no licence. Development dependencies do not
link into the crate and are not checked.
"""

from __future__ import annotations

GOVERNS = ['host/obc-data/Cargo.toml', 'Cargo.lock']
RULE = 'host/obc-data is MIT OR Apache-2.0 and depends on no GPL crate.'

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRATE = "obc-data"
LICENCE = "MIT OR Apache-2.0"


def copyleft(expression: str | None) -> bool:
    """Whether a licence expression leaves no choice but a GPL family licence."""
    if not expression:
        return True
    alternatives = re.split(r"\s+OR\s+|/", expression.replace("(", " ").replace(")", " "))
    return all("GPL" in alternative for alternative in alternatives)


def violations(metadata: dict, crate: str = CRATE) -> list[str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root = next(id for id, package in packages.items() if package["name"] == crate and package["source"] is None)
    found = []
    if packages[root].get("license") != LICENCE:
        found.append(f"{crate} declares `{packages[root].get('license')}`, not `{LICENCE}`")
    seen, todo = {root}, [root]
    while todo:
        for dep in nodes[todo.pop()]["deps"]:
            linked = any(kind["kind"] in (None, "build") for kind in dep["dep_kinds"])
            if not linked or dep["pkg"] in seen:
                continue
            seen.add(dep["pkg"])
            todo.append(dep["pkg"])
            package = packages[dep["pkg"]]
            name = f"{package['name']} {package['version']}"
            if package["source"] is None:
                found.append(f"{name} is a crate of this repository, which is GPL-3.0-only")
            elif copyleft(package.get("license")):
                found.append(f"{name} is `{package.get('license') or 'unlicensed'}`")
    return found


def main() -> int:
    output = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")],
        cwd=ROOT, text=True)
    found = violations(json.loads(output))
    if found:
        print(f"{CRATE} must stay {LICENCE}; its dependency graph reaches:", file=sys.stderr)
        for line in found:
            print(f"  {line}", file=sys.stderr)
        return 1
    print(f"{CRATE}: no GPL crate in its dependency graph")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
