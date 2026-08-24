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
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable, Mapping, Sequence


LEVELS = {"unit", "component", "contract", "fixture", "end-to-end", "live", "hardware"}
# The only two spoken aliases: `obc test fixtures` and `obc test e2e`.
LEVEL_ALIASES = {"fixtures": "fixture", "e2e": "end-to-end"}
# Cargo subcommands that compile, lint, format, or execute a named package. `deny` is
# lockfile policy rather than package execution, so it never routes a package to a job.
CARGO_EXECUTION_VERBS = {"build", "clippy", "fmt", "run", "test"}
PLATFORMS = {"linux": "linux", "darwin": "macos", "win32": "windows"}
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
RUST_FOUNDATION_PATHS = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rustfmt.toml",
    ".cargo/config.toml",
    ".cargo/config",
}
TEST_POLICY_PATTERNS = (
    ".github/workflows/**",
    ".github/actions/**",
    "testing/**",
    "tools/suite_registry.py",
    "tools/ci_aggregate.py",
    "docs/testing.md",
    "CONTRIBUTING.md",
    "AGENTS.md",
    "CLAUDE.md",
    # These three define what the registry's `obc check` and `obc test` suites execute.
    "tools/justfile",
    "tools/obc",
    "tools/obc-dev.sh",
)
CODE_OR_POLICY_SUFFIXES = {
    ".c",
    ".h",
    ".js",
    ".json",
    ".py",
    ".rs",
    ".swift",
    ".toml",
    ".ts",
    ".tsx",
    ".yaml",
    ".yml",
}

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


@dataclass(frozen=True, order=True)
class WorkflowStep:
    command: str
    job: str
    working_directory: str = ""


@dataclass(frozen=True)
class WorkflowJob:
    name: str
    runs_on: str
    needs: tuple[str, ...]
    plan_gated: bool
    gates_on: str = ""


@dataclass
class Inventory:
    suites: list[dict[str, Any]]
    coverage: list[dict[str, Any]]
    discovered: list[Discovered]
    matches: dict[str, list[Discovered]]


@dataclass(frozen=True)
class CargoPackage:
    name: str
    root: str
    manifest: str
    dependencies: frozenset[str]
    root_workspace: bool = True


@dataclass(frozen=True)
class CargoGraph:
    packages: Mapping[str, CargoPackage]
    reverse_dependencies: Mapping[str, frozenset[str]]


@dataclass
class SuiteSelection:
    suite: dict[str, Any]
    jobs: list[str]
    reasons: list[str] = field(default_factory=list)

    @property
    def selected(self) -> bool:
        return bool(self.reasons)


@dataclass
class SelectionPlan:
    base: str
    head: str
    changed_paths: list[str]
    suites: list[SuiteSelection]
    errors: list[str]

    @property
    def selected(self) -> list[SuiteSelection]:
        return [selection for selection in self.suites if selection.selected]


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
    for relative in ("companion-ios/Packages/OBCKit/Package.swift",):
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


def scan_workflow(root: Path) -> list[WorkflowStep]:
    """Read every repository-behavior command in the workflow with its job and directory."""

    workflow = root / ".github/workflows/ci.yml"
    if not workflow.exists():
        return []
    steps: list[WorkflowStep] = []
    block_indent: int | None = None
    current_job = ""
    job_directory = ""
    step_directory = ""
    in_defaults = False
    in_jobs = False

    def record(command: str) -> None:
        if any(marker in command for marker in WORKFLOW_MARKERS):
            steps.append(WorkflowStep(command, current_job, step_directory or job_directory))

    for raw in workflow.read_text(encoding="utf-8").splitlines():
        indent = len(raw) - len(raw.lstrip())
        stripped = raw.strip()
        if raw.rstrip() and not raw.startswith(" "):
            in_jobs = raw.startswith("jobs:")
            continue
        if not in_jobs:
            continue
        job_match = re.fullmatch(r"  ([A-Za-z0-9_-]+):", raw)
        if job_match:
            current_job = job_match.group(1)
            block_indent = None
            job_directory = ""
            step_directory = ""
            in_defaults = False
            continue
        if stripped.startswith("export "):
            continue
        if block_indent is not None:
            if stripped and indent <= block_indent:
                block_indent = None
            elif stripped and not stripped.startswith("#"):
                record(stripped.rstrip("\\").strip())
                continue
        if stripped.startswith("- "):
            step_directory = ""
            in_defaults = False
        if stripped == "defaults:":
            in_defaults = True
            continue
        directory = re.fullmatch(r"-?\s*working-directory:\s*(\S+)", stripped)
        if directory:
            if in_defaults:
                job_directory = directory.group(1)
            else:
                step_directory = directory.group(1)
            continue
        match = re.search(r"(?:^|[-{,]\s)(?:run|cmd):\s*(.*?)(?:\s*}\s*)?$", stripped)
        if not match:
            continue
        value = match.group(1).strip()
        if value in {"|", ">", "|-", ">-"}:
            block_indent = indent
            continue
        record(_strip_yaml_scalar(value))
    return steps


