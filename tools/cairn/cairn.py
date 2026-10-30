#!/usr/bin/env python3
"""Check the code against the system docs.

Each doc under `docs/design/` has a header that names the files its system or shell owns and the
systems it depends on. `check` finds the violations of the ownership rule, the dependency rule and
the rules R1 to R4, and compares them with the baseline. A violation that is not in the baseline,
or that grew, fails only when it touches a system whose doc has `status: approved`. A stale map
always fails. `--base REF` also fails when the baseline grew since the merge base with REF.

Usage: python3 tools/cairn/cairn.py
         check [--base REF] [--update | --accept]
         map
         leaves SYSTEM
         model ROLE [--harness claude-code|codex]
         report [--publish] [--drift]
"""

from __future__ import annotations

GOVERNS = ['**']
RULE = 'Each source file has one owner, and each dependency between systems is declared in a system doc.'

import argparse
import importlib
import json
import os
import re
import subprocess
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CONFIG = ".cairn/config.json"
FRONT = re.compile(r"\A---\n(.*?)\n---\n", re.S)
KINDS = {"system", "shell"}
STATUSES = {"draft", "approved"}
SLUG = re.compile(r"[a-z0-9-]+")

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
    #: The dependencies between systems as (user, used, test only).
    edges: set[tuple[str, str, bool]] = field(default_factory=set)
    #: The edges that the user's doc does not declare.
    undeclared: set[tuple[str, str, bool]] = field(default_factory=set)


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
        if not SLUG.fullmatch(doc.system):
            raise CairnError(f"{doc.system}: a system name uses only a-z, 0-9 and -")
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


def cyclic(loops: list[list[str]], a: str, b: str) -> bool:
    return any(a in members and b in members for members in loops)


# ── the rules ────────────────────────────────────────────────────────────────


def crate_dirs(units: list[dict]) -> list[str]:
    return sorted((u["dir"] for u in units), key=len, reverse=True)


def crate_dir(path: str, dirs: list[str]) -> str | None:
    """The deepest crate folder that holds the file. `dirs` comes from `crate_dirs`."""
    return next((d for d in dirs if path.startswith(d + "/")), None)


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
    undeclared = set()
    for (a, b), test in sorted(pairs.items()):
        crates = ", ".join(via[(a, b)])
        if b not in by_name[a].depends_on + (by_name[a].test_depends_on if test else ()):
            undeclared.add((a, b, test))
        if not test and cyclic(loops, a, b):
            out.append(Violation("cycle", f"cycle {a} -> {b}", frozenset({a, b}), f"{a} -> {b} closes a cycle: {crates}"))
        elif not test and (a, b, test) in undeclared:
            out.append(Violation("dependency", f"dependency {a} -> {b}", frozenset({a}), f"{a} uses {b} but does not list it in depends_on: {crates}"))
        elif (a, b, test) in undeclared:
            out.append(Violation("dependency", f"test dependency {a} -> {b}", frozenset({a}), f"{a} tests use {b} but test_depends_on does not list it: {crates}"))

    # R1: a crate's files belong to the system that owns its entry file.
    dirs = crate_dirs(units)
    home = {u["dir"]: u for u in units}
    foreign: dict[tuple[str, str], int] = {}
    for path in files:
        crate = crate_dir(path, dirs)
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

    return Result(out, loops, {(a, b, test) for (a, b), test in pairs.items()}, undeclared)


# ── leaves ───────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Leaf:
    crate: str | None
    files: tuple[str, ...]
    lines: int
    #: The other systems that the crate uses, as "system (crate, crate)".
    uses: tuple[str, ...]
    test_uses: tuple[str, ...]


def entry_key(path: str, base: str, folders: set[str]) -> str:
    """The entry of the folder `base` that holds the file. A file `x.rs` joins its folder `x/`,
    because the two are one module. A file outside `base` is the module file of `base` itself."""
    if not path.startswith(base):
        return path
    head, slash, _ = path[len(base):].partition("/")
    if slash:
        return base + head + "/"
    folder = base + head.rsplit(".", 1)[0] + "/"
    return folder if folder in folders else path


