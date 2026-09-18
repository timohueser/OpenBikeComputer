#!/usr/bin/env python3
"""Read and write the verification console from a terminal or an agent session.

    obc req SYS-003                    statement, coverage state, criteria 1..n, pending proposal
    obc req SYS-003 --json             the raw requirement record
    obc req list                       every requirement, one line each
    obc req list --gaps                only those with an open gap
    obc req list --no-plan             only those with no approved plan
    obc req list --pending             only those with a proposal waiting
    obc req list --flagged             only pending proposals the console warns about
    obc req proposal SYS-003           the pending proposal in full
    obc req tests orientation          search the CI catalogue
    obc req changed                    what changed in the newest revision
    obc req changed --since 16         what changed since revision 16
    obc req propose plan.json [...]    validate the plans, then submit them
    obc req propose plan.json --check  validate only, submit nothing

A plan file holds one object, or a list of them:

    {"requirementId": "SYS-003", "plan": {"rationale": ..., "criteria": [...]}, "procedures": []}

which is what `POST /api/coverage-proposals` takes, minus `baseRevision` and `sourceSha`. Those are
filled in here from the console and from `git rev-parse HEAD`, so a proposal always names the exact
commit it was written against. Pass `--sha=<40 chars>` to override.

Reads the agent token from ~/.config/openbikecomputer/verification-agent.token (override with
OBC_VERIFICATION_TOKEN_FILE) and the console at https://releases.openbikecomputer.com (override
with OBC_VERIFICATION_URL). "Criterion 2 of SYS-003" means the second criterion in this listing.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_URL = "https://releases.openbikecomputer.com"
DEFAULT_TOKEN = Path.home() / ".config/openbikecomputer/verification-agent.token"
LEVELS = ("unit", "integration", "system")
"""Requirement fields the console treats as part of the requirement itself, for `changed`."""
TRACKED = ("title", "statement", "group", "todo", "implementationNeeded", "active")


class Problem(Exception):
    """Anything the caller should see as one line rather than a traceback."""


class Console:
    """The console's HTTP surface, with the token read once."""

    def __init__(self) -> None:
        token_file = Path(os.environ.get("OBC_VERIFICATION_TOKEN_FILE", DEFAULT_TOKEN))
        if not token_file.exists():
            raise Problem(f"no agent token at {token_file}. Create one in the console under Account → Agent access.")
        self.token = token_file.read_text().strip()
        self.url = os.environ.get("OBC_VERIFICATION_URL", DEFAULT_URL).rstrip("/")

    def call(self, path: str, body: dict | None = None):
        request = urllib.request.Request(
            f"{self.url}{path}",
            headers={"Authorization": f"Bearer {self.token}", "content-type": "application/json"},
            data=json.dumps(body).encode() if body is not None else None,
            method="POST" if body is not None else "GET",
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors="replace")[:300]
            hint = " An expired or revoked token gives 401." if error.code == 401 else ""
            raise Problem(f"{self.url} answered HTTP {error.code}.{hint} {detail}") from None
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            raise Problem(f"could not read {self.url}: {error}") from None


# ───────────────────────────────── reading ─────────────────────────────────


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


def pending_for(proposals: list[dict], requirement_id: str) -> dict | None:
    return next((p for p in proposals if p["requirementId"] == requirement_id and p["status"] == "pending"), None)


def coverage_header(plan: dict) -> str:
    review = plan.get("review")
    if not review:
        return "Coverage — not approved"
    header = f"Coverage — approved by {review['author']}, {review['createdAt'][:10]}"
    if review.get("sourceSha"):
        header += f", commit {review['sourceSha'][:10]}"
    return header


def evidence_name(requirement: dict, evidence: dict, known: set[str] | None) -> str:
    """What to call a piece of evidence in a terminal.

    A case id is the citable thing and already carries the suite, the file and the test name, so
    printing the catalogue's title beside it only repeats it at length. A manual procedure has no
    id worth reading, so it goes by its title.
    """
    if evidence.get("caseId"):
        gone = known is not None and evidence["caseId"] not in known
        return evidence["caseId"] + ("   (not in the catalogue)" if gone else "")
    test = evidence_test(requirement, evidence)
    return f"manual: {test['title']}" if test else f"unlinked manual test {evidence.get('testId')}"


