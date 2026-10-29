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
        config = {**CONFIG, "fix": {"tests": ["**/tests/**"]}, "report": {**CONFIG["report"], **settings}}
        return report.drift_inputs(self.repository.root, docs, config, lambda path: self.PULLS[path])

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.repository = Repository(folder.name)
        self.repository.write({"docs/design/routing.md": header("routing", "route/**", "approved"),
                               "docs/design/search.md": header("search", "search/**", "draft"),
                               "route/a.rs": "a\n", "search/b.rs": "b\n"})
        self.repository.git("commit", "--quiet", "-m", "start")
        self.repository.merge(2, {"route/a.rs": "a\nsnap\n", "route/tests/t.rs": "test\n", "search/b.rs": "b\nsearched\n"})
        self.repository.merge(3, {"route/c.rs": "c\n"})
        self.repository.merge(4, {"route/tests/u.rs": "test\n"})
        self.repository.merge(5, {"search/b.rs": "b\n"})

    def test_an_approved_system_gets_the_tldr_of_a_change_and_the_diff_of_a_fix_without_tests(self):
        [found] = self.inputs()
        self.assertEqual((found.system, found.pulls, found.cut), ("routing", (3, 2), 0))
        self.assertIn("**Decision:** split the router.", found.text)
        self.assertNotIn("Secret", found.text)
        self.assertIn("+snap", found.text)
        for absent in ("route/tests", "searched", "route/c.rs"):
            self.assertNotIn(absent, found.text)

    def test_the_cap_cuts_the_input_and_a_doc_change_starts_again(self):
        [whole] = self.inputs()
        [cut] = self.inputs(max_input_chars=100)
        self.assertEqual((len(cut.text), cut.cut), (100, len(whole.text)))
        self.repository.merge(6, {"docs/design/routing.md": header("routing", "route/**", "approved") + "Body.\n"})
        self.assertEqual(self.inputs(), [])


class FakeApi:
    """The Anthropic API: every request counts `tokens` input tokens, and the batch ends after one poll."""

    def __init__(self, tokens, answers):
        self.tokens, self.answers, self.calls = tokens, answers, []

    def __call__(self, method, url, body):
        self.calls.append((method, url.removeprefix(report.API + "/"), body))
        path = self.calls[-1][1]
        if path.startswith("models"):
            return json.dumps({"data": [{"id": "claude-sonnet-4-6", "created_at": "2026-02-17T00:00:00Z"},
                                        {"id": "claude-opus-5-5", "created_at": "2026-09-21T00:00:00Z"},
                                        {"id": "claude-sonnet-5-5", "created_at": "2026-09-01T00:00:00Z"}]})
        if path == "messages/count_tokens":
            return json.dumps({"input_tokens": self.tokens})
        if path == "messages/batches":
            return json.dumps({"id": "b1", "processing_status": "in_progress"})
        if path == "messages/batches/b1":
            return json.dumps({"id": "b1", "processing_status": "ended", "results_url": "https://results/b1"})
        if url == "https://results/b1":
            return "\n".join(json.dumps({"custom_id": system, "result": {"type": "succeeded", "message": {
                "stop_reason": "end_turn", "content": [{"type": "text", "text": text}]}}}) for system, text in self.answers.items())
        raise AssertionError(url)


class DriftCallTests(unittest.TestCase):
    def test_the_batch_stops_at_the_budget_and_a_bad_answer_is_reported(self):
        good = {"false_statements": [{"quote": "Q", "evidence": "#2", "replacement": "R"}], "missing_behaviour": []}
        api = FakeApi(100_000, {"a": json.dumps(good), "b": '{"false_statements": []}'})
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "docs/design").mkdir(parents=True)
            inputs = []
            for system in "abc":
                (root / f"docs/design/{system}.md").write_text(f"# {system}\n")
                inputs.append(report.Inputs(system, f"docs/design/{system}.md", (2,), "diff"))
            # Each request costs at worst 100,000 x $1 x 2 + 16,000 x $5 = $0.28, so two fit in $0.60.
            config = {**CONFIG, "report": {**CONFIG["report"], "budget_usd": 0.6}}
            sleeps = []
            found = report.drift(api, root, inputs, config, sleep=sleeps.append)
        self.assertEqual((found.model, found.skipped, round(found.cost, 2)), ("claude-sonnet-5-5", ["c"], 0.56))
        self.assertEqual(found.found["a"], good)
        self.assertRegex(found.found["b"], r"^bad JSON \(stop reason end_turn\)")
        self.assertEqual(len(sleeps), 1)
        counted = [body for method, path, body in api.calls if path == "messages/count_tokens"]
        self.assertEqual(len(counted), 3)
        self.assertNotIn("max_tokens", counted[0])
        [batch] = [body for method, path, body in api.calls if path == "messages/batches"]
        self.assertEqual([r["custom_id"] for r in batch["requests"]], ["a", "b"])
        params = batch["requests"][0]["params"]
        self.assertEqual(params["system"][0]["cache_control"], report.CACHE)
        self.assertEqual(params["output_config"]["effort"], "low")
        self.assertEqual(params["output_config"]["format"]["schema"], report.SCHEMA)

    def test_a_pin_skips_the_models_api(self):
        config = {**CONFIG, "tiers": {**CONFIG["tiers"], "api": {"balanced": {"family": "claude-sonnet", "pin": "claude-sonnet-5"}}}}
        api = FakeApi(1, {})
        self.assertEqual(report.drift(api, Path("."), [], config).model, "claude-sonnet-5")
        self.assertEqual(api.calls, [])


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