def discover_workflow(root: Path) -> list[Discovered]:
    return sorted(
        {
            Discovered("workflow-command", step.command, ".github/workflows/ci.yml", step.job)
            for step in scan_workflow(root)
        }
    )


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


def _metadata_manifests(root: Path) -> list[Path | None]:
    manifests: list[Path | None] = [None]
    for relative in (
        "firmware/obc-fw-nrf54l/Cargo.toml",
        "firmware/obc-boot/Cargo.toml",
        "apps/obc-desktop/Cargo.toml",
    ):
        candidate = root / relative
        if candidate.exists():
            manifests.append(candidate)
    return manifests


def build_cargo_graph(
    root: Path,
    metadata_loader: Callable[[Path, Path | None], dict[str, Any]] = _cargo_metadata,
) -> CargoGraph:
    """Derive package roots and reverse edges from Cargo rather than registry policy."""

    raw_packages: dict[str, dict[str, Any]] = {}
    for manifest in _metadata_manifests(root):
        metadata = metadata_loader(root, manifest)
        workspace_members = set(metadata.get("workspace_members", []))
        for package in metadata.get("packages", []):
            manifest_path = Path(package["manifest_path"])
            try:
                relative_manifest = _relative(manifest_path, root)
            except ValueError:
                continue
            raw_packages[package["name"]] = {
                "manifest": relative_manifest,
                "dependencies": {dependency["name"] for dependency in package.get("dependencies", [])},
                "root_workspace": manifest is None and package.get("id") in workspace_members,
            }
    names = set(raw_packages)
    packages: dict[str, CargoPackage] = {}
    reverse: dict[str, set[str]] = {name: set() for name in names}
    for name, package in raw_packages.items():
        dependencies = frozenset(package["dependencies"].intersection(names))
        manifest = package["manifest"]
        packages[name] = CargoPackage(
            name,
            str(Path(manifest).parent),
            manifest,
            dependencies,
            bool(package["root_workspace"]),
        )
        for dependency in dependencies:
            reverse[dependency].add(name)
    return CargoGraph(packages, {name: frozenset(values) for name, values in reverse.items()})


def _glob_matches(path: str, pattern: str) -> bool:
    """Match repository globs, including the zero-directory meaning of `**/`."""

    if fnmatch.fnmatchcase(path, pattern):
        return True
    collapsed = pattern
    while "**/" in collapsed:
        collapsed = collapsed.replace("**/", "", 1)
        if fnmatch.fnmatchcase(path, collapsed):
            return True
    return False


def git_changed_paths(root: Path, base: str, head: str) -> list[str]:
    command = ["git", "diff", "--name-only", "--diff-filter=ACMR", f"{base}...{head}"]
    try:
        result = subprocess.run(command, cwd=root, check=True, capture_output=True, text=True)
    except (OSError, subprocess.CalledProcessError) as exc:
        detail = getattr(exc, "stderr", "") or str(exc)
        raise RegistryError(f"cannot read changed paths from Git: {detail.strip()}") from exc
    return sorted({line.strip() for line in result.stdout.splitlines() if line.strip()})


def workflow_jobs(root: Path) -> dict[str, WorkflowJob]:
    """Derive each CI job's runner image, upstream needs, and whether the plan gates it."""

    workflow = root / ".github/workflows/ci.yml"
    if not workflow.exists():
        return {}
    jobs: dict[str, WorkflowJob] = {}
    name = ""
    runs_on = ""
    needs: tuple[str, ...] = ()
    gated = False
    gates_on = ""
    in_jobs = False

    def flush() -> None:
        if name:
            jobs[name] = WorkflowJob(name, runs_on, needs, gated, gates_on)

    for raw in workflow.read_text(encoding="utf-8").splitlines():
        if raw.rstrip() and not raw.startswith(" "):
            flush()
            name, in_jobs = "", raw.startswith("jobs:")
            continue
        if not in_jobs:
            continue
        job_match = re.fullmatch(r"  ([A-Za-z0-9_-]+):", raw)
        if job_match:
            flush()
            name, runs_on, needs, gated, gates_on = job_match.group(1), "", (), False, ""
            continue
        if not name:
            continue
        stripped = raw.strip()
        runner = re.fullmatch(r"runs-on:\s*(.+)", stripped)
        if runner:
            runs_on = runner.group(1).strip()
        need = re.fullmatch(r"needs:\s*(.+)", stripped)
        if need:
            needs = tuple(
                value.strip()
                for value in need.group(1).strip("[] ").split(",")
                if value.strip()
            )
        gate_literal = re.search(r"needs\.selection\.outputs\.jobs\)\s*,\s*'([^']*)'", stripped)
        if gate_literal:
            gated, gates_on = True, gate_literal.group(1)
        elif stripped.startswith("if:") and "needs.selection.outputs" in stripped:
            gated, gates_on = True, ""
    flush()
    return jobs