def criteria_lines(requirement: dict, plan: dict, known: set[str] | None = None, proposed: bool = False) -> list[str]:
    """`proposed` judges a criterion on the plan's own terms, the way the console does while a
    proposal is still pending: its evidence is linked only once the plan is approved."""
    lines: list[str] = []
    for number, criterion in enumerate(plan["criteria"], 1):
        covered = bool(criterion["evidence"]) and not criterion["gap"].strip() if proposed \
            else criterion_covered(requirement, criterion)
        lines.append(f"{number} {'✓' if covered else '○'} {criterion['statement'].strip()}")
        for evidence in criterion["evidence"]:
            lines.append(f"    {evidence_name(requirement, evidence, known)}")
            lines.append(f"        {evidence['rationale'].strip()}")
        if not criterion["evidence"]:
            lines.append("    no evidence")
        if criterion["gap"].strip():
            lines.append(f"    gap: {criterion['gap'].strip()}")
        if criterion.get("next"):
            lines.append(f"    next [{criterion['next']['level']}]: {criterion['next']['summary'].strip()}")
    return lines


def proposal_note(proposal: dict) -> str:
    count = len(proposal["plan"]["criteria"])
    note = proposal.get("conflict") or proposal.get("stale") \
        or f"read it with: obc req proposal {proposal['requirementId']}"
    return (f"Pending proposal by {proposal['author']} ({proposal['createdAt'][:10]}, "
            f"commit {proposal['sourceSha'][:10]}): {count} {'criterion' if count == 1 else 'criteria'}; {note}")


def render(requirement: dict, revision: dict, proposals: list[dict]) -> str:
    head = [requirement["id"], requirement.get("group") or "Ungrouped", state(requirement), *labels(requirement),
            f"r{revision['id']}"]
    lines = [" · ".join(head), requirement["title"], "", requirement["statement"].strip(), ""]
    plan = requirement.get("coverage")
    if not plan:
        lines.append("No approved coverage plan.")
    else:
        lines += [coverage_header(plan), plan["rationale"].strip(), ""]
        lines += criteria_lines(requirement, plan)
    proposal = pending_for(proposals, requirement["id"])
    if proposal:
        lines += ["", proposal_note(proposal)]
    return "\n".join(lines)


def render_proposal(proposal: dict, requirement: dict, known: set[str] | None = None) -> str:
    """The pending plan in full. Evidence resolves against the requirement, the plan's own new
    procedures, and the catalogue, because a proposal may cite a case the requirement does not
    carry yet: approval is what links it."""
    procedures = proposal.get("procedures") or []
    carrier = {**requirement, "tests": requirement["tests"] + [{**p, "kind": "manual"} for p in procedures]}
    lines = [f"{proposal['requirementId']} · proposed by {proposal['author']} · {proposal['createdAt'][:10]} · "
             f"commit {proposal['sourceSha'][:10]} · against r{proposal['baseRevision']}"]
    for flag in ("conflict", "stale"):
        if proposal.get(flag):
            lines.append(f"{flag.upper()}: {proposal[flag]}")
    lines += ["", proposal["plan"]["rationale"].strip(), ""]
    lines += criteria_lines(carrier, proposal["plan"], known, proposed=True)
    for procedure in procedures:
        lines += ["", f"New manual procedure [{procedure['id']}]: {procedure['title']}",
                  f"    steps: {' '.join(procedure['steps'].split())}",
                  f"    expected: {' '.join(procedure['expected'].split())}"]
    return "\n".join(lines)


def list_lines(revision: dict, proposals: list[dict], only: set[str]) -> list[str]:
    lines = []
    for requirement in revision["requirements"]:
        plan = requirement.get("coverage")
        gaps = sum(1 for c in (plan or {}).get("criteria", []) if c["gap"].strip())
        proposal = pending_for(proposals, requirement["id"])
        flagged = bool(proposal and (proposal.get("conflict") or proposal.get("stale")))
        if ("gaps" in only and not gaps) or ("no-plan" in only and plan):
            continue
        if ("pending" in only and not proposal) or ("flagged" in only and not flagged):
            continue
        marks = [state(requirement)]
        if gaps:
            marks.append(f"{gaps} {'gap' if gaps == 1 else 'gaps'}")
        if proposal:
            marks.append("flagged proposal" if flagged else "proposal")
        marks += labels(requirement)
        lines.append(f"{requirement['id']}  {requirement['title'][:44]:<44}  {' · '.join(marks)}")
    return lines


