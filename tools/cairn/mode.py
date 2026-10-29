"""The mode of a change: what a pull request is, and what its diff may do.

The mode of a branch lives in the local git config as `branch.<name>.cairnMode`. Its value is the
text after `Mode:` in the pull request body, so the branch and the body use one parser.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

KINDS = ("fix", "change", "prototype", "override")
KEY = "cairnMode"
FORMS = (
    re.compile(r"(?P<kind>fix)(?:, over limit: (?P<reason>\S.*))?"),
    re.compile(r"(?P<kind>change) #(?P<issue>[1-9]\d*)"),
    re.compile(r"(?P<kind>prototype)"),
    re.compile(r"(?P<kind>override): (?P<reason>\S.*)"),
)
COMMENT = re.compile(r"<!--.*?-->", re.S)
FENCE = re.compile(r"^```.*?^```", re.S | re.M)
ASK = (
    "cairn: the branch `{branch}` has no declared mode. Ask the user which mode applies: "
    "fix, change N (N is an approved issue), prototype, or override with a reason. Then declare "
    'it with `obc cairn mode fix`, `obc cairn mode change N`, `obc cairn mode prototype` or '
    '`obc cairn mode override "<reason>"`, and do the edit again.'
)


@dataclass(frozen=True)
class Mode:
    kind: str
    issue: int = 0
    # The reason of an override, or of a fix over the size limit.
    reason: str = ""

    def line(self) -> str:
        if self.kind == "change":
            return f"change #{self.issue}"
        if self.kind == "override":
            return f"override: {self.reason}"
        return f"fix, over limit: {self.reason}" if self.reason else self.kind


def parse(value: str) -> Mode:
    for form in FORMS:
        match = form.fullmatch(value.strip())
        if match:
            found = match.groupdict()
            return Mode(found["kind"], int(found.get("issue") or 0), (found.get("reason") or "").strip())
    raise ValueError(f"`{value.strip()}` is not a mode; use fix, fix, over limit: <reason>, "
                     "change #N, prototype or override: <reason>")


def declaration(kind: str, value: str | None, over_limit: str | None) -> Mode:
    """The mode that `cairn mode KIND [VALUE] [--over-limit REASON]` declares."""
    if over_limit is not None and kind != "fix":
        raise ValueError("--over-limit applies to fix only")
    if kind == "change":
        return parse(f"change #{(value or '').lstrip('#')}")
    if kind == "override":
        return parse(f"override: {value or ''}")
    if value is not None:
        raise ValueError(f"{kind} takes no value")
    return parse(f"fix, over limit: {over_limit}" if over_limit is not None else kind)


def body_value(body: str) -> str | None:
    """The text after the first `Mode:` line of a pull request body, or None when it is empty."""
    for line in COMMENT.sub("", body).splitlines():
        if line.startswith("Mode:"):
            return line[len("Mode:"):].strip() or None
    return None


# ── the store ────────────────────────────────────────────────────────────────


def git(folder: Path, *args: str) -> str | None:
    done = subprocess.run(["git", "-C", str(folder), *args], capture_output=True, text=True)
    return done.stdout.strip() if done.returncode == 0 else None


def branch(folder: Path) -> str | None:
    """The current branch, or None for a detached HEAD or a folder outside git."""
    name = git(folder, "symbolic-ref", "--quiet", "--short", "HEAD")
    return name or None


def declared(folder: Path) -> str | None:
    name = branch(folder)
    return git(folder, "config", "--get", f"branch.{name}.{KEY}") if name else None


def declare(folder: Path, mode: Mode) -> str:
    name = branch(folder)
    if not name:
        raise ValueError("HEAD is not on a branch")
    if git(folder, "config", f"branch.{name}.{KEY}", mode.line()) is None:
        raise ValueError(f"git config refused branch.{name}.{KEY}")
    return name


# ── the approved issue ───────────────────────────────────────────────────────


def approval_hash(body: str) -> str:
    """The first 12 hex digits of SHA-256 over the body without its `Status:` and `Approved:`
    lines. Trailing whitespace does not count, so an edit that only adds those lines keeps it."""
    kept = [line.rstrip() for line in body.splitlines() if not line.startswith(("Status:", "Approved:"))]
    return hashlib.sha256("\n".join(kept).rstrip().encode()).hexdigest()[:12]


def listed(body: str, names: dict[str, str]) -> set[str]:
    """The systems on the `**Systems:**` line outside code blocks, by slug or by name, with notes in
    brackets removed."""
    match = re.search(r"^\*\*Systems:\*\*(.*)$", FENCE.sub("", body), re.M)
    items = re.sub(r"\([^)]*\)", "", match.group(1)).split(",") if match else []
    wanted = {item.strip().lower() for item in items}
    return {slug for slug, name in names.items() if slug.lower() in wanted or name.lower() in wanted}


def issue_problems(number: int, body: str) -> list[str]:
    problems = []
    status = next((line for line in body.splitlines() if line.startswith("Status:")), "")
    if not re.match(r"Status: approved\b", status):
        problems.append(f"the first `Status:` line of issue #{number} is not `Status: approved`")
    approved = re.search(r"^Approved: ([0-9a-f]{12})\s*$", body, re.M)
    if not approved:
        problems.append(f"issue #{number} has no `Approved: <hash>` line")
    elif approved.group(1) != approval_hash(body):
        problems.append(f"issue #{number} changed after its approval: the hash is {approval_hash(body)}, "
                        f"the approval holds {approved.group(1)}")
    return problems


def issue_body(number: int, data: dict) -> str:
    """The body of an issue from the API. The issues API also gives pull requests, and a pull
    request cannot approve a change, so it is refused."""
    if "pull_request" in data:
        raise ValueError(f"#{number} is a pull request, not an issue")
    return data.get("body") or ""


def fetch_issue(root: Path, number: int) -> str:
    """The issue body through `gh api`, which reads `GH_TOKEN` and `GH_REPO` in CI."""
    done = subprocess.run(["gh", "api", f"repos/{{owner}}/{{repo}}/issues/{number}"],
                          cwd=root, capture_output=True, text=True)
    if done.returncode:
        raise OSError(done.stderr.strip() or "gh api failed")
    return issue_body(number, json.loads(done.stdout))


# ── the rules ────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Diff:
    #: The systems and shells that own a changed file or whose doc changed.
    touched: frozenset[str]
    #: The changed lines outside tests.
    lines: int = 0
    #: The dependencies between systems that HEAD has and the base does not.
    new_edges: frozenset[tuple[str, str]] = frozenset()
    #: The systems whose doc changed, and those whose doc header changed.
    docs: frozenset[str] = frozenset()
    headers: frozenset[str] = frozenset()
    specs: tuple[str, ...] = ()


def violations(mode: Mode, diff: Diff, limit: int, issue: str | None = None, names: dict[str, str] | None = None) -> list[str]:
    """What the diff does that the mode does not allow. `issue` is the body of a change's issue."""
    out = []
    if mode.kind == "fix":
        if len(diff.touched) > 1:
            out.append(f"a fix touches one system, and this one touches {', '.join(sorted(diff.touched))}")
        out += [f"a fix adds no dependency between systems: {a} -> {b}" for a, b in sorted(diff.new_edges)]
        if diff.headers:
            out.append(f"a fix changes no doc header, and this one changes {', '.join(sorted(diff.headers))}")
        if diff.specs:
            out.append(f"a fix changes no spec, and this one changes {', '.join(diff.specs)}")
        if diff.lines > limit and not mode.reason:
            out.append(f"a fix changes at most {limit:,} lines outside tests, and this one changes {diff.lines:,}")
    elif mode.kind == "change":
        if issue is None:
            return [f"issue #{mode.issue} could not be read"]
        out += issue_problems(mode.issue, issue)
        allowed = listed(issue, names or {})
        out += [f"issue #{mode.issue} does not list {system} on its Systems line"
                for system in sorted(diff.touched - allowed)]
        out += [f"the change adds {a} -> {b}, so it changes docs/design/{a}.md too"
                for a, b in sorted(diff.new_edges) if a not in diff.docs]
    elif mode.kind == "prototype":
        out.append("a prototype never merges")
    return out