def aggregate_job(root: Path) -> str:
    """The gate job evaluating the plan; it is never a route for the suites it reports."""

    for step in scan_workflow(root):
        if "ci_aggregate.py" in step.command:
            return step.job
    return ""


def _cargo_invocations(command: str) -> list[list[str]]:
    """Split a shell command into the argument list of each `cargo` invocation it runs."""

    try:
        words = shlex.split(command.replace("\n", " "))
    except ValueError:
        return []
    invocations: list[list[str]] = []
    current: list[str] | None = None
    for word in words:
        if word in {"&&", "||", ";", "|"} or word.endswith(";"):
            if current is not None:
                invocations.append(current)
            current = None
            continue
        if word == "cargo" or word.endswith("/cargo"):
            if current is not None:
                invocations.append(current)
            current = []
            continue
        if current is not None:
            current.append(word)
    if current is not None:
        invocations.append(current)
    return invocations


def _cargo_packages(args: Sequence[str], directory: str, graph: CargoGraph) -> set[str]:
    """Resolve which Cargo packages one invocation compiles, lints, formats, or runs."""

    verb = next((word for word in args if not word.startswith(("-", "+"))), "")
    if verb not in CARGO_EXECUTION_VERBS:
        return set()
    named: set[str] = set()
    manifest = ""
    workspace = False
    index = 0
    while index < len(args):
        word = args[index]
        following = args[index + 1] if index + 1 < len(args) else ""
        if word in {"-p", "--package"} and following:
            named.add(following)
            index += 2
            continue
        if word.startswith("--package="):
            named.add(word.split("=", 1)[1])
        elif word in {"--workspace", "--all"}:
            workspace = True
        elif word == "--manifest-path" and following:
            manifest = following
            index += 2
            continue
        elif word.startswith("--manifest-path="):
            manifest = word.split("=", 1)[1]
        index += 1
    if workspace:
        return {name for name, package in graph.packages.items() if package.root_workspace}
    if manifest:
        return {name for name, package in graph.packages.items() if package.manifest == manifest}
    if named:
        return named.intersection(graph.packages)
    if directory:
        return _directory_package(directory, graph)
    return set()


SCRIPT_RE = re.compile(r"(?:^|\s)((?:[\w.-]+/)*[\w.-]+\.sh)(?:\s|$)")
WASM_PACK_RE = re.compile(r"\bwasm-pack\s+build\s+([\w./-]+)")
TRUNK_RE = re.compile(r"\btrunk\s+build\b[^|;&]*?--config\s+([\w./-]+)")
TRUNK_LINK_RE = re.compile(r"<link[^>]*data-trunk[^>]*>")
HREF_RE = re.compile(r'href="([^"]+)"')


def _trunk_packages(command: str, root: Path, graph: CargoGraph) -> set[str]:
    """Packages a `trunk build --config CFG` compiles, read from the config's HTML target."""

    packages: set[str] = set()
    for match in TRUNK_RE.finditer(command):
        config = root / match.group(1)
        if not config.is_file():
            continue
        page = config.parent / _read_toml(config).get("build", {}).get("target", "index.html")
        if not page.is_file():
            continue
        for tag in TRUNK_LINK_RE.findall(page.read_text(encoding="utf-8")):
            href = HREF_RE.search(tag)
            if not href or 'rel="rust"' not in tag:
                continue
            try:
                manifest = _relative(page.parent / href.group(1), root)
            except (ValueError, OSError):
                continue
            packages |= {name for name, item in graph.packages.items() if item.manifest == manifest}
    return packages


def _executed_commands(root: Path, step: WorkflowStep) -> list[tuple[str, str]]:
    """A step's command plus the lines of any repository script that command runs."""

    commands = [(step.command, step.working_directory)]
    for match in SCRIPT_RE.finditer(step.command):
        script = root / match.group(1)
        if script.is_file():
            commands.extend((line.strip(), "") for line in script.read_text(encoding="utf-8").splitlines())
    return commands


def _directory_package(directory: str, graph: CargoGraph) -> set[str]:
    root = directory.rstrip("/")
    return {name for name, package in graph.packages.items() if package.root == root}


