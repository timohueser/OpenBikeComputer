#!/usr/bin/env python3
"""Pin the LOC counting basis for the Device Object System v3 epic (#1256).

The epic runs a hard budget — the storage layer must land in **6,000
implementation lines** — and the ledger is ticked after every merge.  Three
independent hand counts of PR #1414 differed by ~120 lines on *basis* alone,
#1417's ledger was disputed, and #1418's was first posted as ``+359`` and
corrected in review to ``+39`` production.  Every one of those disputes was
about *what counts*, never about arithmetic.  This script is the answer: one
committed, deterministic definition, so a ledger figure is reproducible rather
than re-derived.

Usage (the file is committed 100644 — ``core.fileMode`` is off in this repo, so
always invoke it through the interpreter, never as ``./tools/loc_ledger.py``)::

    python3 tools/loc_ledger.py                      # HEAD vs merge-base with origin/develop
    python3 tools/loc_ledger.py --base <ref> --head <ref>
    python3 tools/loc_ledger.py --pr 1418            # both sides of a merge commit
    python3 tools/loc_ledger.py --storage-series     # + the #1256 budget line to post
    python3 tools/loc_ledger.py --basis code         # lead with non-blank, non-comment
    python3 tools/loc_ledger.py --json               # machine-readable

Or through the task runner: ``obc loc-ledger [args]``.

``--pr N`` finds the merge commit and counts ``merge-base(parents)..pr-head``,
**not** first-parent-to-second: a branch that was not rebased before merging
would otherwise have everything develop gained meanwhile counted against it,
backwards.  (That artefact alone moved #1417's figure by ~440 lines.)

The basis, in full
------------------

*Unit.*  Two are reported, always, side by side, because the epic's own series
has been ticked on both (see "basis drift" below):

  ``raw``   every line of a ``.rs`` file — **the default**, and the headline
            figure, because the ≤ 6,000 ceiling and the 19,933 lines it is
            measured against are raw file lengths (#1256 quotes ``sd.rs``
            at 5,057 and ``object_store.rs`` at 2,226, both plain ``wc -l``).
            A ceiling and its subject have to be counted the same way.
  ``code``  non-blank, non-comment lines (``//``, ``///``, ``//!`` and
            ``/* … */``).  This is what #1418's correction and #1425's ledger
            used.

``--basis`` picks which one leads the per-file table and the "post this"
line; the totals block prints both either way, so a ledger can never again be
read without knowing which basis it is on.

*Basis drift — the thing that tripped twice.*  Reconstructed with this script
against the merged history, the published ticks were:

===========  ==============  ============  ============  ==================
PR           posted          raw basis     code basis    ticked on
===========  ==============  ============  ============  ==================
#1403 FS3    +2,499          +2,489        +1,836        raw
#1414        +169            +171          +67           raw
#1417 FS6    +303            +325          +111          raw
#1418 FS7.1  +39             +223          +39           **code**
#1425        +58             +229          +100          **code**
===========  ==============  ============  ============  ==================

So the running ~3,650 is a mixture, and the two PRs whose ledgers were
disputed are exactly the two where the basis silently changed.  Reconciling
the series onto one basis is FS11's job (#1393); this script's job is to make
the choice visible and mechanical rather than re-derived per PR.

*Scope.*  Only ``.rs`` files are counted at all.  Specs, docs, workflows, JSON,
TypeScript and Swift land in a third **other** bucket that is reported but
counted in neither total — a spec rewrite is not an implementation line.

*Production vs test/harness.*  A file is test/harness if any rule below
matches; the first match wins and is reported as that file's reason.  What is
left is production, minus its own ``#[cfg(test)]`` regions:

 1. it lives under a ``tests/`` directory (integration tests);
 2. it lives under a ``benches/`` directory;
 3. it is a bench binary — ``src/bin/`` with ``bench`` in the file name;
 4. its name is test-shaped: ``*_test.rs``, ``*_tests.rs``, ``test_*.rs``;
 5. it belongs to an oracle/harness crate (``ORACLE_CRATES`` below);
 6. it lives under a fixture directory;
 7. its module is declared under a ``test``-mentioning ``cfg`` by its parent
    module — e.g. ``#[cfg(test)] mod granularity;`` makes the whole of
    ``granularity.rs`` test/harness, transitively.  This is the rule that moves
    #1418's 376-line ``flat::granularity`` module out of production, and it is
    also what classifies ``sim``/``model`` (``#[cfg(any(test, feature =
    "std"))]``) as harness rather than firmware.

Anything else is production **per line**: an added or removed line inside a
``#[cfg(test)]`` block of an otherwise-production file is test/harness.  Added
lines are judged against the *head* version of the file, removed lines against
the *base* version, so a hunk is attributed to the tree it actually existed in.

*``#[cfg(test)]`` region detection is deliberately approximate, and
deterministic.*  The scanner strips line comments, block comments and string
and char literal *contents* (so a brace inside ``"}"`` cannot mislead it), finds
attributes whose ``cfg`` expression mentions the token ``test``, and then brace-
matches the item that follows — or, for ``mod foo;``, records the module name
for rule 7.  It does not parse Rust.  A macro that emits unbalanced braces, or
``cfg_attr`` indirection, can defeat it.  Neither exists in the counted set
today, and the trade is intentional: a stdlib-only script that is wrong in ways
you can read beats a syn dependency in the tool that arbitrates a budget.

*Renames.*  Detected (``git diff -M``); a pure rename is 0/0 and a rewrite
counts only its real changes, rather than a whole file added and another
removed.
"""

