#!/usr/bin/env python3
"""`obc ci` — the failures of one CI run, and nothing else.

The raw log of one red job is about a megabyte, nearly all of it passing tests, compiler
progress, runner timestamps and colour codes. A reader needs the failed step's name and the
lines just before its `##[error]` marker. That is what this prints, per failed job, from the
latest `ci.yml` run of the current branch unless a run id, a pull request or a branch is named.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from typing import Iterable, Sequence

WORKFLOW = "ci.yml"

#: The runner prefixes every line with an ISO timestamp; tools colour their output.
TIMESTAMP = re.compile(r"^\d{4}-\d\d-\d\dT[\d:.]+Z ")
ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
ERROR = "##[error]"

#: Progress lines: they never explain a failure, and they are most of the log.
NOISE = re.compile(
    r"^\s*("
    r"PASS \[|SKIP \[|Compiling |Checking |Downloaded |Downloading |Fresh |Blocking |"
    r"Generated XML report:|##\[endgroup\]|##\[debug\]"
    r")"
)


def clean(line: str) -> str:
    return ANSI.sub("", TIMESTAMP.sub("", line.rstrip("\r\n")))


def excerpts(log: str, lines: int) -> list[list[str]]:
    """The kept lines before each error marker, as windows; overlapping windows are merged."""
    kept = [line for line in map(clean, log.splitlines()) if not NOISE.match(line)]
    marks = [index for index, line in enumerate(kept) if line.startswith(ERROR)]
    windows: list[list[int]] = []
    for mark in marks:
        start = max(0, mark - lines)
        if windows and start <= windows[-1][1] + 1:
            windows[-1][1] = mark
        else:
            windows.append([start, mark])
    return [kept[start : end + 1] for start, end in windows]


def gh(*arguments: str) -> str:
    result = subprocess.run(["gh", *arguments], capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"obc ci: gh {' '.join(arguments[:2])} failed: {result.stderr.strip()}")
    return result.stdout


def current_branch() -> str:
    return subprocess.run(
        ["git", "branch", "--show-current"], capture_output=True, text=True, check=True
    ).stdout.strip()


def latest_run(branch: str) -> dict:
    fields = "databaseId,status,conclusion,url,displayTitle"
    runs = json.loads(gh("run", "list", "--workflow", WORKFLOW, "--branch", branch, "--limit", "1", "--json", fields))
    if not runs:
        sys.exit(f"obc ci: no {WORKFLOW} run for branch {branch}")
    return runs[0]


def run_by_id(run_id: str) -> dict:
    fields = "databaseId,status,conclusion,url,displayTitle"
    return json.loads(gh("run", "view", run_id, "--json", fields))


def failed_jobs(run_id: int) -> list[dict]:
    payload = json.loads(gh("api", f"repos/{{owner}}/{{repo}}/actions/runs/{run_id}/jobs?per_page=100"))
    return [job for job in payload["jobs"] if job["conclusion"] == "failure"]


def job_log(job_id: int) -> str:
    return gh("api", f"repos/{{owner}}/{{repo}}/actions/jobs/{job_id}/logs")


def render(run: dict, jobs: Sequence[dict], lines: int) -> Iterable[str]:
    state = run["conclusion"] or run["status"]
    yield f"run {run['databaseId']} {state} — {run['displayTitle']}"
    yield run["url"]
    if not jobs:
        yield "no failed job"
        return
    yield "failed jobs: " + ", ".join(job["name"] for job in jobs)
    for job in jobs:
        steps = [step["name"] for step in job["steps"] if step["conclusion"] == "failure"]
        yield ""
        yield f"── {job['name']} / {', '.join(steps) or 'no failed step'} ──"
        windows = excerpts(job_log(job["id"]), lines)
        if not windows:
            yield "(no error marker in the log)"
        for index, window in enumerate(windows):
            if index:
                yield "…"
            yield from window


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="obc ci", description=__doc__.splitlines()[0])
    parser.add_argument("run", nargs="?", help="a run id; default: the latest run of the branch")
    parser.add_argument("--pr", help="the latest run of this pull request's branch")
    parser.add_argument("--branch", help="the latest run of this branch")
    parser.add_argument("--lines", type=int, default=40, help="kept lines before each error marker")
    args = parser.parse_args(argv)
    if args.lines < 0:
        parser.error("--lines must be zero or greater")

    if args.run:
        run = run_by_id(args.run)
    else:
        branch = args.branch or current_branch()
        if args.pr:
            branch = json.loads(gh("pr", "view", args.pr, "--json", "headRefName"))["headRefName"]
        run = latest_run(branch)
    jobs = failed_jobs(run["databaseId"]) if run["status"] == "completed" else []
    for line in render(run, jobs, args.lines):
        print(line)
    if run["status"] != "completed":
        return 3
    return 1 if run["conclusion"] != "success" else 0


if __name__ == "__main__":
    sys.exit(main())