def _value(requirement: dict, field: str):
    """A missing flag and a false flag are the same requirement, and so are a missing and an empty group."""
    return requirement.get(field) or ("" if field in ("title", "statement", "group") else False)


def changed_lines(before: dict, after: dict) -> list[str]:
    """Added, removed and changed requirements between two revisions, for someone picking up the delta."""
    was = {r["id"]: r for r in before["requirements"]}
    now = {r["id"]: r for r in after["requirements"]}
    lines = [f"r{before['id']} → r{after['id']}"]
    coverage_only: list[str] = []
    for rid, requirement in now.items():
        if rid not in was:
            lines += ["", f"ADDED {rid} · {requirement['title']}", f"  {' '.join(requirement['statement'].split())}"]
            if labels(requirement):
                lines.append(f"  labels: {', '.join(labels(requirement))}")
    for rid, requirement in was.items():
        if rid not in now:
            lines += ["", f"REMOVED {rid} · {requirement['title']}"]
    for rid, requirement in now.items():
        old = was.get(rid)
        if not old:
            continue
        fields = [f for f in TRACKED if _value(old, f) != _value(requirement, f)]
        if not fields:
            if json.dumps(old.get("coverage"), sort_keys=True) != json.dumps(requirement.get("coverage"), sort_keys=True):
                coverage_only.append(rid)
            continue
        lines += ["", f"CHANGED {rid} · {requirement['title']} ({', '.join(fields)})"]
        if "statement" in fields:
            lines += [f"  was: {' '.join(old['statement'].split())}",
                      f"  now: {' '.join(requirement['statement'].split())}"]
        if "title" in fields:
            lines.append(f"  title was: {old['title']}")
        for flag in ("todo", "implementationNeeded", "active"):
            if flag in fields:
                lines.append(f"  {flag}: {bool(_value(old, flag))} → {bool(_value(requirement, flag))}")
    if len(lines) == 1 and not coverage_only:
        lines.append("No requirement changed.")
    if coverage_only:
        lines += ["", f"Coverage only, prose unchanged: {', '.join(coverage_only)}"]
    return lines


# ───────────────────────────────── writing ─────────────────────────────────


