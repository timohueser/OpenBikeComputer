#!/usr/bin/env python3
"""Discover and validate OpenBikeComputer test and validation suites.

The registries contain policy and ownership facts.  Cargo targets, source files,
counts, and dependency edges are deliberately discovered instead of copied into
TOML.  The module is importable so its rejection rules can be tested against
temporary repositories without contacting GitHub or any other live service.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from typing import Any, Callable, Iterable, Sequence


LEVELS = {"unit", "component", "contract", "fixture", "end-to-end", "live", "hardware"}
PR_CADENCES = {"always", "affected", "never"}
SCHEDULED_CADENCES = {"none", "nightly", "weekly", "manual", "release"}
OWNERSHIP_KINDS = {"rust-package", "path", "swift-target", "swift-package", "workflow"}
DERIVED_FIELDS = {
    "count",
    "duration",
    "dependencies",
    "reverse_dependencies",
    "source_files",
    "test_count",
}
SAFETY_COMPONENTS = {"format-protocol-codecs", "crc", "storage", "dfu", "boot"}
ISSUE_RE = re.compile(r"^(?:#\d+|https://github\.com/[^/]+/[^/]+/issues/\d+)$")
SWIFT_TARGET_RE = re.compile(r"\.testTarget\s*\(\s*name:\s*\"([^\"]+)\"", re.MULTILINE)
REAL_SLEEP_RE = re.compile(r"(?:Task\.sleep|time\.sleep|std::thread::sleep)\s*\(")
LIVE_COMMAND_RE = re.compile(r"(?:^|\s)(?:curl|wget|ssh)\s|https?://")

# Only commands that exercise repository behavior belong to workflow discovery.
# Dependency installation, cache setup, artifact upload, and shell bookkeeping do not.
WORKFLOW_MARKERS = (
    "cargo test",
    "cargo clippy",
    "cargo fmt",
    "cargo deny",
    "cargo build",
    "cargo run",
    "python3 ",
    "npm test",
    "npm run check",
    "npm run build",
    "swift test",
    "xcodebuild build",
    "trunk build",
    "build-wasm-bridges.sh",
    "capture-website-screenshots.sh",
    "gen-third-party.sh",
)


class RegistryError(Exception):
    """One or more registry invariants failed."""


@dataclass(frozen=True, order=True)
class Discovered:
    kind: str
    name: str
    path: str = ""
    detail: str = ""

    @property
    def label(self) -> str:
        suffix = f" ({self.path})" if self.path else ""
        return f"{self.kind}:{self.name}{suffix}"


@dataclass
class Inventory:
    suites: list[dict[str, Any]]
    coverage: list[dict[str, Any]]
    discovered: list[Discovered]
    matches: dict[str, list[Discovered]]


def repository_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _read_toml(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise RegistryError(f"cannot parse {path}: {exc}") from exc


def _relative(path: Path, root: Path) -> str:
    return path.resolve().relative_to(root.resolve()).as_posix()


def _cargo_metadata(root: Path, manifest: Path | None = None) -> dict[str, Any]:
    command = ["cargo", "metadata", "--format-version", "1", "--locked", "--no-deps"]
    if manifest is not None:
        command.extend(["--manifest-path", str(manifest)])
    try:
        result = subprocess.run(command, cwd=root, check=True, capture_output=True, text=True)
        return json.loads(result.stdout)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as exc:
        detail = getattr(exc, "stderr", "") or str(exc)
        raise RegistryError(f"cargo metadata failed: {detail.strip()}") from exc


def discover_rust(root: Path, metadata_loader: Callable[[Path, Path | None], dict[str, Any]] = _cargo_metadata) -> list[Discovered]:
    manifests: list[Path | None] = [None]
    for relative in (
        "firmware/obc-fw-nrf54l/Cargo.toml",
        "firmware/obc-boot/Cargo.toml",
        "apps/obc-desktop/Cargo.toml",
    ):
        candidate = root / relative
        if candidate.exists():
            manifests.append(candidate)

    found: dict[tuple[str, str, str], Discovered] = {}
    for manifest in manifests:
        metadata = metadata_loader(root, manifest)
        for package in metadata.get("packages", []):
            manifest_path = Path(package["manifest_path"])
            relative_manifest = _relative(manifest_path, root)
            package_name = package["name"]
            test_targets = [
                target
                for target in package.get("targets", [])
                if target.get("test") or "test" in target.get("kind", [])
            ]
            for target in test_targets:
                target_name = target["name"]
                item = Discovered(
                    "rust-target",
                    f"{package_name}:{target_name}",
                    _relative(Path(target.get("src_path", manifest_path)), root),
                    f"{relative_manifest}:{'+'.join(target.get('kind', []))}",
                )
                found[(package_name, target_name, relative_manifest)] = item
            if not test_targets:
                item = Discovered("rust-manifest", package_name, relative_manifest)
                found[(package_name, "", relative_manifest)] = item
    return sorted(found.values())


def discover_paths(root: Path) -> list[Discovered]:
    rules = (
        ("web-test", "builder/app", ("*.test.ts", "*.test.tsx", "*.test.js")),
        ("python-test", "tools/tests", ("test_*.py",)),
        ("python-test", "firmware/tools/tests", ("test_*.py",)),
        ("python-test", "ops/weather/tests", ("test_*.py",)),
        ("python-test", "builder/tests", ("test_*.py",)),
        ("rain-radar-test", "tools/rain-radar-demo/tests", ("*.test.ts", "*.test.js")),
        ("xcuitest", "companion-ios/OBCCompanionUITests", ("*.swift",)),
    )
    found: list[Discovered] = []
    for kind, base_name, patterns in rules:
        base = root / base_name
        if not base.exists():
            continue
        for pattern in patterns:
            for path in base.rglob(pattern):
                if path.is_file() and not {"node_modules", "dist", "target"}.intersection(path.parts):
                    relative = _relative(path, root)
                    found.append(Discovered(kind, relative, relative))
    return sorted(set(found))


def discover_swift(root: Path) -> list[Discovered]:
    found: list[Discovered] = []
    for relative in ("companion-ios/Packages/OBCKit/Package.swift", "companion-ios/EchoHarness/Package.swift"):
        manifest = root / relative
        if not manifest.exists():
            continue
        text = manifest.read_text(encoding="utf-8")
        targets = SWIFT_TARGET_RE.findall(text)
        if targets:
            found.extend(Discovered("swift-target", name, relative) for name in targets)
        else:
            found.append(Discovered("swift-package", manifest.parent.name, relative))
    return sorted(found)


def _strip_yaml_scalar(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        return value[1:-1]
    return value


def discover_workflow(root: Path) -> list[Discovered]:
    workflow = root / ".github/workflows/ci.yml"
    if not workflow.exists():
        return []
    lines = workflow.read_text(encoding="utf-8").splitlines()
    commands: list[str] = []
    block_indent: int | None = None
    for raw in lines:
        indent = len(raw) - len(raw.lstrip())
        stripped = raw.strip()
        if stripped.startswith("export "):
            continue
        if block_indent is not None:
            if stripped and indent <= block_indent:
                block_indent = None
            elif stripped and not stripped.startswith("#"):
                command = stripped.rstrip("\\").strip()
                if any(marker in command for marker in WORKFLOW_MARKERS):
                    commands.append(command)
                continue
        match = re.search(r"(?:^|[-{,]\s)(?:run|cmd):\s*(.*?)(?:\s*}\s*)?$", stripped)
        if not match:
            continue
        value = match.group(1).strip()
        if value in {"|", ">", "|-", ">-"}:
            block_indent = indent
            continue
        value = _strip_yaml_scalar(value)
        if any(marker in value for marker in WORKFLOW_MARKERS):
            commands.append(value)
    return [Discovered("workflow-command", command, ".github/workflows/ci.yml") for command in sorted(set(commands))]


def discover_all(root: Path, metadata_loader: Callable[[Path, Path | None], dict[str, Any]] = _cargo_metadata) -> list[Discovered]:
    return sorted(discover_rust(root, metadata_loader) + discover_paths(root) + discover_swift(root) + discover_workflow(root))


def _path_matches(root: Path, pattern: str, item: Discovered) -> bool:
    if item.path and fnmatch.fnmatchcase(item.path, pattern):
        return True
    return any(path.is_file() and _relative(path, root) == item.path for path in root.glob(pattern))


def ownership_matches(root: Path, owner: dict[str, Any], item: Discovered) -> bool:
    kind = owner.get("kind")
    if kind == "rust-package":
        return item.kind in {"rust-target", "rust-manifest"} and item.name.split(":", 1)[0] == owner.get("name")
    if kind == "path":
        return item.kind == owner.get("source") and _path_matches(root, owner.get("pattern", ""), item)
    if kind == "swift-target":
        return item.kind == "swift-target" and item.name == owner.get("name") and item.path == owner.get("package")
    if kind == "swift-package":
        return item.kind == "swift-package" and item.path == owner.get("package")
    if kind == "workflow":
        return item.kind == "workflow-command" and fnmatch.fnmatchcase(item.name, owner.get("pattern", ""))
    return False


def _validate_issue_block(suite_id: str, field: str, value: Any, errors: list[str]) -> None:
    if not isinstance(value, dict):
        errors.append(f"{suite_id}: {field} must be a table")
        return
    reason = value.get("reason", "")
    issue = value.get("issue", "")
    if not isinstance(reason, str) or not reason.strip():
        errors.append(f"{suite_id}: {field} requires a reason")
    if not isinstance(issue, str) or not ISSUE_RE.fullmatch(issue):
        errors.append(f"{suite_id}: {field} requires an open GitHub issue reference")


def _validate_command(root: Path, suite: dict[str, Any], rust_packages: set[str], errors: list[str]) -> None:
    suite_id = suite.get("id", "<missing-id>")
    command = suite.get("command")
    if not isinstance(command, str) or not command.strip():
        errors.append(f"{suite_id}: command must be one non-empty string")
        return
    try:
        words = shlex.split(command.replace("\n", " "))
    except ValueError as exc:
        errors.append(f"{suite_id}: command cannot be parsed: {exc}")
        return
    if not words:
        errors.append(f"{suite_id}: command is empty")
        return
    known_tools = {"bash", "cargo", "npm", "python3", "swift", "trunk", "xcodebuild"}
    expect_executable = True
    skip_cd_path = False
    for word in words:
        if skip_cd_path:
            working_directory = root / word
            if not working_directory.is_dir():
                errors.append(f"{suite_id}: command working directory does not exist: {word}")
            skip_cd_path = False
            expect_executable = False
            continue
        if word in {"&&", ";", "|"}:
            expect_executable = True
            continue
        if word == "cd" and expect_executable:
            skip_cd_path = True
            continue
        if word == "env" and expect_executable:
            continue
        if expect_executable and "=" in word and not word.startswith(("./", "/")):
            continue
        if not expect_executable:
            continue
        executable = word[2:] if word.startswith("./") else word
        if "/" in executable:
            candidate = root / executable
            if not candidate.is_file():
                errors.append(f"{suite_id}: command path does not exist: {executable}")
        elif executable == "obc":
            if not (root / "tools/obc").is_file():
                errors.append(f"{suite_id}: repository obc command cannot resolve")
        elif executable not in known_tools and shutil.which(executable) is None:
            errors.append(f"{suite_id}: command executable cannot resolve: {executable}")
        expect_executable = False
    package_match = re.search(r"(?:^|\s)(?:-p|--package)\s+([^\s]+)", command)
    if package_match and package_match.group(1) not in rust_packages:
        errors.append(f"{suite_id}: command names unknown Cargo package {package_match.group(1)}")


def _collect_rust_packages(discovered: Iterable[Discovered]) -> set[str]:
    return {item.name.split(":", 1)[0] for item in discovered if item.kind in {"rust-target", "rust-manifest"}}


def validate(root: Path, suites_doc: dict[str, Any], coverage_doc: dict[str, Any], discovered: list[Discovered]) -> Inventory:
    errors: list[str] = []
    if suites_doc.get("schema") != 1:
        errors.append("testing/suites.toml: schema must be 1")
    if coverage_doc.get("schema") != 1:
        errors.append("testing/coverage-policy.toml: schema must be 1")
    suites = suites_doc.get("suite", [])
    coverage = coverage_doc.get("component", [])
    if not isinstance(suites, list):
        errors.append("testing/suites.toml: suite must be an array of tables")
        suites = []
    if not isinstance(coverage, list):
        errors.append("testing/coverage-policy.toml: component must be an array of tables")
        coverage = []

    ids = [suite.get("id") for suite in suites]
    duplicate_ids = sorted({value for value in ids if value and ids.count(value) > 1})
    if duplicate_ids:
        errors.append(f"duplicate suite IDs: {', '.join(duplicate_ids)}")

    component_ids = [component.get("id") for component in coverage]
    duplicate_components = sorted({value for value in component_ids if value and component_ids.count(value) > 1})
    if duplicate_components:
        errors.append(f"duplicate coverage component IDs: {', '.join(duplicate_components)}")
    components = {value for value in component_ids if isinstance(value, str)}

    rust_packages = _collect_rust_packages(discovered)
    for suite in suites:
        suite_id = suite.get("id", "<missing-id>")
        if not isinstance(suite_id, str) or not suite_id.strip():
            errors.append("suite has no stable id")
        forbidden = sorted(DERIVED_FIELDS.intersection(suite))
        if forbidden:
            errors.append(f"{suite_id}: derived fields are forbidden: {', '.join(forbidden)}")
        if suite.get("level") not in LEVELS:
            errors.append(f"{suite_id}: invalid level {suite.get('level')!r}")
        if suite.get("pull_request") not in PR_CADENCES:
            errors.append(f"{suite_id}: invalid pull_request cadence {suite.get('pull_request')!r}")
        if suite.get("scheduled") not in SCHEDULED_CADENCES:
            errors.append(f"{suite_id}: invalid scheduled cadence {suite.get('scheduled')!r}")
        fixtures = suite.get("fixtures", [])
        if suite.get("level") == "fixture" and not fixtures:
            errors.append(f"{suite_id}: fixture suite must declare fixtures")
        if suite.get("level") in {"live", "hardware"} and suite.get("pull_request") != "never":
            errors.append(f"{suite_id}: {suite.get('level')} suites cannot be required on pull requests")
        if suite.get("level") in {"unit", "component", "contract", "fixture"} and LIVE_COMMAND_RE.search(str(suite.get("command", ""))):
            errors.append(f"{suite_id}: hermetic suite command appears to contact a live service")
        component = suite.get("coverage_component")
        if component is not None and component not in components:
            errors.append(f"{suite_id}: unknown coverage component {component!r}")
        owners = suite.get("ownership", [])
        if not isinstance(owners, list) or not owners:
            errors.append(f"{suite_id}: ownership must contain at least one execution route")
        else:
            for owner in owners:
                if not isinstance(owner, dict) or owner.get("kind") not in OWNERSHIP_KINDS:
                    errors.append(f"{suite_id}: invalid ownership rule {owner!r}")
        for field in ("budget_exception", "quarantine", "cadence_conflict", "sleep_exception"):
            if field in suite:
                _validate_issue_block(suite_id, field, suite[field], errors)
        _validate_command(root, suite, rust_packages, errors)
        for pattern in suite.get("extra_triggers", []):
            if not isinstance(pattern, str) or not any(root.glob(pattern)):
                errors.append(f"{suite_id}: extra trigger matches no maintained path: {pattern!r}")

    for component in coverage:
        component_id = component.get("id", "<missing-id>")
        forbidden = sorted(DERIVED_FIELDS.intersection(component))
        if forbidden:
            errors.append(f"coverage {component_id}: derived fields are forbidden: {', '.join(forbidden)}")
        enforcement = component.get("enforcement")
        if enforcement not in {"ratchet", "report"}:
            errors.append(f"coverage {component_id}: invalid enforcement {enforcement!r}")
        if component_id in SAFETY_COMPONENTS and enforcement != "ratchet":
            errors.append(f"coverage {component_id}: safety-critical component must be planned as ratchet")
        includes = component.get("include", [])
        if not includes:
            errors.append(f"coverage {component_id}: include must name production paths")
        for pattern in includes:
            if not any(root.glob(pattern)):
                errors.append(f"coverage {component_id}: included path does not resolve: {pattern!r}")
        for exclusion in component.get("exclude", []):
            if not isinstance(exclusion, dict) or not exclusion.get("path") or not exclusion.get("evidence"):
                errors.append(f"coverage {component_id}: every exclusion needs path and replacement evidence")
        if "baseline" in component and component["baseline"] in {None, "pending", ""}:
            errors.append(f"coverage {component_id}: omit pending baseline instead of inventing a value")

    matches: dict[str, list[Discovered]] = {str(suite.get("id")): [] for suite in suites}
    for item in discovered:
        owners = [
            suite
            for suite in suites
            if any(ownership_matches(root, owner, item) for owner in suite.get("ownership", []))
        ]
        if not owners:
            errors.append(f"orphan discovered suite: {item.label}")
        elif len(owners) > 1:
            errors.append(f"duplicate ownership for {item.label}: {', '.join(str(s.get('id')) for s in owners)}")
        else:
            matches[str(owners[0].get("id"))].append(item)
            if item.path and item.kind != "workflow-command":
                source = root / item.path
                if source.is_file() and source.suffix in {".rs", ".py", ".swift", ".ts", ".js"}:
                    try:
                        has_real_sleep = bool(REAL_SLEEP_RE.search(source.read_text(encoding="utf-8")))
                    except UnicodeDecodeError:
                        has_real_sleep = False
                    if has_real_sleep and "sleep_exception" not in owners[0]:
                        errors.append(f"{owners[0].get('id')}: real sleep in {item.path} needs an approved exception")
    for suite_id, owned in matches.items():
        if not owned:
            errors.append(f"dead registry entry: {suite_id}")

    if errors:
        raise RegistryError("\n".join(f"- {error}" for error in errors))
    return Inventory(suites, coverage, discovered, matches)


def load_inventory(root: Path | None = None) -> Inventory:
    root = (root or repository_root()).resolve()
    suites = _read_toml(root / "testing/suites.toml")
    coverage = _read_toml(root / "testing/coverage-policy.toml")
    return validate(root, suites, coverage, discover_all(root))


def _suite_summary(suite: dict[str, Any], owned: Sequence[Discovered]) -> dict[str, Any]:
    return {
        "id": suite["id"],
        "surface": suite["surface"],
        "level": suite["level"],
        "pull_request": suite["pull_request"],
        "scheduled": suite["scheduled"],
        "command": suite["command"],
        "discovered_units": len(owned),
    }


def command_check(args: argparse.Namespace) -> int:
    inventory = load_inventory(args.root)
    by_surface: dict[str, int] = {}
    by_level: dict[str, int] = {}
    for suite in inventory.suites:
        by_surface[suite["surface"]] = by_surface.get(suite["surface"], 0) + 1
        by_level[suite["level"]] = by_level.get(suite["level"], 0) + 1
    print(f"suite registry OK: {len(inventory.suites)} suites, {len(inventory.discovered)} discovered execution units")
    print("by surface: " + ", ".join(f"{key}={value}" for key, value in sorted(by_surface.items())))
    print("by level: " + ", ".join(f"{key}={value}" for key, value in sorted(by_level.items())))
    return 0


def command_list(args: argparse.Namespace) -> int:
    inventory = load_inventory(args.root)
    rows = [_suite_summary(suite, inventory.matches[suite["id"]]) for suite in inventory.suites]
    if args.json:
        print(json.dumps(rows, indent=2, sort_keys=True))
    else:
        for row in rows:
            print(
                f"{row['id']:<36} {row['surface']:<16} {row['level']:<12} "
                f"pr={row['pull_request']:<8} schedule={row['scheduled']:<7} units={row['discovered_units']}"
            )
    return 0


def command_explain(args: argparse.Namespace) -> int:
    inventory = load_inventory(args.root)
    suite = next((item for item in inventory.suites if item["id"] == args.suite_id), None)
    if suite is None:
        raise RegistryError(f"unknown suite ID: {args.suite_id}")
    ordered = (
        "id",
        "surface",
        "level",
        "command",
        "fixtures",
        "pull_request",
        "scheduled",
        "extra_triggers",
        "platforms",
        "coverage_component",
        "budget_exception",
        "quarantine",
        "cadence_conflict",
        "sleep_exception",
    )
    for field in ordered:
        if field in suite:
            value = suite[field]
            rendered = json.dumps(value, sort_keys=True) if isinstance(value, (dict, list)) else str(value)
            print(f"{field}: {rendered}")
    print("execution_units:")
    for item in inventory.matches[suite["id"]]:
        print(f"  - {item.label}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Inspect and validate testing/suites.toml")
    parser.add_argument("--root", type=Path, help=argparse.SUPPRESS)
    subparsers = parser.add_subparsers(dest="action", required=True)
    check = subparsers.add_parser("check", help="validate registries, discovery, commands, and CI routes")
    check.set_defaults(func=command_check)
    listing = subparsers.add_parser("list", help="derive and print the current suite inventory")
    listing.add_argument("--json", action="store_true", help="emit JSON")
    listing.set_defaults(func=command_list)
    explain = subparsers.add_parser("explain", help="explain one suite")
    explain.add_argument("suite_id")
    explain.set_defaults(func=command_explain)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except RegistryError as exc:
        print(f"suite registry check failed:\n{exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
