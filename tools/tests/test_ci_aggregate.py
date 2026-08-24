from __future__ import annotations

from pathlib import Path
import sys
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ci_aggregate as aggregate
import suite_registry as registry


ALL_JOBS = ("selection", "policy", "test", "wasm-bridges", "web", "desktop-frontend", "desktop")
UPSTREAM = {
    "web": ("selection", "wasm-bridges"),
    "desktop-frontend": ("selection", "wasm-bridges"),
    "desktop": ("selection", "desktop-frontend"),
}


def plan(*suites):
    return {"schema": 1, "suites": list(suites)}


def suite(suite_id, selected, jobs=("test",)):
    return {
        "id": suite_id,
        "selected": selected,
        "jobs": list(jobs),
        "reasons": ["table case" if selected else "unrelated change"],
    }


def needs(**overrides):
    jobs = {job: {"result": "success"} for job in ALL_JOBS}
    jobs.update({job.replace("_", "-"): {"result": result} for job, result in overrides.items()})
    return jobs


def evaluate(*args, **kwargs):
    return aggregate.evaluate(*args, upstream_jobs=UPSTREAM, **kwargs)


class AggregateTests(unittest.TestCase):
    def test_table_driven_states(self):
        cases = [
            ("selected success", suite("rust.ok", True), needs(), "pass", True),
            ("selected failure", suite("rust.fail", True), needs(test="failure"), "fail", False),
            ("selected skipped", suite("rust.skip", True), needs(test="skipped"), "selected but not run", False),
            ("selected missing route", suite("missing", True, ()), needs(), "selected but not run", False),
            ("not selected skipped", suite("web.no", False, ("web",)), needs(web="skipped"), "not selected", True),
            ("failed selection", suite("rust.ok", True), needs(selection="failure"), "pass", False),
        ]
        for name, selected_suite, job_results, state, passed in cases:
            with self.subTest(name=name):
                result = evaluate(plan(selected_suite), job_results)
                self.assertEqual(result.suites[0].state, state)
                self.assertEqual(result.passed, passed)

    def test_a_failed_selection_job_publishes_no_plan(self):
        with self.assertRaisesRegex(aggregate.AggregateError, "publishes no output"):
            aggregate._json_argument(None, "OBC_SELECTION_PLAN")

    def test_every_routed_job_of_a_selected_suite_must_run(self):
        selected = suite("rust.multi", True, ("test", "desktop"))
        result = evaluate(plan(selected), needs(desktop="skipped"))
        self.assertEqual(result.suites[0].state, "selected but not run")
        self.assertIn("desktop", result.suites[0].reason)

    def test_upstream_failure_is_distinct_from_skipped_required_work(self):
        job_results = needs(wasm_bridges="failure", web="skipped")
        result = evaluate(plan(suite("web.contract", True, ("web",))), job_results)
        self.assertEqual(result.suites[0].state, "blocked by an upstream failure")
        self.assertFalse(result.passed)

        # GitHub can expose only the skipped downstream route in a reduced needs fixture.
        job_results.pop("wasm-bridges")
        job_results["web"] = {"result": "skipped"}
        result = evaluate(plan(suite("web.contract", True, ("web",))), job_results)
        self.assertEqual(result.suites[0].state, "selected but not run")

    def test_blocking_producer_is_found_two_hops_up(self):
        # wasm-bridges -> desktop-frontend -> desktop: the failure is not a direct `needs` entry.
        job_results = needs(wasm_bridges="failure", desktop_frontend="skipped", desktop="skipped")
        result = evaluate(plan(suite("desktop.contract", True, ("desktop",))), job_results)
        self.assertEqual(result.suites[0].state, "blocked by an upstream failure")
        self.assertIn("wasm-bridges", result.suites[0].reason)

    def test_blocked_state_when_a_downstream_job_is_skipped(self):
        job_results = needs(desktop_frontend="failure", desktop="skipped")
        result = evaluate(plan(suite("desktop.contract", True, ("desktop",))), job_results)
        self.assertEqual(result.suites[0].state, "blocked by an upstream failure")
        self.assertIn("desktop-frontend", result.global_failures)

    def test_always_run_policy_job_must_succeed(self):
        result = evaluate(plan(suite("ci.policy", True, ("policy",))), needs(policy="skipped"))
        self.assertEqual(result.suites[0].state, "selected but not run")
        self.assertFalse(result.passed)

    def test_summary_lists_every_suite_and_reason(self):
        result = evaluate(plan(suite("rust.ok", True), suite("web.no", False, ("web",))), needs())
        summary = aggregate.markdown_summary(result)
        self.assertIn("`rust.ok`", summary)
        self.assertIn("`web.no`", summary)
        self.assertIn("unrelated change", summary)

    def test_upstream_relation_is_read_from_the_shipped_workflow(self):
        relation = aggregate.upstream_jobs()
        self.assertEqual(relation["web"], ("selection", "wasm-bridges"))
        self.assertEqual(relation["desktop"], ("selection", "desktop-frontend"))

    def test_shipped_plan_shape_feeds_the_aggregate(self):
        """The selector's own JSON is what the gate consumes; no second routing table exists."""

        root = registry.repository_root()
        inventory = registry.load_inventory(root)
        graph = registry.build_cargo_graph(root)
        routes = registry.suite_workflow_jobs(inventory, root, graph)
        selection = registry.select_suites(
            inventory,
            ["docs/content/ride.md"],
            graph,
            routes,
            unconditional=registry.unconditional_jobs(root),
        )
        data = registry.selection_plan_data(selection, registry.workflow_jobs(root))
        results = {job: {"result": "success"} for job in data["required_jobs"]}
        results.update({job: {"result": "skipped"} for job in registry.workflow_jobs(root) if job not in results})
        result = aggregate.evaluate(data, results, aggregate.upstream_jobs())
        self.assertTrue(result.passed)
        self.assertIn("docs", data["required_jobs"])


if __name__ == "__main__":
    unittest.main()
