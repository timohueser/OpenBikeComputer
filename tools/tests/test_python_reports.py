"""The pinned reporter must keep unittest outcomes and useful subtest identities."""

from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest
import xml.etree.ElementTree as ET


class PythonReportTests(unittest.TestCase):
    def test_unittest_discovery_and_xml_outcomes(self):
        source = """
            import unittest

            class Outcomes(unittest.TestCase):
                def test_pass(self):
                    print("captured output <&>")

                @unittest.skip("deliberate skip")
                def test_skip(self):
                    self.fail("must not execute")

                @unittest.expectedFailure
                def test_expected_failure(self):
                    self.fail("known failure")

                @unittest.expectedFailure
                def test_unexpected_success(self):
                    pass

                def test_successful_subtests(self):
                    for value in (1, 2):
                        with self.subTest(value=value):
                            self.assertGreater(value, 0)

                def test_subtest_outcomes(self):
                    for value in ("pass", "failure", "error", "skip"):
                        with self.subTest(value=value):
                            if value == "failure":
                                self.fail("subtest failure <&>")
                            if value == "error":
                                raise ValueError("subtest error")
                            if value == "skip":
                                self.skipTest("subtest skip")
        """
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "test_outcomes.py").write_text(textwrap.dedent(source))
            for runner in ("unittest", "xmlrunner"):
                command = [sys.executable, "-m", runner, "discover", "-s", directory, "-v"]
                if runner == "xmlrunner":
                    command += ["-o", str(root / "reports")]
                run = subprocess.run(command, capture_output=True, text=True, timeout=30)
                self.assertEqual(run.returncode, 1, run.stderr)
                self.assertIn("Ran 6 tests", run.stderr)
                self.assertIn(
                    "FAILED (failures=1, errors=1, skipped=2, expected failures=1, unexpected successes=1)",
                    run.stderr,
                )
            reports = list((root / "reports").glob("*.xml"))
            self.assertTrue(reports)
            suites = [ET.parse(report).getroot() for report in reports]
            cases = {case.attrib["name"]: case for suite in suites for case in suite.findall("testcase")}
            self.assertEqual(len(cases), 8)
            for case in cases.values():
                self.assertEqual(case.attrib["classname"], "test_outcomes.Outcomes")
                self.assertGreaterEqual(float(case.attrib["time"]), 0)
            self.assertEqual(len(cases["test_pass"]), 1)  # Captured stdout, no outcome tag.
            self.assertIn("captured output <&>", cases["test_pass"].findtext("system-out"))
            self.assertEqual(len(cases["test_successful_subtests"]), 0)
            self.assertEqual(cases["test_skip"].find("skipped").attrib["message"], "deliberate skip")
            self.assertEqual(cases["test_expected_failure"].find("skipped").attrib["type"], "XFAIL")
            self.assertEqual(cases["test_unexpected_success"].find("error").attrib["type"], "UnexpectedSuccess")
            for value, outcome in (("failure", "failure"), ("error", "error"), ("skip", "skipped")):
                case = cases[f"test_subtest_outcomes (value='{value}')"]
                self.assertIsNotNone(case.find(outcome))
            for field, expected in (("tests", 8), ("failures", 1), ("errors", 2), ("skipped", 3)):
                self.assertEqual(sum(int(suite.attrib[field]) for suite in suites), expected)
