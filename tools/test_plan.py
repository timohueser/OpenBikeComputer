#!/usr/bin/env python3
"""Decide which CI jobs and test suites a change requires.

Cargo's own graph decides Rust selection: every root-workspace package, its reverse
dependencies and its test targets come from `cargo metadata`.  Only the facts Cargo
cannot see live here — the job table below, and `testing/suites.toml`, which holds the
suites no Cargo package owns plus the per-package path triggers and platform limits.

Selection is standard library only.  The structural workflow check in `validate-filters`
is the one command that needs PyYAML (`tools/requirements-test.txt`).
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import shlex
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable, Mapping, Sequence

sys.path.insert(0, str(Path(__file__).resolve().parent))

import test_exceptions

# ─────────────────────────────────────────────────────────── the job table ──
# One row per CI job.  `roots` and `packages` say which Cargo packages a job
# compiles; nothing else in this file restates Cargo.

ROOT_WORKSPACE = "."
PRODUCT_ROOTS = (
    ROOT_WORKSPACE,
    "firmware/obc-fw-nrf54l",
    "firmware/obc-boot",
    "apps/obc-desktop",
)

@dataclass(frozen=True)
class Job:
    needs: tuple[str, ...] = ()
    roots: tuple[str, ...] = ()
    packages: tuple[str, ...] = ()
    script: str = ""
    unconditional: bool = False

JOBS: dict[str, Job] = {
    "selection": Job(unconditional=True),
    "guards": Job(unconditional=True),
    "fmt": Job(needs=("selection",), roots=PRODUCT_ROOTS),
    "clippy": Job(needs=("selection",), roots=(ROOT_WORKSPACE,)),
    "test": Job(needs=("selection",), roots=(ROOT_WORKSPACE,), script="tools/ci/test.sh"),
    "ui-snapshots": Job(needs=("selection",), packages=("obc-sim",)),
    "builder-python": Job(needs=("selection",), packages=("obc-pack",)),
    "embedded": Job(needs=("selection",), roots=("firmware/obc-fw-nrf54l",)),
    "boot": Job(needs=("selection",), roots=("firmware/obc-boot",)),
    "device": Job(needs=("selection",), packages=("obc-app", "obc-link")),
    "deny": Job(needs=("selection",)),
    # Trunk bundles the demo and the engine is built for wasm32 by hand; wasm-pack drives
    # the four bridges.  No `cargo` argument list names them, so they are stated here.
    "wasm": Job(needs=("selection",), packages=("obc-web-demo", "obcm-assemble")),
    "wasm-bridges": Job(
        needs=("selection",),
        packages=("obc-web-convert", "obc-web-assemble", "obc-skin-preview", "obc-flat-device"),
    ),
    "docs": Job(needs=("selection",)),
    "ios-unit": Job(needs=("selection",)),
    "ios-app": Job(needs=("selection",)),
    "ios-release": Job(needs=("selection",)),
    "web": Job(needs=("selection", "wasm-bridges")),
    "web-browser": Job(needs=("selection", "wasm-bridges")),
    "verification": Job(needs=("selection",)),
    "desktop-frontend": Job(needs=("selection", "wasm-bridges")),
    "desktop": Job(needs=("selection", "desktop-frontend"), roots=("apps/obc-desktop",)),
    "desktop-launch": Job(needs=("selection", "desktop")),
}

# The job that evaluates the plan.  It is never a route for the work it reports.
AGGREGATE = "ci"

# `obc check <gate>` runs one job's work locally.  A gate that names several jobs runs
# all of them.
GATES: dict[str, tuple[str, ...]] = {
    "fmt": ("fmt",),
    "clippy": ("clippy",),
    "test": ("test",),
    "device": ("device",),
    "board": ("embedded", "boot"),
    "deny": ("deny",),
    "wasm": ("wasm", "wasm-bridges"),
    "frontend": ("web", "desktop-frontend"),
    "docs": ("docs",),
}

PLATFORMS = {"linux": "linux", "darwin": "macos", "win32": "windows"}
KNOWN_PLATFORMS = set(PLATFORMS.values())

# A suite route says when the suite runs, and nothing else.
ROUTES = {
    "ordinary": "selected by the change",
    "required": "runs whenever one of its jobs starts",
    "manual": "explicitly invoked only",
    "live": "contacts a live service; explicitly invoked only",
}
CI_ROUTES = {"ordinary", "required"}

RUST_FOUNDATION_PATHS = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rustfmt.toml",
    ".cargo/config.toml",
    ".cargo/config",
}
# A change to how this repository decides or executes verification selects every declared
# suite: the decision itself is what changed. Agent prose (CLAUDE.md, AGENTS.md) is not on
# this list: it instructs an agent, it does not decide or execute anything. It is owned by the
# documentation route so it is not an unowned path; the check that reads it is the
# unconditional `guards` job, not a platform build.
TEST_POLICY_PATTERNS = (
    ".config/nextest.toml",
    ".github/workflows/**",
    ".github/actions/**",
    "testing/**",
    "tools/ci/**",
    "tools/test_plan.py",
    "tools/ci_aggregate.py",
    "tools/coverage_report.py",
    "tools/requirements-coverage.txt",
    "docs/testing.md",
    "CONTRIBUTING.md",
    "tools/justfile",
    "tools/obc",
    "tools/obc-dev.sh",
)
CODE_OR_POLICY_SUFFIXES = {
    ".c", ".h", ".js", ".json", ".py", ".rs", ".sh", ".swift", ".toml", ".ts", ".tsx", ".yaml", ".yml",
}
# The captured-fixture tier is exactly the targets Cargo gates on this feature.
FIXTURE_FEATURE = "external-fixtures"
# The reason prefix that means "run everything, including the snapshot sweep".
WHOLE_GRAPH = "whole graph:"
# The other two reasons that claim the graph as a whole instead of following an edge.
POLICY_CHANGED = "test policy changed:"
FOUNDATION_CHANGED = "foundational Rust input changed:"
WHOLESALE = (WHOLE_GRAPH, POLICY_CHANGED, FOUNDATION_CHANGED)

class PlanError(Exception):
    """The plan or its inputs are invalid."""

# ────────────────────────────────────────────────────────────── Cargo graph ──

@dataclass(frozen=True)
class Package:
    name: str
    root: str
    manifest: str
    product_root: str
    dependencies: frozenset[str]
    ordinary_targets: tuple[str, ...]
    fixture_targets: tuple[str, ...]
    example_targets: tuple[str, ...]

@dataclass(frozen=True)
class CargoGraph:
    packages: Mapping[str, Package]
    reverse_dependencies: Mapping[str, frozenset[str]]

    def reverse_closure(self, package: str) -> dict[str, str]:
        """Reverse dependents mapped to the edge that first reached them."""

        reached: dict[str, str] = {}
        pending = [package]
        while pending:
            dependency = pending.pop(0)
            for consumer in sorted(self.reverse_dependencies.get(dependency, frozenset())):
                if consumer == package or consumer in reached:
                    continue
                reached[consumer] = dependency
                pending.append(consumer)
        return reached

def repository_root() -> Path:
    return Path(__file__).resolve().parents[1]

def _cargo_metadata(root: Path, manifest: str | None) -> dict[str, Any]:
    command = ["cargo", "metadata", "--format-version", "1", "--locked", "--no-deps"]
    if manifest is not None:
        command.extend(["--manifest-path", manifest])
    try:
        result = subprocess.run(command, cwd=root, check=True, capture_output=True, text=True)
        return json.loads(result.stdout)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as exc:
        detail = getattr(exc, "stderr", "") or str(exc)
        raise PlanError(f"cargo metadata failed: {detail.strip()}") from exc

def _target_split(package: Mapping[str, Any]) -> tuple[tuple[str, ...], ...]:
    """Split a package's targets into the ordinary tier, the fixture tier and examples.

    An example is a generator or a probe: it compiles on demand, never in a tier.
    """

    ordinary: list[str] = []
    fixtures: list[str] = []
    examples: list[str] = []
    for target in package.get("targets", []):
        required = target.get("required-features") or ()
        if "example" in set(target.get("kind", [])):
            examples.append(target["name"])
        elif not target.get("test"):
            continue
        elif FIXTURE_FEATURE in required:
            fixtures.append(target["name"])
        else:
            # Every other feature gate compiles: the workspace run is `--all-features`.
            ordinary.append(target["name"])
    return tuple(sorted(ordinary)), tuple(sorted(fixtures)), tuple(sorted(examples))

def build_cargo_graph(
    root: Path,
    metadata_loader: Callable[[Path, str | None], dict[str, Any]] = _cargo_metadata,
) -> CargoGraph:
    raw: dict[str, dict[str, Any]] = {}
    for product_root in PRODUCT_ROOTS:
        manifest = None if product_root == ROOT_WORKSPACE else f"{product_root}/Cargo.toml"
        if manifest is not None and not (root / manifest).exists():
            continue
        for package in metadata_loader(root, manifest).get("packages", []):
            try:
                relative = Path(package["manifest_path"]).resolve().relative_to(root.resolve()).as_posix()
            except ValueError:
                continue
            ordinary, fixtures, examples = _target_split(package)
            raw[package["name"]] = {
                "manifest": relative,
                "product_root": product_root,
                "dependencies": {edge["name"] for edge in package.get("dependencies", [])},
                "ordinary": ordinary,
                "fixtures": fixtures,
                "examples": examples,
            }
    names = set(raw)
    packages: dict[str, Package] = {}
    reverse: dict[str, set[str]] = {name: set() for name in names}
    for name, entry in raw.items():
        dependencies = frozenset(entry["dependencies"] & names)
        packages[name] = Package(
            name,
            str(Path(entry["manifest"]).parent),
            entry["manifest"],
            entry["product_root"],
            dependencies,
            entry["ordinary"],
            entry["fixtures"],
            entry["examples"],
        )
        for dependency in dependencies:
            reverse[dependency].add(name)
    return CargoGraph(packages, {name: frozenset(value) for name, value in reverse.items()})

def package_jobs(graph: CargoGraph, package: str) -> list[str]:
    """Every job that compiles, lints, formats or runs one package."""

    product_root = graph.packages[package].product_root
    return sorted(
        name
        for name, job in JOBS.items()
        if product_root in job.roots or package in job.packages
    )

# ─────────────────────────────────────────────────────── declared documents ──

@dataclass
class Unit:
    """One reported piece of verification: a Cargo package, or a declared suite."""

    id: str
    jobs: list[str]
    route: str = "ordinary"
    command: str = ""
    triggers: tuple[str, ...] = ()
    platforms: tuple[str, ...] = ()
    fixtures: bool = False
    foundation: bool = False
    ci_only: bool = False
    declared: bool = False
    package: str = ""
    reasons: list[str] = field(default_factory=list)

    @property
    def selected(self) -> bool:
        return bool(self.reasons)

def read_toml(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise PlanError(f"cannot parse {path}: {exc}") from exc

def load_document(root: Path) -> dict[str, Any]:
    document = read_toml(root / "testing/suites.toml")
    if document.get("schema") != 2:
        raise PlanError("testing/suites.toml: schema must be 2")
    return document

def carved_targets(document: Mapping[str, Any]) -> set[tuple[str, str]]:
    """Targets a manual, weekly or live suite owns, so no tier compiles them."""

    return {
        (suite["package"], target)
        for suite in document.get("suite", [])
        if suite.get("package")
        for target in suite.get("targets", ())
    }

def build_units(graph: CargoGraph, document: Mapping[str, Any]) -> list[Unit]:
    facts = {entry["name"]: entry for entry in document.get("package", [])}
    units: list[Unit] = []
    for name, package in sorted(graph.packages.items()):
        entry = facts.get(name, {})
        units.append(
            Unit(
                id=f"rust.{name}",
                jobs=package_jobs(graph, name),
                route=entry.get("route", "ordinary"),
                command=entry.get("command", ""),
                triggers=tuple(entry.get("triggers", ())),
                platforms=tuple(entry.get("platforms", ())),
                fixtures=bool(package.fixture_targets) or bool(entry.get("fixtures")),
                package=name,
            )
        )
    for suite in document.get("suite", []):
        units.append(
            Unit(
                id=suite["id"],
                jobs=list(suite.get("jobs", ())),
                route=suite.get("route", "ordinary"),
                command=suite.get("command", ""),
                triggers=tuple(suite.get("triggers", ())),
                platforms=tuple(suite.get("platforms", ())),
                fixtures=bool(suite.get("fixtures")),
                foundation=bool(suite.get("foundation")),
                ci_only=bool(suite.get("ci_only")),
                declared=True,
                package=suite.get("package", ""),
            )
        )
    return units

# ──────────────────────────────────────────────────────────────── selection ──

@dataclass
class Plan:
    base: str
    head: str
    changed_paths: list[str]
    units: list[Unit]
    packages: list[str]  # root-workspace packages only: the `-p` arguments
    errors: list[str]

    @property
    def selected(self) -> list[Unit]:
        return [unit for unit in self.units if unit.selected]

def glob_matches(path: str, pattern: str) -> bool:
    """Match repository globs, including the zero-directory meaning of `**/`."""

    if fnmatch.fnmatchcase(path, pattern):
        return True
    collapsed = pattern
    while "**/" in collapsed:
        collapsed = collapsed.replace("**/", "", 1)
        if fnmatch.fnmatchcase(path, collapsed):
            return True
    return False

def _git(root: Path, *arguments: str) -> str:
    try:
        return subprocess.run(
            ["git", *arguments], cwd=root, check=True, capture_output=True, text=True
        ).stdout
    except (OSError, subprocess.CalledProcessError) as exc:
        detail = getattr(exc, "stderr", "") or str(exc)
        raise PlanError(f"cannot read changed paths from Git: {detail.strip()}") from exc

def git_changed_paths(root: Path, base: str, head: str) -> tuple[list[str], set[str]]:
    """Changed paths for a revision range, plus the subset that no longer exists.

    A rename reports both of its paths so the source it left is still owned.
    """

    changed: set[str] = set()
    deleted: set[str] = set()
    for line in _git(root, "diff", "--name-status", "-M", f"{base}...{head}").splitlines():
        fields = line.split("\t")
        if len(fields) < 2:
            continue
        status, paths = fields[0], [value.strip() for value in fields[1:] if value.strip()]
        changed.update(paths)
        if status.startswith("D"):
            deleted.update(paths)
        elif status.startswith("R"):
            deleted.add(paths[0])
    return sorted(changed), deleted

def working_tree_paths(root: Path) -> list[str]:
    """Tracked edits and new source files that are not committed yet."""

    lines = _git(root, "diff", "--name-only", "HEAD").splitlines()
    lines += _git(root, "ls-files", "--others", "--exclude-standard").splitlines()
    return sorted({value.strip() for value in lines if value.strip()})

def is_policy_path(path: str) -> bool:
    return any(glob_matches(path, pattern) for pattern in TEST_POLICY_PATTERNS)

def looks_like_production(path: str) -> bool:
    # `scratch/` is working material kept for the next reader, not production: nothing builds it
    # and nothing depends on it. Each directory under it carries a README saying when to delete it.
    if path.startswith(("docs/", "artifacts/", "scratch/", ".claude/", ".repowise/")):
        return False
    name = Path(path).name
    if name.startswith("test_") or "/tests/" in path or "/test/" in path:
        return False
    return Path(path).suffix.lower() in CODE_OR_POLICY_SUFFIXES

def select(
    units: Sequence[Unit],
    graph: CargoGraph,
    changed_paths: Sequence[str],
    *,
    deleted: Iterable[str] = (),
    base: str = "",
    head: str = "HEAD",
) -> Plan:
    """Build the deterministic plan from the changed paths and the Cargo graph."""

    units = [
        Unit(**{key: value for key, value in vars(unit).items() if key != "reasons"})
        for unit in units
    ]
    by_id = {unit.id: unit for unit in units}
    by_package = {unit.package: unit for unit in units if unit.package and not unit.declared}
    gone = set(deleted)
    errors: list[str] = []
    selected_packages: set[str] = set()

    def claim(unit: Unit, reason: str) -> None:
        if unit.route not in CI_ROUTES or reason in unit.reasons:
            return
        unit.reasons.append(reason)
        if unit.package and not unit.declared and graph.packages[unit.package].product_root == ROOT_WORKSPACE:
            selected_packages.add(unit.package)

    def claim_package(name: str, reason: str) -> None:
        if name in by_package:
            claim(by_package[name], reason)

    for path in sorted(set(changed_paths)):
        owned = False
        if is_policy_path(path):
            owned = True
            for unit in units:
                if unit.declared:
                    claim(unit, f"{POLICY_CHANGED} {path}")

        if path in RUST_FOUNDATION_PATHS:
            owned = True
            reason = f"{FOUNDATION_CHANGED} {path}"
            for name, package in graph.packages.items():
                if path in {"Cargo.toml", "Cargo.lock"} and package.product_root != ROOT_WORKSPACE:
                    continue
                claim_package(name, reason)
            for unit in units:
                if unit.foundation:
                    claim(unit, reason)

        for name, package in sorted(graph.packages.items()):
            prefix = "" if package.root == "." else f"{package.root}/"
            if path != package.manifest and not (prefix and path.startswith(prefix)):
                continue
            owned = True
            claim_package(name, f"changed Rust package {name}: {path}")
            for consumer, dependency in graph.reverse_closure(name).items():
                claim_package(
                    consumer,
                    f"reverse dependency {consumer} compiles {dependency} after {name} changed",
                )

        if path.startswith("fixtures/"):
            owned = True
            for unit in units:
                if unit.fixtures:
                    claim(unit, f"declared fixture input changed: {path}")

        for unit in units:
            for pattern in unit.triggers:
                if glob_matches(path, pattern):
                    owned = True
                    claim(unit, f"trigger {pattern} matched {path}")

        if owned:
            continue
        if path in gone:
            # The owner may have been deleted with it, and the base tree's Cargo graph is
            # not available here, so this runs the whole graph rather than nothing.
            reason = f"{WHOLE_GRAPH} deleted path with no owner in the head tree: {path}"
            for name in graph.packages:
                claim_package(name, reason)
            for unit in units:
                if unit.declared and unit.jobs:
                    claim(unit, reason)
        elif looks_like_production(path):
            errors.append(
                f"changed production path has no owner: {path}; "
                "add a trigger in testing/suites.toml or a Cargo package that contains it"
            )

    # The snapshot sweep has an explicit rendering-input budget: broad policy and fixture
    # changes must not add an otherwise unrelated full UI render run. A deleted path with no
    # owner is the exception, because it may have been a rendering input.
    if sweep := by_id.get("ci.ui-snapshots"):
        sweep.reasons = [
            reason
            for reason in sweep.reasons
            if reason.startswith("trigger ") or reason.startswith(WHOLE_GRAPH)
        ]

    started = {job for unit in units if unit.selected for job in unit.jobs}
    always = {name for name, job in JOBS.items() if job.unconditional}
    # A required unit rides the jobs a change already started. It is not affected by the
    # change, so it never joins the package set the local run compiles.
    for unit in units:
        if unit.route != "required" or unit.selected:
            continue
        if riding := always.intersection(unit.jobs) or started.intersection(unit.jobs):
            unit.jobs = sorted(riding)
            unit.reasons.append("required whenever one of its CI jobs starts: " + ", ".join(unit.jobs))

    for unit in units:
        if unit.selected and not unit.jobs:
            errors.append(f"selected suite {unit.id} has no executable CI route")
        unit.reasons.sort()
    return Plan(
        base=base,
        head=head,
        changed_paths=sorted(set(changed_paths)),
        units=units,
        packages=sorted(selected_packages),
        errors=sorted(set(errors)),
    )

def wholesale_reason(units: Iterable[Unit]) -> str:
    """The first reason that claimed the graph as a whole, or "" when every unit came by edge.

    A foundation input, the test policy and a deleted path with no owner each select nearly
    everything. That selection says what changed reached the whole repository; it does not say
    that the whole repository is worth compiling again on one machine.
    """

    return next(
        (reason for unit in units for reason in unit.reasons if reason.startswith(WHOLESALE)),
        "",
    )

# A release candidate is verified whole, not by its diff. The sweep keeps its
# rendering-input budget, and no live or physical route is ever pulled in.
RELEASE_EXCLUDED = {"ci.ui-snapshots"}

def select_release(plan: Plan) -> Plan:
    """Require every unit with a CI route, whatever the change touched."""

    for unit in plan.units:
        if unit.jobs and unit.route in CI_ROUTES and unit.id not in RELEASE_EXCLUDED:
            reason = "release candidate requires the complete CI suite"
            if reason not in unit.reasons:
                unit.reasons.append(reason)
    return plan

def required_jobs(plan: Plan) -> list[str]:
    """Close the selected jobs over the prerequisite graph so producers still run."""

    required = {job for unit in plan.selected for job in unit.jobs}
    pending = sorted(required)
    while pending:
        for upstream in JOBS.get(pending.pop(), Job()).needs:
            if upstream not in required:
                required.add(upstream)
                pending.append(upstream)
    return sorted(required)

NOT_SELECTED = "no changed path, Cargo edge, or required route selected this suite"

def plan_data(plan: Plan) -> dict[str, Any]:
    return {
        "schema": 1,
        "base": plan.base,
        "head": plan.head,
        "changed_paths": plan.changed_paths,
        "packages": plan.packages,
        "selected_suite_ids": [unit.id for unit in plan.selected],
        "required_jobs": required_jobs(plan),
        "errors": plan.errors,
        "suites": [
            {
                "id": unit.id,
                "route": unit.route,
                "platforms": list(unit.platforms),
                "selected": unit.selected,
                "jobs": unit.jobs,
                "reasons": unit.reasons if unit.selected else [NOT_SELECTED],
            }
            for unit in plan.units
        ],
    }

def render_text(plan: Plan) -> str:
    lines = [f"selection {plan.base}...{plan.head}: {len(plan.changed_paths)} changed path(s)"]
    for unit in plan.units:
        state = "SELECTED" if unit.selected else "not selected"
        jobs = ",".join(unit.jobs) if unit.jobs else "no CI job"
        lines.append(f"{state:<12} {unit.id:<36} jobs={jobs}")
        lines.extend(f"  - {reason}" for reason in unit.reasons or [NOT_SELECTED])
    lines.append("packages: " + (", ".join(plan.packages) or "none"))
    if plan.errors:
        lines.append("errors:")
        lines.extend(f"  - {error}" for error in plan.errors)
    return "\n".join(lines)

# ─────────────────────────────────────────────────────────────── execution ──

def cargo_filter(
    graph: CargoGraph,
    tier: str,
    packages: Iterable[str] | None = None,
    carved: Iterable[tuple[str, str]] = (),
) -> str:
    """A nextest expression over whole test binaries of the root workspace."""

    wanted = None if packages is None else set(packages)
    skip = set(carved)
    terms = set()
    for name, package in graph.packages.items():
        if package.product_root != ROOT_WORKSPACE or (wanted is not None and name not in wanted):
            continue
        targets = package.ordinary_targets if tier == "fast" else package.fixture_targets
        terms.update(
            f"(package(={name}) & binary(={target}))"
            for target in targets
            if (name, target) not in skip
        )
    if not terms:
        raise PlanError(f"no Rust binaries for {tier}")
    return " | ".join(sorted(terms))

def host_platform() -> str:
    return PLATFORMS.get(sys.platform, sys.platform)

def run_plan(
    plan: Plan,
    graph: CargoGraph,
    root: Path,
    carved: Iterable[tuple[str, str]] = (),
    *,
    dry_run: bool = False,
) -> int:
    """Run the selected Rust binaries once, then every selected suite command."""

    packages = list(plan.packages)
    commands: list[list[str]] = []
    if packages:
        # Package flags narrow compilation; the tier filter narrows execution. An empty
        # package set produces no Cargo invocation, never an implicit whole workspace.
        #
        # `--no-tests warn`, not `fail`: a narrowed set can legitimately contain only
        # packages that declare no test yet, and that is not a failure of the change. The
        # workspace run in `tools/ci/test.sh` keeps `fail`, where an empty run really does
        # mean the filter is wrong.
        commands.append(
            ["cargo", "nextest", "run", "--locked", "--all-features", "--no-tests", "warn"]
            + [argument for name in packages for argument in ("-p", name)]
            + ["--filter-expr", cargo_filter(graph, "fast", packages, carved)]
        )
    suites = [unit for unit in plan.selected if unit.command]
    print(f"selected {len(plan.selected)} of {len(plan.units)} units, {len(packages)} Cargo package(s)")
    for unit in plan.selected:
        print(f"  {unit.id} [{','.join(unit.jobs) or 'no CI job'}]")
        print(f"      - {unit.reasons[0]}")
        if len(unit.reasons) > 1:
            print(f"        (+{len(unit.reasons) - 1} more reasons)")
    if plan.errors:
        for error in plan.errors:
            print(f"selection error: {error}", file=sys.stderr)
        return 1
    if dry_run:
        for command in commands:
            print("would run: " + shlex.join(command))
        for unit in suites:
            print(f"would run: {unit.id}: {unit.command}")
        print("dry run: nothing was executed")
        return 0
    sys.stdout.flush()
    for command in commands:
        print("running  " + shlex.join(command), flush=True)
        if subprocess.run(command, cwd=root).returncode != 0:
            return 1
    for unit in suites:
        if unit.platforms and host_platform() not in unit.platforms:
            print(f"skipped  {unit.id}: runs only on {', '.join(unit.platforms)}")
            continue
        print(f"running  {unit.id}: {unit.command}", flush=True)
        if subprocess.run(["bash", "-c", unit.command], cwd=root).returncode != 0:
            print(f"failed   {unit.id}", file=sys.stderr)
            return 1
    return 0

def gate_jobs(gates: Sequence[str]) -> set[str]:
    unknown = [gate for gate in gates if gate not in GATES]
    if unknown:
        raise PlanError(
            f"unknown gate(s) {', '.join(unknown)}; known gates: {', '.join(sorted(GATES))}"
        )
    return {job for gate in gates for job in GATES[gate]}

def reproduced(units: Sequence[Unit], gates: Sequence[str]) -> list[Unit]:
    """Units whose whole CI route one local gate run re-executes."""

    claimed = gate_jobs(gates)
    return [
        unit
        for unit in units
        if unit.jobs and not unit.ci_only and set(unit.jobs).issubset(claimed)
    ]

# ─────────────────────────────────────────────────────────────── validation ──

def _command_errors(root: Path, unit: Unit, graph: CargoGraph) -> list[str]:
    """Reject a command whose executable, working directory or package does not exist."""

    if not unit.command.strip():
        return [f"{unit.id}: command must be one non-empty string"]
    try:
        words = shlex.split(unit.command.replace("\n", " "))
    except ValueError as exc:
        return [f"{unit.id}: command cannot be parsed: {exc}"]
    errors: list[str] = []
    expect_executable = True
    cargo = False
    for index, word in enumerate(words):
        if word in {"&&", "||", ";", "|"}:
            expect_executable, cargo = True, False
            continue
        if cargo and word in {"-p", "--package"} and index + 1 < len(words):
            if words[index + 1] not in graph.packages:
                errors.append(f"{unit.id}: command names unknown Cargo package {words[index + 1]}")
        if not expect_executable:
            continue
        if "=" in word and not word.startswith(("./", "/")):
            continue  # an environment prefix, not the executable
        if word in {"cd", "env", "rm", "mkdir"}:
            continue
        expect_executable = False
        cargo = word == "cargo"
        executable = word[2:] if word.startswith("./") else word
        if "/" in executable and not (root / executable).exists():
            errors.append(f"{unit.id}: command path does not exist: {executable}")
    return errors

def _coverage_errors(root: Path) -> list[str]:
    document = read_toml(root / "testing/coverage-policy.toml")
    errors: list[str] = []
    if document.get("schema") != 1:
        errors.append("testing/coverage-policy.toml: schema must be 1")
    for exclusion in document.get("exclude", []):
        if not isinstance(exclusion, dict) or not all(
            isinstance(exclusion.get(key), str) and exclusion[key].strip() for key in ("path", "evidence")
        ):
            errors.append("coverage: every global exclusion needs path and replacement evidence")
    identifiers = [component.get("id") for component in document.get("component", [])]
    duplicates = sorted({value for value in identifiers if value and identifiers.count(value) > 1})
    if duplicates:
        errors.append(f"duplicate coverage component IDs: {', '.join(duplicates)}")
    for component in document.get("component", []):
        name = component.get("id", "<missing-id>")
        if component.get("enforcement") not in {"ratchet", "report"}:
            errors.append(f"coverage {name}: invalid enforcement {component.get('enforcement')!r}")
        if name in SAFETY_COMPONENTS and component.get("enforcement") != "ratchet":
            errors.append(f"coverage {name}: safety-critical component must be planned as ratchet")
        if not component.get("include"):
            errors.append(f"coverage {name}: include must name production paths")
        for pattern in component.get("include", []):
            if not any(root.glob(pattern)):
                errors.append(f"coverage {name}: included path does not resolve: {pattern!r}")
        for exclusion in component.get("exclude", []):
            if not isinstance(exclusion, dict) or not exclusion.get("path") or not exclusion.get("evidence"):
                errors.append(f"coverage {name}: every exclusion needs path and replacement evidence")
        if component.get("baseline") in {None, "pending", ""} and "baseline" in component:
            errors.append(f"coverage {name}: omit pending baseline instead of inventing a value")
    return errors

SAFETY_COMPONENTS = {"format-protocol-codecs", "crc", "storage", "dfu", "boot"}

def validate(root: Path, graph: CargoGraph, document: Mapping[str, Any], units: Sequence[Unit]) -> list[str]:
    errors: list[str] = []
    identifiers = [unit.id for unit in units]
    duplicates = sorted({value for value in identifiers if identifiers.count(value) > 1})
    if duplicates:
        errors.append(f"duplicate unit IDs: {', '.join(duplicates)}")
    for entry in document.get("package", []):
        if entry["name"] not in graph.packages:
            errors.append(f"package facts name an unknown Cargo package: {entry['name']}")
    for unit in units:
        for pattern in unit.triggers:
            if not any(root.glob(pattern)):
                errors.append(f"{unit.id}: trigger matches no maintained path: {pattern!r}")
        for platform in unit.platforms:
            if platform not in KNOWN_PLATFORMS:
                errors.append(f"{unit.id}: unknown platform {platform!r}")
        if unit.route not in ROUTES:
            errors.append(f"{unit.id}: unknown route {unit.route!r}")
        for job in unit.jobs:
            if job not in JOBS:
                errors.append(f"{unit.id}: routes to unknown job {job}")
        if unit.route in CI_ROUTES and not unit.jobs:
            errors.append(f"{unit.id}: an {unit.route} suite needs at least one CI job")
        if unit.route not in CI_ROUTES and unit.jobs:
            errors.append(f"{unit.id}: a {unit.route} suite runs no CI job, so it may not name one")
        if unit.command:
            errors.extend(_command_errors(root, unit, graph))
        if unit.package and unit.package not in graph.packages:
            errors.append(f"{unit.id}: names an unknown Cargo package {unit.package}")
    for suite in document.get("suite", []):
        for name in test_exceptions.EXCEPTION_FIELDS:
            if name in suite:
                errors.extend(test_exceptions.block_errors(f"{suite['id']}.{name}", suite[name]))
        package = graph.packages.get(suite.get("package", ""))
        if package is None:
            continue
        known = package.ordinary_targets + package.fixture_targets + package.example_targets
        for target in suite.get("targets", ()):
            if target not in known:
                errors.append(f"{suite['id']}: {package.name} has no target {target}")
    routed = {job for unit in units for job in unit.jobs}
    for name, job in JOBS.items():
        if job.script and not (root / job.script).is_file():
            errors.append(f"job {name} names a missing script {job.script}")
        if not (job.unconditional or job.roots or job.packages or name in routed):
            errors.append(f"job {name} runs no declared suite")
    errors.extend(_coverage_errors(root))
    errors.extend(_ui_frame_errors(root))
    return errors

def _ui_frame_errors(root: Path) -> list[str]:
    """Every UI frame has a digest row, and every digest row has a frame.

    A frame is only in the net once `ui-snapshots.sha256` names it, and the sweep itself cannot say
    so: it compares the manifest against the frames it just rendered, so a frame added without its
    row, or a row left behind, both look clean there.
    """
    sys.path.insert(0, str(root / "firmware/tools"))
    try:
        from ui_frames import manifest, table
    finally:
        sys.path.pop(0)
    try:
        frames = table.load(root / "firmware/ui-frames.toml")
        problems = manifest.stale([frame.name for frame in frames], root / "firmware/ui-snapshots.sha256")
    except (table.TableError, manifest.ManifestError) as exc:
        problems = [str(exc)]
    return [f"ui frames: {problem}" for problem in problems]

def validate_workflow(root: Path) -> list[str]:
    """One parsed-YAML structural check over the workflow's routing."""

    try:
        import yaml
    except ModuleNotFoundError as exc:  # pragma: no cover - depends on the environment
        raise PlanError(
            "validate-filters needs PyYAML: pip install -r tools/requirements-test.txt"
        ) from exc
    with (root / ".github/workflows/ci.yml").open("rb") as handle:
        workflow = yaml.safe_load(handle)
    jobs = workflow.get("jobs", {})
    errors: list[str] = []
    for name in sorted(set(jobs) - set(JOBS) - {AGGREGATE}):
        errors.append(f"workflow job {name} is not in the job table")
    for name, job in sorted(JOBS.items()):
        definition = jobs.get(name)
        if definition is None:
            errors.append(f"job {name} has no workflow route")
            continue
        if not definition.get("runs-on"):
            errors.append(f"job {name} provisions no runner image")
        needs = definition.get("needs") or []
        needs = [needs] if isinstance(needs, str) else list(needs)
        if sorted(needs) != sorted(job.needs):
            errors.append(f"job {name} needs {sorted(needs)}, the job table says {sorted(job.needs)}")
        condition = str(definition.get("if") or "")
        gated = "needs.selection.outputs.jobs" in condition
        if job.unconditional and gated:
            errors.append(f"job {name} is gated, but the job table calls it unconditional")
        if not job.unconditional and not gated:
            errors.append(f"job {name} is not gated on the plan's job list")
        if gated and f"'{name}'" not in condition:
            errors.append(f"job {name} gates on another job's name, so the plan can never start it")
        if name not in (jobs.get(AGGREGATE, {}).get("needs") or []):
            errors.append(f"job {name} is not in the aggregate gate's needs")
    return errors