def fails(mode: Mode | None, found: list[str], touched: frozenset[str], approved: set[str]) -> bool:
    """A prototype always fails. Any other violation fails only when the diff touches an approved
    system."""
    if mode is not None and mode.kind == "prototype":
        return True
    return bool(found) and bool(touched & approved)


# ── the edit hook ────────────────────────────────────────────────────────────


def hook_message(event: dict, root: Path, sources: dict) -> str | None:
    """The reason to block an edit, or None. The hook blocks an edit of a source file inside the
    repository while its branch has no declared mode. A detached HEAD has no branch, so it passes."""
    tool_input, cwd = event.get("tool_input"), event.get("cwd")
    target = (tool_input.get("file_path") or tool_input.get("notebook_path")) if isinstance(tool_input, dict) else None
    if not isinstance(target, str) or not target:
        return None
    path = (Path(cwd if isinstance(cwd, str) and cwd else os.getcwd()) / target).resolve()
    root = root.resolve()
    if not path.is_relative_to(root):
        return None
    relative = path.relative_to(root).as_posix()
    if not (relative.endswith(tuple(sources["extensions"])) or relative in sources["files"]):
        return None
    folder = next(p for p in path.parents if p.is_dir())
    name = branch(folder)
    if not name or git(folder, "config", "--get", f"branch.{name}.{KEY}"):
        return None
    return ASK.format(branch=name)
