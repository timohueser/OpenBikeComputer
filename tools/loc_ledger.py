#!/usr/bin/env python3
"""Count committed Rust source as production or test/harness.

Usage::

    python3 tools/loc_ledger.py --storage-total [--head REF] [--check-budget]
    python3 tools/loc_ledger.py --base REF --head REF [--storage-series]
    python3 tools/loc_ledger.py --pr NUMBER [--basis code]

The absolute storage report counts the fixed STORAGE_SERIES_PATHS recursively.
It uses raw production lines for the 6,000-line ceiling and shows structural
code lines as supplemental evidence. See tools/storage-line-budget.md for the
scope, exclusions, current reconciliation and scanner limits.

Raw means every physical source line, including blanks and comments; a final
unterminated line counts once. Code means a line with a token left by
strip_noise, which removes comments and literal contents. This is a structural
counter, not a Rust compiler or a general code-coverage measurement.

The absolute report excludes only exact cfg(test) gates and named harness
files. Mixed test/std gates remain counted. The historical delta mode keeps its
broader test-mentioning cfg rule: any(test, feature = "std") selects harness,
not(test) selects production, and cfg_attr does not gate an item. Do not add
historical deltas to the absolute total.

The scanner balances item braces after removing comments and string/character
literal contents. It handles nested comments, raw strings, lifetimes and array
semicolons. It does not expand macros, follow include! or #[path] indirection,
or evaluate general cfg expressions. Scope changes and new syntax need review.

Delta mode defaults to HEAD against the merge base with origin/develop. --pr
uses the merge base of both parents and the PR head; git diff -M recognizes
renames. Specs, documentation and non-Rust source are outside both Rust totals.
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

_HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
# cfg_attr changes attributes, not whether the item is present.
_CFG_ATTR = re.compile(r"^#!?\[\s*cfg\s*\(")
_MOD_DECL = re.compile(r"\bmod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
_TEST_TOKEN = re.compile(r"\btest\b")


def gates_on_test(attr_text: str) -> bool:
    """Does this ``cfg`` expression select the *test* configuration?

    Every ``not( … )`` group is removed first, balanced, so `not(test)` — which
    is the **production** half of a build — cannot be read as a test gate.
    `all(not(test), feature = "x")` is production; `any(test, miri)` is test.
    """
    out: list[str] = []
    i = 0
    while i < len(attr_text):
        j = attr_text.find("not(", i)
        if j < 0:
            out.append(attr_text[i:])
            break
        # `not` must be a whole token, not the tail of `cannot(`.
        if j > 0 and (attr_text[j - 1].isalnum() or attr_text[j - 1] == "_"):
            out.append(attr_text[i : j + 4])
            i = j + 4
            continue
        out.append(attr_text[i:j])
        depth = 0
        k = j + 3
        while k < len(attr_text):
            if attr_text[k] == "(":
                depth += 1
            elif attr_text[k] == ")":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        i = k + 1
    return bool(_TEST_TOKEN.search("".join(out)))


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


def scan_cfg_test(text: str, *, exact_test: bool = False) -> tuple[set[int], set[str]]:
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
        selected = (
            bool(re.match(r"^\s*#!?\[\s*cfg\s*\(\s*test\s*\)\s*\]", attr_text))
            if exact_test else gates_on_test(attr_text)
        )
        if not selected:
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

    def __init__(self, repo: str, ref: str, *, exact_test: bool = False) -> None:
        self.repo = repo
        self.ref = ref
        self.exact_test = exact_test
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
            self._scan[path] = scan_cfg_test(text, exact_test=self.exact_test) if text is not None else (set(), set())
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
        lines.append("    Historical delta only. Use --storage-total for the current absolute budget.")
        lines.append("")
    return "\n".join(lines)


def storage_total(repo: str, ref: str) -> Ledger:
    """Count one immutable tree, with conservative test exclusions."""
    head = run_git(repo, "rev-parse", "--verify", f"{ref}^{{commit}}").strip()
    tree = Tree(repo, head, exact_test=True)
    paths = run_git(repo, "ls-tree", "-r", "--name-only", head, "--", *STORAGE_SERIES_PATHS).splitlines()
    paths = [path for path in paths if path.endswith(".rs")]
    if not paths:
        raise SystemExit("storage scope contains no Rust files; update and review the fixed scope")
    result = Ledger(base="", head=head, basis="raw")
    for path in sorted(paths):
        source = tree.text(path)
        if source is None:
            raise SystemExit(f"cannot read counted source: {path}")
        bucket, reason = classify_file(path, tree, tree)
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
            counts.add(True, is_code_line(code))
        result.files.append(entry)
    return result


def render_storage_total(total: Ledger) -> str:
    rows = [
        "Flat-store absolute line budget (raw production basis)",
        f"commit {total.head} (committed tree only; working-tree edits excluded)",
        "scope  " + ", ".join(STORAGE_SERIES_PATHS),
        "", "module | production raw | production code | excluded test raw | classification",
    ]
    for entry in total.files:
        rows.append(f"{entry.path} | {entry.prod.raw_add} | {entry.prod.code_add} | "
                    f"{entry.test.raw_add} | {entry.reason}")
    raw = total.totals(PRODUCTION, "raw")[0]
    code = total.totals(PRODUCTION, "code")[0]
    test = total.totals(TEST, "raw")[0]
    rows.extend([
        f"TOTAL | {raw} | {code} | {test}",
        f"Reconciliation: {raw} production + {test} excluded = {raw + test} Rust source lines",
        f"Budget: {raw} / {STORAGE_LIMIT} raw production lines; "
        + (f"OVER by {raw - STORAGE_LIMIT}" if raw > STORAGE_LIMIT else f"within by {STORAGE_LIMIT - raw}"),
        "Flat-layer scope only. Adapter/FAT removal and physical acceptance remain separate.",
    ])
    return "\n".join(rows)


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
    ap.add_argument("--storage-total", action="store_true", help="count the absolute committed flat-store layer")
    ap.add_argument("--check-budget", action="store_true", help="with --storage-total, fail above 6,000 raw production lines")
    ap.add_argument("--base", help="base ref (default: merge-base with origin/develop)")
    ap.add_argument("--head", help="head ref (default: HEAD)")
    ap.add_argument("--pr", type=int, help="count a merged PR by number (both sides of its merge commit)")
    ap.add_argument("--develop", default="origin/develop", help="ref the default base is taken against")
    ap.add_argument(
        "--basis",
        choices=("raw", "code"),
        default="raw",
        help="which basis leads the table (both are always totalled): "
        "raw = every line; "
        "code = non-blank, non-comment",
    )
    ap.add_argument("--storage-series", action="store_true", help="also print the #1256 budget line")
    ap.add_argument("--show-other", action="store_true", help="list uncounted non-Rust files too")
    args = ap.parse_args(argv)

    repo = run_git(os.path.dirname(os.path.abspath(__file__)) or ".", "rev-parse", "--show-toplevel").strip()
    if args.check_budget and not args.storage_total:
        ap.error("--check-budget requires --storage-total")
    if args.storage_total:
        if args.base or args.pr or args.storage_series or args.basis != "raw" or args.show_other:
            ap.error("--storage-total accepts only --head and --check-budget")
        total = storage_total(repo, args.head or "HEAD")
        print(render_storage_total(total))
        return int(args.check_budget and total.totals(PRODUCTION, "raw")[0] > STORAGE_LIMIT)
    base, head = resolve_range(repo, args)

    ledger = build_ledger(repo, base, head, args.basis, None)
    storage = None
    if args.storage_series:
        storage = build_ledger(repo, base, head, args.basis, list(STORAGE_SERIES_PATHS))

    print(render(ledger, storage, args.show_other))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
