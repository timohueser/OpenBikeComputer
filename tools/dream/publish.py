#!/usr/bin/env python3
"""Publish the pages of the weekly dream job: the area pages in a pull request that merges on its
own, the other pages in one that waits for the owner, and a failed run in the dream issue.

The agent could edit each file of its working directory, this script too. So this script runs
from its own checkout, takes only the bytes of page files from the artifact, and puts no text of
the agent in the body of the pull request that merges without a review.

Usage: python3 tools/dream/publish.py --result FILE --pages DIR --since ISO --through ISO
"""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent)]
import check_paths  # noqa: E402
from collect import WINDOW, quote  # noqa: E402

BASE = "develop"
AREA_BRANCH = "dream/pages"
REVIEW_PREFIX = "dream/review-"
LABEL = "dream"
ISSUE_TITLE = "Weekly dream job"
RESULT_CHARS = 2000
# GitHub refuses a body of more than 65,536 characters; the result artifact holds the whole text.
MESSAGE_CHARS = 50_000
# Both ends of the window go into the body that merges with no review.
STAMP = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ")

Gh = Callable[..., str]


def gh(*args: str, stdin: str | None = None) -> str:
    return subprocess.run(["gh", *args], input=stdin, stdout=subprocess.PIPE, text=True, check=True).stdout


def git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), *args], stdout=subprocess.PIPE, text=True, check=True).stdout


def read_result(path: Path) -> dict | None:
    try:
        result = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return result if isinstance(result, dict) else None


def failure(result: dict | None, pages: Path, since: str, through: str) -> str:
    """Why nothing may be published, or an empty string."""
    if result is None:
        return "the result file is missing or is not a JSON object"
    # A failed run can have the subtype `success` with `is_error` set.
    if result.get("is_error") or result.get("subtype") != "success":
        return "the dream step reported an error"
    if not pages.is_dir():
        return "the pages are missing"
    if not STAMP.fullmatch(since) or not STAMP.fullmatch(through):
        return "the window is missing"
    return ""


def is_page_file(rel: str) -> bool:
    return (rel == "AGENTS.md" or os.path.basename(rel) == "README.md") and check_paths.is_page(rel)


def artifact_pages(pages: Path) -> dict[str, bytes]:
    """The regular page files of the artifact. `os.walk` does not enter a linked directory, and
    `lstat` does not follow a linked file."""
    found = {}
    for here, _, names in os.walk(pages):
        for name in names:
            path = Path(here, name)
            rel = path.relative_to(pages).as_posix()
            if is_page_file(rel) and stat.S_ISREG(path.lstat().st_mode):
                found[rel] = path.read_bytes()
    return found


def writable(root: Path, rel: str) -> bool:
    """True when `rel` reaches a file of the checkout through real directories only. The tree
    holds symlinks, and a linked directory can lead out of the checkout."""
    parent = os.path.dirname(rel)
    real = os.path.realpath(root / parent)
    dest = root / rel
    return (
        ".git" not in rel.split("/")
        and os.path.isdir(real)
        and real == os.path.normpath(os.path.join(os.path.realpath(root), parent))
        and (not os.path.lexists(dest) or stat.S_ISREG(os.lstat(dest).st_mode))
    )


def changes(root: Path, found: dict[str, bytes]) -> dict[str, tuple[bytes | None, bytes]]:
    """Each page whose bytes differ from the checkout: its text before and after."""
    changed = {}
    for rel, after in sorted(found.items()):
        if writable(root, rel):
            before = (root / rel).read_bytes() if (root / rel).exists() else None
            if before != after:
                changed[rel] = (before, after)
    return changed


def write(root: Path, rel: str, data: bytes | None) -> None:
    if data is None:
        (root / rel).unlink()
    else:
        (root / rel).write_bytes(data)


def lines(data: bytes | None) -> int:
    return len((data or b"").decode("utf-8", "replace").splitlines())


