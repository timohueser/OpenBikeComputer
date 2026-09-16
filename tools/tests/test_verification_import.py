"""Native evidence import keeps stable identities and fails closed on ambiguous reports."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("verification_import", Path(__file__).resolve().parents[2] / "apps/obc-verification/ops/import_results.py")
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)
publish_spec = importlib.util.spec_from_file_location("verification_publish", Path(__file__).resolve().parents[2] / "apps/obc-verification/ops/publish.py")
publish = importlib.util.module_from_spec(publish_spec)
publish_spec.loader.exec_module(publish)


class VerificationImportTests(unittest.TestCase):
    def test_release_notes_do_not_present_excepted_requirements_as_verified(self):
        candidate = {"id": "candidate", "sourceSha": "a" * 40, "version": "v0.1.0-alpha"}
        self.assertIn("Verified candidate", publish.release_notes(candidate, "owner/repo"))
        candidate["exceptions"] = [{"requirementId": "SYS-001"}]
        notes = publish.release_notes(candidate, "owner/repo")
        self.assertIn("Accepted requirement exceptions: 1", notes)
        self.assertIn("`SYS-001`", notes)
        self.assertIn("not counted as verified", notes)
        self.assertIn("verification-report.html", notes)
        self.assertNotIn("Verified candidate", notes)
        candidate["revision"] = {"requirements": [{"id": "SYS-002", "active": False}, {"id": "SYS-003", "active": True}]}
        notes = publish.release_notes(candidate, "owner/repo")
        self.assertIn("exceptions and exclusions", notes)
        self.assertIn("Excluded requirements: 1", notes)
        self.assertIn("`SYS-002`", notes)
        self.assertNotIn("`SYS-003`", notes)
        candidate["exceptions"] = []
        self.assertIn("Candidate accepted with requirement exclusions", publish.release_notes(candidate, "owner/repo"))

    def test_native_statuses_and_stable_identities_ignore_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "python-repository-tools-7"
            artifact.mkdir()
            (artifact / "junit.xml").write_text('''<testsuite>
              <testcase classname="Route" name="uploads"/>
              <testcase classname="Route" name="fails"><failure message="bad route"/></testcase>
              <testcase classname="Route" name="skips"><skipped/></testcase>
              <testcase classname="Route" name="errors"><error/></testcase>
              <testcase classname="Route" name="flaky"><flakyFailure/></testcase>
            </testsuite>''')
            (artifact / "coverage.xml").write_text('<coverage><lines/></coverage>')
            cases, results, namespaces = report.read_reports(root)
            self.assertEqual(namespaces, ["python-repository-tools"])
            self.assertEqual([r['status'] for r in results], ['pass', 'fail', 'skip', 'error', 'fail'])
            self.assertEqual(cases[0]['id'], 'python-repository-tools::Route::uploads')
            artifact.rename(root / 'python-repository-tools-8')
            self.assertEqual(report.read_reports(root)[0], cases)

    def test_duplicate_or_empty_results_cannot_create_green_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / 'rust-test-1'
            artifact.mkdir()
            path = artifact / 'junit.xml'
            path.write_text('<testsuite><testcase name="x"/><testcase name="x"/></testsuite>')
            with self.assertRaisesRegex(ValueError, 'Ambiguous'):
                report.read_reports(root)
            path.write_text('<testsuite/>')
            with self.assertRaisesRegex(ValueError, 'No native'):
                report.read_reports(root)
            path.write_text('<testsuite><testcase/></testsuite>')
            with self.assertRaisesRegex(ValueError, 'Unnamed'):
                report.read_reports(root)

    def test_swift_uses_native_case_identity_and_unknown_status_is_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / 'ios-tests-coverage-1'
            artifact.mkdir()
            (artifact / 'tests.json').write_text(json.dumps({'testNodes': [{
                'nodeType': 'Test Suite', 'children': [
                    {'nodeType': 'Test Case', 'nodeIdentifierURL': 'test://app/Target/Route/upload', 'result': 'Passed'},
                    {'nodeType': 'Test Case', 'nodeIdentifierURL': 'test://app/Target/Route/download', 'result': 'Unknown'},
                ]}]}))
            cases, results, _ = report.read_reports(root)
            self.assertEqual(cases[0]['id'], 'ios-tests-coverage::test://app/Target/Route::upload')
            self.assertEqual([r['status'] for r in results], ['pass', 'error'])
