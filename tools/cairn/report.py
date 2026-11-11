"""The weekly report: the skipped pull requests, the ratchet, and the approved system docs to check.

The `Mode:` line of a pull request body is a record: `change #N`, `fix` or `skip: <reason>`. Nothing
checks it, and the report does not count an unknown value.

The report reads git and GitHub and costs no tokens. For each approved doc, it lists the pull
requests merged since the doc last changed that touch the files of its system.
"""

from __future__ import annotations

import functools
import itertools
import json
import re
import subprocess
import sys
from collections import Counter
from collections.abc import Callable
from datetime import datetime, timedelta, timezone
from pathlib import Path

import cairn

LABEL = "cairn-report"
TITLE = "cairn report"
MERGE = re.compile(r"Merge pull request #([1-9]\d*)\b")
# GitHub refuses an issue body longer than 65,536 characters.
BODY_LIMIT = 65_000
PIN = "mutation($id: ID!) { pinIssue(input: {issueId: $id}) { issue { number } } }"
MODE = re.compile(r"change #(?P<issue>[1-9]\d*)|fix|skip: (?P<reason>\S.*)")
# GitHub hides an unclosed comment up to the end of the body.
COMMENT = re.compile(r"<!--.*?(?:-->|\Z)", re.S)
FENCE = re.compile(r"^```.*?^```", re.S | re.M)

#: `api(path, method="GET", payload=None)` on `repos/{owner}/{repo}/`.
Api = Callable[..., object]


def untrusted(text: str) -> str:
    """Text from a pull request, on one line. A zero-width space after each `@` keeps
    GitHub from notifying the user that it names, and `&lt;` keeps `<!--` from hiding the rest of the
    report."""
    return " ".join(str(text).split()).replace("@", "@\u200b").replace("<", "&lt;")


def visible(body: str) -> str:
    """The body without HTML comments and code blocks."""
    return FENCE.sub("", COMMENT.sub("", body))


def pull_mode(pull: dict) -> re.Match | None:
    """The value of the first `Mode:` line of a pull request body, or None when it is unknown."""
    for line in visible(pull.get("body") or "").splitlines():
        if line.startswith("Mode:"):
            return MODE.fullmatch(line[len("Mode:"):].strip())
    return None


def git(root: Path, *args: str) -> str | None:
    done = subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True)
    return done.stdout.strip() if done.returncode == 0 else None


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


def merged(api: Api, now: datetime) -> list[dict]:
    """The pull requests merged in the 7 days before `now`, by number. The list is newest update
    first, so it stops at a page that ends before the window."""
    since = now - timedelta(days=7)
    found = []
    for page in itertools.count(1):
        pulls = api(f"pulls?state=closed&sort=updated&direction=desc&per_page=100&page={page}")
        found += [pull for pull in pulls if pull.get("merged_at") and datetime.fromisoformat(pull["merged_at"]) >= since]
        if len(pulls) < 100 or datetime.fromisoformat(pulls[-1]["updated_at"]) < since:
            return sorted(found, key=lambda pull: pull["number"])


def skips(pulls: list[dict]) -> list[tuple[int, str, str]]:
    """The pull requests with `Mode: skip: <reason>`, as (number, title, reason)."""
    return [(pull["number"], pull["title"], mode["reason"]) for pull in pulls
            if (mode := pull_mode(pull)) and mode["reason"]]


def merges(root: Path, since: str) -> list[tuple[str, int]]:
    """The merge commits of pull requests after `since` on the first-parent line of HEAD, newest
    first, with their numbers."""
    found = []
    for line in history(root, "log", "--first-parent", "--format=%H %s", f"{since}..HEAD").splitlines():
        commit, _, subject = line.partition(" ")
        match = MERGE.match(subject)
        if match:
            found.append((commit, int(match.group(1))))
    return found


def history(root: Path, *args: str) -> str:
    """The output of a git command that the report needs. A failure must not read as "no change"."""
    out = git(root, *args)
    if out is None:
        raise OSError(f"git {' '.join(args[:2])} failed")
    return out


def docs_to_check(root: Path, docs: list[cairn.Doc], config: dict) -> list[tuple[str, list[int]]]:
    """For each approved doc, the pull requests merged since the doc last changed that touch the
    files of its system outside the `tests` globs and the map, newest first."""
    folder = config["docs"].rstrip("/")
    found = []
    for doc in docs:
        path = f"{folder}/{doc.system}.md"
        since = history(root, "log", "-1", "--first-parent", "--format=%H", "HEAD", "--", path) if doc.status == "approved" else None
        numbers = []
        for commit, number in merges(root, since) if since else []:
            changed = history(root, "diff", "--name-only", "--no-renames", "-z", f"{commit}^1", commit).split("\0")
            kept = [f for f in changed if f and f not in (f"{folder}/{cairn.MAP}", f"{folder}/{cairn.MAP_DATA}")
                    and not cairn.is_test(f, config["tests"])]
            if any(doc.system in hits for hits in cairn.owners(kept, docs).values()):
                numbers.append(number)
        if numbers:
            found.append((doc.system, numbers))
    return found