def table(files: dict[str, tuple[bytes | None, bytes]]) -> str:
    rows = [f"| `{f}` | {lines(before)} | {lines(after)} |" for f, (before, after) in files.items()]
    return "\n".join(["| Page | Lines before | Lines after |", "| --- | ---: | ---: |", *rows])


def cost_of(result: dict | None) -> str:
    cost = (result or {}).get("total_cost_usd")
    return f"${cost:.2f}" if isinstance(cost, int | float) else "unknown"


def ensure_label(gh: Gh) -> None:
    if LABEL not in gh("label", "list", "--search", LABEL, "--json", "name", "--jq", ".[].name").split():
        gh("label", "create", LABEL, "--description", "Pull requests and reports of the weekly dream job")


def to_issue(gh: Gh, body: str) -> str:
    """Comment on the dream issue, made when it is missing; returns its address."""
    ensure_label(gh)
    issues = json.loads(gh("issue", "list", "--label", LABEL, "--state", "open", "--json", "title,url", "--limit", "100"))
    url = next((i["url"] for i in issues if i["title"] == ISSUE_TITLE), "")
    if not url:
        about = "The weekly dream job reports its failed runs and its final messages here."
        url = gh("issue", "create", "--title", ISSUE_TITLE, "--label", LABEL, "--body", about).strip()
    gh("issue", "comment", url, "--body-file", "-", stdin=body)
    return url


def close_replaced(gh: Gh, since: str, proposals: bool, run_url: str) -> None:
    """Close what this run makes again: the area pull request that did not merge and, when this
    run has proposals, the proposals of a run with the same window start. Proposals of an
    earlier window stay: no later run reads that window again."""
    fields = "number,headRefName,isCrossRepository,body"
    for pr in json.loads(gh("pr", "list", "--state", "open", "--limit", "500", "--json", fields)):
        head = pr["headRefName"]
        window = WINDOW.search((pr["body"] or "").replace("\r\n", "\n"))
        same_window = proposals and head.startswith(REVIEW_PREFIX) and window and window[1] == since
        # A fork can use the same branch name.
        if not pr["isCrossRepository"] and (head == AREA_BRANCH or same_window):
            gh("pr", "close", str(pr["number"]), "--delete-branch", "--comment", f"The dream run {run_url} replaces this pull request.")


def commit_and_push(root: Path, base: str, branch: str, files: dict, identity: list[str], message: str) -> None:
    for f, (_, after) in files.items():
        write(root, f, after)
    git(root, "switch", "--quiet", "-C", branch)
    git(root, "add", "--", *files)
    git(root, *identity, "commit", "--quiet", "-m", message)
    git(root, "push", "--quiet", "--force", "origin", f"HEAD:refs/heads/{branch}")
    git(root, "switch", "--quiet", "--detach", base)


def open_pr(gh: Gh, branch: str, title: str, body: str) -> str:
    return gh("pr", "create", "--base", BASE, "--head", branch, "--title", title, "--label", LABEL, "--body-file", "-", stdin=body).strip()


def summary(text: str) -> None:
    print(text)
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a", encoding="utf-8") as out:
            out.write(text + "\n")