def cut(paths: list[str], lines: dict[str, int], limit: int, base: str) -> list[list[str]]:
    """The files below the folder `base` in leaves of at most `limit` lines. A folder that is too
    large is cut by its entries, and small entries of one folder share a leaf. A file that is too
    large is a leaf of its own."""
    if sum(lines[p] for p in paths) <= limit:
        return [paths]
    folders = {entry_key(p, base, set()) for p in paths if p.startswith(base) and "/" in p[len(base):]}
    entries: dict[str, list[str]] = {}
    for path in paths:
        entries.setdefault(entry_key(path, base, folders), []).append(path)
    out: list[list[str]] = []
    pack: list[str] = []
    size = 0
    for key, members in sorted(entries.items()):
        count = sum(lines[p] for p in members)
        if count > limit and key in folders:
            out += cut(members, lines, limit, key)
            continue
        if pack and size + count > limit:
            out.append(pack)
            pack, size = [], 0
        pack += members
        size += count
    return out + [pack] if pack else out


def leaves(system: str, docs: list[Doc], files: list[str], units: list[dict],
           edges: list[tuple[str, str, bool]], lines: dict[str, int], limit: int) -> list[Leaf]:
    """The system's files in leaves, each inside one crate. A file in no crate is cut from the
    repository root."""
    owner = {path: hits[0] for path, hits in owners(files, docs).items() if hits}
    unit_owner = {u["name"]: owner.get(u["entry"]) for u in units}
    names = {u["dir"]: u["name"] for u in units}
    dirs = crate_dirs(units)
    crates: dict[str, list[str]] = {}
    for path in sorted(p for p in files if owner.get(p) == system):
        crates.setdefault(crate_dir(path, dirs) or "", []).append(path)
    out = []
    for folder, paths in sorted(crates.items()):
        crate = names.get(folder)
        used: dict[bool, dict[str, list[str]]] = {False: {}, True: {}}
        for source, target, test in edges:
            other = unit_owner.get(target)
            if crate and source == crate and other and other != system:
                used[test].setdefault(other, []).append(target)
        named = {test: tuple(f"{s} ({', '.join(sorted(c))})" for s, c in sorted(found.items()))
                 for test, found in used.items()}
        for group in cut(paths, lines, limit, folder + "/" if folder else ""):
            out.append(Leaf(crate, tuple(group), sum(lines[p] for p in group), named[False], named[True]))
    return out


def print_leaves(system: str, found: list[Leaf], lines: dict[str, int], limit: int) -> None:
    total = sum(leaf.lines for leaf in found)
    print(f"{system}: {total:,} lines in {len(found)} leaves; a leaf holds at most {limit:,} lines, or one larger file")
    for i, leaf in enumerate(found, 1):
        where = leaf.files[0] if len(leaf.files) == 1 else os.path.commonpath(leaf.files) + "/"
        print(f"\nleaf {i}: {where} ({leaf.lines:,} lines)")
        if leaf.crate:
            print(f"  crate: {leaf.crate}")
            print(f"  uses: {', '.join(leaf.uses) or 'no other system'}")
            print(f"  tests use: {', '.join(leaf.test_uses) or 'no other system'}")
        else:
            print("  crate: none, so no adapter measures its uses")
        for path in leaf.files:
            print(f"  {lines[path]:>6,}  {path}")


# ── the skill budget ─────────────────────────────────────────────────────────


