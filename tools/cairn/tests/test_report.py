import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from datetime import datetime, timezone
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parents[1]))
import cairn  # noqa: E402
import report  # noqa: E402

# The roles and tiers of this repository; each test sets its own `report` settings.
CONFIG = json.loads((cairn.ROOT / cairn.CONFIG).read_text())


def header(name, owns, status):
    return f"---\nsystem: {name}\nkind: system\narea: Device\npurpose: P.\nowns: [{owns}]\nstatus: {status}\n---\n# {name}\n"


class Repository:
    def __init__(self, folder):
        self.root = Path(folder)
        self.git("init", "--quiet", "--initial-branch=develop")

    def git(self, *args):
        subprocess.run(["git", "-C", str(self.root), "-c", "user.name=t", "-c", "user.email=t@t", *args],
                       check=True, capture_output=True)

    def write(self, files):
        for path, text in files.items():
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_text(text)
        self.git("add", "-A")

    def merge(self, number, files):
        """Merge a pull request the way GitHub does: a merge commit on the first-parent line."""
        self.git("checkout", "--quiet", "-b", f"pr{number}")
        self.write(files)
        self.git("commit", "--quiet", "-m", f"pull request {number}")
        self.git("checkout", "--quiet", "develop")
        self.git("merge", "--quiet", "--no-ff", "-m", f"Merge pull request #{number} from someone/pr{number}", f"pr{number}")


class DriftInputTests(unittest.TestCase):
    PULLS = {
        "pulls/2": {"title": "Fix the snap", "body": "Mode: fix"},
        "pulls/3": {"title": "Split the router", "body": "Mode: change #7"},
        "pulls/4": {"title": "Only tests", "body": "Mode: fix"},
        "pulls/5": {"title": "Search", "body": "Mode: fix"},
        "issues/7": {"body": "Status: approved\n\n## TL;DR\n\n**Decision:** split the router.\n\n---\n\n## Plan\n\nSecret."},
    }

    def inputs(self, **settings):
        docs = cairn.load_docs(self.repository.root, "docs/design")
        config = {**CONFIG, "fix": {"tests": ["**/tests/**"]},
                  "report": {"max_input_chars": 40_000, "ignore": ["*.lock"], **settings}}
        return report.drift_inputs(self.repository.root, docs, config, lambda path: self.PULLS[path])

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.repository = Repository(folder.name)
        self.repository.write({"docs/design/routing.md": header("routing", "route/**", "approved"),
                               "docs/design/search.md": header("search", "search/**", "draft"),
                               "route/a.rs": "a\n", "search/b.rs": "b\n"})
        self.repository.git("commit", "--quiet", "-m", "start")
        self.repository.merge(2, {"route/a.rs": "a\nsnap\n", "route/tests/t.rs": "test\n", "route/Cargo.lock": "lock\n",
                                  "search/b.rs": "b\nsearched\n"})
        self.repository.merge(3, {"route/c.rs": "c\n"})
        self.repository.merge(4, {"route/tests/u.rs": "test\n"})
        self.repository.merge(5, {"search/b.rs": "b\n"})

    def test_an_approved_system_gets_the_tldr_of_a_change_and_the_diff_of_a_fix_without_ignored_files(self):
        [found] = self.inputs()
        self.assertEqual((found.system, found.pulls, found.cut), ("routing", (3, 2), 0))
        self.assertIn("**Decision:** split the router.", found.text)
        self.assertNotIn("Secret", found.text)
        self.assertIn("+snap", found.text)
        for absent in ("route/tests", "Cargo.lock", "searched", "route/c.rs"):
            self.assertNotIn(absent, found.text)

    def test_the_cap_cuts_the_input_and_a_doc_change_starts_again(self):
        [whole] = self.inputs()
        [cut] = self.inputs(max_input_chars=100)
        self.assertEqual((len(cut.text), cut.cut), (100, len(whole.text)))
        self.repository.merge(6, {"docs/design/routing.md": header("routing", "route/**", "approved") + "Body.\n"})
        self.assertEqual(self.inputs(), [])


MODELS = [{"id": "claude-sonnet-4-6", "created_at": "2026-02-17T00:00:00Z"},
          {"id": "claude-sonnet-5", "created_at": "2026-06-01T00:00:00Z"},
          {"id": "claude-opus-5-5", "created_at": "2026-09-21T00:00:00Z"},
          {"id": "claude-sonnet-5-5", "created_at": "2026-09-01T00:00:00Z"}]


def succeeded(text, **usage):
    return {"type": "succeeded", "message": {"stop_reason": "end_turn", "usage": usage,
                                             "content": [{"type": "text", "text": text}]}}