def plan_problems(entry: dict, requirement: dict, baseline: dict | None,
                  catalog_ids: set[str]) -> tuple[list[str], list[str]]:
    """What is wrong with one plan, split into what blocks it and what is worth saying.

    Errors are what the server refuses, plus what is plainly broken; one anywhere in a batch stops
    the whole batch, so a half-submitted run cannot happen. Warnings are judgement calls a reviewer
    might raise and an author may have meant: a criterion that is new, a gap with no test named yet.
    Those print and do not block, because adding a criterion is the usual reason to revise a plan.
    """
    rid = entry.get("requirementId", "?")
    found: list[str] = []
    notes: list[str] = []
    say = found.append
    note = notes.append
    plan = entry.get("plan") or {}
    procedures = entry.get("procedures") or []
    if not as_text(plan.get("rationale")).strip():
        say(f"{rid}: the plan has no rationale")
    if len(as_text(plan.get("rationale"))) > 900:
        note(f"{rid}: the rationale is {len(plan['rationale'])} characters, which reads long on the card")
    if not plan.get("criteria"):
        say(f"{rid}: the plan has no criteria")
        return found, notes
    if len(plan["criteria"]) > 100:
        say(f"{rid}: {len(plan['criteria'])} criteria; the server takes at most 100")
    if len(procedures) > 20:
        say(f"{rid}: {len(procedures)} procedures; the server takes at most 20")
    now = [c.get("id") for c in plan["criteria"]]
    for repeated in sorted({i for i in now if now.count(i) > 1}):
        say(f"{rid}: criterion id {repeated} is used more than once")
    if baseline:
        was = [c["id"] for c in baseline["criteria"]]
        for lost in sorted(set(was) - set(now)):
            note(f"{rid}: criterion {lost} was in the plan being revised and is not in this one")
        for gained in sorted(set(now) - set(was)):
            note(f"{rid}: criterion {gained} is new")
    linked_cases = {t.get("caseId") for t in requirement["tests"] if t["kind"] == "automated"}
    existing = {t["id"] for t in requirement["tests"]}
    cited_keys: set[tuple[str, str]] = set()
    proposed = {p.get("id") for p in procedures}
    cited: set[str] = set()
    for criterion in plan["criteria"]:
        cid = criterion.get("id", "?")
        if not identifier_ok(cid):
            say(f"{rid}: criterion id {cid!r} is not one the server accepts")
        if not as_text(criterion.get("statement")).strip():
            say(f"{rid}: criterion {cid} has no statement")
        # The server insists both fields are present and of the right type. A generator that emits
        # `null` for "nothing here" is the common way to trip this.
        if not isinstance(criterion.get("gap"), str):
            say(f"{rid}: criterion {cid} needs a gap string; use \"\" when there is none")
        if not isinstance(criterion.get("evidence"), list):
            say(f"{rid}: criterion {cid} needs an evidence list; use [] when there is none")
        if len(criterion.get("evidence") or []) > 100:
            say(f"{rid}: criterion {cid} has more than 100 pieces of evidence")
        seen: set[tuple[str, str]] = set()
        for evidence in criterion.get("evidence") or []:
            # The server discriminates on the type, not on truthiness, so an empty string counts.
            if isinstance(evidence.get("caseId"), str) == isinstance(evidence.get("testId"), str):
                say(f"{rid}: criterion {cid} has evidence naming neither exactly one case nor one manual test")
                continue
            key = ("case", evidence["caseId"]) if isinstance(evidence.get("caseId"), str) \
                else ("manual", evidence["testId"])
            if key in seen:
                say(f"{rid}: criterion {cid} cites {key[1]} twice")
            seen.add(key)
            cited_keys.add(key)
            # A case the requirement already links stays valid evidence even once the catalogue has
            # moved on, which is what the server accepts.
            if evidence.get("caseId") and evidence["caseId"] not in catalog_ids | linked_cases:
                say(f"{rid}: criterion {cid} cites {evidence['caseId']}, which is neither in the catalogue nor already linked")
            if evidence.get("testId"):
                cited.add(evidence["testId"])
                if evidence["testId"] not in existing | proposed:
                    say(f"{rid}: criterion {cid} cites manual test {evidence['testId']}, which does not exist")
            if not as_text(evidence.get("rationale")).strip():
                say(f"{rid}: criterion {cid} has evidence with no rationale")
        gap = as_text(criterion.get("gap")).strip()
        nxt = criterion.get("next")
        if nxt is not None and not isinstance(nxt, dict):
            say(f"{rid}: criterion {cid} has a next that is not an object")
            nxt = None
        if gap and not nxt:
            note(f"{rid}: criterion {cid} has a gap and names no test to build")
        if nxt and not gap:
            say(f"{rid}: criterion {cid} names a test to build but records no gap; the server refuses it")
        if nxt and nxt.get("level") not in LEVELS:
            say(f"{rid}: criterion {cid} has level {nxt.get('level')!r}; use one of {', '.join(LEVELS)}")
        if nxt and not as_text(nxt.get("summary")).strip():
            say(f"{rid}: criterion {cid} names a test to build with no summary")
        if nxt and len(as_text(nxt.get("summary"))) > 300:
            say(f"{rid}: criterion {cid} has a {len(nxt['summary'])}-character summary; keep it to one sentence")
    for procedure in procedures:
        if not identifier_ok(procedure.get("id", "")):
            say(f"{rid}: procedure id {procedure.get('id')!r} has characters the server refuses")
        if procedure.get("kind", "manual") != "manual":
            say(f"{rid}: procedure {procedure.get('id')} is not a manual test")
        if procedure.get("id") not in cited:
            say(f"{rid}: procedure {procedure.get('id')} is cited by no criterion, so the server refuses it")
        if procedure.get("id") in existing:
            say(f"{rid}: procedure {procedure['id']} collides with a test the requirement already has")
        for field in ("title", "steps", "expected"):
            if not as_text(procedure.get(field)).strip():
                say(f"{rid}: procedure {procedure.get('id')} has no {field}")
    if len(cited_keys) > 100:
        say(f"{rid}: the plan cites {len(cited_keys)} distinct tests; the server takes at most 100")
    return found, notes


def identifier_ok(value) -> bool:
    """The server's own identifier rule (`identifier` in src/lib/server/domain.ts), length and all."""
    return isinstance(value, str) and 0 < len(value) <= 200 and value[0].isascii() and value[0].isalnum() \
        and all(c.isascii() and (c.isalnum() or c in "._-") for c in value)


def as_text(value) -> str:
    """A field the server insists is a string. An explicit `null` is not one, and is caught by name."""
    return value if isinstance(value, str) else ""


