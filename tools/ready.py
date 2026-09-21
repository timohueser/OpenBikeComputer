#!/usr/bin/env python3
"""`obc ready` — the gates a change selects, before a push.

The rule table below maps the changed paths to gates. Every gate prints `run` or `skip` with one
reason, so the plan shows what it leaves out as plainly as what it does. That is the budget rule:
a pre-flight that runs everything is not a pre-flight.

`obc test affected` executes the declared suites of `testing/suites.toml`. A gate whose work is
one of those suites is therefore skipped, and the line names the suite that does it. Nothing runs
twice — the snapshot sweep least of all.

A foundation input or the test policy selects the graph as a whole. That run is CI's, so the plan
does not repeat it here: it keeps the selected suites that build nothing, gives each one a line,
and names every other suite as left to CI. A check that costs a fraction of a second then stays
visible instead of hiding behind an expensive one.

The format gate writes. When it rewrites a file, the tree is no longer the tree that was about to
be pushed, so the command names the files and stops instead of printing the skeleton.

The gates run in the order below: the static checks first, the compiling and rendering gates last.
"""

from __future__ import annotations

import argparse
import shlex
import subprocess
import sys
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Iterable, Mapping, Sequence

sys.path.insert(0, str(Path(__file__).resolve().parent))

import docs_copy
import docs_review
import test_plan

#: Cargo roots beside the root workspace. Each one formats and lints through its own manifest.
STANDALONE_ROOTS = tuple(root for root in test_plan.PRODUCT_ROOTS if root != test_plan.ROOT_WORKSPACE)

#: Paths that make `obc suites check` necessary, beside the test policy `test_plan` already names.
TEST_SOURCE_PATTERNS = (
    "**/tests/**",
    "**/test_*.py",
    "**/*_test.py",
    "**/*Tests.swift",
    "**/*.test.ts",
    "firmware/ui-frames.toml",
    "firmware/ui-snapshots.sha256",
)

#: The documentation surface each documentation gate reads.
DOCS = "docs/"
DOCS_CONTENT = "docs/content/"

#: The declared suite that owns the snapshot sweep. Its triggers define the rendering inputs.
SWEEP = "ci.ui-snapshots"

#: Executables that run a check instead of a build. A script can still start a compiler probe,
#: but it does not build the repository, so it belongs in a pre-flight. Any other executable may
#: drive a build behind a wrapper, so a command this table does not name is CI's work.
FREE_EXECUTABLES = {"cd", "mkdir", "python", "python3", "rm"}

#: The words that separate one command from the next inside a suite command.
SEPARATORS = {"&", "&&", "||", ";", "|"}


@dataclass(frozen=True)
class Gate:
    """One command, and why the change does or does not select it."""

    command: str
    reason: str
    run: bool
    #: The declared suite whose command does this same work, if there is one.
    covered_by: str = ""
    #: The gate rewrites files, so the tree is compared before and after it.
    writes: bool = False


def cargo_root(path: str) -> str:
    """The Cargo root that owns a path."""

    for root in STANDALONE_ROOTS:
        if path.startswith(f"{root}/"):
            return root
    return test_plan.ROOT_WORKSPACE


def owns(package: test_plan.Package, path: str) -> bool:
    """Whether a Cargo package contains a path, by the same rule the test plan uses."""

    prefix = "" if package.root == test_plan.ROOT_WORKSPACE else f"{package.root}/"
    return path == package.manifest or (bool(prefix) and path.startswith(prefix))


def _format_gates(changed: Sequence[str]) -> list[Gate]:
    rust = sorted(path for path in changed if path.endswith(".rs"))
    if not rust:
        return [Gate("cargo fmt --all", "no Rust source changed", False)]
    gates = []
    for root in sorted({cargo_root(path) for path in rust}):
        first = next(path for path in rust if cargo_root(path) == root)
        command = (
            "cargo fmt --all"
            if root == test_plan.ROOT_WORKSPACE
            else f"cargo fmt --manifest-path {root}/Cargo.toml"
        )
        gates.append(Gate(command, f"Rust changed in {root}: {first}", True, writes=True))
    return gates


def _clippy_gates(changed: Sequence[str], packages: Mapping[str, test_plan.Package]) -> list[Gate]:
    gates = []
    for name, package in sorted(packages.items()):
        hit = next((path for path in sorted(changed) if owns(package, path)), "")
        if not hit:
            continue
        scope = (
            f"-p {name}"
            if package.product_root == test_plan.ROOT_WORKSPACE
            else f"--manifest-path {package.product_root}/Cargo.toml"
        )
        gates.append(
            Gate(
                f"cargo clippy {scope} --all-targets -- -D warnings",
                f"package {name} changed: {hit}",
                True,
                covered_by="ci.rust-clippy",
            )
        )
    if not gates:
        return [Gate("cargo clippy", "no Cargo package changed", False)]
    return gates


def _first_match(changed: Iterable[str], predicate) -> str:
    return next((path for path in sorted(changed) if predicate(path)), "")