class FakeApi:
    """The Anthropic API. The shared prefix counts 10,000 tokens and each request 100,000. Each poll
    of the batch takes the next of `statuses`; an exception there is a failed request."""

    def __init__(self, rows, statuses=("ended",)):
        self.rows, self.statuses, self.calls = rows, list(statuses), []

    def __call__(self, method, url, body):
        path = url.removeprefix(report.API + "/")
        self.calls.append((method, path, body))
        if path.startswith("models"):
            return json.dumps({"data": MODELS})
        if path == "messages/count_tokens":
            return json.dumps({"input_tokens": 10_000 if body["messages"][0]["content"] == "." else 100_000})
        if path == "messages/batches":
            return json.dumps({"id": "b1", "processing_status": "in_progress"})
        if path == "messages/batches/b1/cancel":
            return json.dumps({"id": "b1", "processing_status": "canceling"})
        if path == "messages/batches/b1":
            status = self.statuses.pop(0)
            if isinstance(status, Exception):
                raise status
            return json.dumps({"id": "b1", "processing_status": status, "results_url": "https://results/b1"})
        if url == "https://results/b1":
            return "\n".join(json.dumps({"custom_id": system, "result": result}) for system, result in self.rows.items())
        raise AssertionError(url)

    def paths(self, wanted):
        return [body for method, path, body in self.calls if path == wanted]


class DriftCallTests(unittest.TestCase):
    PRICES = {"claude-sonnet-5-5": {"input": 1, "output": 5, "cache_read": 0.1}}

    def run_drift(self, api, systems, budget=5.0, clock=None):
        settings = {"budget_usd": budget, "max_tokens": 16_000, "timeout_minutes": 10, "batch_prices": self.PRICES}
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "docs/design").mkdir(parents=True)
            inputs = []
            for system in systems:
                (root / f"docs/design/{system}.md").write_text(f"# {system}\n")
                inputs.append(report.Inputs(system, f"docs/design/{system}.md", (2,), "diff"))
            sleeps = []
            found = report.drift(api, root, inputs, {**CONFIG, "report": settings}, sleep=sleeps.append,
                                 clock=clock or (lambda: 0))
        return found, sleeps

    def test_the_batch_stops_at_the_budget_and_a_bad_answer_is_reported(self):
        good = {"false_statements": [{"quote": "Q @owner", "evidence": "#2", "replacement": "R"}], "missing_behaviour": []}
        api = FakeApi({"a": succeeded(json.dumps(good), input_tokens=1000, cache_creation_input_tokens=2000, output_tokens=500),
                       "b": succeeded('{"false_statements": []}', input_tokens=1000, cache_read_input_tokens=2000, output_tokens=500)},
                      statuses=["in_progress", "ended"])
        # At worst a request costs 10,000 x $1 x 2 + 90,000 x $1 + 16,000 x $5 = $0.19, so two fit in $0.40.
        found, sleeps = self.run_drift(api, "abc", budget=0.4)
        self.assertEqual((found.model, found.skipped, round(found.estimate, 2)), ("claude-sonnet-5-5", ["c"], 0.38))
        # a: 1,000 + 2,000 x 2 + 500 x 5; b: 1,000 + 2,000 x 0.1 + 500 x 5; in millionths of a dollar.
        self.assertAlmostEqual(found.spent, (7_500 + 3_700) / 1e6)
        self.assertEqual(found.found["a"], good)
        self.assertRegex(found.found["b"], r"^bad JSON \(stop reason end_turn\)")
        self.assertIn("- **False:** Q @\u200bowner", report.drift_lines(found, 10))
        self.assertEqual(len(sleeps), 2)
        counted = api.paths("messages/count_tokens")
        self.assertEqual(len(counted), 4)
        self.assertEqual(counted[0]["output_config"], counted[1]["output_config"])
        self.assertNotIn("max_tokens", counted[1])
        [batch] = api.paths("messages/batches")
        self.assertEqual([r["custom_id"] for r in batch["requests"]], ["a", "b"])
        params = batch["requests"][0]["params"]
        self.assertEqual(params["system"][0]["cache_control"], report.CACHE)
        self.assertEqual(params["output_config"]["effort"], "low")
        self.assertEqual(params["output_config"]["format"]["schema"], report.SCHEMA)

    def test_at_the_timeout_the_batch_is_canceled_and_its_results_are_still_read(self):
        api = FakeApi({"a": succeeded('{"false_statements": [], "missing_behaviour": []}', output_tokens=100),
                       "b": {"type": "canceled"}},
                      statuses=[OSError("reset"), "in_progress", "in_progress", "ended"])
        times = iter([0, 0, 0, 601])
        found, sleeps = self.run_drift(api, "ab", clock=lambda: next(times, 601))
        self.assertTrue(found.canceled)
        self.assertEqual(len(api.paths("messages/batches/b1/cancel")), 1)
        self.assertEqual(found.found["a"], {"false_statements": [], "missing_behaviour": []})
        self.assertRegex(found.found["b"], "^the request canceled")
        self.assertAlmostEqual(found.spent, 500 / 1e6)

    def test_a_permanent_http_error_while_polling_stops_the_run_and_names_the_batch(self):
        revoked = report.ApiError(401, "GET messages/batches/b1: HTTP 401: invalid x-api-key")
        with self.assertRaisesRegex(cairn.CairnError, "^batch b1: .*HTTP 401"):
            self.run_drift(FakeApi({}, statuses=[report.ApiError(503, "overloaded"), revoked]), "a")

    def test_an_errored_row_has_no_findings_and_no_cost(self):
        error = {"type": "error", "error": {"type": "invalid_request_error", "message": "bad schema"}}
        found, _ = self.run_drift(FakeApi({"a": {"type": "errored", "error": error}}), "a")
        self.assertRegex(found.found["a"], "^the request errored: .*invalid_request_error")
        self.assertEqual(found.spent, 0)

    def test_an_exact_model_id_is_a_pin_and_needs_a_price(self):
        config = {**CONFIG, "tiers": {**CONFIG["tiers"], "api": {"balanced": "claude-sonnet-5"}},
                  "report": {"budget_usd": 5, "batch_prices": self.PRICES}}
        with self.assertRaisesRegex(cairn.CairnError, "^no price for claude-sonnet-5; add it to .cairn/config.json$"):
            report.drift(FakeApi({}), Path("."), [], config)