def head_sha() -> str:
    try:
        result = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        raise Problem("could not read the current commit; run inside the repository or pass --sha=<40 chars>") from None
    sha = result.stdout.strip()
    if len(sha) != 40:
        raise Problem(f"git returned {sha!r}, which is not a 40-character commit")
    return sha


def propose(console: Console, paths: list[str], sha: str, check_only: bool) -> int:
    revision = console.call("/api/bootstrap")["revision"]
    requirements = {r["id"]: r for r in revision["requirements"]}
    proposals = console.call("/api/coverage-proposals")
    catalog_ids = {c["id"] for c in console.call("/api/catalog")["cases"]}
    entries: list[dict] = []
    for path in paths:
        try:
            loaded = json.loads(Path(path).read_text())
        except OSError as error:
            raise Problem(f"could not read {path}: {error}") from None
        except json.JSONDecodeError as error:
            raise Problem(f"{path} is not valid JSON: {error}") from None
        for entry in loaded if isinstance(loaded, list) else [loaded]:
            if not isinstance(entry, dict) or not isinstance(entry.get("plan"), dict):
                raise Problem(f"{path} must hold a plan object, or a list of them, each with a `plan`")
            entries.append(entry)
    problems: list[str] = []
    notes: list[str] = []
    for entry in entries:
        requirement = requirements.get(entry.get("requirementId"))
        if not requirement:
            problems.append(f"{entry.get('requirementId')}: no such requirement in r{revision['id']}")
            continue
        pending = pending_for(proposals, requirement["id"])
        baseline = (pending or {}).get("plan") or requirement.get("coverage")
        found, said = plan_problems(entry, requirement, baseline, catalog_ids)
        problems += found
        notes += said
    criteria = [c for e in entries for c in e.get("plan", {}).get("criteria", [])]
    print(f"r{revision['id']} · {len(entries)} plans · {len(criteria)} criteria · "
          f"{sum(1 for c in criteria if as_text(c.get('gap')).strip())} gaps · "
          f"{sum(1 for c in criteria if c.get('next'))} named tests · "
          f"{sum(len(e.get('procedures') or []) for e in entries)} procedures · commit {sha[:10]}")
    for note in notes:
        print(f"  note · {note}")
    for problem in problems:
        print(f"  {problem}")
    if problems:
        print(f"\n{len(problems)} problems. Nothing was submitted.")
        return 1
    if check_only:
        print("\nEvery plan is valid. Drop --check to submit them.")
        return 0
    known_ids = {p["id"] for p in proposals}
    failed = 0
    for entry in entries:
        body = {"baseRevision": revision["id"], "requirementId": entry["requirementId"], "sourceSha": sha,
                "plan": entry["plan"]}
        if entry.get("procedures"):
            body["procedures"] = entry["procedures"]
        try:
            result = console.call("/api/coverage-proposals", body)
            if result["id"] in known_ids:
                print(f"  unchanged {entry['requirementId']}: the console already holds this exact plan")
            else:
                print(f"  submitted {entry['requirementId']}"
                      + (" (superseded the pending one)" if result.get("supersedes") else ""))
        except Problem as error:
            failed += 1
            print(f"  FAILED {entry['requirementId']}: {error}")
    return 1 if failed else 0


# ──────────────────────────────── dispatch ────────────────────────────────


def flag_value(argv: list[str], name: str) -> str | None:
    """`--since 16` and `--since=16` both, because both get typed."""
    for index, word in enumerate(argv):
        if word == f"--{name}" and index + 1 < len(argv):
            return argv[index + 1]
        if word.startswith(f"--{name}="):
            return word.split("=", 1)[1]
    return None


"""The flags each command takes. Anything else is refused, because `--checks` must not submit."""
ALLOWED = {"": {"--json"}, "list": {"--gaps", "--no-plan", "--pending", "--flagged"},
           "proposal": {"--json"}, "tests": set(), "changed": {"--since"}, "propose": {"--check", "--sha"}}