def builds_nothing(command: str) -> bool:
    """Whether a command runs a check instead of a build.

    The executables decide it, not the suite name: an interpreter reads a script, and a wrapper
    may start a build, so every executable the command names must be a known free one.
    """

    # `punctuation_chars` makes the lexer cut a separator out of the word it is glued to, so
    # `a.py; cargo build` reports both executables and not one.
    lexer = shlex.shlex(command.replace("\n", " ; "), posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    lexer.commenters = ""
    try:
        words = list(lexer)
    except ValueError:
        return False
    executables, expect = [], True
    for word in words:
        if word in SEPARATORS:
            expect = True
        elif expect and "=" in word and not word.startswith(("./", "/")):
            continue  # an environment prefix, not the executable
        elif expect:
            executables.append(word)
            expect = False
    return bool(executables) and all(word in FREE_EXECUTABLES for word in executables)


def _suite_gate(unit: test_plan.Unit) -> Gate:
    """The line one selected suite gets when the affected run is left to CI."""

    if unit.platforms and test_plan.host_platform() not in unit.platforms:
        return Gate(unit.command, f"{unit.id} runs only on {', '.join(unit.platforms)}", False)
    if builds_nothing(unit.command):
        return Gate(unit.command, f"{unit.id} builds nothing", True)
    return Gate(unit.command, f"{unit.id} is left to CI", False)


def plan(
    changed: Sequence[str],
    *,
    base: str,
    packages: Mapping[str, test_plan.Package],
    selected: Sequence[test_plan.Unit],
    rendering: Sequence[str] = (),
) -> list[Gate]:
    """The gates the changed paths select, in the order they run.

    `selected` holds the units the test plan selects. It decides the `obc test affected` gate,
    through `covered_by` which gates that run would repeat, and, when the selection took the
    graph as a whole, which suites run here and which are left to CI. `rendering` holds the
    snapshot sweep's declared triggers, so the rendering inputs have one definition.
    """

    policy = _first_match(
        changed,
        lambda path: test_plan.is_policy_path(path)
        or any(test_plan.glob_matches(path, pattern) for pattern in TEST_SOURCE_PATTERNS),
    )
    docs = _first_match(changed, lambda path: path.startswith(DOCS))
    content = _first_match(changed, lambda path: path.startswith(DOCS_CONTENT))
    manifest = _first_match(changed, lambda path: Path(path).name in {"Cargo.toml", "Cargo.lock"})
    frame = _first_match(
        changed, lambda path: any(test_plan.glob_matches(path, trigger) for trigger in rendering)
    )

    gates = _format_gates(changed)
    gates.append(
        Gate(
            "obc suites check",
            f"test policy or test source changed: {policy}" if policy else "no test source or test policy changed",
            bool(policy),
        )
    )
    gates.append(
        Gate(
            "python3 docs/build_docs.py --check-links",
            f"documentation changed: {docs}" if docs else f"nothing under {DOCS} changed",
            bool(docs),
            covered_by="ci.docs",
        )
    )
    gates.append(
        Gate(
            "obc docs check",
            f"a public page changed: {content}" if content else f"no page under {DOCS_CONTENT} changed",
            bool(content),
        )
    )
    # The licence script is called directly, not through `obc licenses --check`: the task adds
    # no setup, and this is the command `ci.licenses` declares.
    gates.append(
        Gate(
            "tools/licenses/gen-third-party.sh --check",
            f"a Cargo manifest changed: {manifest}" if manifest else "no Cargo manifest changed",
            bool(manifest),
            covered_by="ci.licenses",
        )
    )
    gates.extend(_clippy_gates(changed, packages))
    whole = test_plan.wholesale_reason(selected)
    gates.append(
        Gate(
            f"obc test affected --base {base}",
            f"the plan selects {len(selected)} unit(s), and {whole} — that run is CI's"
            if whole
            else f"the plan selects {len(selected)} unit(s)"
            if selected
            else "the plan selects no unit",
            bool(selected) and not whole,
        )
    )
    gates.append(
        Gate(
            "obc shot --check",
            f"a rendering, screen or i18n input changed: {frame}"
            if frame
            else "no rendering, screen or i18n input changed",
            bool(frame),
            covered_by=SWEEP,
        )
    )

    if not selected:
        return gates
    identifiers = {unit.id for unit in selected}
    if not whole:
        return [
            replace(gate, run=False, reason=f"obc test affected runs it as {gate.covered_by}")
            if gate.run and gate.covered_by in identifiers
            else gate
            for gate in gates
        ]
    # The affected run is CI's, so this plan keeps only what builds nothing. A gate that runs
    # repeats a selected suite, so it speaks for that suite and the suite gets no second line.
    # A gate its own rule already skipped speaks for nothing, and the suite keeps its line.
    spoken, kept = set(), []
    for gate in gates:
        if gate.run and gate.covered_by in identifiers:
            spoken.add(gate.covered_by)
            gate = (
                replace(gate, reason=f"{gate.reason}, and it runs {gate.covered_by}")
                if builds_nothing(gate.command)
                else replace(gate, run=False, reason=f"{gate.covered_by} is left to CI")
            )
        kept.append(gate)
    return kept + [
        _suite_gate(unit) for unit in selected if unit.command and unit.id not in spoken
    ]


def render(gates: Sequence[Gate], changed: Sequence[str], base: str) -> str:
    lines = [f"obc ready: base {base}, {len(changed)} changed path(s)", ""]
    for gate in gates:
        lines.append(f"{'run ' if gate.run else 'skip'}  {gate.command}")
        lines.append(f"        {gate.reason}")
    running = sum(gate.run for gate in gates)
    lines.append("")
    lines.append(f"{running} gate(s) to run, {len(gates) - running} skipped")
    return "\n".join(lines)


def git_status(root: Path) -> dict[str, str]:
    """Each path Git reports as changed, mapped to its status code."""

    result = subprocess.run(
        ["git", "status", "--porcelain"], cwd=root, check=True, capture_output=True, text=True
    )
    return {line[3:]: line[:2] for line in result.stdout.splitlines() if line[3:]}


def rewritten(before: Mapping[str, str], after: Mapping[str, str]) -> list[str]:
    return sorted(path for path, state in after.items() if before.get(path) != state)


def run_gates(gates: Sequence[Gate], root: Path, status=git_status) -> tuple[int, list[str]]:
    """Run the selected gates, and report the files the writing gates rewrote."""

    writing = [index for index, gate in enumerate(gates) if gate.run and gate.writes]
    before = status(root) if writing else {}
    changed: list[str] = []
    for index, gate in enumerate(gates):
        if not gate.run:
            continue
        print(f"\nrunning  {gate.command}", flush=True)
        if subprocess.run(["bash", "-c", gate.command], cwd=root).returncode != 0:
            print(f"\nfailed   {gate.command}", file=sys.stderr)
            print("repeat it alone with:", file=sys.stderr)
            print(f"  {gate.command}", file=sys.stderr)
            return 1, []
        if writing and index == writing[-1]:
            changed = rewritten(before, status(root))
    return 0, changed


def surfaces(changed: Iterable[str]) -> list[str]:
    return sorted({path.split("/")[0] if "/" in path else "(repository root)" for path in changed})


def human_pages(root: Path, changed: Iterable[str]) -> list[str]:
    """Public pages with human-owned prose that cite a changed source."""

    content = root / DOCS_CONTENT
    pages = [path.relative_to(root).as_posix() for path in sorted(content.rglob("*.md"))]
    queue = docs_review.review_queue(root, set(changed), pages)
    stale = []
    for name in sorted(queue):
        path = root / name
        if not path.is_relative_to(content):
            continue
        page, _errors = docs_copy.parse_page(path, content)
        if page is not None and (page.copy == "human" or page.human_blocks):
            stale.append(name)
    return stale


def skeleton(root: Path, changed: Sequence[str]) -> str:
    lines = [
        "",
        "pull request skeleton",
        "",
        f"Surfaces: {', '.join(surfaces(changed)) or 'none'}",
        "Requirements: <SYS-nnn, or `none`>",
    ]
    for name in human_pages(root, changed):
        lines.append(f"Copy: {name} owns human prose and cites a changed source; it may be stale.")
    return "\n".join(lines)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run the gates a change selects")
    parser.add_argument("--base", default="origin/develop")
    parser.add_argument("--dry-run", action="store_true", help="print the plan and stop")
    args = parser.parse_args(argv)

    root = test_plan.repository_root().resolve()
    try:
        graph, _document, units = test_plan.load(root)
        committed, deleted = test_plan.git_changed_paths(root, args.base, "HEAD")
        changed = sorted(set(committed) | set(test_plan.working_tree_paths(root)))
        selection = test_plan.select(units, graph, changed, deleted=deleted, base=args.base)
    except test_plan.PlanError as exc:
        print(f"obc ready failed:\n{exc}", file=sys.stderr)
        return 1

    # An unowned path selects nothing, so the plan would otherwise report a quiet all-clear.
    for error in selection.errors:
        print(f"selection error: {error}", file=sys.stderr)
    if selection.errors:
        return 1

    sweep = next((unit for unit in units if unit.id == SWEEP), None)
    gates = plan(
        changed,
        base=args.base,
        packages=graph.packages,
        selected=selection.selected,
        rendering=sweep.triggers if sweep else (),
    )
    print(render(gates, changed, args.base))
    if args.dry_run:
        print("\ndry run: nothing was executed")
        return 0
    code, formatted = run_gates(gates, root)
    if code:
        return code
    if formatted:
        # The gates passed, but the tree is no longer the tree that was about to be pushed.
        print("\nthe format gate rewrote:", file=sys.stderr)
        for path in formatted:
            print(f"  {path}", file=sys.stderr)
        print("commit these files, then run obc ready again.", file=sys.stderr)
        return 1
    print(skeleton(root, changed))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
