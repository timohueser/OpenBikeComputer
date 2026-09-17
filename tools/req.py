#!/usr/bin/env python3
"""Print one system requirement from the verification console with its numbered criteria.

    obc req SYS-003          # statement, coverage state, criteria 1..n with evidence and gaps
    obc req SYS-003 --json   # the raw requirement record

Reads the agent token from ~/.config/openbikecomputer/verification-agent.token
(override with OBC_VERIFICATION_TOKEN_FILE) and the console at
https://releases.openbikecomputer.com (override with OBC_VERIFICATION_URL).
"Criterion 2 of SYS-003" means the second criterion in this listing.
"""
from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_URL = "https://releases.openbikecomputer.com"
DEFAULT_TOKEN = Path.home() / ".config/openbikecomputer/verification-agent.token"


def fetch(url: str, path: str, token: str) -> dict | list:
    request = urllib.request.Request(f"{url}{path}", headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def evidence_test(requirement: dict, evidence: dict) -> dict | None:
    """The linked test a piece of evidence names; the same rule as `evidenceTest` in src/lib/coverage.ts."""
    for test in requirement["tests"]:
        if evidence.get("caseId"):
            if test["kind"] == "automated" and test.get("caseId") == evidence["caseId"]:
                return test
        elif test["kind"] == "manual" and test["id"] == evidence.get("testId"):
            return test
    return None


def criterion_covered(requirement: dict, criterion: dict) -> bool:
    return bool(criterion["evidence"]) and not criterion["gap"].strip() and all(
        evidence_test(requirement, e) is not None for e in criterion["evidence"])


def state(requirement: dict) -> str:
    plan = requirement.get("coverage")
    if not plan:
        return "Not assessed"
    covered = sum(criterion_covered(requirement, c) for c in plan["criteria"])
    total = len(plan["criteria"])
    label = "Needs review" if not plan.get("review") else "Covered" if total and covered == total else "Partial"
    return f"{label} {covered}/{total}"


def labels(requirement: dict) -> list[str]:
    return [name for flag, name in (("todo", "definition incomplete"), ("implementationNeeded", "implementation needed"))
            if requirement.get(flag)] + ([] if requirement.get("active") else ["excluded from releases"])


def render(requirement: dict, revision: dict, proposals: list[dict]) -> str:
    head = [requirement["id"], requirement.get("group") or "Ungrouped", state(requirement), *labels(requirement), f"r{revision['id']}"]
    lines = [" · ".join(head), requirement["title"], "", requirement["statement"].strip(), ""]
    plan = requirement.get("coverage")
    if not plan:
        lines.append("No coverage plan.")
        return "\n".join(lines)
    review = plan.get("review")
    header = "Coverage"
    if review:
        header += f" — approved by {review['author']}, {review['createdAt'][:10]}"
        if review.get("sourceSha"):
            header += f", commit {review['sourceSha'][:10]}"
    lines += [header, plan["rationale"].strip(), ""]
    for number, criterion in enumerate(plan["criteria"], 1):
        mark = "✓" if criterion_covered(requirement, criterion) else "○"
        lines.append(f"{number} {mark} {criterion['statement'].strip()}")
        for evidence in criterion["evidence"]:
            test = evidence_test(requirement, evidence)
            title = f"{test['title']} [{test.get('caseId') or 'manual'}]" if test else f"unlinked [{evidence.get('caseId') or evidence.get('testId')}]"
            lines.append(f"    {title} — {evidence['rationale'].strip()}")
        if not criterion["evidence"]:
            lines.append("    no evidence")
        if criterion["gap"].strip():
            lines.append(f"    gap: {criterion['gap'].strip()}")
        if criterion.get("next"):
            lines.append(f"    next [{criterion['next']['level']}]: {criterion['next']['summary'].strip()}")
    for proposal in (p for p in proposals if p["requirementId"] == requirement["id"] and p["status"] == "pending"):
        note = proposal.get("conflict") or proposal.get("stale") or "read it with GET /api/coverage-proposals."
        lines += ["", f"Pending proposal by {proposal['author']} ({proposal['createdAt'][:10]}, commit {proposal['sourceSha'][:10]}): "
                      f"{len(proposal['plan']['criteria'])} criteria; {note}"]
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    args = [a for a in argv if not a.startswith("--")]
    if len(args) != 1:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    wanted = args[0].upper()
    token_file = Path(os.environ.get("OBC_VERIFICATION_TOKEN_FILE", DEFAULT_TOKEN))
    if not token_file.exists():
        print(f"obc req: no agent token at {token_file}. Create one in the console under Account → Agent access.", file=sys.stderr)
        return 1
    url = os.environ.get("OBC_VERIFICATION_URL", DEFAULT_URL).rstrip("/")
    token = token_file.read_text().strip()
    try:
        data = fetch(url, "/api/bootstrap", token)
        proposals = [] if "--json" in argv else fetch(url, "/api/coverage-proposals", token)
    except urllib.error.HTTPError as error:
        print(f"obc req: {url} answered HTTP {error.code}. An expired or revoked token gives 401.", file=sys.stderr)
        return 1
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
        print(f"obc req: could not read {url}: {error}", file=sys.stderr)
        return 1
    revision = data["revision"]
    requirement = next((r for r in revision["requirements"] if r["id"] == wanted), None)
    if requirement is None:
        print(f"obc req: {wanted} is not in revision r{revision['id']}.", file=sys.stderr)
        return 1
    if "--json" in argv:
        print(json.dumps(requirement, indent=2, ensure_ascii=False))
    else:
        print(render(requirement, revision, proposals))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