def cargo_job_coverage(root: Path, graph: CargoGraph) -> dict[str, set[str]]:
    """Map each Cargo package to the CI jobs whose steps build, lint, or run it."""

    coverage: dict[str, set[str]] = {}
    for step in scan_workflow(root):
        for command, directory in _executed_commands(root, step):
            packages = {
                package
                for args in _cargo_invocations(command)
                for package in _cargo_packages(args, directory, graph)
            }
            for match in WASM_PACK_RE.finditer(command):
                packages |= _directory_package(match.group(1), graph)
            packages |= _trunk_packages(command, root, graph)
            for package in packages:
                coverage.setdefault(package, set()).add(step.job)
    return coverage


def suite_workflow_jobs(inventory: Inventory, root: Path, graph: CargoGraph) -> dict[str, list[str]]:
    """Derive each suite's CI jobs; an empty list is an intentionally visible missing route."""

    coverage = cargo_job_coverage(root, graph)
    gate = aggregate_job(root)
    routes: dict[str, list[str]] = {}
    for suite in inventory.suites:
        suite_id = suite["id"]
        jobs = {
            item.detail
            for item in inventory.matches[suite_id]
            if item.kind == "workflow-command" and item.detail
        }
        for owner in suite.get("ownership", []):
            if owner.get("kind") == "rust-package":
                jobs |= coverage.get(owner.get("name", ""), set())
        routes[suite_id] = sorted(jobs - {gate})
    return routes


def unconditional_jobs(root: Path) -> set[str]:
    """Jobs that start on every run: the plan gates everything else."""

    gate = aggregate_job(root)
    return {name for name, job in workflow_jobs(root).items() if not job.plan_gated and name != gate}


def required_jobs(plan: SelectionPlan, jobs: Mapping[str, WorkflowJob]) -> list[str]:
    """Close the plan's jobs over the workflow `needs` graph so artifact producers run."""

    required = {job for selection in plan.selected for job in selection.jobs}
    pending = sorted(required)
    while pending:
        job = pending.pop()
        for upstream in jobs.get(job, WorkflowJob(job, "", (), False)).needs:
            if upstream not in required:
                required.add(upstream)
                pending.append(upstream)
    return sorted(required)


def _add_reason(selection: SuiteSelection, reason: str) -> None:
    if reason not in selection.reasons:
        selection.reasons.append(reason)


def _rust_suite_names(suite: dict[str, Any]) -> set[str]:
    return {
        owner["name"]
        for owner in suite.get("ownership", [])
        if owner.get("kind") == "rust-package"
    }


def _reverse_dependency_closure(graph: CargoGraph, package: str) -> dict[str, str]:
    """Return reverse dependents mapped to the edge that first selected them."""

    selected: dict[str, str] = {}
    pending = [package]
    while pending:
        dependency = pending.pop(0)
        for consumer in sorted(graph.reverse_dependencies.get(dependency, frozenset())):
            if consumer == package or consumer in selected:
                continue
            selected[consumer] = dependency
            pending.append(consumer)
    return selected


def _suite_owns_changed_test(suite: dict[str, Any], path: str) -> bool:
    for owner in suite.get("ownership", []):
        if owner.get("kind") == "path" and _glob_matches(path, owner.get("pattern", "")):
            return True
        if owner.get("kind") in {"swift-target", "swift-package"}:
            package = owner.get("package", "")
            package_root = str(Path(package).parent)
            if path == package or path.startswith(f"{package_root}/"):
                return True
    return False


def _is_policy_path(path: str) -> bool:
    return any(_glob_matches(path, pattern) for pattern in TEST_POLICY_PATTERNS)


def _looks_like_production(path: str) -> bool:
    if path.startswith(("docs/", "artifacts/", ".claude/", ".repowise/")):
        return False
    name = Path(path).name
    if name.startswith("test_") or "/tests/" in path or "/test/" in path:
        return False
    return Path(path).suffix.lower() in CODE_OR_POLICY_SUFFIXES