def skill_tokens(root: Path, settings: dict) -> list[tuple[str, int, int]]:
    """Each skill file as (path, tokens, limit). Four bytes count as one token."""
    folder = root / settings["path"]
    files = [(folder / "SKILL.md", settings["router_tokens"])]
    files += [(path, settings["reference_tokens"]) for path in sorted((folder / "reference").glob("*.md"))]
    return [(path.relative_to(folder).as_posix(), path.stat().st_size // 4, limit) for path, limit in files]


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


def base_baseline(show: Callable[[str], str | None]) -> dict[str, int] | None:
    """The baseline at the base commit, or None when it has none.

    The path comes from the base commit's own config, so a change that moves the baseline cannot
    hide the old one.
    """
    config = show(CONFIG)
    text = show(json.loads(config)["baseline"]) if config is not None else None
    return json.loads(text) if text is not None else None


def approved_at(root: Path, commit: str, folder: str) -> set[str]:
    """The systems whose doc has `status: approved` at the commit."""
    listed = subprocess.run(["git", "-C", str(root), "ls-tree", "--name-only", f"{commit}:{folder}"],
                            capture_output=True, text=True).stdout.split()
    show = shown_at(root, commit)
    texts = {name.removesuffix(".md"): show(f"{folder}/{name}") or "" for name in listed if name.endswith(".md")}
    return {name for name, text in texts.items() if FRONT.match(text) and parse_header(text).get("status") == "approved"}


def merge_base(root: Path, ref: str) -> str:
    merge = subprocess.run(["git", "-C", str(root), "merge-base", ref, "HEAD"], capture_output=True, text=True)
    if merge.returncode:
        raise CairnError(f"no merge base with {ref}: {merge.stderr.strip()}")
    return merge.stdout.strip()


def shown_at(root: Path, commit: str) -> Callable[[str], str | None]:
    """A reader of files at a commit; it gives None for a missing file."""

    def show(path: str) -> str | None:
        shown = subprocess.run(["git", "-C", str(root), "show", f"{commit}:{path}"], capture_output=True, text=True)
        return shown.stdout if shown.returncode == 0 else None

    return show


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


# ── the map ──────────────────────────────────────────────────────────────────


MAP = "README.md"
MAP_BEGIN = "<!-- cairn map: generated by `obc cairn map`; edits between this comment and the end comment are lost -->"
MAP_END = "<!-- cairn map: end -->"
MAP_LEGEND = (
    "The figure leaves out the shells, and each arrow that a longer path implies. A thick arrow is part of a "
    "cycle or is not in `depends_on`. A dashed arrow is in `depends_on`, but not in the code. The table lists "
    "every direct dependency in production code."
)


def figure_edges(found: list[tuple[str, str]], keep: set[tuple[str, str]]) -> list[tuple[str, str]]:
    """The edges without those that another path implies. An edge in `keep` always stays.

    Each removal keeps the reachability of the rest, so two edges cannot each imply the other away.
    """
    graph: dict[str, set[str]] = {}
    for a, b in found:
        graph.setdefault(a, set()).add(b)

    def reaches(a: str, b: str) -> bool:
        seen, todo = {a}, [a]
        while todo:
            for nxt in graph.get(todo.pop(), ()):
                if nxt == b:
                    return True
                if nxt not in seen:
                    seen.add(nxt)
                    todo.append(nxt)
        return False

    shown = []
    for a, b in found:
        graph[a].discard(b)
        if (a, b) in keep or not reaches(a, b):
            graph[a].add(b)
            shown.append((a, b))
    return shown


def system_map(docs: list[Doc], result: Result) -> str:
    """The generated block of the map: the figure, a table for each area and the tangles."""
    found = sorted((a, b) for a, b, test in result.edges if not test)
    tangled = {(a, b) for a, b in found if (a, b, False) in result.undeclared or cyclic(result.cycles, a, b)}
    systems = {doc.system for doc in docs if doc.kind == "system"}
    missing = sorted((doc.system, b) for doc in docs if doc.system in systems
                     for b in doc.depends_on if b in systems and (doc.system, b) not in found)
    shown = figure_edges([(a, b) for a, b in found if a in systems and b in systems], tangled)
    areas = [(area, [doc for doc in docs if doc.area == area]) for area in sorted({doc.area for doc in docs} - {""})]
    shells = [doc for doc in docs if doc.kind == "shell"]

    def node(name: str) -> str:
        return "s_" + name.replace("-", "_")

    lines = [MAP_BEGIN, "", "## Dependencies", "", MAP_LEGEND, "", "```mermaid", "flowchart LR"]
    for i, (area, members) in enumerate(areas):
        lines += [f'  subgraph area{i}["{area}"]', *(f'    {node(doc.system)}["{doc.system}"]' for doc in members), "  end"]
    for a, b in sorted(shown + missing):
        arrow = "==>" if (a, b) in tangled else "-.->" if (a, b) in missing else "-->"
        lines.append(f"  {node(a)} {arrow} {node(b)}")
    lines += ["```", "", "## Systems", ""]

    uses: dict[str, list[str]] = {}
    used_by: dict[str, list[str]] = {}
    for a, b in found:
        uses.setdefault(a, []).append(b)
        used_by.setdefault(b, []).append(a)
    for area, members in areas + ([("Shells", shells)] if shells else []):
        lines += [f"### {area}", "",
                  "| System | Purpose | Status | Uses (production code) | Used by (production code) |",
                  "| --- | --- | --- | --- | --- |"]
        for doc in members:
            purpose = doc.purpose.replace("|", "\\|")
            lines.append(f"| [{doc.system}]({doc.system}.md) | {purpose} | {doc.status} | "
                         f"{', '.join(uses.get(doc.system, []))} | {', '.join(used_by.get(doc.system, []))} |")
        lines.append("")

    tangles = [f"- A cycle between {', '.join(members)}: "
               + ", ".join(f"{a} -> {b}" for a, b in found if a in members and b in members) for members in result.cycles]
    # A cycle already lists its own edges, and declaring them does not resolve it.
    tangles += [f"- {a} -> {b}{' (test)' if test else ''} is not in the doc of {a}"
                for a, b, test in sorted(result.undeclared) if test or not cyclic(result.cycles, a, b)]
    lines += ["## Tangles", "", *(tangles or ["- None"])]
    return "\n".join([*lines, "", MAP_END])


def with_map(page: str, block: str, name: str) -> str:
    """The page with its generated block replaced by `block`. `name` is the page's path."""
    head, *rest = page.split(MAP_BEGIN)
    tail = rest[0].split(MAP_END) if len(rest) == 1 else []
    if len(tail) != 2 or MAP_END in head:
        raise CairnError(f"{name}: the map needs one {MAP_BEGIN} comment, followed by one {MAP_END} comment")
    return head + block + tail[1]


# ── commands ─────────────────────────────────────────────────────────────────


HARNESSES = ("claude-code", "codex")


def harness() -> str | None:
    """Claude Code sets `CLAUDECODE=1`. Codex sets no variable on every command it runs, so it
    needs `--harness codex`."""
    return "claude-code" if os.environ.get("CLAUDECODE") == "1" else None


def model_for(config: dict, role: str, harness: str | None) -> tuple[str, str] | None:
    """The model of a role in a harness and its effort, or None when the role runs on the
    session's own model. An `api` model is a family prefix, or an exact model ID as a pin."""
    roles = config["roles"]
    if role not in roles:
        raise CairnError(f"unknown role `{role}`; the roles are {', '.join(roles)}")
    tier = roles[role]["tier"]
    models = config["tiers"].get(harness or "", {})
    return (models[tier], roles[role].get("effort", "")) if tier in models else None


def model_command(args: argparse.Namespace) -> int:
    try:
        found = model_for(json.loads((ROOT / CONFIG).read_text()), args.role, args.harness or harness())
    except (CairnError, OSError, KeyError, ValueError) as error:
        print(f"cairn: {error}", file=sys.stderr)
        return 1
    if found is None:
        print("inherit")
        return 0
    print(f"model: {found[0]}\neffort: {found[1]}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check", help="check the code against the system docs")
    check.add_argument("--base", metavar="REF", help="also fail when the baseline grew since the merge base with REF")
    write = check.add_mutually_exclusive_group()
    write.add_argument("--update", action="store_true", help="remove gone entries and lower shrunk sizes in the baseline")
    write.add_argument("--accept", action="store_true", help="write the first baseline from the current state")
    commands.add_parser("map", help="rewrite the generated block of the system map")
    leaf = commands.add_parser("leaves", help="print a system's files in leaves, with the systems that each leaf's crate uses")
    leaf.add_argument("system")
    model = commands.add_parser("model", help="print the model and effort of a role in the current harness")
    model.add_argument("role")
    model.add_argument("--harness", choices=HARNESSES, help="the harness, instead of the one the environment shows")
    weekly = commands.add_parser("report", help="print the weekly report on skipped pull requests, sizes, the ratchet and drift")
    weekly.add_argument("--publish", action="store_true", help="also write it to the open issue labelled cairn-report")
    weekly.add_argument("--drift", action="store_true", help="run step 2, which spends tokens of the Anthropic API")
    args = parser.parse_args(argv)

    if args.command == "model":
        return model_command(args)
    if args.command == "report":
        # Not `import report`: that name is the ratchet report below.
        from report import main as weekly

        return weekly(args.publish, args.drift)

    try:
        config = json.loads((ROOT / CONFIG).read_text())
        docs, files, units, edges, lines = gather(ROOT, config)
        if args.command == "leaves":
            if args.system not in {doc.system for doc in docs}:
                raise CairnError(f"no system doc for `{args.system}`")
            found = leaves(args.system, docs, files, units, edges, lines, config["leaf_lines"])
            print_leaves(args.system, found, lines, config["leaf_lines"])
            return 0
        result = analyse(docs, files, units, edges, lines, config)
        page = ROOT / config["docs"] / MAP
        drawn = page.read_text(encoding="utf-8")
        current = with_map(drawn, system_map(docs, result), str(page.relative_to(ROOT)))
        if args.command == "map":
            page.write_text(current, encoding="utf-8")
            print(f"cairn: wrote {page.relative_to(ROOT)}")
            return 0
        commit = merge_base(ROOT, args.base) if args.base else None
        base = base_baseline(shown_at(ROOT, commit)) if commit else None
        # An approval at the base still holds, so a change cannot lift it to hide its own violation.
        was_approved = approved_at(ROOT, commit, config["docs"].rstrip("/")) if commit else set()
        skill = skill_tokens(ROOT, config["skill"]) if "skill" in config else []
    except (CairnError, OSError, RuntimeError, KeyError, ValueError, subprocess.CalledProcessError) as error:
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

    approved = {doc.system for doc in docs if doc.status == "approved"} | was_approved
    failing = report(result, baseline, docs, approved)
    # The ratchet protects itself while every system is still a draft.
    added = grown(baseline, base, result.violations) if args.base else []
    if added:
        print(f"\ncairn: the baseline grew since the merge base with {args.base}; only `--update` changes it:")
        for line in added:
            print(f"  {line}")
    over = [name for name, tokens, limit in skill if tokens > limit]
    if skill:
        print(f"\ncairn: skill tokens (bytes / 4) in {config['skill']['path']}:")
        for name, tokens, limit in skill:
            print(f"  {'FAIL' if name in over else '    '} {name}: {tokens:,} of {limit:,}")
    if current != drawn:
        print(f"\ncairn: {page.relative_to(ROOT)} is out of date; run `obc cairn map` or `python3 tools/cairn/cairn.py map`")
    return 1 if failing or added or over or current != drawn else 0


if __name__ == "__main__":
    sys.exit(main())
