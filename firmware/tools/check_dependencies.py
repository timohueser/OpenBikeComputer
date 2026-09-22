#!/usr/bin/env python3
"""Reject forbidden production dependency edges and features in the firmware workspace.

Rules are group-to-group so later architecture issues can tighten one allowlist entry instead of
rewriting a graph snapshot. Development-only edges are deliberately ignored: test fixtures may
depend on their consumers, while production `normal`/`build` dependencies must point downward.
Feature rules inspect the effective dependency graph of the Cargo root they name.
"""

from __future__ import annotations

GOVERNS = ['**/Cargo.toml', 'firmware/tools/dependency_rules.json']
RULE = 'Production dependencies point downward, and forbidden dependency features stay disabled.'

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


def local_edges(metadata: dict[str, object], local_names: set[str] | None = None) -> set[Edge]:
    members = set(metadata.get("workspace_members", ()))
    packages = [package for package in metadata.get("packages", ()) if package["id"] in members]
    if local_names is None:
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


def dependency_graph(metadatas: list[dict[str, object]]) -> tuple[set[str], set[Edge]]:
    """Combine workspace and standalone metadata into one production package graph."""
    packages = set().union(*(workspace_packages(metadata) for metadata in metadatas))
    edges = set().union(*(local_edges(metadata, packages) for metadata in metadatas))
    return packages, edges


def metadata_manifests(primary: Path, rules: dict[str, object]) -> list[Path]:
    """Resolve every Cargo root that participates in the production graph."""
    manifests = [primary]
    for relative in rules.get("standalone_manifests", ()):
        manifest = primary.parent / relative
        if manifest not in manifests:
            manifests.append(manifest)
    return manifests


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

    return groups


def check_edges(edges: set[Edge], rules: dict[str, object], packages: set[str] | None = None) -> list[str]:
    groups = validate_rules(rules)
    forbidden = {
        (item["from_group"], item["to_group"]): item["reason"]
        for item in rules.get("forbidden", ())
    }
    violations: list[str] = []

    if packages is not None:
        unclassified = sorted(packages - groups.keys())
        if unclassified:
            violations.append(
                "unclassified production package(s): "
                + ", ".join(f"`{package}`" for package in unclassified)
                + "; add each package to exactly one dependency group before merging"
            )

    for edge in sorted(edges):
        pair = (groups.get(edge.source), groups.get(edge.target))
        reason = forbidden.get(pair)
        if reason is None:
            continue
        violations.append(
            f"forbidden dependency edge `{edge.source} -> {edge.target}` "
            f"({pair[0]} -> {pair[1]}): {reason}"
        )

    return violations


def cargo_metadata(manifest: Path, *, include_dependencies: bool = False) -> dict[str, object]:
    command = [
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--manifest-path",
        str(manifest),
    ]
    if not include_dependencies:
        command.append("--no-deps")
    try:
        output = subprocess.run(
            command,
            check=True,
            text=True,
            capture_output=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "").strip() or str(error)
        raise DependencyError(f"cargo metadata failed: {detail}") from error
    return json.loads(output)


def check_forbidden_features(metadata: dict[str, object], policies: list[dict[str, object]]) -> list[str]:
    packages = metadata.get("packages", ())
    nodes = {node["id"]: node for node in metadata.get("resolve", {}).get("nodes", ())}
    violations: list[str] = []

    for policy in policies:
        package_name = policy["package"]
        matches = [package for package in packages if package["name"] == package_name]
        if len(matches) != 1:
            raise DependencyError(
                f"feature policy package `{package_name}` resolved {len(matches)} times; expected exactly once"
            )
        node = nodes.get(matches[0]["id"])
        if node is None:
            raise DependencyError(f"feature policy package `{package_name}` is absent from the resolved graph")
        enabled = set(node.get("features", ()))
        for feature in policy.get("features", ()):
            if feature in enabled:
                violations.append(
                    f"forbidden dependency feature `{package_name}/{feature}` is enabled: {policy['reason']}"
                )

    return violations


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--rules", type=Path, default=DEFAULT_RULES)
    # The workspace root is the repo root (HERE is firmware/tools/), not firmware/: the
    # workspace spans firmware/, host/ and apps/. `standalone_manifests` in the rules file
    # are resolved relative to this manifest's directory.
    result.add_argument("--manifest-path", type=Path, default=HERE.parent.parent / "Cargo.toml")
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        rules = load_json(args.rules)
        if rules.get("schema_version") != 1:
            raise DependencyError("unsupported dependency rule schema; expected schema_version 1")
        metadatas = [cargo_metadata(manifest) for manifest in metadata_manifests(args.manifest_path, rules)]
        packages, edges = dependency_graph(metadatas)
        violations = check_edges(edges, rules, packages)
        feature_policies = rules.get("forbidden_features", ())
        policies_by_manifest: dict[Path, list[dict[str, object]]] = {}
        for policy in feature_policies:
            manifest = args.manifest_path.parent / policy["manifest"]
            policies_by_manifest.setdefault(manifest, []).append(policy)
        for manifest, policies in policies_by_manifest.items():
            violations.extend(check_forbidden_features(cargo_metadata(manifest, include_dependencies=True), policies))
        if violations:
            raise DependencyError("\n".join(violations))
    except (DependencyError, json.JSONDecodeError, KeyError, TypeError) as error:
        print(f"dependency check failed: {error}", file=sys.stderr)
        return 1
    print(
        f"dependency policy check passed ({len(edges)} production local edges, "
        f"{len(feature_policies)} forbidden feature rules)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