def select_suites(
    inventory: Inventory,
    changed_paths: Sequence[str],
    cargo_graph: CargoGraph,
    routes: Mapping[str, Sequence[str]],
    *,
    base: str = "",
    head: str = "HEAD",
    unconditional: Iterable[str] = (),
) -> SelectionPlan:
    """Build the deterministic suite-level selection plan from repository facts."""

    always_running = set(unconditional)
    selections = {
        suite["id"]: SuiteSelection(suite=suite, jobs=list(routes.get(suite["id"], ())))
        for suite in inventory.suites
    }
    errors: list[str] = []
    owned_paths: set[str] = set()
    rust_suites = {
        package: [suite for suite in inventory.suites if package in _rust_suite_names(suite)]
        for package in cargo_graph.packages
    }

    for path in sorted(set(changed_paths)):
        path_owned = False
        if _is_policy_path(path):
            path_owned = True
            # Every suite whose CI execution the workflow itself defines, derived from ownership
            # rather than from an ID prefix.
            for suite in inventory.suites:
                if any(owner.get("kind") == "workflow" for owner in suite.get("ownership", [])):
                    _add_reason(selections[suite["id"]], f"test policy changed: {path}")

        if path in RUST_FOUNDATION_PATHS:
            path_owned = True
            for suite in inventory.suites:
                rust_owners = _rust_suite_names(suite)
                selected_rust_owner = any(
                    owner in cargo_graph.packages
                    and (path not in {"Cargo.toml", "Cargo.lock"} or cargo_graph.packages[owner].root_workspace)
                    for owner in rust_owners
                )
                if selected_rust_owner or suite["id"] in {
                    "ci.rust-workspace-tests",
                    "ci.rust-format",
                    "ci.rust-clippy",
                    "ci.rust-builds",
                    "ci.dependencies",
                    "ci.resource-contracts",
                    "ci.wasm-size",
                    "ci.licenses",
                }:
                    _add_reason(selections[suite["id"]], f"foundational Rust input changed: {path}")

        changed_packages: list[str] = []
        for package in cargo_graph.packages.values():
            package_prefix = f"{package.root}/" if package.root != "." else ""
            if path == package.manifest or (package_prefix and path.startswith(package_prefix)):
                changed_packages.append(package.name)
        for package_name in sorted(set(changed_packages)):
            path_owned = True
            for suite in rust_suites.get(package_name, []):
                _add_reason(selections[suite["id"]], f"changed Rust package {package_name}: {path}")
            for consumer, dependency in _reverse_dependency_closure(cargo_graph, package_name).items():
                for suite in rust_suites.get(consumer, []):
                    _add_reason(
                        selections[suite["id"]],
                        f"reverse dependency {consumer} compiles {dependency} after {package_name} changed",
                    )

        if path == "fixtures/catalog.toml" or path.startswith("fixtures/"):
            path_owned = True
            for suite in inventory.suites:
                if suite.get("fixtures"):
                    _add_reason(selections[suite["id"]], f"declared fixture input changed: {path}")

        for suite in inventory.suites:
            suite_id = suite["id"]
            matched_triggers = [
                pattern for pattern in suite.get("extra_triggers", []) if _glob_matches(path, pattern)
            ]
            if matched_triggers:
                path_owned = True
                for pattern in matched_triggers:
                    _add_reason(selections[suite_id], f"registry trigger {pattern} matched {path}")
            if _suite_owns_changed_test(suite, path):
                path_owned = True
                _add_reason(selections[suite_id], f"owned test or package source changed: {path}")

        if path_owned:
            owned_paths.add(path)
        elif _looks_like_production(path):
            errors.append(
                f"changed production path has no suite owner: {path}; add a registry trigger or build-graph owner"
            )

    selected_jobs = {
        job for selection in selections.values() if selection.selected for job in selection.jobs
    }
    # A required-cadence suite rides the jobs a change already started. It is narrowed to those
    # jobs so riding one leg cannot pull a whole surface in behind it.
    for selection in selections.values():
        if selection.suite.get("pull_request") != "always" or selection.selected:
            continue
        if always_running.intersection(selection.jobs):
            selection.jobs = sorted(always_running.intersection(selection.jobs))
            _add_reason(selection, "always-run policy suite")
        elif selected_jobs.intersection(selection.jobs):
            selection.jobs = sorted(selected_jobs.intersection(selection.jobs))
            _add_reason(
                selection,
                "required whenever one of its CI jobs starts: " + ", ".join(selection.jobs),
            )

    for selection in selections.values():
        if selection.selected and not selection.jobs:
            errors.append(
                f"selected suite {selection.suite['id']} has no executable CI route; "
                f"command is `{selection.suite['command']}`"
            )
        selection.reasons.sort()
    return SelectionPlan(
        base=base,
        head=head,
        changed_paths=sorted(set(changed_paths)),
        suites=[selections[suite["id"]] for suite in inventory.suites],
        errors=sorted(set(errors)),
    )


NOT_SELECTED = "no changed path, Cargo edge, or required cadence selected this suite"


def selection_plan_data(
    plan: SelectionPlan, jobs: Mapping[str, WorkflowJob] | None = None
) -> dict[str, Any]:
    return {
        "schema": 1,
        "base": plan.base,
        "head": plan.head,
        "changed_paths": plan.changed_paths,
        "selected_suite_ids": [selection.suite["id"] for selection in plan.selected],
        "required_jobs": required_jobs(plan, jobs or {}),
        "errors": plan.errors,
        "suites": [
            {
                "id": selection.suite["id"],
                "surface": selection.suite["surface"],
                "platforms": selection.suite.get("platforms", []),
                "selected": selection.selected,
                "jobs": selection.jobs,
                "reasons": selection.reasons if selection.selected else [NOT_SELECTED],
            }
            for selection in plan.suites
        ],
    }


