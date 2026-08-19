from __future__ import annotations

from pathlib import Path
import sys
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ci_aggregate as aggregate


def plan(*suites):
    return {"schema": 1, "suites": list(suites)}


def suite(suite_id, selected, runner="rust"):
    return {
        "id": suite_id,
        "selected": selected,
        "runner_surfaces": [runner] if runner else [],
        "reasons": ["table case" if selected else "unrelated change"],
    }


def needs(**overrides):
    jobs = {job: {"result": "success"} for values in aggregate.RUNNER_JOBS.values() for job in values}
    jobs["changes"]["outputs"] = {
        runner: "true" for runner in aggregate.RUNNER_JOBS if runner != "always"
    }
    jobs.update({job.replace("_", "-"): {"result": result} for job, result in overrides.items()})
    return jobs


class AggregateTests(unittest.TestCase):
    def test_table_driven_states(self):
        cases = [
            ("selected success", suite("rust.ok", True), needs(), "pass", True),
            ("selected failure", suite("rust.fail", True), needs(test="failure"), "fail", False),
            ("selected skipped", suite("rust.skip", True), needs(test="skipped"), "selected but not run", False),
            ("selected missing route", suite("missing", True, ""), needs(), "selected but not run", False),
            ("not selected skipped", suite("web.no", False, "web"), needs(web="skipped"), "not selected", True),
        ]
        for name, selected_suite, job_results, state, passed in cases:
            with self.subTest(name=name):
                result = aggregate.evaluate(plan(selected_suite), job_results)
                self.assertEqual(result.suites[0].state, state)
                self.assertEqual(result.passed, passed)

    def test_upstream_failure_is_distinct_from_skipped_required_work(self):
        job_results = needs(wasm_bridges="failure", web="skipped")
        result = aggregate.evaluate(plan(suite("web.contract", True, "web")), job_results)
        self.assertEqual(result.suites[0].state, "blocked by an upstream failure")
        self.assertFalse(result.passed)

        # GitHub can expose only the skipped downstream route in a reduced needs fixture.
        job_results.pop("wasm-bridges")
        job_results["web"] = {"result": "skipped"}
        result = aggregate.evaluate(plan(suite("web.contract", True, "web")), job_results)
        self.assertEqual(result.suites[0].state, "selected but not run")

    def test_blocked_state_when_a_downstream_job_is_skipped(self):
        job_results = needs(desktop_frontend="failure", desktop="skipped")
        # Ignore the explicit upstream suite failure to inspect the downstream classification.
        result = aggregate.evaluate(plan(suite("desktop.contract", True, "desktop")), job_results)
        self.assertEqual(result.suites[0].state, "blocked by an upstream failure")
        self.assertIn("desktop-frontend", result.global_failures)

    def test_always_run_policy_job_must_succeed(self):
        result = aggregate.evaluate(plan(suite("ci.policy", True, "always")), needs(selection="skipped"))
        self.assertEqual(result.suites[0].state, "selected but not run")
        self.assertFalse(result.passed)

    def test_only_active_candidate_runner_is_required(self):
        selected_suite = suite("rust.shared", True)
        selected_suite["runner_surfaces"] = ["rust", "desktop"]
        job_results = needs(desktop="skipped", desktop_frontend="skipped")
        job_results["changes"]["outputs"]["desktop"] = "false"

        result = aggregate.evaluate(plan(selected_suite), job_results)

        self.assertEqual(result.suites[0].state, "pass")
        self.assertTrue(result.passed)

    def test_summary_lists_every_suite_and_reason(self):
        result = aggregate.evaluate(plan(suite("rust.ok", True), suite("web.no", False, "web")), needs())
        summary = aggregate.markdown_summary(result)
        self.assertIn("`rust.ok`", summary)
        self.assertIn("`web.no`", summary)
        self.assertIn("unrelated change", summary)


if __name__ == "__main__":
    unittest.main()
