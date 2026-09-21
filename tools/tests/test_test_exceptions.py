from __future__ import annotations

import json
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import test_exceptions


class ExceptionIssueStateTests(unittest.TestCase):
    """The offline shape rules and the one bounded request per distinct issue."""

    def suite(self, reference: str, field: str = "sleep_exception", name: str = "demo") -> dict:
        return {"id": name, field: {"reason": "Pending repair", "issue": reference}}

    def response(self, value: object) -> subprocess.CompletedProcess:
        return subprocess.CompletedProcess([], 0, stdout=json.dumps(value))

    def check(self, suites, repository="owner/repo"):
        with patch.object(test_exceptions, "read_suites", return_value=suites):
            return test_exceptions.main(["--repo", repository])

    @patch.object(test_exceptions.subprocess, "run")
    def test_local_and_full_references_share_one_lookup(self, run) -> None:
        run.return_value = self.response({"state": "open"})
        suites = [self.suite("#7"), self.suite("https://github.com/OWNER/REPO/issues/007", "sleep_exception")]
        with patch("sys.stdout"):
            self.assertEqual(self.check(suites), 0)
        run.assert_called_once_with(
            ["gh", "api", "repos/owner/repo/issues/7"],
            check=True, capture_output=True, text=True, timeout=20,
        )

    @patch.object(test_exceptions.subprocess, "run")
    def test_distinct_issues_report_all_owning_fields(self, run) -> None:
        run.side_effect = [self.response({"state": "closed"}), self.response({"state": "closed"})]
        suites = [self.suite("#1"), self.suite("#1", name="other"), self.suite("#2")]
        with patch("sys.stderr") as stderr:
            self.assertEqual(self.check(suites), 1)
        reported = "".join(str(call.args[0]) for call in stderr.write.call_args_list)
        for owner in ("demo.sleep_exception", "other.sleep_exception"):
            self.assertIn(owner, reported)
        self.assertEqual(run.call_count, 2)

    @patch.object(test_exceptions.subprocess, "run")
    def test_invalid_input_is_rejected_before_any_request(self, run) -> None:
        cases = [
            [self.suite("#1"), self.suite("garbage", "sleep_exception")],
            [self.suite("https://example.com/o/r/issues/3")],
            [self.suite("https://github.com/o/r?bad/issues/3")],
            [{"id": "demo", "sleep_exception": {"reason": "", "issue": "#1"}}],
            [{"id": "demo", "sleep_exception": "not a table"}],
        ]
        for suites in cases:
            with self.subTest(suites=suites), patch("sys.stderr"):
                self.assertEqual(self.check(suites), 1)
        with patch("sys.stderr"):
            self.assertEqual(self.check([self.suite("#1")], "not-a-repository"), 1)
        run.assert_not_called()

    @patch.object(test_exceptions.subprocess, "run")
    def test_pull_requests_and_malformed_responses_are_not_open_issues(self, run) -> None:
        for response in ({"state": "open", "pull_request": {}}, {}, [], {"state": []}, {"state": "unknown"}):
            with self.subTest(response=response):
                run.return_value = self.response(response)
                with patch("sys.stderr") as stderr:
                    self.assertEqual(self.check([self.suite("#1")]), 1)
                reported = "".join(str(call.args[0]) for call in stderr.write.call_args_list)
                self.assertIn("demo.sleep_exception: owner/repo#1", reported)

    @patch.object(test_exceptions.subprocess, "run")
    def test_api_auth_timeout_and_decode_errors_are_visible_without_retry(self, run) -> None:
        failures = [
            subprocess.CalledProcessError(1, ["gh"], stderr="HTTP 403: authentication failed"),
            subprocess.TimeoutExpired(["gh"], 20),
            FileNotFoundError("gh is not installed"),
            None,
        ]
        for failure in failures:
            with self.subTest(failure=type(failure).__name__):
                run.reset_mock(side_effect=True)
                if failure is None:
                    run.return_value = subprocess.CompletedProcess([], 0, stdout="not JSON")
                else:
                    run.side_effect = failure
                with patch("sys.stderr") as stderr:
                    self.assertEqual(self.check([self.suite("#1")]), 1)
                reported = "".join(str(call.args[0]) for call in stderr.write.call_args_list)
                self.assertIn("demo.sleep_exception: owner/repo#1", reported)
                run.assert_called_once()


class ShippedExceptionTests(unittest.TestCase):
    def test_every_shipped_exception_block_has_a_reason_and_an_issue(self) -> None:
        root = Path(__file__).resolve().parents[2]
        _, errors = test_exceptions.collect_references(
            test_exceptions.read_suites(root), "timohueser/OpenBikeComputer"
        )
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