def render_selection_text(plan: SelectionPlan) -> str:
    lines = [f"selection {plan.base}...{plan.head}: {len(plan.changed_paths)} changed path(s)"]
    for selection in plan.suites:
        state = "SELECTED" if selection.selected else "not selected"
        jobs = ",".join(selection.jobs) if selection.jobs else "missing route"
        lines.append(f"{state:<12} {selection.suite['id']:<36} jobs={jobs}")
        lines.extend(f"  - {reason}" for reason in selection.reasons or [NOT_SELECTED])
    if plan.errors:
        lines.append("errors:")
        lines.extend(f"  - {error}" for error in plan.errors)
    return "\n".join(lines)


def select_by_level(
    inventory: Inventory,
    routes: Mapping[str, Sequence[str]],
    level: str,
    surface: str | None = None,
) -> SelectionPlan:
    """Select every suite of one registry level, optionally narrowed to one surface."""

    resolved = LEVEL_ALIASES.get(level, level)
    if resolved not in LEVELS:
        known = ", ".join(sorted(LEVELS | set(LEVEL_ALIASES)))
        raise RegistryError(f"unknown test level {level!r}; known levels: {known}")
    surfaces = {suite["surface"] for suite in inventory.suites}
    if surface is not None and surface not in surfaces:
        raise RegistryError(
            f"unknown surface {surface!r}; known surfaces: {', '.join(sorted(surfaces))}"
        )
    reason = f"registry level {resolved}"
    if surface is not None:
        reason += f" on surface {surface}"
    selections = []
    errors: list[str] = []
    for suite in inventory.suites:
        selection = SuiteSelection(suite=suite, jobs=list(routes.get(suite["id"], ())))
        if suite["level"] == resolved and surface in (None, suite["surface"]):
            selection.reasons.append(reason)
            if not selection.jobs:
                errors.append(
                    f"selected suite {suite['id']} has no executable CI route; "
                    f"command is `{suite['command']}`"
                )
        selections.append(selection)
    return SelectionPlan(base="", head="", changed_paths=[], suites=selections, errors=errors)


def host_platform() -> str:
    return PLATFORMS.get(sys.platform, sys.platform)


def run_plan(plan: SelectionPlan, root: Path, *, dry_run: bool = False) -> int:
    """Print the plan with one reason per suite, then run each selected suite command."""

    print(f"selected {len(plan.selected)} of {len(plan.suites)} registry suites")
    for selection in plan.selected:
        print(f"  {selection.suite['id']} [{','.join(selection.jobs) or 'no CI job'}]")
        # One reason per suite; the rest are counted so a wide selection stays readable.
        print(f"      - {selection.reasons[0]}")
        if len(selection.reasons) > 1:
            print(f"        (+{len(selection.reasons) - 1} more reasons)")
    if plan.errors:
        for error in plan.errors:
            print(f"selection error: {error}", file=sys.stderr)
        return 1
    if dry_run:
        print("dry run: no suite command was executed")
        return 0
    sys.stdout.flush()
    for selection in plan.selected:
        suite = selection.suite
        platforms = suite.get("platforms", [])
        if platforms and host_platform() not in platforms:
            print(f"skipped  {suite['id']}: runs only on {', '.join(platforms)}")
            continue
        # Flushed: the plan and every suite banner must precede that suite's own output.
        print(f"running  {suite['id']}: {suite['command']}", flush=True)
        if subprocess.run(["bash", "-c", suite["command"]], cwd=root).returncode != 0:
            print(f"failed   {suite['id']}", file=sys.stderr)
            return 1
    return 0


def _check_gates(command: str) -> list[str]:
    match = re.fullmatch(r"obc check\s+(.+)", str(command).strip())
    return match.group(1).split() if match else []


def gate_claims(
    suites: Sequence[dict[str, Any]], graph: CargoGraph | None = None
) -> dict[str, set[str]]:
    """Derive which registry suites each `obc check` gate reproduces, from suite commands."""

    claims: dict[str, set[str]] = {}
    for suite in suites:
        for gate in _check_gates(suite.get("command", "")):
            claims.setdefault(gate, set()).add(suite["id"])
    if graph is None:
        return claims
    by_id = {suite["id"]: suite for suite in suites}
    by_package: dict[str, set[str]] = {}
    for suite in suites:
        for owner in suite.get("ownership", []):
            if owner.get("kind") == "rust-package":
                by_package.setdefault(owner.get("name", ""), set()).add(suite["id"])
    # A gate that reproduces a workspace-wide Cargo command also reproduces every package
    # suite that command compiles, so the claim does not understate what the run covers.
    for gate, claimed in claims.items():
        expanded = set(claimed)
        for suite_id in claimed:
            for owner in by_id[suite_id].get("ownership", []):
                pattern = owner.get("pattern", "")
                if owner.get("kind") != "workflow" or set("*?[").intersection(pattern):
                    continue
                for args in _cargo_invocations(pattern):
                    for package in _cargo_packages(args, "", graph):
                        expanded |= by_package.get(package, set())
        claims[gate] = expanded
    return claims