AUDITED_PATHS = (
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rustfmt.toml",
    ".cargo/config.toml",
    "specs/vectors/obcm-v2.json",
    "tools/test_plan.py",
    "builder/server/nested/handler.py",
    "fixtures/catalog.toml",
    ".github/workflows/bake.yml",
    "testing/suites.toml",
    "builder/app/src/lib/example.ts",
    "companion-ios/Packages/OBCKit/Sources/OBCFormats/example.swift",
    "apps/obc-desktop/src/main.rs",
    "docs/index.md",
)

# ──────────────────────────────────────────────────────────────── commands ──

def load(root: Path) -> tuple[CargoGraph, dict[str, Any], list[Unit]]:
    document = load_document(root)
    graph = build_cargo_graph(root)
    return graph, document, build_units(graph, document)

def command_select(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    graph, _, units = load(root)
    changed, deleted = git_changed_paths(root, args.base, args.head)
    plan = select(units, graph, changed, deleted=deleted, base=args.base, head=args.head)
    if args.release:
        plan = select_release(plan)
    data = plan_data(plan)
    if args.jobs_file:
        Path(args.jobs_file).write_text(json.dumps(data["required_jobs"]), encoding="utf-8")
    print(json.dumps(data, sort_keys=True, separators=(",", ":")) if args.format == "json" else render_text(plan))
    return 1 if plan.errors else 0

def command_run(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    if not args.base:
        raise PlanError("`affected` needs an explicit base revision: obc test affected --base origin/develop")
    graph, document, units = load(root)
    changed, deleted = git_changed_paths(root, args.base, args.head)
    if args.head == "HEAD":
        changed = sorted(set(changed) | set(working_tree_paths(root)))
    plan = select(units, graph, changed, deleted=deleted, base=args.base, head=args.head)
    return run_plan(plan, graph, root, carved_targets(document), dry_run=args.dry_run)

def command_cargo_filter(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    graph, document, _ = load(root)
    print(cargo_filter(graph, args.tier, args.packages or None, carved_targets(document)))
    return 0

def command_gates(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    if args.list:
        print(" ".join(sorted(GATES)))
        return 0
    if not args.gate:
        raise PlanError("name at least one gate, or pass --list")
    _, _, units = load(root)
    covered = {unit.id for unit in reproduced(units, args.gate)}
    if not args.unreproduced:
        print("reproduces: " + ", ".join(sorted(covered)))
        return 0
    host = host_platform()
    missing = [unit for unit in units if unit.route == "required" and unit.id not in covered]
    if not missing:
        print("this run reproduces every suite required on a pull request")
        return 0
    print("required suites this run does not reproduce:")
    for unit in missing:
        reason = (
            f"runs only on {', '.join(unit.platforms)}"
            if unit.platforms and host not in unit.platforms
            else "no gate in this run runs its work"
        )
        print(f"  - {unit.id}: {reason}")
    return 0

def command_check(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    graph, document, units = load(root)
    errors = validate(root, graph, document, units)
    if errors:
        raise PlanError("\n".join(f"- {error}" for error in sorted(set(errors))))
    declared = sum(1 for unit in units if unit.declared)
    targets = sum(len(package.ordinary_targets) + len(package.fixture_targets) for package in graph.packages.values())
    print(
        f"test plan OK: {len(graph.packages)} Cargo packages with {targets} test binaries, "
        f"{declared} declared suites, {len(JOBS)} CI jobs"
    )
    return 0

def command_validate_filters(args: argparse.Namespace) -> int:
    root = (args.root or repository_root()).resolve()
    graph, _, units = load(root)
    errors = validate_workflow(root)
    for path in AUDITED_PATHS:
        errors.extend(select(units, graph, [path]).errors)
    if errors:
        raise PlanError("\n".join(f"- {error}" for error in sorted(set(errors))))
    print(f"the job table and the workflow agree on {len(JOBS)} jobs over {len(AUDITED_PATHS)} audited paths")
    return 0

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Decide which CI jobs and suites a change requires")
    parser.add_argument("--root", type=Path, help=argparse.SUPPRESS)
    subparsers = parser.add_subparsers(dest="action", required=True)

    select_parser = subparsers.add_parser("select", help="select affected jobs and suites from a Git range")
    select_parser.add_argument("--base", required=True)
    select_parser.add_argument("--head", default="HEAD")
    select_parser.add_argument("--format", choices=("text", "json"), default="text")
    select_parser.add_argument(
        "--release", action="store_true", help="require every CI suite for a release candidate"
    )
    select_parser.add_argument("--jobs-file", help="also write the required jobs as a JSON array")
    select_parser.set_defaults(func=command_select)

    run_parser = subparsers.add_parser("run", help="run the work a Git range selects")
    run_parser.add_argument("--base")
    run_parser.add_argument("--head", default="HEAD")
    run_parser.add_argument("--dry-run", action="store_true")
    run_parser.set_defaults(func=command_run)

    filter_parser = subparsers.add_parser("cargo-filter", help="a nextest expression over whole test binaries")
    filter_parser.add_argument("--tier", choices=["fast", "fixtures"], required=True)
    filter_parser.add_argument("--packages", nargs="*", help="narrow to these packages")
    filter_parser.set_defaults(func=command_cargo_filter)

    gates_parser = subparsers.add_parser("gates", help="report what `obc check` gates reproduce")
    gates_parser.add_argument("gate", nargs="*")
    gates_parser.add_argument("--list", action="store_true")
    gates_parser.add_argument("--unreproduced", action="store_true")
    gates_parser.set_defaults(func=command_gates)

    check_parser = subparsers.add_parser("check", help="validate the plan documents and the coverage policy")
    check_parser.set_defaults(func=command_check)

    workflow_parser = subparsers.add_parser(
        "validate-filters", help="prove the job table and the workflow describe the same jobs"
    )
    workflow_parser.set_defaults(func=command_validate_filters)
    return parser

def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except PlanError as exc:
        print(f"test plan failed:\n{exc}", file=sys.stderr)
        return 1

if __name__ == "__main__":
    raise SystemExit(main())