from __future__ import annotations

import argparse
import json
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

#: The storage-budget crate set for ``--storage-series``: the paths whose lines
#: are ticked against the ≤ 6,000 in #1256.  Adjudicated from the epic's own
#: ticks, and deliberately narrow:
#:
#:  * ``obc-storage/src/flat/`` — the flat store itself (FS3's 2,499) *and* the
#:    obc-link binder (FS5's "216-line binder in obc-storage"), which lives at
#:    ``flat/wire.rs``.
#:  * ``obc-link``'s ``flat`` engine is **not** here.  FS5 counted it at 2,189
#:    lines and said so explicitly: "obc-link, outside the storage budget".
#:  * ``obc-formats`` is **not** here.  FS7.5's writer work (#1425, +58
#:    production) and the deletion slice (#1424, −4,273) were both reported as
#:    standalone figures and the running storage total did not move for either.
#:    They are map-format lines, not storage-layer lines.
#:  * The v1/OBC2 paths being deleted (``obc2/``, ``fat_extents.rs``, the FAT
#:    half of ``sd.rs``) are **not** here either.  The budget is a ceiling on
#:    the *new* layer; the 19,933 lines they give back are the epic's separate
#:    before/after figure, and crediting a deletion against the ceiling would
#:    let the new layer grow by exactly what the old one cost.
STORAGE_SERIES_PATHS = ("firmware/obc-storage/src/flat/",)

#: Directory names that make everything under them fixture data.
FIXTURE_DIRS = ("fixtures", "testdata", "test-data", "golden", "vectors")

_HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
_CFG_ATTR = re.compile(r"^#!?\[\s*cfg(_attr)?\s*\(")
_MOD_DECL = re.compile(r"\bmod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
_TEST_TOKEN = re.compile(r"\btest\b")


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
            if ch in '"\'':
                # A lifetime (`'a`) is not a char literal; a char literal always
                # closes on the same line.
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
        if not _TEST_TOKEN.search(attr_text):
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
        opened = False
        end = None
        while k < n:
            line = code[k]
            # Further attributes on the same item (`#[cfg(test)] #[allow(…)] mod x`)
            # carry neither a brace nor a `;`, so the walk passes straight over them.
            for ch in line:
                if ch == "{":
                    brace += 1
                    opened = True
                elif ch == "}":
                    brace -= 1
                    if opened and brace <= 0:
                        end = k
                        break
                elif ch == ";" and not opened and brace == 0:
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


def classify_file(path: str, head: Tree, base: Tree) -> tuple[str, str]:
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
    if head.declared_under_cfg_test(path) or base.declared_under_cfg_test(path):
        return TEST, "rule 7: cfg(test) module file"
    return PRODUCTION, "production"


# --------------------------------------------------------------------------
# Diff walking
# --------------------------------------------------------------------------


@dataclass
class Counts:
    """Added/removed counts for one bucket, on both bases."""

    code_add: int = 0
    code_del: int = 0
    raw_add: int = 0
    raw_del: int = 0

    def add(self, added: bool, is_code: bool) -> None:
        if added:
            self.raw_add += 1
            if is_code:
                self.code_add += 1
        else:
            self.raw_del += 1
            if is_code:
                self.code_del += 1

    def on(self, basis: str) -> tuple[int, int, int]:
        a, d = (self.code_add, self.code_del) if basis == "code" else (self.raw_add, self.raw_del)
        return a, d, a - d

    def merge(self, other: Counts) -> None:
        self.code_add += other.code_add
        self.code_del += other.code_del
        self.raw_add += other.raw_add
        self.raw_del += other.raw_del


@dataclass
class FileLedger:
    path: str
    bucket: str
    reason: str
    prod: Counts = field(default_factory=Counts)
    test: Counts = field(default_factory=Counts)
    other: Counts = field(default_factory=Counts)
    cfg_test_hunks: bool = False

    def counts(self, bucket: str) -> Counts:
        return {PRODUCTION: self.prod, TEST: self.test, OTHER: self.other}[bucket]

    @property
    def display_reason(self) -> str:
        if self.bucket == PRODUCTION and self.cfg_test_hunks:
            return "production (+ cfg(test) lines split out)"
        return self.reason


def parse_diff(repo: str, base: str, head: str, paths: list[str] | None) -> dict[str, list[tuple[int, int]]]:
    """Return ``{path: [(head_line, base_line)]}`` — added lines carry a head
    line number and base ``0``; removed lines carry base and head ``0``."""
    args = ["diff", "-U0", "-M", "--no-color", "--no-ext-diff", f"{base}", f"{head}"]
    if paths:
        args += ["--", *paths]
    out = run_git(repo, *args)
    files: dict[str, list[tuple[int, int]]] = {}
    path: str | None = None
    old_ln = new_ln = 0
    for line in out.split("\n"):
        if line.startswith("diff --git "):
            path = None
            continue
        if line.startswith("+++ "):
            target = line[4:].strip()
            if target == "/dev/null":
                continue
            path = target[2:] if target.startswith("b/") else target
            files.setdefault(path, [])
            continue
        if line.startswith("--- "):
            src = line[4:].strip()
            if path is None and src != "/dev/null":
                pass  # `+++` always follows; nothing to do
            continue
        m = _HUNK.match(line)
        if m:
            old_ln = int(m.group(1))
            new_ln = int(m.group(3))
            continue
        if path is None or not line:
            continue
        if line.startswith("+"):
            files[path].append((new_ln, 0))
            new_ln += 1
        elif line.startswith("-"):
            files[path].append((0, old_ln))
            old_ln += 1
    # A file that was deleted has no `+++ b/…`; catch it from the `--- a/…` side.
    path = None
    old_ln = 0
    for line in out.split("\n"):
        if line.startswith("--- "):
            src = line[4:].strip()
            path = None if src == "/dev/null" else (src[2:] if src.startswith("a/") else src)
            continue
        if line.startswith("+++ "):
            if line[4:].strip() != "/dev/null":
                path = None  # handled above
            continue
        m = _HUNK.match(line)
        if m:
            old_ln = int(m.group(1))
            continue
        if path is None or not line:
            continue
        if line.startswith("-"):
            files.setdefault(path, []).append((0, old_ln))
            old_ln += 1
    return files


@dataclass
class Ledger:
    base: str
    head: str
    basis: str
    files: list[FileLedger] = field(default_factory=list)

    def totals(self, bucket: str, basis: str | None = None) -> tuple[int, int, int]:
        acc = Counts()
        for f in self.files:
            acc.merge(f.counts(bucket))
        return acc.on(basis or self.basis)


def build_ledger(repo: str, base: str, head: str, basis: str, paths: list[str] | None) -> Ledger:
    head_tree = Tree(repo, head)
    base_tree = Tree(repo, base)
    diff = parse_diff(repo, base, head, paths)
    ledger = Ledger(base=base, head=head, basis=basis)
    for path in sorted(diff):
        bucket, reason = classify_file(path, head_tree, base_tree)
        entry = FileLedger(path=path, bucket=bucket, reason=reason)
        head_text = head_tree.text(path)
        base_text = base_tree.text(path)
        head_lines = head_text.split("\n") if head_text is not None else []
        base_lines = base_text.split("\n") if base_text is not None else []
        head_code = strip_noise(head_lines)
        base_code = strip_noise(base_lines)
        head_cfg = head_tree.cfg_test_lines(path) if bucket == PRODUCTION else set()
        base_cfg = base_tree.cfg_test_lines(path) if bucket == PRODUCTION else set()
        for new_ln, old_ln in diff[path]:
            added = new_ln > 0
            idx = (new_ln if added else old_ln) - 1
            src_raw = head_lines if added else base_lines
            src_code = head_code if added else base_code
            if 0 <= idx < len(src_raw):
                is_code = is_code_line(src_code[idx])
            else:
                is_code = True  # defensive: a line the tree no longer has
            if bucket == OTHER:
                entry.other.add(added, is_code)
                continue
            in_test = bucket == TEST
            if not in_test:
                cfg = head_cfg if added else base_cfg
                if (new_ln if added else old_ln) in cfg:
                    in_test = True
                    entry.cfg_test_hunks = True
            (entry.test if in_test else entry.prod).add(added, is_code)
        ledger.files.append(entry)
    return ledger


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def signed(n: int) -> str:
    return f"{n:+d}"


def _totals_block(ledger: Ledger, indent: str, show_other: bool) -> list[str]:
    """Both bases, always, side by side — the drift between them is the thing
    two hand-counted ledgers disagreed about, so neither number is hidden."""
    out = [f"{indent}{'':<20} {'code basis':>22}   {'raw basis':>22}"]
    for label, bucket in (
        ("production", PRODUCTION),
        ("test/harness", TEST),
        ("other (uncounted)", OTHER),
    ):
        ca, cd, cn = ledger.totals(bucket, "code")
        ra, rd, rn = ledger.totals(bucket, "raw")
        if bucket == OTHER and not show_other and ra == rd == 0:
            continue
        code = f"+{ca} -{cd} net {signed(cn)}"
        raw = f"+{ra} -{rd} net {signed(rn)}"
        out.append(f"{indent}{label:<20} {code:>22}   {raw:>22}")
    return out


def render(ledger: Ledger, storage: Ledger | None, show_other: bool) -> str:
    basis = ledger.basis
    lines: list[str] = []
    lines.append("LOC ledger (tools/loc_ledger.py)")
    lines.append(f"  base   {ledger.base}")
    lines.append(f"  head   {ledger.head}")
    lines.append(
        f"  basis  {basis} lines in .rs files"
        + ("  (non-blank, non-comment)" if basis == "code" else "  (every line)")
    )
    lines.append("")
    width = max((len(f.path) for f in ledger.files), default=4)
    width = min(max(width, 4), 72)
    header = f"  {'file'.ljust(width)}  {'+':>6} {'-':>6} {'net':>7}  classification"
    lines.append(header)
    lines.append("  " + "-" * (len(header) - 2))
    for f in ledger.files:
        if f.bucket == OTHER and not show_other:
            continue
        acc = Counts()
        acc.merge(f.prod)
        acc.merge(f.test)
        acc.merge(f.other)
        add, dele, net = acc.on(basis)
        path = f.path if len(f.path) <= width else "…" + f.path[-(width - 1) :]
        detail = f.display_reason
        if f.bucket == PRODUCTION and f.cfg_test_hunks:
            detail += f"  [prod {signed(f.prod.on(basis)[2])}, test {signed(f.test.on(basis)[2])}]"
        lines.append(f"  {path.ljust(width)}  {add:>6} {dele:>6} {signed(net):>7}  {detail}")
    lines.append("")
    lines.extend(_totals_block(ledger, "  ", show_other))
    lines.append("")
    if storage is not None:
        lines.append("  storage series — the #1256 budget set")
        for p in STORAGE_SERIES_PATHS:
            lines.append(f"    counted path  {p}")
        lines.extend(_totals_block(storage, "    ", show_other))
        lines.append("")
        other = "code" if basis == "raw" else "raw"
        _, _, net = storage.totals(PRODUCTION, basis)
        _, _, alt = storage.totals(PRODUCTION, other)
        lines.append(f"  POST ON #1256: storage-layer delta {signed(net)} production lines ({basis} basis)")
        lines.append(f"                 {signed(alt)} on the {other} basis — quote both, or say which.")
        lines.append(
            "    The ≤ 6,000 ceiling and the 19,933 lines it is measured against are raw file"
        )
        lines.append(
            "    lengths, which is why raw leads. The published series has been ticked on both"
        )
        lines.append("    bases — see 'basis drift' in this script's docstring.")
        lines.append("")
    return "\n".join(lines)


def to_json(ledger: Ledger, storage: Ledger | None) -> str:
    def bucket_dump(counts_on, bucket: str) -> dict:
        out = {}
        for b in ("code", "raw"):
            a, d, n = counts_on(bucket, b)
            out[b] = {"added": a, "removed": d, "net": n}
        return out

    def dump(l: Ledger) -> dict:
        return {
            "base": l.base,
            "head": l.head,
            "basis": l.basis,
            "production": bucket_dump(l.totals, PRODUCTION),
            "test": bucket_dump(l.totals, TEST),
            "other": bucket_dump(l.totals, OTHER),
            "files": [
                {
                    "path": f.path,
                    "bucket": f.bucket,
                    "reason": f.reason,
                    "production": bucket_dump(lambda b, ba, f=f: f.prod.on(ba), PRODUCTION),
                    "test": bucket_dump(lambda b, ba, f=f: f.test.on(ba), TEST),
                    "other": bucket_dump(lambda b, ba, f=f: f.other.on(ba), OTHER),
                }
                for f in l.files
            ],
        }

    out = {"ledger": dump(ledger)}
    if storage is not None:
        out["storage_series"] = dump(storage)
        out["storage_series"]["counted_paths"] = list(STORAGE_SERIES_PATHS)
    return json.dumps(out, indent=2, sort_keys=False)


def resolve_range(repo: str, args: argparse.Namespace) -> tuple[str, str]:
    if args.pr is not None:
        merge = run_git(
            repo, "log", "--all", "--format=%H %s", "--grep", f"Merge pull request #{args.pr} ", "-1"
        ).strip()
        if not merge:
            raise SystemExit(f"no merge commit found for PR #{args.pr}")
        sha = merge.split()[0]
        parents = run_git(repo, "rev-list", "--parents", "-n", "1", sha).split()
        if len(parents) < 3:
            raise SystemExit(f"{sha[:8]} is not a merge commit — pass --base/--head")
        # The base is the *merge base* of the two parents, not the first parent:
        # a branch that was not rebased before merging would otherwise have every
        # commit develop gained meanwhile counted against it, backwards.
        base = run_git(repo, "merge-base", parents[1], parents[2]).strip()
        return base, parents[2]
    head = args.head or "HEAD"
    base = args.base
    if base is None:
        base = run_git(repo, "merge-base", args.develop, head).strip()
    return base, head


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(
        prog="loc_ledger.py",
        description="Deterministic production-vs-test LOC ledger for the #1256 storage budget.",
    )
    ap.add_argument("--base", help="base ref (default: merge-base with origin/develop)")
    ap.add_argument("--head", help="head ref (default: HEAD)")
    ap.add_argument("--pr", type=int, help="count a merged PR by number (both sides of its merge commit)")
    ap.add_argument("--develop", default="origin/develop", help="ref the default base is taken against")
    ap.add_argument(
        "--basis",
        choices=("raw", "code"),
        default="raw",
        help="which basis leads the table (both are always totalled): "
        "raw = every line, the basis the 6,000-line ceiling is on; "
        "code = non-blank, non-comment",
    )
    ap.add_argument("--storage-series", action="store_true", help="also print the #1256 budget line")
    ap.add_argument("--show-other", action="store_true", help="list uncounted non-Rust files too")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args(argv)

    repo = run_git(os.path.dirname(os.path.abspath(__file__)) or ".", "rev-parse", "--show-toplevel").strip()
    base, head = resolve_range(repo, args)

    ledger = build_ledger(repo, base, head, args.basis, None)
    storage = None
    if args.storage_series:
        storage = build_ledger(repo, base, head, args.basis, list(STORAGE_SERIES_PATHS))

    if args.json:
        print(to_json(ledger, storage))
    else:
        print(render(ledger, storage, args.show_other))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