# ── the report ───────────────────────────────────────────────────────────────


def write(root: Path, config: dict, docs: list[cairn.Doc], result: cairn.Result, baseline: set[str],
          api: Api, now: datetime) -> tuple[str, bool]:
    """The report, and whether a part of it failed."""
    failed = False
    lines = ["# cairn report", "", f"Written on {now:%Y-%m-%d} at {git(root, 'rev-parse', '--short', 'HEAD')}.",
             "", "## Skips merged in the last 7 days", ""]
    try:
        rows = [f"| #{n} | {untrusted(t).replace('|', '/')} | {untrusted(r).replace('|', '/')} |" for n, t, r in skips(merged(api, now))]
        lines += ["| Pull request | Title | Reason |", "| --- | --- | --- |", *rows] if rows else ["None."]
    except (OSError, ValueError, KeyError, TypeError) as error:
        failed = True
        lines.append(f"The pull requests cannot be read: {error}")

    counts = Counter(system for v in result.violations for system in v.systems)
    lines += ["", "## Ratchet", "", f"{len(result.violations)} violations; the baseline holds {len(baseline)}.", "",
              "| System | Violations |", "| --- | --- |",
              *(f"| {system} | {n} |" for system, n in sorted(counts.items(), key=lambda pair: (-pair[1], pair[0])))]
    unowned = sorted(v.id.removeprefix("unowned ") for v in result.violations if v.id.startswith("unowned "))
    lines += ["", "## Files with no owner", "", *([f"- `{path}`" for path in unowned] or ["None."])]

    lines += ["", "## System docs to check", "",
              "The pull requests merged since each approved doc last changed that touch its system.", ""]
    try:
        found = docs_to_check(root, docs, config)
    except (OSError, ValueError, KeyError, TypeError) as error:
        return "\n".join([*lines, f"The history cannot be read: {error}"]), True
    lines += [f"- {system}: {', '.join(f'#{n}' for n in numbers)}" for system, numbers in found]
    if not found:
        approved = sum(doc.status == "approved" for doc in docs)
        lines.append(f"{approved} approved doc(s); no pull request touched their systems since they changed.")
    return "\n".join(lines), failed


def publish(body: str, api: Api, pin: Callable[[str], object]) -> str:
    """Replace the body of the open issue labelled cairn-report, or create that issue and pin it.
    A failed pin only adds a note to the body. Gives the URL of the issue."""
    if len(body) > BODY_LIMIT:
        body = body[:BODY_LIMIT] + "\n\nThe report is cut here, at the size limit of an issue body."
    issues = [issue for issue in api(f"issues?labels={LABEL}&state=open") if "pull_request" not in issue]
    if issues:
        return api(f"issues/{issues[0]['number']}", "PATCH", {"body": body})["html_url"]
    try:
        api(f"labels/{LABEL}")
    except OSError as error:
        if "Not Found" not in str(error):
            raise
        api("labels", "POST", {"name": LABEL, "color": "5319e7", "description": "The weekly cairn report"})
    issue = api("issues", "POST", {"title": TITLE, "body": body, "labels": [LABEL]})
    try:
        pin(issue["node_id"])
    except OSError as error:
        api(f"issues/{issue['number']}", "PATCH", {"body": f"{body}\n\nThe issue could not be pinned: {error}"})
    return issue["html_url"]


def main(publishing: bool) -> int:
    root = cairn.ROOT
    try:
        config = json.loads((root / cairn.CONFIG).read_text())
        docs, files, units, edges, lines = cairn.gather(root, config)
        result = cairn.analyse(docs, files, units, edges, config)
        path = root / config["baseline"]
        baseline = set(json.loads(path.read_text())) if path.exists() else set()
        read = functools.partial(api, root)
        text, failed = write(root, config, docs, result, baseline, read, datetime.now(timezone.utc))
        print(text)
        if publishing:
            def pin(node: str) -> None:
                answer = gh(root, "graphql", "POST", {"query": PIN, "variables": {"id": node}})
                if (answer or {}).get("errors"):
                    raise OSError(json.dumps(answer["errors"]))

            print(f"cairn: wrote {publish(text, read, pin)}", file=sys.stderr)
    except (cairn.CairnError, OSError, RuntimeError, KeyError, ValueError, subprocess.CalledProcessError) as error:
        print(f"cairn: {error}", file=sys.stderr)
        return 1
    return 1 if failed else 0