def coarse_filters(root: Path) -> dict[str, list[str]]:
    workflow = root / ".github/workflows/ci.yml"
    filters: dict[str, list[str]] = {}
    current = ""
    in_filters = False
    for raw in workflow.read_text(encoding="utf-8").splitlines():
        if raw.strip() == "filters: |":
            in_filters = True
            continue
        if not in_filters:
            continue
        key_match = re.fullmatch(r"            ([A-Za-z0-9_-]+):", raw)
        if key_match:
            current = key_match.group(1)
            filters[current] = []
            continue
        item_match = re.fullmatch(r"              - ['\"](.+)['\"]", raw)
        if item_match and current:
            filters[current].append(item_match.group(1))
            continue
        if raw and not raw.startswith(" "):
            break
    return filters


AUDITED_PATHS = (
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rustfmt.toml",
    ".cargo/config.toml",
    "specs/vectors/obcm-v2.json",
    "ops/weather/check_freshness.py",
    "tools/suite_registry.py",
    "builder/server/nested/handler.py",
    "fixtures/catalog.toml",
    ".github/workflows/bake.yml",
    "testing/suites.toml",
    "builder/app/src/lib/example.ts",
    "companion-ios/Packages/OBCKit/Sources/OBCFormats/example.swift",
    "apps/obc-desktop/src/main.rs",
    "docs/index.md",
)


def _representative_path(pattern: str) -> str:
    return pattern.replace("**", "x").replace("*", "x").strip("/") or "x"


def validate_ci_routing(
    root: Path,
    inventory: Inventory,
    cargo_graph: CargoGraph,
    routes: Mapping[str, Sequence[str]],
) -> list[str]:
    """Prove the plan and the workflow describe the same jobs after the selector cutover."""

    jobs = workflow_jobs(root)
    gate = aggregate_job(root)
    started = unconditional_jobs(root)
    errors: list[str] = []
    for suite_id, suite_jobs in sorted(routes.items()):
        for job in suite_jobs:
            if job not in jobs:
                errors.append(f"suite {suite_id} routes to unknown workflow job {job}")
            elif not jobs[job].runs_on:
                errors.append(f"job {job} provisions no runner image for suite {suite_id}")
    claimed = {job for suite_jobs in routes.values() for job in suite_jobs}
    reported = set(jobs[gate].needs) if gate in jobs else set()
    for name in sorted(jobs):
        if name == gate:
            continue
        if name not in claimed:
            errors.append(f"workflow job {name} runs no registry suite")
        if name not in reported:
            errors.append(f"workflow job {name} is not in the aggregate gate's needs")
        job = jobs[name]
        if job.plan_gated and job.gates_on != name:
            errors.append(
                f"workflow job {name} gates on {job.gates_on or 'no job literal'}, "
                "so the plan can never start it"
            )
        if not job.plan_gated:
            # An ungated job runs on every pull request, so calling any of its suites
            # `affected` would be a lie about when that suite runs.
            for suite in inventory.suites:
                if name in routes.get(suite["id"], ()) and suite.get("pull_request") != "always":
                    errors.append(
                        f"workflow job {name} runs unconditionally but hosts "
                        f"{suite['id']}, whose cadence is {suite.get('pull_request')}"
                    )
    # Only provisioning questions the plan cannot answer may remain in the coarse layer.
    for name, patterns in sorted(coarse_filters(root).items()):
        for pattern in patterns:
            plan = select_suites(
                inventory, [_representative_path(pattern)], cargo_graph, routes, unconditional=started
            )
            if plan.selected:
                errors.append(
                    f"coarse filter {name} entry {pattern!r} encodes suite policy already owned by "
                    f"{plan.selected[0].suite['id']}"
                )
    for path in AUDITED_PATHS:
        plan = select_suites(inventory, [path], cargo_graph, routes, unconditional=started)
        errors.extend(plan.errors)
    if errors:
        raise RegistryError("\n".join(f"- {error}" for error in sorted(set(errors))))
    return list(AUDITED_PATHS)


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


def _affected_plan(root: Path, base: str, head: str) -> SelectionPlan:
    inventory = load_inventory(root)
    graph = build_cargo_graph(root)
    return select_suites(
        inventory,
        git_changed_paths(root, base, head),
        graph,
        suite_workflow_jobs(inventory, root, graph),
        base=base,
        head=head,
        unconditional=unconditional_jobs(root),
    )