class OverrideTests(unittest.TestCase):
    def test_only_overrides_merged_in_the_last_7_days_are_listed(self):
        def pull(number, body, merged, updated="2026-10-28T00:00:00Z"):
            return {"number": number, "title": f"PR {number}", "body": body, "merged_at": merged, "updated_at": updated}

        pulls = [
            pull(10, "Mode: override: a release fix", "2026-10-27T09:00:00Z"),
            pull(11, "Mode: fix", "2026-10-27T09:00:00Z"),
            pull(12, "Mode: override: closed, never merged", None),
            pull(13, "Mode: override: merged long ago", "2026-10-01T09:00:00Z"),
            pull(14, "<!-- Mode: override: hidden -->\nMode: fix", "2026-10-27T09:00:00Z"),
        ]
        api = mock.Mock(return_value=pulls)
        found = report.overrides(api, datetime(2026, 10, 29, tzinfo=timezone.utc))
        self.assertEqual(found, [(10, "PR 10", "a release fix")])
        api.assert_called_once()


class PublishTests(unittest.TestCase):
    def fake(self, issues):
        calls = []

        def api(path, method="GET", payload=None):
            calls.append((method, path, payload))
            if path.startswith("issues?"):
                return issues
            if path.startswith("labels/"):
                raise OSError("gh: Not Found (HTTP 404)")
            return {"number": 9, "node_id": "I_9", "html_url": "https://github.com/o/r/issues/9"}

        return api, calls

    def test_a_new_issue_is_labelled_and_pinned_and_a_failed_pin_is_noted(self):
        api, calls = self.fake([{"number": 1, "pull_request": {}}])
        pin = mock.Mock(side_effect=OSError("GraphQL is blocked"))
        self.assertEqual(report.publish("Report.", api, pin), "https://github.com/o/r/issues/9")
        pin.assert_called_once_with("I_9")
        writes = [(method, path, payload) for method, path, payload in calls if method != "GET"]
        self.assertEqual(writes[0][:2], ("POST", "labels"))
        self.assertEqual(writes[1], ("POST", "issues", {"title": "cairn report", "body": "Report.", "labels": ["cairn-report"]}))
        self.assertEqual(writes[2], ("PATCH", "issues/9", {"body": "Report.\n\nThe issue could not be pinned: GraphQL is blocked"}))

    def test_an_open_issue_gets_the_new_body_and_no_pin(self):
        api, calls = self.fake([{"number": 4}])
        pin = mock.Mock()
        report.publish("Report.", api, pin)
        pin.assert_not_called()
        self.assertEqual([c for c in calls if c[0] != "GET"], [("PATCH", "issues/4", {"body": "Report."})])


class ModelTests(unittest.TestCase):
    def test_each_harness_has_its_model_and_an_unknown_harness_inherits(self):
        self.assertEqual(cairn.model_for(CONFIG, "code-review", "claude-code"), ("opus", "high"))
        self.assertEqual(cairn.model_for(CONFIG, "drift", "codex"), ("6.1 Terra", "low"))
        self.assertEqual(cairn.model_for(CONFIG, "drift", "api"), ("claude-sonnet", "low"))
        self.assertIsNone(cairn.model_for(CONFIG, "drift", None))
        self.assertIsNone(cairn.model_for(CONFIG, "implementer", "claude-code"))
        with self.assertRaisesRegex(cairn.CairnError, "unknown role `writer`"):
            cairn.model_for(CONFIG, "writer", "codex")

    def test_the_harness_comes_from_the_environment_unless_given(self):
        def run(*args, claude_code):
            with mock.patch.dict(os.environ), redirect_stdout(io.StringIO()) as out:
                os.environ.pop("CLAUDECODE", None)
                if claude_code:
                    os.environ["CLAUDECODE"] = "1"
                self.assertEqual(cairn.main(["model", *args]), 0)
            return out.getvalue()

        self.assertEqual(run("doc-hay", claude_code=True), "model: sonnet\neffort: medium\n")
        self.assertEqual(run("doc-hay", claude_code=False), "inherit\n")
        self.assertEqual(run("doc-hay", "--harness", "codex", claude_code=True), "model: 6.1 Terra\neffort: medium\n")


if __name__ == "__main__":
    unittest.main()
