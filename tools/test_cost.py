#!/usr/bin/env python3
"""Print an on-demand comparison of existing CI job metadata and native XML reports."""
from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime
import json
import math
from pathlib import Path
import statistics
import xml.etree.ElementTree as ET


def seconds(start: str, end: str) -> float:
    return (datetime.fromisoformat(end) - datetime.fromisoformat(start)).total_seconds()


def run_cost(run: dict, data: dict) -> tuple[float, float]:
    jobs = data["jobs"]
    if data.get("total_count", len(jobs)) != len(jobs):
        raise ValueError(f"run {run['id']}: incomplete job pagination")
    active = [job for job in jobs if job["conclusion"] != "skipped"]
    if not active or any(job["status"] != "completed" for job in active):
        raise ValueError(f"run {run['id']}: no completed job sample")
    if any(job["run_id"] != run["id"] or job["run_attempt"] != run["run_attempt"] for job in jobs):
        raise ValueError(f"run {run['id']}: jobs belong to another run attempt")
    wall = seconds(run["run_started_at"], max(job["completed_at"] for job in active))
    runner = sum(seconds(job["started_at"], job["completed_at"]) for job in active)
    if wall < 0 or runner < 0:
        raise ValueError(f"run {run['id']}: invalid timestamps")
    return wall / 60, runner / 60


@dataclass
class NativeTimes:
    suites: dict[str, float]
    missing_suite_times: int
    runners: dict[str, float]


def native_times(directory: Path) -> NativeTimes:
    reports = sorted(directory.rglob("*.xml"))
    if not reports:
        raise ValueError(f"no native XML reports under {directory}")
    result, runners = {}, {}
    missing = 0
    for path in reports:
        tree = ET.parse(path)
        root = tree.getroot()
        if root.tag == "testsuites" and "time" in root.attrib:
            runners[str(path.relative_to(directory))] = float(root.attrib["time"])
        for index, suite in enumerate(tree.iter("testsuite")):
            # Native suite time is kept as supplied. Missing time is unknown, not zero.
            if "time" not in suite.attrib:
                missing += 1
                continue
            identity = f"{path.relative_to(directory)}::{suite.get('name', str(index))}"
            if identity in result:
                raise ValueError(f"duplicate native suite identity: {identity}")
            duration = float(suite.attrib["time"])
            if not math.isfinite(duration) or duration < 0:
                raise ValueError(f"invalid native suite duration: {identity}")
            result[identity] = duration
    return NativeTimes(result, missing, runners)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=Path, help="saved GitHub workflow-runs response")
    parser.add_argument("--jobs-dir", type=Path, help="directory of jobs-RUN_ID.json responses")
    parser.add_argument("--reports", type=Path, help="downloaded native XML artifacts")
    parser.add_argument("--compare-reports", type=Path, help="earlier artifacts with matching relative paths")
    args = parser.parse_args()
    if bool(args.runs) != bool(args.jobs_dir) or not (args.runs or args.reports):
        parser.error("use --runs with --jobs-dir, --reports, or both")
    if args.compare_reports and not args.reports:
        parser.error("--compare-reports requires --reports")
    try:
        if args.runs:
            runs = json.loads(args.runs.read_text())["workflow_runs"]
            if not runs:
                raise ValueError("empty workflow-run sample")
            walls, costs = [], []
            print("run / attempt / result / elapsed minutes / runner-minutes")
            for run in runs:
                jobs = json.loads((args.jobs_dir / f"jobs-{run['id']}.json").read_text())
                wall, cost = run_cost(run, jobs)
                walls.append(wall)
                costs.append(cost)
                print(f"{run['id']} / {run['run_attempt']} / {run['conclusion']} / {wall:.2f} / {cost:.2f}")
            p95 = sorted(walls)[math.ceil(len(walls) * .95) - 1]
            print(f"sample n={len(runs)}: median elapsed {statistics.median(walls):.2f} min; "
                  f"nearest-rank p95 {p95:.2f} min; max cost {max(costs):.2f} runner-min")
            print("Guidance: elapsed <=10 min, p95 <=20 min, cross-surface cost <=40 runner-min.")
            print("Elapsed includes queue/dependency delays through the final job; it is not a reconstructed DAG critical path.")
            print("Runner-minutes are unweighted active job time, not billing. Sample scope and success bias are caller-owned.")
        if args.reports:
            native = native_times(args.reports)
            head = native.suites
            base = native_times(args.compare_reports).suites if args.compare_reports else {}
            print(f"Native suite duration available: {len(head)}; missing: {native.missing_suite_times}.")
            for report, duration in native.runners.items():
                print(f"Native whole-run report {report}: {duration:.3f} s (not a binary/file duration).")
            print("native suite / reported seconds / delta seconds (matching reports only)")
            for identity, duration in sorted(head.items(), key=lambda item: item[1], reverse=True)[:20]:
                delta = f"{duration - base[identity]:+.3f}" if identity in base else "unknown"
                print(f"{identity} / {duration:.3f} / {delta}")
            if args.compare_reports:
                print(f"matched {len(head.keys() & base.keys())}; added {len(head.keys() - base.keys())}; "
                      f"absent from head {len(base.keys() - head.keys())}")
            print("Guidance: required binary/file <=30 s; unit suite near 2 s. Native aggregate suites may span many files.")
            print("Native durations can include concurrent cases; do not sum them into wall time or runner cost.")
            print("Missing, filtered and ignored tests are not passes. Reports do not establish complete command coverage.")
    except (OSError, ValueError, KeyError, ET.ParseError) as error:
        parser.exit(1, f"test cost: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