def main(argv: list[str]) -> int:
    flags = {a.split("=", 1)[0] for a in argv if a.startswith("--")}
    consumed = {index + 1 for index, word in enumerate(argv) if word in ("--since", "--sha")}
    words = [a for index, a in enumerate(argv) if not a.startswith("--") and index not in consumed]
    if not words:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    command, rest = words[0], words[1:]
    allowed = ALLOWED.get("" if command.upper().startswith("SYS-") else command)
    if allowed is None:
        raise Problem(f"unknown command {command!r}. Run `obc req` with no arguments for the list.")
    for flag in sorted(flags - allowed):
        raise Problem(f"{command} does not take {flag}. It takes {', '.join(sorted(allowed)) or 'no flags'}.")
    console = Console()

    if command.upper().startswith("SYS-") and not rest:
        wanted = command.upper()
        revision = console.call("/api/bootstrap")["revision"]
        requirement = next((r for r in revision["requirements"] if r["id"] == wanted), None)
        if requirement is None:
            raise Problem(f"{wanted} is not in revision r{revision['id']}.")
        if "--json" in flags:
            print(json.dumps(requirement, indent=2, ensure_ascii=False))
        else:
            print(render(requirement, revision, console.call("/api/coverage-proposals")))
        return 0

    if command == "list":
        revision = console.call("/api/bootstrap")["revision"]
        lines = list_lines(revision, console.call("/api/coverage-proposals"),
                           {f.lstrip("-") for f in flags} & {"gaps", "no-plan", "pending", "flagged"})
        print("\n".join(lines) if lines else "Nothing matches.")
        print(f"\n{len(lines)} of {len(revision['requirements'])} requirements · r{revision['id']}")
        return 0

    if command == "proposal":
        if not rest:
            raise Problem("name a requirement, for example: obc req proposal SYS-003")
        wanted = rest[0].upper()
        proposal = pending_for(console.call("/api/coverage-proposals"), wanted)
        if not proposal:
            raise Problem(f"{wanted} has no proposal waiting.")
        if "--json" in flags:
            print(json.dumps(proposal, indent=2, ensure_ascii=False))
            return 0
        revision = console.call("/api/bootstrap")["revision"]
        requirement = next((r for r in revision["requirements"] if r["id"] == wanted), None)
        if requirement is None:
            # `coverageConflict` produces exactly this, and the conflict line is the useful answer.
            print(f"{wanted} · proposed by {proposal['author']} · against r{proposal['baseRevision']}")
            print(f"CONFLICT: {proposal.get('conflict') or 'the requirement is no longer in this revision.'}")
            return 0
        known = {c["id"] for c in console.call("/api/catalog")["cases"]}
        print(render_proposal(proposal, requirement, known))
        return 0

    if command == "tests":
        if not rest:
            raise Problem("give a search term, for example: obc req tests orientation")
        terms = [word.lower() for word in rest]
        catalog = console.call("/api/catalog")
        hits = [c for c in catalog["cases"]
                if all(term in f"{c['id']} {c['suite']} {c['name']}".lower() for term in terms)]
        for case in hits[:40]:
            print(f"{case['id']}\n    {case['name']}  ·  {case['suite']}")
        print(f"\n{len(hits)} of {len(catalog['cases'])} cases match · catalogue {catalog['sourceSha'][:10]}"
              + ("  ·  showing the first 40" if len(hits) > 40 else ""))
        return 0

    if command == "changed":
        if rest:
            raise Problem(f"changed takes no arguments; did you mean --since {rest[0]}?")
        latest = console.call("/api/bootstrap")["revision"]
        raw = flag_value(argv, "since")
        if "--since" in flags and not raw:
            raise Problem("--since needs a revision number")
        try:
            since = int(raw.lstrip("rR")) if raw else latest["id"] - 1
        except ValueError:
            raise Problem(f"--since takes a revision number, not {raw!r}") from None
        if since >= latest["id"]:
            raise Problem(f"r{since} is not older than the current r{latest['id']}.")
        if since < 1:
            raise Problem("there is no revision before r1.")
        print("\n".join(changed_lines(console.call(f"/api/revisions/{since}"), latest)))
        return 0

    if command == "propose":
        if not rest:
            raise Problem("name at least one plan file, for example: obc req propose plan.json")
        sha = flag_value(argv, "sha")
        if "--sha" in flags and not sha:
            raise Problem("--sha needs a 40-character commit")
        if sha and (len(sha) != 40 or any(c not in "0123456789abcdef" for c in sha.lower())):
            raise Problem(f"--sha takes the exact 40-character commit, not {sha!r}")
        return propose(console, rest, sha or head_sha(), "--check" in flags)

    raise Problem(f"unknown command {command!r}. Run `obc req` with no arguments for the list.")


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Problem as problem:
        print(f"obc req: {problem}", file=sys.stderr)
        sys.exit(1)
