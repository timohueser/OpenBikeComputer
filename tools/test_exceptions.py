#!/usr/bin/env python3
"""Check the `sleep_exception` blocks declared in `testing/suites.toml`.

A suite that waits on real time declares one, with a reason and an open GitHub issue,
so the exception expires with the issue. This validates each block offline, then asks
GitHub once per distinct issue whether it is still open.

Usage: python3 tools/test_exceptions.py --repo OWNER/REPO
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any, Sequence

EXCEPTION_FIELDS = ("sleep_exception",)
REPOSITORY_RE = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
ISSUE_RE = re.compile(r"^(?:#\d+|https://github\.com/[^/]+/[^/]+/issues/\d+)$")


def read_suites(root: Path) -> list[dict[str, Any]]:
    with (root / "testing/suites.toml").open("rb") as handle:
        return tomllib.load(handle)["suite"]


def block_errors(owner: str, block: Any) -> list[str]:
    """The offline shape rule, shared with `test_plan.py check`."""

    if not isinstance(block, dict):
        return [f"{owner}: must be a table"]
    errors: list[str] = []
    reason = block.get("reason", "")
    reference = block.get("issue", "")
    if not isinstance(reason, str) or not reason.strip():
        errors.append(f"{owner}: requires a reason")
    if not isinstance(reference, str) or not ISSUE_RE.fullmatch(reference):
        errors.append(f"{owner}: requires a GitHub issue reference")
    return errors


def collect_references(
    suites: Sequence[dict[str, Any]], repository: str
) -> tuple[dict[tuple[str, int], list[str]], list[str]]:
    """Map each distinct issue to the suite fields that claim it, rejecting bad blocks."""

    references: dict[tuple[str, int], list[str]] = {}
    errors: list[str] = []
    for suite in suites:
        for field in EXCEPTION_FIELDS:
            if field not in suite:
                continue
            owner = f"{suite['id']}.{field}"
            block = suite[field]
            shape = block_errors(owner, block)
            errors.extend(shape)
            if shape:
                continue
            reference = block["issue"]
            if reference.startswith("#"):
                repo, number = repository, reference[1:]
            else:
                repo, number = reference.removeprefix("https://github.com/").rsplit("/issues/", 1)
            if not REPOSITORY_RE.fullmatch(repo):
                errors.append(f"{owner}: has an invalid issue repository")
                continue
            references.setdefault((repo.lower(), int(number)), []).append(owner)
    return references, errors


def issue_error(repo: str, number: int, label: str) -> str | None:
    """One bounded request: report anything that is not an open issue."""

    try:
        result = subprocess.run(
            ["gh", "api", f"repos/{repo}/issues/{number}"],
            check=True, capture_output=True, text=True, timeout=20,
        )
        issue = json.loads(result.stdout)
    except subprocess.CalledProcessError as exc:
        return f"{label}: GitHub API failed: {(exc.stderr or str(exc)).strip()}"
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError) as exc:
        return f"{label}: could not check issue: {exc}"
    if not isinstance(issue, dict) or issue.get("state") not in ("open", "closed"):
        return f"{label}: API returned an invalid issue response"
    if "pull_request" in issue:
        return f"{label}: references a pull request, not an issue"
    if issue["state"] != "open":
        return f"{label}: issue is closed"
    return None


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", required=True, help="OWNER/REPO for local #issue references")
    parser.add_argument("--root", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    root = (args.root or Path(__file__).resolve().parents[1]).resolve()
    if not REPOSITORY_RE.fullmatch(args.repo):
        print("test exceptions: --repo must be OWNER/REPO", file=sys.stderr)
        return 1

    references, errors = collect_references(read_suites(root), args.repo)
    if not errors:
        errors = [
            error
            for (repo, number), owners in sorted(references.items())
            if (error := issue_error(repo, number, f"{', '.join(owners)}: {repo}#{number}"))
        ]
    if errors:
        print("test exception check failed:", file=sys.stderr)
        print("\n".join(f"- {error}" for error in errors), file=sys.stderr)
        return 1
    print(f"exception issue state OK: {len(references)} distinct open issues")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
