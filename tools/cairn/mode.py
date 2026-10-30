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
SIZES = ("S", "M", "L")
#: The rules when the base config does not have them. HEAD never sets them.
DEFAULTS = {
    "fix": {"max_lines": 1000, "tests": ["**/tests/**", "tests.rs", "*_tests.rs", "*_test.py", "*Tests.swift", "*.test.ts"]},
    "size": {"S": {"systems": 1, "lines": 500}, "M": {"systems": 3, "lines": 1500}},
}
KEY = "cairnMode"
FORMS = (
    re.compile(r"(?P<kind>fix)(?:, over limit: (?P<reason>\S.*))?"),
    re.compile(r"(?P<kind>change) #(?P<issue>[1-9]\d*)"),
    re.compile(r"(?P<kind>prototype)"),
    re.compile(r"(?P<kind>override): (?P<reason>\S.*)"),
)
#: GitHub hides an unclosed comment up to the end of the body.
COMMENT = re.compile(r"<!--.*?(?:-->|\Z)", re.S)
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


def read(value: str | None, source: str) -> tuple[Mode | None, str | None]:
    """The mode of a `Mode:` value from `source`, or None and the reason."""
    if not value:
        return None, f"no `Mode:` line in {source}"
    try:
        return parse(value), None
    except ValueError as error:
        return None, str(error)


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


def visible(body: str) -> str:
    """The body without HTML comments and code blocks."""
    return FENCE.sub("", COMMENT.sub("", body))


def issue_size(body: str) -> str | None:
    """The first letter after `**Size:**` on its line, when it is S, M or L."""
    match = re.search(r"^\*\*Size:\*\*[ \t]*([SML])", visible(body), re.M)
    return match.group(1) if match else None


def listed(body: str, names: dict[str, str]) -> set[str]:
    """The systems on the `**Systems:**` line, by slug or by name, with notes in brackets removed."""
    match = re.search(r"^\*\*Systems:\*\*(.*)$", visible(body), re.M)
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


def gh(root: Path, endpoint: str, method: str = "GET", payload: dict | None = None):
    """`gh api ENDPOINT`, which reads `GH_TOKEN` and `GH_REPO` in CI. A payload is the JSON body."""
    body = ["--input", "-"] if payload is not None else []
    done = subprocess.run(["gh", "api", "--method", method, endpoint, *body], cwd=root, capture_output=True, text=True,
                          input=json.dumps(payload) if payload is not None else None)
    if done.returncode:
        raise OSError(done.stderr.strip() or "gh api failed")
    return json.loads(done.stdout) if done.stdout.strip() else None


def api(root: Path, path: str, method: str = "GET", payload: dict | None = None):
    return gh(root, f"repos/{{owner}}/{{repo}}/{path}", method, payload)


def fetch_issue(root: Path, number: int) -> str:
    return issue_body(number, api(root, f"issues/{number}"))


def fetch_epic(root: Path, number: int) -> tuple[int, str] | None:
    """The number and body of the parent issue from GitHub's sub-issue relation, or None. For an
    issue with no parent, the endpoint answers 404 with the message `No parent issue found`."""
    try:
        data = api(root, f"issues/{number}/parent")
    except OSError as error:
        if "No parent issue found" in str(error):
            return None
        raise
    parent = data.get("number") if isinstance(data, dict) else None
    if not isinstance(parent, int):
        raise ValueError(f"the parent of issue #{number} has no number")
    return parent, issue_body(parent, data)


# ── the rules ────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Diff:
    #: The systems and shells that own a changed file or whose doc changed.
    touched: frozenset[str]
    #: The changed lines outside tests.
    lines: int = 0
    #: The dependencies between systems that HEAD has and the base does not.
    new_edges: frozenset[tuple[str, str]] = frozenset()
    #: The systems whose doc changed, those whose doc header changed, and those whose doc is new.
    docs: frozenset[str] = frozenset()
    headers: frozenset[str] = frozenset()
    specs: tuple[str, ...] = ()
    new_docs: frozenset[str] = frozenset()


def size(diff: Diff, limits: dict) -> tuple[str, list[str]]:
    """The size of the diff and the signals that decide it. Each signal gives the smallest size
    that allows it, and the largest of those is the size."""
    def fit(count: int, key: str) -> str:
        return next((s for s in SIZES[:2] if count <= limits[s][key]), "L")

    systems = len(diff.touched)
    signals = [(fit(systems, "systems"), f"{systems} system{'' if systems == 1 else 's'}"),
               (fit(diff.lines, "lines"), f"{diff.lines:,} changed lines outside tests")]
    signals += [("M", f"a contract change in {', '.join(diff.specs)}")] if diff.specs else []
    signals += [("M", f"a new dependency {a} -> {b}") for a, b in sorted(diff.new_edges)]
    signals += [("L", f"a new system doc for {name}") for name in sorted(diff.new_docs)]
    found = max((s for s, _ in signals), key=SIZES.index)
    return found, [text for s, text in signals if s == found]


def size_note(number: int, issue: str, diff: Diff, limits: dict) -> str | None:
    """A note when the diff is larger than the `**Size:**` line of the issue. It never fails."""
    declared = issue_size(issue)
    found, signals = size(diff, limits)
    if declared is None or SIZES.index(found) <= SIZES.index(declared):
        return None
    return f"`change #{number}` is sized {declared}, but the diff is {found}: {', '.join(signals)}"


def violations(mode: Mode, diff: Diff, rules: dict, issue: str | None = None, names: dict[str, str] | None = None,
               epic: tuple[int, str] | None = None) -> list[str]:
    """What the diff does that the mode does not allow. `issue` is the body of a change's issue,
    and `epic` the number and body of its parent. The epic's approval replaces the issue's own."""
    out = []
    limit = rules["fix"]["max_lines"]
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
        allowed = listed(issue, names or {})
        if epic is None:
            out += issue_problems(mode.issue, issue)
        else:
            number, body = epic
            out += issue_problems(number, body)
            # A list item only, so that prose such as "Depends on #8" does not count, and #80 is not #8.
            if not re.search(rf"^[ \t]*[-*+][ \t]+(?:\[[ xX]\][ \t]+)?#{mode.issue}\b", visible(body), re.M):
                out.append(f"issue #{mode.issue} is not on the list of its epic #{number}")
            if issue_size(issue) == "L":
                out.append(f"issue #{mode.issue} is sized L, but a sub-issue is one pull request: size S or M")
            out += [f"issue #{mode.issue} lists {system} on its Systems line, and its epic #{number} does not"
                    for system in sorted(allowed - listed(body, names or {}))]
        out += [f"issue #{mode.issue} does not list {system} on its Systems line"
                for system in sorted(diff.touched - allowed)]
        out += [f"the change adds {a} -> {b}, so it changes docs/design/{a}.md too"
                for a, b in sorted(diff.new_edges) if a not in diff.docs]
        if issue_size(issue) is None:
            out.append(f"issue #{mode.issue} has no `**Size:** S|M|L` line")
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