def publish(
    root: Path,
    result_path: Path,
    pages: Path,
    since: str,
    through: str,
    run_id: str,
    run_url: str,
    gh: Gh = gh,
    check: Callable[[], list[str]] = check_paths.check,
) -> int:
    """Publish one run from the checkout `root`; `check` runs the path check on that checkout."""
    result = read_result(result_path)
    cost = cost_of(result)
    if reason := failure(result, pages, since, through):
        text = str((result or {}).get("result") or "")
        subtype = (result or {}).get("subtype", "none")
        body = f"The dream run failed: {reason}. Nothing is published.\n\nCost: {cost}\nSubtype: {subtype}\nRun: {run_url}\n\n{quote(text, RESULT_CHARS)}"
        summary(f"Dream run failed: {reason}. Cost: {cost}. Reported in {to_issue(gh, body)}")
        return 1

    area_pages = {f"{a}/README.md" for a in check_paths.areas(git(root, "ls-files").splitlines())}
    changed = changes(root, artifact_pages(pages))
    area = {f: c for f, c in changed.items() if f in area_pages}
    for f, (_, after) in area.items():
        write(root, f, after)
    moved: list[str] = []
    while True:
        problems = check()
        failing = sorted({p.split(":", 1)[0] for p in problems} & area.keys())
        if not failing:
            break
        moved += [p for p in problems if p.split(":", 1)[0] in failing]
        for f in failing:
            write(root, f, area.pop(f)[0])
    if problems:
        body = (
            "The path check fails on problems that no area page of this run causes, so nothing "
            f"is published. The artifact of the run holds the pages.\n\nRun: {run_url}\n\n{quote(chr(10).join(problems), MESSAGE_CHARS)}"
        )
        summary(f"Dream run: the path check failed. Cost: {cost}. Reported in {to_issue(gh, body)}")
        return 1
    review = {f: c for f, c in changed.items() if f not in area}
    for f, (before, _) in area.items():
        write(root, f, before)

    ensure_label(gh)
    close_replaced(gh, since, bool(review), run_url)
    final = str(result.get("result") or "").strip()
    message = quote(final, MESSAGE_CHARS)
    window = f"Dream window: {since} to {through}"
    report = [f"Dream run: cost {cost}"]
    if area or review:
        user = json.loads(gh("api", "user"))
        identity = ["-c", f"user.name={user.get('name') or user['login']}",
                    "-c", f"user.email={user['id']}+{user['login']}@users.noreply.github.com"]
        gh("auth", "setup-git")
        base = git(root, "rev-parse", "HEAD").strip()
    if review:
        title = "memory: the proposals of the weekly update"
        branch = f"{REVIEW_PREFIX}{run_id}"
        commit_and_push(root, base, branch, review, identity, title)
        body = f"{window}\n\nCost of the run: {cost}\n\n{table(review)}\n\nRun: {run_url}\n"
        if moved:
            body += f"\nThese area pages fail the path check:\n\n{quote(chr(10).join(moved), MESSAGE_CHARS // 5)}"
        body += f"\nThe final message of the agent:\n\n{message}"
        report.append(f"Proposals: {open_pr(gh, branch, title, body)}")
    elif final:
        body = f"The final message of the dream run {run_url} (cost {cost}):\n\n{message}"
        report.append(f"Final message: {to_issue(gh, body)}")
    # Last: when this pull request merges, the next window starts after this one, so the rest
    # of this window must be out by then.
    if area:
        title = "memory: the weekly update of the area pages"
        commit_and_push(root, base, AREA_BRANCH, area, identity, title)
        body = f"{window}\n\nCost of the run: {cost}\n\n{table(area)}\n\nRun: {run_url}\n"
        url = open_pr(gh, AREA_BRANCH, title, body)
        gh("pr", "merge", url, "--auto", "--merge")
        report.append(f"Area pages: {url}")
    summary("\n\n".join(report))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--pages", type=Path, required=True)
    parser.add_argument("--since", required=True)
    parser.add_argument("--through", required=True)
    args = parser.parse_args()
    env = os.environ
    # Without GH_REPO, `gh` reads the repository from the remote of the current directory.
    env.setdefault("GH_REPO", env["GITHUB_REPOSITORY"])
    run_url = f"{env['GITHUB_SERVER_URL']}/{env['GITHUB_REPOSITORY']}/actions/runs/{env['GITHUB_RUN_ID']}"
    return publish(check_paths.ROOT, args.result, args.pages, args.since, args.through, env["GITHUB_RUN_ID"], run_url)


if __name__ == "__main__":
    sys.exit(main())
