#!/usr/bin/env python3
"""Reject forbidden production dependency edges in the firmware workspace.

Rules are group-to-group so later architecture issues can tighten one allowlist entry instead of
rewriting a graph snapshot. Development-only edges are deliberately ignored: test fixtures may
depend on their consumers, while production `normal`/`build` dependencies must point downward.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


HERE = Path(__file__).resolve().parent
DEFAULT_RULES = HERE / "dependency_rules.json"


class DependencyError(RuntimeError):
    pass


@dataclass(frozen=True, order=True)
class Edge:
    source: str
    target: str


def load_json(path: Path) -> dict[str, object]:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise DependencyError(f"cannot read {path}: {error}") from error
    return value


def workspace_packages(metadata: dict[str, object]) -> set[str]:
    members = set(metadata.get("workspace_members", ()))
    packages = [package for package in metadata.get("packages", ()) if package["id"] in members]
    return {package["name"] for package in packages}


def local_edges(metadata: dict[str, object]) -> set[Edge]:
    members = set(metadata.get("workspace_members", ()))
    packages = [package for package in metadata.get("packages", ()) if package["id"] in members]
    local_names = {package["name"] for package in packages}
    edges: set[Edge] = set()
    for package in packages:
        for dependency in package.get("dependencies", ()):
            # Cargo emits `None` for normal dependencies and "build"/"dev" for the other kinds.
            if dependency.get("kind") == "dev":
                continue
            target = dependency["name"]
            if target in local_names:
                edges.add(Edge(package["name"], target))
    return edges


def group_index(rules: dict[str, object]) -> dict[str, str]:
    index: dict[str, str] = {}
    for group, packages in rules.get("groups", {}).items():
        for package in packages:
            if package in index:
                raise DependencyError(
                    f"dependency rules are ambiguous: `{package}` is in both `{index[package]}` and `{group}`"
                )
            index[package] = group
    return index


def validate_rules(rules: dict[str, object]) -> dict[str, str]:
    groups = group_index(rules)
    group_names = set(rules.get("groups", ()))
    forbidden_pairs: set[tuple[str, str]] = set()
    for item in rules.get("forbidden", ()):
        source = item["from_group"]
        target = item["to_group"]
        for role, group in (("from_group", source), ("to_group", target)):
            if group not in group_names:
                raise DependencyError(
                    f"dependency rule references unknown {role} `{group}`; declared groups: "
                    + ", ".join(sorted(group_names))
                )
        pair = (source, target)
        if pair in forbidden_pairs:
            raise DependencyError(f"duplicate forbidden dependency pair `{source} -> {target}`")
        forbidden_pairs.add(pair)

    exception_edges: set[Edge] = set()
    for item in rules.get("exceptions", ()):
        edge = Edge(item["from"], item["to"])
        if edge in exception_edges:
            raise DependencyError(f"duplicate dependency exception `{edge.source} -> {edge.target}`")
        exception_edges.add(edge)
        for package in (edge.source, edge.target):
            if package not in groups:
                raise DependencyError(f"dependency exception references unclassified package `{package}`")
    return groups


def check_edges(edges: set[Edge], rules: dict[str, object], packages: set[str] | None = None) -> list[str]:
    groups = validate_rules(rules)
    exceptions = {
        Edge(item["from"], item["to"]): item for item in rules.get("exceptions", ())
    }
    forbidden = {
        (item["from_group"], item["to_group"]): item["reason"]
        for item in rules.get("forbidden", ())
    }
    violations: list[str] = []
    excepted: list[str] = []

    if packages is not None:
        unclassified = sorted(packages - groups.keys())
        if unclassified:
            violations.append(
                "unclassified production workspace package(s): "
                + ", ".join(f"`{package}`" for package in unclassified)
                + "; add each package to exactly one dependency group before merging"
            )

    for edge in sorted(edges):
        pair = (groups.get(edge.source), groups.get(edge.target))
        reason = forbidden.get(pair)
        if reason is None:
            continue
        exception = exceptions.get(edge)
        if exception is not None:
            excepted.append(f"{edge.source} -> {edge.target} ({exception['issue']})")
            continue
        violations.append(
            f"forbidden dependency edge `{edge.source} -> {edge.target}` "
            f"({pair[0]} -> {pair[1]}): {reason}"
        )

    for edge, exception in sorted(exceptions.items()):
        if edge not in edges:
            violations.append(
                f"stale dependency exception `{edge.source} -> {edge.target}` ({exception['issue']}): "
                "the edge is gone; remove the exception to tighten the allowlist"
            )
        elif (groups.get(edge.source), groups.get(edge.target)) not in forbidden:
            violations.append(
                f"invalid dependency exception `{edge.source} -> {edge.target}`: no forbidden group rule matches it"
            )
    if excepted:
        print("temporary dependency exceptions:")
        for line in excepted:
            print(f"  {line}")
    return violations


def cargo_metadata(manifest: Path) -> dict[str, object]:
    try:
        output = subprocess.run(
            [
                "cargo",
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--no-deps",
                "--manifest-path",
                str(manifest),
            ],
            check=True,
            text=True,
            capture_output=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "").strip() or str(error)
        raise DependencyError(f"cargo metadata failed: {detail}") from error
    return json.loads(output)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--rules", type=Path, default=DEFAULT_RULES)
    result.add_argument("--manifest-path", type=Path, default=HERE.parent / "Cargo.toml")
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        rules = load_json(args.rules)
        if rules.get("schema_version") != 1:
            raise DependencyError("unsupported dependency rule schema; expected schema_version 1")
        metadata = cargo_metadata(args.manifest_path)
        packages = workspace_packages(metadata)
        edges = local_edges(metadata)
        violations = check_edges(edges, rules, packages)
        if violations:
            raise DependencyError("\n".join(violations))
    except (DependencyError, json.JSONDecodeError, KeyError, TypeError) as error:
        print(f"dependency check failed: {error}", file=sys.stderr)
        return 1
    print(f"dependency direction check passed ({len(edges)} production workspace edges)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
