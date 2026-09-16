#!/usr/bin/env python3
"""Count committed Rust source in the flat-store layer as production or test.

Usage::

    python3 tools/loc_ledger.py --storage-total [--head REF] [--check-budget]

The report counts the fixed STORAGE_SERIES_PATHS recursively at one commit. It
uses raw production lines for the 6,000-line ceiling and shows structural code
lines as supplemental evidence. See tools/storage-line-budget.md for the scope,
exclusions, current reconciliation and scanner limits.

Raw means every physical source line, including blanks and comments; a final
unterminated line counts once. Code means a line with a token left by
strip_noise, which removes comments and literal contents. This is a structural
counter, not a Rust compiler or a general code-coverage measurement.

The report excludes only exact cfg(test) gates and named harness files. Mixed
test/std gates remain counted.

The scanner balances item braces after removing comments and string/character
literal contents. It handles nested comments, raw strings, lifetimes and array
semicolons. It does not expand macros, follow include! or #[path] indirection,
or evaluate general cfg expressions. Scope changes and new syntax need review.

Specs, documentation and non-Rust source are outside the total.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field

# --------------------------------------------------------------------------
# The pinned sets.  Every entry here is a decision, not a heuristic — adding to
# any of them is a deliberate act that changes a published budget figure, so
# say why in the comment beside it and say so in the PR that moves it.
# --------------------------------------------------------------------------

#: Crates that exist to check other crates: oracles, vector producers, replay
#: harnesses, bench drivers.  Never production, wherever their files sit.
ORACLE_CRATES = (
    "obcm-testkit",  # the independent packer/reader oracle
    "obc-vectors",  # golden-vector producer
    "obc-replay",  # replay harness
    "obc-bench",  # host bench driver
)

# The flat-store layer includes its wire binder, metadata and route cleanup.
# Protocol engines, format codecs, platform adapters and old FAT paths are
# outside this boundary; deleting them cannot offset growth inside it.
STORAGE_SERIES_PATHS = ("firmware/obc-storage/src/flat/",)
STORAGE_LIMIT = 6_000
# Explicit fault/model backends, exposed with std for external test crates.
STORAGE_HARNESS_FILES = {
    "firmware/obc-storage/src/flat/model.rs",
    "firmware/obc-storage/src/flat/sim.rs",
}

#: Directory names that make everything under them fixture data.
FIXTURE_DIRS = ("fixtures", "testdata", "test-data", "golden", "vectors")

# cfg_attr changes attributes, not whether the item is present.
_CFG_ATTR = re.compile(r"^#!?\[\s*cfg\s*\(")
_MOD_DECL = re.compile(r"\bmod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")


def run_git(repo: str, *args: str) -> str:
    """Run git in ``repo`` and return stdout, or raise with git's own message."""
    proc = subprocess.run(
        ["git", "-C", repo, *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="replace",
    )
    if proc.returncode != 0:
        raise SystemExit(f"git {' '.join(args)}: {proc.stderr.strip()}")
    return proc.stdout


# --------------------------------------------------------------------------
# Rust lexing, to the shallow depth this needs
# --------------------------------------------------------------------------


def strip_noise(lines: list[str]) -> list[str]:
    """Blank out comments and the *contents* of string/char literals.

    Returns one entry per input line, same length, with structure (braces,
    brackets, semicolons, identifiers outside literals) preserved.  Raw strings
    (``r#"…"#``) are handled; the goal is only that a brace inside a literal or
    a comment cannot be mistaken for real nesting.
    """
    out: list[str] = []
    in_block = 0  # /* */ nesting depth
    in_raw: str | None = None  # closing delimiter of an open raw string
    for line in lines:
        buf: list[str] = []
        i = 0
        n = len(line)
        while i < n:
            if in_raw is not None:
                end = line.find(in_raw, i)
                if end < 0:
                    i = n
                else:
                    buf.append(" " * len(in_raw))
                    i = end + len(in_raw)
                    in_raw = None
                continue
            if in_block:
                end = line.find("*/", i)
                start = line.find("/*", i)
                if start >= 0 and (end < 0 or start < end):
                    in_block += 1
                    i = start + 2
                    continue
                if end < 0:
                    i = n
                else:
                    in_block -= 1
                    i = end + 2
                continue
            ch = line[i]
            two = line[i : i + 2]
            if two == "//":
                break  # line comment: nothing structural after it
            if two == "/*":
                in_block = 1
                i += 2
                continue
            if ch == "r" and i + 1 < n and line[i + 1] in '#"':
                j = i + 1
                hashes = 0
                while j < n and line[j] == "#":
                    hashes += 1
                    j += 1
                if j < n and line[j] == '"':
                    closer = '"' + "#" * hashes
                    end = line.find(closer, j + 1)
                    buf.append('""')
                    if end < 0:
                        in_raw = closer
                        i = n
                    else:
                        i = end + len(closer)
                    continue
            if ch == "'" and i + 1 < n and (line[i + 1].isalpha() or line[i + 1] == "_"):
                # A lifetime (`'a`), not a char literal — `'a'` would have a
                # closing tick two characters along. Without this, `impl<'a>
                # Foo<'a> {` reads as a char literal spanning both ticks and can
                # swallow the brace between them.
                if i + 2 >= n or line[i + 2] != "'":
                    buf.append(ch)
                    i += 1
                    continue
            if ch in '"\'':
                # A char literal always closes on the same line.
                j = i + 1
                closed = False
                while j < n:
                    if line[j] == "\\":
                        j += 2
                        continue
                    if line[j] == ch:
                        closed = True
                        break
                    j += 1
                if not closed and ch == "'":
                    buf.append(ch)  # lifetime — keep it, it is structural noise only
                    i += 1
                    continue
                buf.append(ch * 2 if closed else ch)
                i = (j + 1) if closed else n
                continue
            buf.append(ch)
            i += 1
        out.append("".join(buf))
    return out


def is_code_line(stripped: str) -> bool:
    """A code line: something survives once comments and blanks are removed."""
    return bool(stripped.strip())


def scan_cfg_test(text: str) -> tuple[set[int], set[str]]:
    """Return (1-based line numbers inside test-gated items, test-gated module names).

    Approximate by construction — see the module docstring.
    """
    lines = text.split("\n")
    code = strip_noise(lines)
    test_lines: set[int] = set()
    test_mods: set[str] = set()
    i = 0
    n = len(code)
    while i < n:
        s = code[i].lstrip()
        if not _CFG_ATTR.match(s):
            i += 1
            continue
        # Consume the whole attribute (it may wrap across lines).
        start = i
        depth = 0
        j = i
        attr: list[str] = []
        while j < n:
            attr.append(code[j])
            depth += code[j].count("[") - code[j].count("]")
            if depth <= 0 and code[j].strip():
                break
            j += 1
        attr_text = " ".join(attr)
        if not re.match(r"^\s*#!?\[\s*cfg\s*\(\s*test\s*\)\s*\]", attr_text):
            i = j + 1
            continue
        # `#![cfg(test)]` at the top of a file gates the whole file.
        if attr_text.lstrip().startswith("#!["):
            test_lines.update(range(1, n + 1))
            i = j + 1
            continue
        # Find the item this attribute decorates: a braced body, or a `;`.
        k = j
        brace = 0
        nest = 0  # `(` and `[` depth — a `;` inside `[u8; 4]` ends nothing
        opened = False
        end = None
        while k < n:
            line = code[k]
            # Further attributes on the same item (`#[cfg(test)] #[allow(…)] mod x`)
            # carry neither a brace nor a `;`, so the walk passes straight over them.
            for ch in line:
                if ch in "([":
                    nest += 1
                elif ch in ")]":
                    nest -= 1
                elif ch == "{":
                    brace += 1
                    opened = True
                elif ch == "}":
                    brace -= 1
                    if opened and brace <= 0:
                        end = k
                        break
                elif ch == ";" and not opened and brace == 0 and nest <= 0:
                    end = k
                    m = _MOD_DECL.search(line)
                    if m:
                        test_mods.add(m.group(1))
                    break
            if end is not None:
                break
            k += 1
        if end is None:
            end = n - 1
        test_lines.update(range(start + 1, end + 2))
        i = end + 1
    return test_lines, test_mods


# --------------------------------------------------------------------------
# File classification
# --------------------------------------------------------------------------

PRODUCTION = "production"
TEST = "test"
OTHER = "other"


class Tree:
    """Cached read access to one git tree, plus the cfg(test) facts it implies."""

    def __init__(self, repo: str, ref: str) -> None:
        self.repo = repo
        self.ref = ref
        self._text: dict[str, str | None] = {}
        self._scan: dict[str, tuple[set[int], set[str]]] = {}
        self._is_test_mod_file: dict[str, bool] = {}

    def text(self, path: str) -> str | None:
        if path not in self._text:
            proc = subprocess.run(
                ["git", "-C", self.repo, "show", f"{self.ref}:{path}"],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                errors="replace",
            )
            self._text[path] = proc.stdout if proc.returncode == 0 else None
        return self._text[path]

    def scan(self, path: str) -> tuple[set[int], set[str]]:
        if path not in self._scan:
            text = self.text(path)
            self._scan[path] = scan_cfg_test(text) if text is not None else (set(), set())
        return self._scan[path]

    def cfg_test_lines(self, path: str) -> set[int]:
        return self.scan(path)[0]

    def declared_under_cfg_test(self, path: str) -> bool:
        """Rule 7: is this file's module declared under a test-mentioning cfg?

        Walks the parent-module chain, so a test module's submodules inherit it.
        """
        if path in self._is_test_mod_file:
            return self._is_test_mod_file[path]
        self._is_test_mod_file[path] = False  # cycle guard
        result = False
        for parent, name in self._parents(path):
            if self.text(parent) is None:
                continue
            if name in self.scan(parent)[1]:
                result = True
                break
            if self.declared_under_cfg_test(parent):
                result = True
                break
        self._is_test_mod_file[path] = result
        return result

    @staticmethod
    def _parents(path: str) -> list[tuple[str, str]]:
        """Candidate (parent file, module name) pairs for a module file."""
        parts = path.split("/")
        base = parts[-1]
        directory = parts[:-1]
        if base == "mod.rs":
            if not directory:
                return []
            name = directory[-1]
            up = directory[:-1]
        elif base.endswith(".rs"):
            name = base[:-3]
            up = directory
        else:
            return []
        if name in ("lib", "main"):
            return []
        cands = []
        for sibling in ("mod.rs", "lib.rs", "main.rs"):
            cands.append(("/".join(up + [sibling]), name))
        if up:  # 2018-style `foo.rs` beside `foo/`
            cands.append(("/".join(up[:-1] + [up[-1] + ".rs"]), name))
        return cands


def crate_of(path: str) -> str | None:
    """The crate directory name for a repo path, if it looks like one."""
    parts = path.split("/")
    for root in ("firmware", "host", "apps"):
        if len(parts) > 2 and parts[0] == root:
            return parts[1]
    return None


def classify_file(path: str, tree: Tree) -> tuple[str, str]:
    """Return (bucket, reason).  First matching rule wins — order is the basis."""
    parts = path.split("/")
    stem = parts[-1]
    if not stem.endswith(".rs"):
        return OTHER, "not Rust — uncounted"
    if "tests" in parts[:-1]:
        return TEST, "rule 1: under tests/"
    if "benches" in parts[:-1]:
        return TEST, "rule 2: under benches/"
    if "bin" in parts[:-1] and "bench" in stem:
        return TEST, "rule 3: bench binary"
    name = stem[:-3]
    if name.endswith("_test") or name.endswith("_tests") or name.startswith("test_"):
        return TEST, "rule 4: test-named file"
    crate = crate_of(path)
    if crate in ORACLE_CRATES:
        return TEST, f"rule 5: oracle crate ({crate})"
    if any(d in FIXTURE_DIRS for d in parts[:-1]):
        return TEST, "rule 6: fixture directory"
    if tree.declared_under_cfg_test(path):
        return TEST, "rule 7: cfg(test) module file"
    return PRODUCTION, "production"


# --------------------------------------------------------------------------
# Counting
# --------------------------------------------------------------------------


@dataclass
class Counts:
    """Lines counted on both bases: every line, and every line with a token."""

    raw: int = 0
    code: int = 0

    def add(self, is_code: bool) -> None:
        self.raw += 1
        if is_code:
            self.code += 1

    def merge(self, other: Counts) -> None:
        self.raw += other.raw
        self.code += other.code


@dataclass
class FileLedger:
    path: str
    bucket: str
    reason: str
    prod: Counts = field(default_factory=Counts)
    test: Counts = field(default_factory=Counts)

    def counts(self, bucket: str) -> Counts:
        return self.test if bucket == TEST else self.prod


@dataclass
class StorageTotal:
    """One committed tree, counted file by file."""

    head: str
    files: list[FileLedger] = field(default_factory=list)

    def totals(self, bucket: str) -> Counts:
        acc = Counts()
        for entry in self.files:
            acc.merge(entry.counts(bucket))
        return acc


def storage_total(repo: str, ref: str) -> StorageTotal:
    """Count one immutable tree, with conservative test exclusions."""
    head = run_git(repo, "rev-parse", "--verify", f"{ref}^{{commit}}").strip()
    tree = Tree(repo, head)
    paths = run_git(repo, "ls-tree", "-r", "--name-only", head, "--", *STORAGE_SERIES_PATHS).splitlines()
    paths = [path for path in paths if path.endswith(".rs")]
    if not paths:
        raise SystemExit("storage scope contains no Rust files; update and review the fixed scope")
    result = StorageTotal(head=head)
    for path in sorted(paths):
        source = tree.text(path)
        if source is None:
            raise SystemExit(f"cannot read counted source: {path}")
        bucket, reason = classify_file(path, tree)
        if path in STORAGE_HARNESS_FILES:
            bucket, reason = TEST, "explicit fault/model harness"
        entry = FileLedger(path=path, bucket=bucket, reason=reason)
        # split on LF only, without inventing a line after the final newline.
        lines = source.split("\n")
        if lines[-1] == "":
            lines.pop()
        gated = tree.cfg_test_lines(path) if bucket == PRODUCTION else set()
        for number, code in enumerate(strip_noise(lines), 1):
            counts = entry.test if bucket == TEST or number in gated else entry.prod
            counts.add(is_code_line(code))
        result.files.append(entry)
    return result


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def render_storage_total(total: StorageTotal) -> str:
    rows = [
        "Flat-store absolute line budget (raw production basis)",
        f"commit {total.head} (committed tree only; working-tree edits excluded)",
        "scope  " + ", ".join(STORAGE_SERIES_PATHS),
        "", "module | production raw | production code | excluded test raw | classification",
    ]
    for entry in total.files:
        rows.append(f"{entry.path} | {entry.prod.raw} | {entry.prod.code} | "
                    f"{entry.test.raw} | {entry.reason}")
    production = total.totals(PRODUCTION)
    test = total.totals(TEST).raw
    rows.extend([
        f"TOTAL | {production.raw} | {production.code} | {test}",
        f"Reconciliation: {production.raw} production + {test} excluded = "
        f"{production.raw + test} Rust source lines",
        f"Budget: {production.raw} / {STORAGE_LIMIT} raw production lines; "
        + (f"OVER by {production.raw - STORAGE_LIMIT}" if production.raw > STORAGE_LIMIT
           else f"within by {STORAGE_LIMIT - production.raw}"),
        "Flat-layer scope only. Adapter/FAT removal and physical acceptance remain separate.",
    ])
    return "\n".join(rows)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(
        prog="loc_ledger.py",
        description="Deterministic production-vs-test line count for the #1256 storage budget.",
    )
    ap.add_argument("--storage-total", action="store_true", required=True,
                    help="count the absolute committed flat-store layer")
    ap.add_argument("--check-budget", action="store_true",
                    help="fail above 6,000 raw production lines")
    ap.add_argument("--head", help="commit to count (default: HEAD)")
    args = ap.parse_args(argv)

    repo = run_git(os.path.dirname(os.path.abspath(__file__)) or ".", "rev-parse", "--show-toplevel").strip()
    total = storage_total(repo, args.head or "HEAD")
    print(render_storage_total(total))
    return int(args.check_budget and total.totals(PRODUCTION).raw > STORAGE_LIMIT)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
