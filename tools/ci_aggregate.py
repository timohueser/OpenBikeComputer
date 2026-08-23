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


RUNNER_JOBS = {
    "always": ("changes", "selection", "retired-map-stack", "card-scheduler-guard", "fixture-registry"),
    "rust": ("fmt", "clippy", "test", "embedded", "boot", "device", "deny"),
    "wasm": ("wasm",),
    "web": ("wasm-bridges", "web"),
    "docs": ("docs",),
    "ios": ("ios-unit", "ios-app"),
    "desktop": ("desktop-frontend", "desktop"),
}
UPSTREAM_JOBS = {
    "web": ("wasm-bridges",),
    "desktop-frontend": ("wasm-bridges",),
    "desktop": ("desktop-frontend",),
}


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


def _blocked_upstream(needs: Mapping[str, Any], jobs: Sequence[str]) -> list[str]:
    blocked: list[str] = []
    for job in jobs:
        for upstream in UPSTREAM_JOBS.get(job, ()):
            if _job_result(needs, upstream) in {"failure", "cancelled"}:
                blocked.append(upstream)
    return sorted(set(blocked))


def evaluate(plan: Mapping[str, Any], needs: Mapping[str, Any]) -> AggregateResult:
    if plan.get("schema") != 1 or not isinstance(plan.get("suites"), list):
        raise AggregateError("selection plan must use schema 1 and contain a suites list")
    suites: list[SuiteResult] = []
    changes = needs.get("changes", {})
    change_outputs = changes.get("outputs", {}) if isinstance(changes, Mapping) else {}
    for suite in plan["suites"]:
        suite_id = str(suite.get("id", "<missing-id>"))
        reasons = suite.get("reasons") or ["no reason supplied"]
        reason = "; ".join(str(value) for value in reasons)
        if not suite.get("selected"):
            suites.append(SuiteResult(suite_id, "not selected", reason, ()))
            continue
        candidate_runners = suite.get("runner_surfaces") or []
        runners = [
            runner
            for runner in candidate_runners
            if runner == "always"
            or (
                isinstance(change_outputs, Mapping)
                and str(change_outputs.get(str(runner), "")).lower() == "true"
            )
        ]
        jobs = tuple(
            dict.fromkeys(job for runner in runners for job in RUNNER_JOBS.get(str(runner), ()))
        )
        if not jobs:
            suites.append(
                SuiteResult(
                    suite_id,
                    "selected but not run",
                    f"{reason}; no selected coarse runner has an executable route",
                    (),
                )
            )
            continue
        results = {job: _job_result(needs, job) for job in jobs}
        failed = [job for job, result in results.items() if result in {"failure", "cancelled"}]
        incomplete = [job for job, result in results.items() if result in {"skipped", "missing"}]
        blockers = _blocked_upstream(needs, incomplete)
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
        raise AggregateError(f"missing JSON input ({environment})")
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


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        result = evaluate(
            _json_argument(args.plan_json, "OBC_SELECTION_PLAN"),
            _json_argument(args.needs_json, "OBC_NEEDS_RESULTS"),
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
