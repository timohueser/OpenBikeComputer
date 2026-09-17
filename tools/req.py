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


def fetch(url: str, token: str) -> dict:
    request = urllib.request.Request(f"{url}/api/bootstrap", headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def evidence_title(requirement: dict, evidence: dict, catalog: dict) -> str:
    for test in requirement["tests"]:
        if evidence.get("caseId") and test.get("caseId") == evidence["caseId"]:
            return f"{test['title']} [{test['caseId']}]"
        if evidence.get("testId") and test["id"] == evidence["testId"]:
            return f"{test['title']} [manual]"
    for case in catalog.get("cases", []):
        if case["id"] == evidence.get("caseId"):
            return f"{case['name']} [{case['id']}] (not linked yet)"
    return evidence.get("caseId") or evidence.get("testId") or "?"


def criterion_covered(requirement: dict, criterion: dict) -> bool:
    linked = {t.get("caseId") for t in requirement["tests"]} | {t["id"] for t in requirement["tests"]}
    return bool(criterion["evidence"]) and not criterion["gap"].strip() and all(
        (e.get("caseId") or e.get("testId")) in linked for e in criterion["evidence"])


def state(requirement: dict) -> str:
    plan = requirement.get("coverage")
    if not plan:
        return "Not assessed"
    covered = sum(criterion_covered(requirement, c) for c in plan["criteria"])
    total = len(plan["criteria"])
    label = "Needs review" if not plan.get("review") else "Covered" if covered == total else "Partial"
    return f"{label} {covered}/{total}"


def render(requirement: dict, revision: dict, catalog: dict, proposals: list[dict]) -> str:
    lines = [f"{requirement['id']} · {requirement.get('group') or 'Ungrouped'} · {state(requirement)} · r{revision['id']}",
             requirement["title"], "", requirement["statement"].strip(), ""]
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
            lines.append(f"    {evidence_title(requirement, evidence, catalog)} — {evidence['rationale'].strip()}")
        if not criterion["evidence"]:
            lines.append("    no evidence")
        if criterion["gap"].strip():
            lines.append(f"    gap: {criterion['gap'].strip()}")
    pending = [p for p in proposals if p["requirementId"] == requirement["id"] and p["status"] == "pending"]
    for proposal in pending:
        lines += ["", f"Pending proposal by {proposal['author']} ({proposal['createdAt'][:10]}, commit {proposal['sourceSha'][:10]}): "
                      f"{len(proposal['plan']['criteria'])} criteria; read it with GET /api/coverage-proposals."]
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
        data = fetch(url, token)
        proposals = json.load(urllib.request.urlopen(urllib.request.Request(f"{url}/api/coverage-proposals", headers={"Authorization": f"Bearer {token}"}), timeout=30))
    except urllib.error.HTTPError as error:
        print(f"obc req: {url} answered HTTP {error.code}. An expired or revoked token gives 401.", file=sys.stderr)
        return 1
    revision = data["revision"]
    requirement = next((r for r in revision["requirements"] if r["id"] == wanted), None)
    if requirement is None:
        print(f"obc req: {wanted} is not in revision r{revision['id']}.", file=sys.stderr)
        return 1
    if "--json" in argv:
        print(json.dumps(requirement, indent=2, ensure_ascii=False))
    else:
        print(render(requirement, revision, data["catalog"], proposals))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
