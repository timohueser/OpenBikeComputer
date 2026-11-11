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

# The roles, the tiers and the test globs of this repository.
CONFIG = json.loads((cairn.ROOT / cairn.CONFIG).read_text())


def header(name, owns, status):
    return f"---\nsystem: {name}\nkind: system\narea: Device\npurpose: P.\nowns: [{owns}]\nstatus: {status}\n---\n# {name}\n"


class Repository:
    def __init__(self, folder):
        self.root = Path(folder)
        self.git("init", "--quiet", "--initial-branch=develop")

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.root), "-c", "user.name=t", "-c", "user.email=t@t", *args],
                              check=True, capture_output=True, text=True).stdout.strip()

    def write(self, files):
        for path, text in files.items():
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_text(text)
        self.git("add", "-A")

    def merge(self, number, files):
        """Merge a pull request the way GitHub does: a merge commit on the first-parent line. Gives
        the merge commit."""
        self.git("checkout", "--quiet", "-b", f"pr{number}")
        self.write(files)
        self.git("commit", "--quiet", "-m", f"pull request {number}")
        self.git("checkout", "--quiet", "develop")
        self.git("merge", "--quiet", "--no-ff", "-m", f"Merge pull request #{number} from someone/pr{number}", f"pr{number}")
        return self.git("rev-parse", "HEAD")


class DocsToCheckTests(unittest.TestCase):
    def found(self):
        docs = cairn.load_docs(self.repository.root, "docs/design")
        return report.docs_to_check(self.repository.root, docs, {**CONFIG, "tests": ["**/tests/**"]})

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.repository = Repository(folder.name)
        self.repository.write({"docs/design/routing.md": header("routing", "route/**", "approved"),
                               "docs/design/search.md": header("search", "search/**", "draft"),
                               "route/a.rs": "a\n", "search/b.rs": "b\n"})
        self.repository.git("commit", "--quiet", "-m", "start")
        self.repository.merge(2, {"route/a.rs": "a\nsnap\n", "search/b.rs": "b\nsearched\n"})
        self.repository.merge(3, {"route/c.rs": "c\n"})
        self.repository.merge(4, {"route/tests/u.rs": "test\n"})
        self.repository.merge(5, {"search/b.rs": "b\n"})

    def test_an_approved_doc_lists_the_pull_requests_that_touch_its_system_outside_tests(self):
        self.assertEqual(self.found(), [("routing", [3, 2])])

    def test_a_failed_history_read_is_an_error_and_not_an_empty_list(self):
        with tempfile.TemporaryDirectory() as folder:
            docs = [cairn.Doc("routing", "system", "Device", "", ("route/**",), status="approved")]
            with self.assertRaisesRegex(OSError, "^git log -1 failed$"):
                report.docs_to_check(Path(folder), docs, {**CONFIG, "docs": "docs/design"})

    def test_a_doc_change_starts_the_list_again(self):
        self.repository.merge(6, {"docs/design/routing.md": header("routing", "route/**", "approved") + "Body.\n"})
        self.assertEqual(self.found(), [])


class SkipTests(unittest.TestCase):
    def test_only_skips_merged_in_the_last_7_days_are_listed_with_their_reason(self):
        def pull(number, body, merged, updated="2026-10-28T00:00:00Z"):
            return {"number": number, "title": f"PR {number}", "body": body, "merged_at": merged, "updated_at": updated}

        pulls = [
            pull(10, "Mode: skip: a release fix", "2026-10-27T09:00:00Z"),
            pull(11, "Mode: fix", "2026-10-27T09:00:00Z"),
            pull(12, "Mode: skip: closed, never merged", None),
            pull(13, "Mode: skip: merged long ago", "2026-10-01T09:00:00Z"),
            pull(14, "<!-- Mode: skip: hidden -->\n```\nMode: skip: an example\n```\nMode: fix", "2026-10-27T09:00:00Z"),
            pull(15, "Mode: override: an unknown value", "2026-10-27T09:00:00Z"),
        ]
        api = mock.Mock(return_value=pulls)
        found = report.skips(report.merged(api, datetime(2026, 10, 29, tzinfo=timezone.utc)))
        self.assertEqual(found, [(10, "PR 10", "a release fix")])
        api.assert_called_once()


class TestGlobTests(unittest.TestCase):
    def test_a_test_pattern_with_no_folder_matches_the_file_name_only(self):
        for path in ("tools/cairn/tests/test_report.py", "a/src/tests.rs", "a/src/peaks_tests.rs", "web/A.svelte.test.ts"):
            self.assertTrue(cairn.is_test(path, CONFIG["tests"]), path)
        for path in ("tools/test_plan.py", "a/tests_b/lib.rs", "tests.rs.orig"):
            self.assertFalse(cairn.is_test(path, CONFIG["tests"]), path)


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
        self.assertEqual(cairn.model_for(CONFIG, "doc-hay", "codex"), ("gpt-6.1-sol", "medium"))
        self.assertIsNone(cairn.model_for(CONFIG, "doc-hay", None))
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
        self.assertEqual(run("doc-hay", "--harness", "codex", claude_code=True), "model: gpt-6.1-sol\neffort: medium\n")


if __name__ == "__main__":
    unittest.main()
