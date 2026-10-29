#!/usr/bin/env python3
"""Check the code against the system docs.

Each doc under `docs/design/` has a header that names the files its system or shell owns and the
systems it depends on. `check` finds the violations of the ownership rule, the dependency rule and
the rules R1 to R4, and compares them with the baseline. A violation that is not in the baseline,
or that grew, fails only when it touches a system whose doc has `status: approved`.

`--base REF` also fails when the baseline grew since the merge base with REF.

Usage: python3 tools/cairn/cairn.py check [--base REF] [--update | --accept]
"""

from __future__ import annotations

GOVERNS = ['**']
RULE = 'Each source file has one owner, and each dependency between systems is declared in a system doc.'

import argparse
import importlib
import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CONFIG = ".cairn/config.json"
FRONT = re.compile(r"\A---\n(.*?)\n---\n", re.S)
KINDS = {"system", "shell"}
STATUSES = {"draft", "approved"}

# The adapters are plain modules beside this file. The glob rule is the test plan's own.
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / "tools"))
from test_plan import glob_matches  # noqa: E402


class CairnError(RuntimeError):
    pass


@dataclass(frozen=True)
class Doc:
    system: str
    kind: str
    area: str
    purpose: str
    owns: tuple[str, ...]
    depends_on: tuple[str, ...] = ()
    test_depends_on: tuple[str, ...] = ()
    status: str = "draft"


@dataclass(frozen=True)
class Violation:
    rule: str
    id: str
    systems: frozenset[str]
    message: str
    # R1 and R4 count files, R3 counts lines. A size above the baseline is a new violation.
    size: int = 1


@dataclass
class Result:
    violations: list[Violation]
    cycles: list[list[str]] = field(default_factory=list)


# ── system docs ──────────────────────────────────────────────────────────────


def scalar(text: str) -> str:
    text = text.strip()
    return text[1:-1] if len(text) > 1 and text[0] == text[-1] and text[0] in "\"'" else text


def parse_header(text: str) -> dict[str, object]:
    """Read the YAML subset a header uses: scalars, `[a, b]` lists and `- item` lists."""
    match = FRONT.match(text)
    if not match:
        raise CairnError("no header between two `---` lines")
    header: dict[str, object] = {}
    key = ""
    for line in match.group(1).splitlines():
        if not line.strip():
            continue
        if line.lstrip().startswith("- "):
            if not isinstance(header.get(key), list):
                raise CairnError(f"list item outside a list: {line.strip()}")
            header[key].append(scalar(line.lstrip()[2:]))
            continue
        key, colon, value = line.partition(":")
        key, value = key.strip(), value.strip()
        if not colon:
            raise CairnError(f"not a `key: value` line: {line}")
        if not value:
            header[key] = []
        elif value.startswith("[") and value.endswith("]"):
            header[key] = [scalar(v) for v in value[1:-1].split(",") if v.strip()]
        else:
            header[key] = scalar(value)
    return header


