#!/usr/bin/env python3
"""Evaluate the suite selection plan against GitHub Actions `needs` results."""

from __future__ import annotations

import argparse
import json
import os
from dataclasses import dataclass
from pathlib import Path
import sys
from typing import Any, Mapping, Sequence

sys.path.insert(0, str(Path(__file__).resolve().parent))

import suite_registry


class AggregateError(RuntimeError):
    """Malformed plan or GitHub result input."""


@dataclass(frozen=True)
class SuiteResult:
    suite_id: str
    state: str
    reason: str
    jobs: tuple[str, ...]


@dataclass(frozen=True)
class AggregateResult:
    suites: tuple[SuiteResult, ...]
    global_failures: tuple[str, ...]

    @property
    def passed(self) -> bool:
        bad_states = {"fail", "selected but not run", "blocked by an upstream failure"}
        return not self.global_failures and not any(item.state in bad_states for item in self.suites)


def _job_result(needs: Mapping[str, Any], job: str) -> str:
    value = needs.get(job)
    if not isinstance(value, Mapping):
        return "missing"
    result = value.get("result")
    return str(result) if result else "missing"


def _blocked_upstream(
    needs: Mapping[str, Any], jobs: Sequence[str], upstream_jobs: Mapping[str, Sequence[str]]
) -> list[str]:
    """Walk the whole upstream closure: a producer two hops up still blocks the work."""

    blocked: list[str] = []
    seen: set[str] = set()
    pending = list(jobs)
    while pending:
        for upstream in upstream_jobs.get(pending.pop(), ()):
            if upstream in seen:
                continue
            seen.add(upstream)
            if _job_result(needs, upstream) in {"failure", "cancelled"}:
                blocked.append(upstream)
            else:
                # A healthy producer can still hide a failed producer of its own.
                pending.append(upstream)
    return sorted(set(blocked))


def evaluate(
    plan: Mapping[str, Any],
    needs: Mapping[str, Any],
    upstream_jobs: Mapping[str, Sequence[str]] | None = None,
) -> AggregateResult:
    """Report every planned suite against the jobs the plan itself routed it to."""

    if plan.get("schema") != 1 or not isinstance(plan.get("suites"), list):
        raise AggregateError("selection plan must use schema 1 and contain a suites list")
    upstream_jobs = upstream_jobs or {}
    suites: list[SuiteResult] = []
    for suite in plan["suites"]:
        suite_id = str(suite.get("id", "<missing-id>"))
        reasons = suite.get("reasons") or ["no reason supplied"]
        reason = "; ".join(str(value) for value in reasons)
        if not suite.get("selected"):
            suites.append(SuiteResult(suite_id, "not selected", reason, ()))
            continue
        jobs = tuple(dict.fromkeys(str(job) for job in suite.get("jobs") or ()))
        if not jobs:
            suites.append(
                SuiteResult(
                    suite_id,
                    "selected but not run",
                    f"{reason}; the registry routes this suite to no workflow job",
                    (),
                )
            )
            continue
        results = {job: _job_result(needs, job) for job in jobs}
        failed = [job for job, result in results.items() if result in {"failure", "cancelled"}]
        incomplete = [job for job, result in results.items() if result in {"skipped", "missing"}]
        blockers = _blocked_upstream(needs, incomplete, upstream_jobs)
        if incomplete and blockers:
            suites.append(
                SuiteResult(
                    suite_id,
                    "blocked by an upstream failure",
                    f"{reason}; upstream failure: {', '.join(blockers)}",
                    jobs,
                )
            )
            continue
        if failed:
            suites.append(
                SuiteResult(
                    suite_id,
                    "fail",
                    f"{reason}; failed required job(s): {', '.join(failed)}",
                    jobs,
                )
            )
            continue
        if incomplete:
            suites.append(
                SuiteResult(
                    suite_id,
                    "selected but not run",
                    f"{reason}; skipped or missing required job(s): {', '.join(incomplete)}",
                    jobs,
                )
            )
            continue
        suites.append(SuiteResult(suite_id, "pass", reason, jobs))

    global_failures = tuple(
        sorted(
            job
            for job, value in needs.items()
            if isinstance(value, Mapping) and value.get("result") in {"failure", "cancelled"}
        )
    )
    return AggregateResult(tuple(suites), global_failures)


def markdown_summary(result: AggregateResult) -> str:
    lines = [
        "## Test-suite selection result",
        "",
        "| Suite | State | Required jobs | Reason |",
        "| --- | --- | --- | --- |",
    ]
    for suite in result.suites:
        reason = suite.reason.replace("|", "\\|").replace("\n", " ")
        jobs = ", ".join(suite.jobs) if suite.jobs else "—"
        lines.append(f"| `{suite.suite_id}` | **{suite.state}** | {jobs} | {reason} |")
    if result.global_failures:
        lines.extend(
            [
                "",
                "Failed or cancelled required workflow jobs: " + ", ".join(result.global_failures),
            ]
        )
    lines.extend(["", "Aggregate result: **PASS**" if result.passed else "Aggregate result: **FAIL**"])
    return "\n".join(lines) + "\n"


def _json_argument(value: str | None, environment: str) -> Mapping[str, Any]:
    raw = value if value is not None else os.environ.get(environment, "")
    if not raw:
        raise AggregateError(
            f"missing JSON input ({environment}); a failed or cancelled job publishes no output"
        )
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise AggregateError(f"invalid JSON input ({environment}): {exc}") from exc
    if not isinstance(parsed, Mapping):
        raise AggregateError(f"JSON input ({environment}) must be an object")
    return parsed


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    root.add_argument("--plan-json", help="selection-plan JSON; defaults to OBC_SELECTION_PLAN")
    root.add_argument("--needs-json", help="GitHub needs JSON; defaults to OBC_NEEDS_RESULTS")
    return root


def upstream_jobs() -> dict[str, tuple[str, ...]]:
    """The blocking relation stays derived from the workflow `needs` graph."""

    jobs = suite_registry.workflow_jobs(suite_registry.repository_root())
    return {name: job.needs for name, job in jobs.items()}


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        result = evaluate(
            _json_argument(args.plan_json, "OBC_SELECTION_PLAN"),
            _json_argument(args.needs_json, "OBC_NEEDS_RESULTS"),
            upstream_jobs(),
        )
        summary = markdown_summary(result)
        print(summary, end="")
        summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary_path:
            with Path(summary_path).open("a", encoding="utf-8") as handle:
                handle.write(summary)
        return 0 if result.passed else 1
    except AggregateError as exc:
        print(f"CI aggregate failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
