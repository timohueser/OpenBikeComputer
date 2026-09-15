from pathlib import Path
import tempfile
import unittest
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import coverage_report as report


class CoverageReportTests(unittest.TestCase):
    def test_lcov_keeps_zero_hits_and_unions_duplicate_records(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / 'coverage.info'
            raw.write_text('SF:src/a.rs\nDA:2,0\nDA:3,1\nend_of_record\nSF:src/a.rs\nDA:2,4\nDA:4,0\nend_of_record\nSF:/outside/file.rs\nDA:1,8\nend_of_record\n')
            self.assertEqual(report.read_lcov(raw, root, root), {'src/a.rs': {2: True, 3: True, 4: False}})
            raw.write_text('SF:/ci/checkout/src/a.rs\nDA:2,1\nend_of_record\n')
            self.assertEqual(report.read_lcov(raw, root, root, Path('/ci/checkout')), {'src/a.rs': {2: True}})
            raw.write_text('')
            with self.assertRaisesRegex(ValueError, 'no repository files'):
                report.read_lcov(raw, root, root)

    def test_rust_excludes_test_syntax_without_losing_production_after_it(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'mod.rs'
            path.write_text('pub fn before() {}\n#[cfg(test)]\nmod cases {\n fn helper() { let _ = r#"} {"#; }\n}\n#[cfg(test)]\nmod oracle;\npub fn after() {}\n')
            excluded, executable, modules = report.rust_source(path)
            self.assertEqual(excluded, {3, 4, 5, 7})
            self.assertTrue(executable)
            self.assertEqual(modules, [path.parent / 'oracle.rs', path.parent / 'oracle'])
            path.write_text('pub struct Declaration;\n#[cfg(test)]\nmod tests { fn test_only() {} }\n')
            self.assertFalse(report.rust_source(path)[1])

    def test_component_inventory_excludes_test_items_and_rejects_empty_function_source(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'src').mkdir()
            (root / 'src/lib.rs').write_text('pub fn production() {}\n#[cfg(test)]\nmod oracle;\n#[cfg(test)]\nmod tests { fn helper() {} }\n')
            (root / 'src/oracle.rs').write_text('pub fn reference_model() {}\n')
            (root / 'src/empty.rs').write_text('pub fn unmeasured() {}\n')
            (root / 'src/types.rs').write_text('pub struct Declaration;\n')
            policy = {'component': [{'id': 'storage', 'include': ['src/**'], 'enforcement': 'ratchet'}]}
            tracked = b'src/lib.rs\0src/oracle.rs\0src/empty.rs\0src/types.rs\0'
            native = {'src/lib.rs': {1: True, 5: True}, 'src/oracle.rs': {1: True}, 'src/empty.rs': {}}
            with patch.object(report.subprocess, 'check_output', return_value=tracked):
                row, = report.summarize(root, policy, 'rust', native, {})
            self.assertEqual((row['covered'], row['total']), (1, 1))
            self.assertEqual(row['excluded'], ['src/oracle.rs'])
            self.assertEqual(row['unmeasured'], ['src/empty.rs'])
            self.assertEqual(row['declarations_only'], ['src/types.rs'])

    def test_ratchet_requires_every_critical_component_and_exact_fraction(self):
        rows = [{'id': name, 'covered': 2, 'total': 3, 'unmeasured': []} for name in report.CRITICAL]
        baseline = {'source_sha': 'a' * 40, 'evidence': 'https://example.test/run', 'tools': ['native=1'],
                    'components': {name: {'covered': 2, 'total': 3} for name in report.CRITICAL}}
        self.assertEqual(report.check_baseline(rows, baseline), [])
        rows[0]['covered'], rows[0]['total'] = 666666, 1000000
        self.assertIn('line coverage decreased', '\n'.join(report.check_baseline(rows, baseline)))
        rows[0]['covered'], rows[0]['total'] = 2, 3
        rows[0]['unmeasured'] = ['new.rs']
        self.assertIn('unmeasured production source', '\n'.join(report.check_baseline(rows, baseline)))
        self.assertIn('missing or empty coverage', '\n'.join(report.check_baseline(rows[1:], baseline)))
        self.assertIn('no accepted measured baseline', '\n'.join(report.check_baseline(rows, {})))
        rows[0]['total'] = 0
        self.assertIn('missing or empty coverage', '\n'.join(report.check_baseline(rows, baseline)))

    def test_xccov_does_not_add_overlapping_target_percentages(self):
        import json
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / 'xccov.json'
            row = {'path': str(root / 'Source.swift'), 'coveredLines': 3, 'executableLines': 5}
            document = {'targets': [{'files': [row]}, {'files': [dict(row)]}]}
            raw.write_text(json.dumps(document))
            self.assertEqual(report.read_xccov(raw, root), {'Source.swift': (3, 5)})
            document['targets'][1]['files'][0]['coveredLines'] = 4
            raw.write_text(json.dumps(document))
            with self.assertRaisesRegex(ValueError, 'conflicting target counts'):
                report.read_xccov(raw, root)