def command_select(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    plan = _affected_plan(root, args.base, args.head)
    data = selection_plan_data(plan, workflow_jobs(root))
    if args.jobs_file:
        Path(args.jobs_file).write_text(json.dumps(data["required_jobs"]), encoding="utf-8")
    if args.format == "json":
        print(json.dumps(data, sort_keys=True, separators=(",", ":")))
    else:
        print(render_selection_text(plan))
    return 1 if plan.errors else 0


def command_run(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    if args.affected == bool(args.level):
        raise RegistryError("run needs exactly one of --affected or --level")
    if args.affected:
        if not args.base:
            raise RegistryError(
                "`affected` needs an explicit base revision: obc test affected --base origin/develop"
            )
        plan = _affected_plan(root, args.base, args.head)
    else:
        inventory = load_inventory(root)
        routes = suite_workflow_jobs(inventory, root, build_cargo_graph(root))
        plan = select_by_level(inventory, routes, args.level, args.surface)
    return run_plan(plan, root, dry_run=args.dry_run)


def command_gates(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    suites = _read_toml(root / "testing/suites.toml").get("suite", [])
    if args.list:
        print(" ".join(sorted({gate for suite in suites for gate in _check_gates(suite.get("command", ""))})))
        return 0
    if not args.gate:
        raise RegistryError("name at least one gate, or pass --list")
    claims = gate_claims(suites, build_cargo_graph(root) if args.unreproduced else None)
    unknown = [gate for gate in args.gate if gate not in claims]
    if unknown:
        raise RegistryError(
            f"gate(s) {', '.join(unknown)} reproduce no registry suite; "
            "add the `obc check` route to the suite they gate or rename them back"
        )
    reproduced = set().union(*(claims[gate] for gate in args.gate))
    # A step that ran a suite's own command reproduces it whatever the gate map says.
    reproduced |= {suite["id"] for suite in suites if suite.get("command") in set(args.ran)}
    if not args.unreproduced:
        print("reproduces registry suites: " + ", ".join(sorted(reproduced)))
        return 0
    host = host_platform()
    missing = [
        suite
        for suite in suites
        if suite.get("pull_request") == "always" and suite["id"] not in reproduced
    ]
    if not missing:
        print("this run reproduces every registry suite required on a pull request")
        return 0
    print("required registry suites this run does not reproduce:")
    for suite in missing:
        platforms = suite.get("platforms", [])
        reason = (
            f"runs only on {', '.join(platforms)}"
            if platforms and host not in platforms
            else "no gate in this run runs its command"
        )
        print(f"  - {suite['id']}: {reason}")
    return 0


def command_validate_filters(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    inventory = load_inventory(root)
    graph = build_cargo_graph(root)
    audited = validate_ci_routing(root, inventory, graph, suite_workflow_jobs(inventory, root, graph))
    print(
        f"plan-derived CI routing covers {len(workflow_jobs(root))} workflow jobs "
        f"and {len(audited)} audited selection classes"
    )
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
    select = subparsers.add_parser("select", help="select affected suites from a Git diff")
    select.add_argument("--base", required=True, help="base Git revision")
    select.add_argument("--head", default="HEAD", help="head Git revision (default: HEAD)")
    select.add_argument("--format", choices=("text", "json"), default="text")
    select.add_argument("--jobs-file", help="also write the required workflow jobs as a JSON array")
    select.set_defaults(func=command_select)
    run = subparsers.add_parser("run", help="run the suites of a level, surface, or Git range")
    run.add_argument("--level", help="registry level (fixtures and e2e are the only aliases)")
    run.add_argument("--surface", help="narrow a level selection to one product surface")
    run.add_argument("--affected", action="store_true", help="select from a Git range instead")
    run.add_argument("--base", help="base Git revision, required by --affected")
    run.add_argument("--head", default="HEAD", help="head Git revision (default: HEAD)")
    run.add_argument("--dry-run", action="store_true", help="print the plan and run nothing")
    run.set_defaults(func=command_run)
    gates = subparsers.add_parser("gates", help="report the suites `obc check` gates reproduce")
    gates.add_argument("gate", nargs="*", help="gate names to resolve")
    gates.add_argument("--list", action="store_true", help="print every gate the registry claims")
    gates.add_argument(
        "--unreproduced", action="store_true", help="list the required suites the run omits"
    )
    gates.add_argument("--ran", nargs="*", default=(), help="commands the run actually executed")
    gates.set_defaults(func=command_gates)
    filters = subparsers.add_parser(
        "validate-filters",
        help="prove the plan and the workflow describe the same jobs",
    )
    filters.set_defaults(func=command_validate_filters)
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
