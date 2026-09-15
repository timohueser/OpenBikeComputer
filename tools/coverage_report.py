#!/usr/bin/env python3
"""Summarize native coverage by production component; ratchet accepted Rust baselines."""
from __future__ import annotations

import argparse
from fractions import Fraction
import fnmatch
import json
from pathlib import Path
import subprocess
import sys
import tomllib

SCOPES = {"rust": {".rs"}, "python": {".py"}, "web": {".ts", ".js", ".svelte"}, "ios": {".swift"}, "desktop": {".rs"}}
CRITICAL = {"format-protocol-codecs", "crc", "storage", "dfu", "boot"}


def matches(path: str, patterns: list[str]) -> bool:
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)


def relative(path: str, root: Path, source_root: Path, source_prefix: Path | None = None) -> str | None:
    candidate = Path(path)
    if candidate.is_absolute() and source_prefix is not None:
        try:
            candidate = root / candidate.relative_to(source_prefix)
        except ValueError:
            pass
    if not candidate.is_absolute():
        candidate = source_root / candidate
    try:
        return candidate.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return None


def read_lcov(path: Path, root: Path, source_root: Path, source_prefix: Path | None = None) -> dict[str, dict[int, bool]]:
    files: dict[str, dict[int, bool]] = {}
    current = None
    for line in path.read_text().splitlines():
        if line.startswith("SF:"):
            current = relative(line[3:], root, source_root, source_prefix)
            if current is not None:
                files.setdefault(current, {})
        elif line.startswith("DA:") and current is not None:
            number, hits, *_ = line[3:].split(",")
            if int(number) < 1 or int(hits) < 0:
                raise ValueError(f"{path}: invalid line coverage {line}")
            bucket = files[current]
            bucket[int(number)] = bucket.get(int(number), False) or int(hits) > 0
        elif line == "end_of_record":
            current = None
    if not files:
        raise ValueError(f"{path}: no repository files in native LCOV")
    return files


def rust_source(path: Path) -> tuple[set[int], bool, list[Path]]:
    """Use Rust syntax nodes, never brace counting, to exclude test-only items."""
    from tree_sitter import Language, Parser
    import tree_sitter_rust

    source = path.read_bytes()
    language = Language(tree_sitter_rust.language())
    parser = Parser(language)
    tree = parser.parse(source)
    if tree.root_node.has_error:
        raise ValueError(f"cannot classify Rust source with syntax errors: {path}")
    excluded: set[int] = set()
    executable = False
    modules = []
    production_lines = set()

    def visit(node):
        nonlocal executable
        pending_test = False
        for child in node.named_children:
            if child.type == "attribute_item":
                attribute = source[child.start_byte:child.end_byte].decode()
                compact = "".join(attribute.split())
                pending_test |= compact in {"#[cfg(test)]", "#[test]"}
                continue
            if child.type in {"line_comment", "block_comment"}:
                continue
            if pending_test:
                if child.type == "mod_item" and child.child_by_field_name("body") is None:
                    name = child.child_by_field_name("name").text.decode()
                    base = path.parent if path.stem in {"lib", "main", "mod"} else path.with_suffix("")
                    modules.extend([base / f"{name}.rs", base / name])
                excluded.update(range(child.start_point.row + 1, child.end_point.row + 2))
                pending_test = False
            else:
                if child.type in {"function_item", "closure_expression", "macro_invocation", "macro_definition"}:
                    executable = True
                    production_lines.update(range(child.start_point.row + 1, child.end_point.row + 2))
                visit(child)

    visit(tree.root_node)
    return excluded - production_lines, executable, modules


def read_xccov(path: Path, root: Path, source_prefix: Path | None = None) -> dict[str, tuple[int, int]]:
    files = {}
    for target in json.loads(path.read_text())["targets"]:
        for row in target.get("files", []):
            name = relative(row["path"], root, root, source_prefix)
            if name is None:
                continue
            counts = (row["coveredLines"], row["executableLines"])
            if name in files and files[name] != counts:
                raise ValueError(f"xccov has conflicting target counts for {name}; do not sum overlapping files")
            files[name] = counts
    if not files:
        raise ValueError(f"{path}: no repository files in xccov report")
    return files


