#!/usr/bin/env python3
"""Print the Cargo package flags one tier of the `test` job compiles.

The selection plan already says which suites a change requires. This turns that
plan into `-p NAME` flags, so the job compiles the selected packages instead of
the whole workspace. It prints `--workspace` when the change can alter what any
package compiles to, and nothing at all when the tier selected no Rust package.
Without a plan (a local `obc check test`) the answer is the whole workspace.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import suite_registry as registry  # noqa: E402

# A change to one of these can change what every package compiles to, or which
# suites the selector itself produces, so no narrowed package set is trustworthy.
FOUNDATION = registry.RUST_FOUNDATION_PATHS | {
    ".config/nextest.toml",
    ".github/workflows/ci.yml",
    "testing/suites.toml",
    "tools/suite_registry.py",
}


def _is_foundation(path: str) -> bool:
    return path in FOUNDATION or path.startswith("tools/ci/")


def packages(plan: dict, tier: str, root: Path) -> list[str]:
    """The root-workspace packages whose binaries this tier's filter can name."""

    inventory = registry.load_inventory(root)
    graph = registry.build_cargo_graph(root)
    levels = registry.cargo_tier_levels(tier)
    selected = set(plan.get("selected_suite_ids", []))
    names: set[str] = set()
    for suite in inventory.suites:
        if suite["id"] not in selected or suite["level"] not in levels or suite["pull_request"] == "never":
            continue
        for item in inventory.matches[suite["id"]]:
            if item.kind != "rust-target" or item.detail.endswith(":example"):
                continue
            name = item.name.split(":", 1)[0]
            package = graph.packages.get(name)
            if package and package.root_workspace:
                names.add(name)
    return sorted(names)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tier", choices=["fast", "fixtures"], default="fast")
    parser.add_argument("--plan", help="selection-plan JSON file; defaults to OBC_SELECTION_PLAN")
    args = parser.parse_args(argv)

    raw = Path(args.plan).read_text(encoding="utf-8") if args.plan else os.environ.get("OBC_SELECTION_PLAN", "")
    if not raw.strip():
        print("--workspace")
        return 0
    plan = json.loads(raw)
    if any(_is_foundation(path) for path in plan.get("changed_paths", [])):
        print("--workspace")
        return 0
    print(" ".join(f"-p {name}" for name in packages(plan, args.tier, registry.repository_root())))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