def load_docs(root: Path, folder: str) -> list[Doc]:
    docs = []
    for path in sorted((root / folder).glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if not text.startswith("---\n"):
            continue
        try:
            header = parse_header(text)
            doc = Doc(
                system=str(header["system"]),
                kind=str(header["kind"]),
                area=str(header.get("area", "")),
                purpose=str(header["purpose"]),
                owns=tuple(header["owns"]),
                depends_on=tuple(header.get("depends_on", ())),
                test_depends_on=tuple(header.get("test_depends_on", ())),
                status=str(header.get("status", "draft")),
            )
        except (CairnError, KeyError, TypeError) as error:
            raise CairnError(f"{path.relative_to(root)}: {error}") from error
        if doc.system != path.stem:
            raise CairnError(f"{path.relative_to(root)}: `system: {doc.system}` must equal the file name")
        docs.append(doc)
    validate(docs)
    return docs


def validate(docs: list[Doc]) -> None:
    names = {doc.system for doc in docs}
    for doc in docs:
        if doc.kind not in KINDS:
            raise CairnError(f"{doc.system}: kind must be one of {sorted(KINDS)}")
        if bool(doc.area) != (doc.kind == "system"):
            raise CairnError(f"{doc.system}: a system needs an area, and a shell has none")
        if doc.status not in STATUSES:
            raise CairnError(f"{doc.system}: status must be one of {sorted(STATUSES)}")
        for other in doc.depends_on + doc.test_depends_on:
            if other not in names or other == doc.system:
                raise CairnError(f"{doc.system}: depends on an unknown system `{other}`")


# ── ownership ────────────────────────────────────────────────────────────────


def owners(files: list[str], docs: list[Doc]) -> dict[str, list[str]]:
    """Each file's owners. A system beats a shell: a shell owns only what no system claims."""
    found = {}
    for path in files:
        hits = [doc for doc in docs if any(glob_matches(path, glob) for glob in doc.owns)]
        systems = [doc for doc in hits if doc.kind == "system"]
        found[path] = [doc.system for doc in systems or hits]
    return found


# ── dependencies ─────────────────────────────────────────────────────────────


def cycles(graph: dict[str, set[str]]) -> list[list[str]]:
    """The strongly connected components with more than one member (Tarjan)."""
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    stack: list[str] = []
    found: list[list[str]] = []

    def visit(node: str) -> None:
        index[node] = low[node] = len(index)
        stack.append(node)
        for nxt in sorted(graph.get(node, ())):
            if nxt not in index:
                visit(nxt)
                low[node] = min(low[node], low[nxt])
            elif nxt in stack:
                low[node] = min(low[node], index[nxt])
        if low[node] == index[node]:
            component = []
            while True:
                member = stack.pop()
                component.append(member)
                if member == node:
                    break
            if len(component) > 1:
                found.append(sorted(component))

    for node in sorted(graph):
        if node not in index:
            visit(node)
    return sorted(found)


# ── the rules ────────────────────────────────────────────────────────────────


def runtime_root(path: str, runtimes: dict[str, str]) -> str | None:
    matches = [root for root in runtimes if path.startswith(root)]
    return max(matches, key=len) if matches else None


def analyse(
    docs: list[Doc],
    files: list[str],
    units: list[dict],
    edges: list[tuple[str, str, bool]],
    lines: dict[str, int],
    config: dict,
) -> Result:
    by_name = {doc.system: doc for doc in docs}
    found = owners(files, docs)
    owner = {path: hits[0] for path, hits in found.items() if hits}
    out: list[Violation] = []

    for path, hits in sorted(found.items()):
        if not hits:
            out.append(Violation("ownership", f"unowned {path}", frozenset(), f"{path} has no owner"))
        elif len(hits) > 1:
            out.append(Violation("ownership", f"owned twice {path}", frozenset(hits), f"{path} is owned by {', '.join(hits)}"))

    # Dependencies: a crate edge counts for the systems that own the two crates' entry files.
    unit_owner = {u["name"]: owner.get(u["entry"]) for u in units}
    pairs: dict[tuple[str, str], bool] = {}
    via: dict[tuple[str, str], list[str]] = {}
    for source, target, test in edges:
        a, b = unit_owner.get(source), unit_owner.get(target)
        if a and b and a != b:
            pairs[(a, b)] = pairs.get((a, b), True) and test
            via.setdefault((a, b), []).append(f"{source} -> {target}" + (" (test)" if test else ""))
    graph: dict[str, set[str]] = {}
    for (a, b), test in pairs.items():
        if not test:
            graph.setdefault(a, set()).add(b)
    loops = cycles(graph)
    component = {member: i for i, members in enumerate(loops) for member in members}
    for (a, b), test in sorted(pairs.items()):
        crates = ", ".join(via[(a, b)])
        if not test and a in component and component[a] == component.get(b):
            out.append(Violation("cycle", f"cycle {a} -> {b}", frozenset({a, b}), f"{a} -> {b} closes a cycle: {crates}"))
        elif not test and b not in by_name[a].depends_on:
            out.append(Violation("dependency", f"dependency {a} -> {b}", frozenset({a}), f"{a} uses {b} but does not list it in depends_on: {crates}"))
        elif test and b not in by_name[a].depends_on + by_name[a].test_depends_on:
            out.append(Violation("dependency", f"test dependency {a} -> {b}", frozenset({a}), f"{a} tests use {b} but test_depends_on does not list it: {crates}"))

    # R1: a crate's files belong to the system that owns its entry file. A file belongs to the
    # deepest crate folder that holds it.
    dirs = sorted((u["dir"] for u in units), key=len, reverse=True)
    home = {u["dir"]: u for u in units}
    foreign: dict[tuple[str, str], int] = {}
    for path in files:
        crate = next((d for d in dirs if path.startswith(d + "/")), None)
        if crate is None or path not in owner:
            continue
        unit = home[crate]
        base = owner.get(unit["entry"])
        if base and owner[path] != base:
            key = (unit["name"], owner[path])
            foreign[key] = foreign.get(key, 0) + 1
    for (crate, other), count in sorted(foreign.items()):
        base = unit_owner[crate]
        out.append(Violation("R1", f"R1 {crate} {other}", frozenset({base, other}), f"crate {crate} ({base}) holds {count} file(s) of {other}", count))

    # R2: one root for each place that a system's code runs.
    runtimes = config["runtimes"]
    roots: dict[tuple[str, str], set[str]] = {}
    for path, system in owner.items():
        root = runtime_root(path, runtimes)
        if root and by_name[system].kind == "system":
            roots.setdefault((system, runtimes[root]), set()).add(root)
    for (system, runtime), found_roots in sorted(roots.items()):
        if len(found_roots) > 1:
            listed = ", ".join(sorted(found_roots))
            for root in sorted(found_roots):
                out.append(Violation("R2", f"R2 {system} {runtime} {root}", frozenset({system}), f"{system} has {runtime} code in {listed}"))

    # R3: a shell's own lines, which are the files that no system owns. The baseline records each
    # count, and a count may not grow.
    for doc in docs:
        if doc.kind == "shell":
            count = sum(lines.get(path, 0) for path, system in owner.items() if system == doc.system)
            if count:
                out.append(Violation("R3", f"R3 {doc.system}", frozenset({doc.system}), f"shell {doc.system} holds {count:,} lines of its own", count))

    # R4: only the tooling area keeps files in the tooling folder.
    folder, area = config["tooling"]["path"], config["tooling"]["area"]
    counts: dict[str, int] = {}
    for path, system in owner.items():
        if path.startswith(folder) and by_name[system].area != area:
            counts[system] = counts.get(system, 0) + 1
    for system, count in sorted(counts.items()):
        out.append(Violation("R4", f"R4 {system}", frozenset({system}), f"{system} ({by_name[system].area}) has {count} file(s) under {folder}", count))

    return Result(out, loops)


# ── the ratchet ──────────────────────────────────────────────────────────────


def new_violations(violations: list[Violation], baseline: dict[str, int]) -> list[Violation]:
    return [v for v in violations if v.size > baseline.get(v.id, 0)]


def fails(systems: frozenset[str], approved: set[str]) -> bool:
    """A change fails when it touches an approved system. An unowned file touches none, so it
    fails as soon as any system is approved."""
    return bool(systems & approved) if systems else bool(approved)


def touched(key: str, violations: list[Violation], names: set[str]) -> frozenset[str]:
    """The systems of a baseline entry: those of its violation, or the system names in its ID
    when the violation is gone."""
    found = next((v.systems for v in violations if v.id == key), None)
    return found if found is not None else frozenset(set(key.split()) & names)


def updated(violations: list[Violation], baseline: dict[str, int]) -> dict[str, int]:
    """Drop the entries that disappeared and lower the sizes that shrank. Never add or raise one."""
    sizes = {v.id: v.size for v in violations}
    return {key: min(size, sizes[key]) for key, size in sorted(baseline.items()) if key in sizes}


def grown(head: dict[str, int], base: dict[str, int] | None, violations: list[Violation]) -> list[str]:
    """The baseline entries that a change added or raised by hand.

    Only `--update` changes an existing baseline, and it only removes and lowers. A base with no
    baseline allows exactly the first baseline that `--accept` writes.
    """
    if base is None:
        base = {v.id: v.size for v in violations}
    return [f"{key}: {size}" for key, size in sorted(head.items()) if size > base.get(key, 0)]


# ── input ────────────────────────────────────────────────────────────────────


def tracked_sources(root: Path, sources: dict) -> list[str]:
    out = subprocess.run(["git", "-C", str(root), "ls-files"], capture_output=True, text=True, check=True).stdout
    extensions = tuple(sources["extensions"])
    return [f for f in out.splitlines() if (f.endswith(extensions) or f in sources["files"]) and (root / f).is_file()]


def base_baseline(root: Path, ref: str, path: str) -> dict[str, int] | None:
    """The baseline at the merge base of REF and HEAD, or None when it has none."""
    git = ["git", "-C", str(root)]
    merge = subprocess.run([*git, "merge-base", ref, "HEAD"], capture_output=True, text=True)
    if merge.returncode:
        raise CairnError(f"no merge base with {ref}: {merge.stderr.strip()}")
    shown = subprocess.run([*git, "show", f"{merge.stdout.strip()}:{path}"], capture_output=True, text=True)
    return json.loads(shown.stdout) if shown.returncode == 0 else None


def line_count(path: Path) -> int:
    with path.open("rb") as handle:
        return sum(1 for _ in handle)


def gather(root: Path, config: dict) -> tuple[list[Doc], list[str], list[dict], list[tuple[str, str, bool]], dict[str, int]]:
    docs = load_docs(root, config["docs"])
    files = tracked_sources(root, config["sources"])
    units: list[dict] = []
    edges: list[tuple[str, str, bool]] = []
    for name, settings in config["adapters"].items():
        adapter = importlib.import_module(f"adapters.{name}")
        found_units, found_edges = adapter.load(root, settings)
        units += found_units
        edges += found_edges
    lines = {path: line_count(root / path) for path in files}
    return docs, files, units, edges, lines


# ── output ───────────────────────────────────────────────────────────────────


RULES = ("ownership", "dependency", "cycle", "R1", "R2", "R3", "R4")


def report(result: Result, baseline: dict[str, int], docs: list[Doc], approved: set[str]) -> list[str]:
    """Print the report and return what fails.

    A new or grown violation fails when it touches an approved system. So does a baseline entry
    that shrank or disappeared: the baseline must follow it, or the violation could grow back
    unseen. For a draft system, the entry is only a hint to run `--update`.
    """
    names = {doc.system for doc in docs}
    fresh = new_violations(result.violations, baseline)
    failing = [v.message for v in fresh if fails(v.systems, approved)]
    sizes = {v.id: v.size for v in result.violations}
    shrunk = [key for key, size in sorted(baseline.items()) if sizes.get(key, 0) < size]
    stale = [key for key in shrunk if fails(touched(key, result.violations, names), approved)]
    systems = sum(doc.kind == "system" for doc in docs)
    print(f"cairn: {systems} systems, {len(docs) - systems} shells, {len(approved)} approved")
    for rule in RULES:
        found = [v for v in result.violations if v.rule == rule]
        new = sum(v in fresh for v in found)
        print(f"  {rule:<11} {len(found):>4} violation(s), {new} new")
    for members in result.cycles:
        print(f"  cycle between {', '.join(members)}")
    if fresh:
        print("\nnew or grown since the baseline:")
        for v in fresh:
            print(f"  {'FAIL' if v.message in failing else '    '} {v.message}")
    if shrunk:
        print("\nshrunk or gone; `obc cairn check --update` records them in the baseline:")
        for key in shrunk:
            print(f"  {'FAIL' if key in stale else '    '} {key}: {baseline[key]} -> {sizes.get(key, 0)}")
    print(f"\ncairn: {len(result.violations)} violation(s), baseline {len(baseline)}, "
          f"{len(fresh)} new, {len(shrunk)} shrunk, {len(failing) + len(stale)} fail")
    return failing + stale


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check", help="check the code against the system docs")
    check.add_argument("--base", metavar="REF", help="also fail when the baseline grew since the merge base with REF")
    mode = check.add_mutually_exclusive_group()
    mode.add_argument("--update", action="store_true", help="remove gone entries and lower shrunk sizes in the baseline")
    mode.add_argument("--accept", action="store_true", help="write the first baseline from the current state")
    args = parser.parse_args(argv)

    try:
        config = json.loads((ROOT / CONFIG).read_text())
        docs, files, units, edges, lines = gather(ROOT, config)
        result = analyse(docs, files, units, edges, lines, config)
        base = base_baseline(ROOT, args.base, config["baseline"]) if args.base else None
    except (CairnError, OSError, RuntimeError, KeyError, ValueError) as error:
        print(f"cairn: {error}", file=sys.stderr)
        return 1

    path = ROOT / config["baseline"]
    baseline = json.loads(path.read_text()) if path.exists() else {}
    if args.accept:
        if path.exists():
            print(f"cairn: {config['baseline']} exists; --accept writes only the first baseline", file=sys.stderr)
            return 1
        baseline = {v.id: v.size for v in sorted(result.violations, key=lambda v: v.id)}
    elif args.update:
        baseline = updated(result.violations, baseline)
    if args.accept or args.update:
        path.write_text(json.dumps(baseline, indent=2) + "\n")
        print(f"cairn: baseline holds {len(baseline)} entries")

    approved = {doc.system for doc in docs if doc.status == "approved"}
    failing = report(result, baseline, docs, approved)
    # The ratchet protects itself while every system is still a draft.
    added = grown(baseline, base, result.violations) if args.base else []
    if added:
        print(f"\ncairn: the baseline grew since the merge base with {args.base}; only `--update` changes it:")
        for line in added:
            print(f"  {line}")
    return 1 if failing or added else 0


if __name__ == "__main__":
    sys.exit(main())