def summarize(root: Path, policy: dict, scope: str, lines: dict, counts: dict) -> list[dict]:
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).decode().split("\0")
    sources = [name for name in tracked if Path(name).suffix in SCOPES[scope]
               and (scope != "desktop" or name.startswith("apps/obc-desktop/src/"))]
    global_exclusions = [row["path"] for row in policy.get("exclude", [])]
    syntax = {name: rust_source(root / name) for name in sources} if scope in {"rust", "desktop"} else {}
    test_modules = [p for _, _, modules in syntax.values() for p in modules]
    result = []
    for component in policy["component"]:
        owned = [name for name in sources if matches(name, component["include"])]
        if not owned:
            continue
        exclusions = global_exclusions + [row["path"] for row in component.get("exclude", [])]
        files, missing, declarations, excluded = {}, [], [], []
        for name in owned:
            source = root / name
            if matches(name, exclusions) or any(source == module or module in source.parents for module in test_modules):
                excluded.append(name)
                continue
            omitted, executable, _ = syntax.get(name, (set(), True, []))
            if name in lines:
                measured = {line: hit for line, hit in lines[name].items() if line not in omitted}
                if executable and not measured:
                    missing.append(name)
                else:
                    files[name] = {"covered": sum(measured.values()), "total": len(measured), "test_lines_removed": len(lines[name]) - len(measured)}
            elif name in counts:
                covered, total = counts[name]
                files[name] = {"covered": covered, "total": total}
            elif not executable:
                declarations.append(name)
            else:
                missing.append(name)
        covered = sum(row["covered"] for row in files.values())
        total = sum(row["total"] for row in files.values())
        result.append({"id": component["id"], "enforcement": component["enforcement"], "covered": covered, "total": total,
                       "files": files, "unmeasured": missing, "declarations_only": declarations, "excluded": excluded})
    return result


def check_baseline(rows: list[dict], baseline: dict) -> list[str]:
    errors = []
    measured = {row["id"]: row for row in rows}
    accepted = baseline.get("components", {})
    for name in sorted(CRITICAL):
        row, old = measured.get(name), accepted.get(name)
        if not row or row["total"] <= 0:
            errors.append(f"{name}: missing or empty coverage")
            continue
        if row["unmeasured"]:
            errors.append(f"{name}: unmeasured production source: {', '.join(row['unmeasured'])}")
        if not old or not 0 <= old.get("covered", -1) <= old.get("total", 0) or old.get("total", 0) <= 0:
            errors.append(f"{name}: no accepted measured baseline")
        elif Fraction(row["covered"], row["total"]) < Fraction(old["covered"], old["total"]):
            errors.append(f"{name}: line coverage decreased ({row['covered']}/{row['total']} < {old['covered']}/{old['total']})")
    if not baseline.get("source_sha") or not baseline.get("evidence") or not baseline.get("tools"):
        errors.append("baseline lacks measurement provenance")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--scope", choices=SCOPES, required=True)
    parser.add_argument("--lcov", type=Path, action="append", default=[])
    parser.add_argument("--xccov", type=Path)
    parser.add_argument("--source-root", type=Path)
    parser.add_argument("--source-prefix", type=Path, help="original checkout root when reading downloaded native reports")
    parser.add_argument("--measurement-sha", help="original source SHA when summarizing downloaded reports")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--tool", action="append", required=True)
    parser.add_argument("--baseline", type=Path, default=Path("testing/coverage-baseline.json"))
    args = parser.parse_args()
    root = args.root.resolve()
    if not args.lcov and not args.xccov:
        parser.error("at least one native report is required")
    lines = {}
    for path in args.lcov:
        for name, data in read_lcov(path, root, (args.source_root or root).resolve(), args.source_prefix).items():
            bucket = lines.setdefault(name, {})
            for number, hits in data.items():
                bucket[number] = bucket.get(number, False) or hits
    counts = read_xccov(args.xccov, root, args.source_prefix) if args.xccov else {}
    policy = tomllib.loads((root / "testing/coverage-policy.toml").read_text())
    rows = summarize(root, policy, args.scope, lines, counts)
    baseline_path = root / args.baseline
    baseline = json.loads(baseline_path.read_text()) if baseline_path.exists() else {}
    errors = check_baseline(rows, baseline) if args.scope == "rust" else []
    policy_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    report = {"schema": 1, "scope": args.scope, "source_sha": args.measurement_sha or policy_sha, "policy_sha": policy_sha,
              "tools": args.tool, "native_reports": [str(p) for p in args.lcov] + ([str(args.xccov)] if args.xccov else []),
              "components": rows, "errors": errors}
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "components.json").write_text(json.dumps(report, indent=2) + "\n")
    text = [f"# Component coverage: {args.scope}", "", f"Source: `{report['source_sha']}`", "", "Percentages cover measured production lines in this native run. Unmeasured files remain listed in components.json.", "", "| Component | Covered / measured lines | Line coverage | Unmeasured files | Policy |", "| --- | ---: | ---: | ---: | --- |"]
    for row in rows:
        percent = f"{100 * row['covered'] / row['total']:.2f}%" if row["total"] else "unmeasured"
        text.append(f"| {row['id']} | {row['covered']} / {row['total']} | {percent} | {len(row['unmeasured'])} | {row['enforcement']} |")
    if errors:
        text += ["", "## Gate failures", ""] + [f"- {error}" for error in errors]
    (args.output / "summary.md").write_text("\n".join(text) + "\n")
    print("\n".join(text))
    return bool(errors)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, KeyError, OSError) as error:
        sys.exit(f"coverage report: {error}")
