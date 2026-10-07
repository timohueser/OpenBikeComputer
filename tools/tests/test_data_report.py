"""A report changes only when structured read-only evidence changes."""

import json
import subprocess
import unittest
from unittest.mock import patch
from tools import data_report as report


class DataReport(unittest.TestCase):
    def status(self):
        return {'products': [{'product': 'planner', 'release': 'a' * 64,
                             'layers': [{'layer': 'planner/data', 'state': 'ok', 'reason': None}]}],
                'attention': [], 'check': {'drift': [], 'leftovers': []},
                'vps': {'host': {'installed': []}, 'unavailable': None,
                        'services': [{'service': name, 'ready': True, 'reason': None}
                                     for name in ('routing', 'search', 'downloads')]}}

    def test_drift_exit_preserves_the_document_and_failed_checks_cannot_clear(self):
        status = self.status()
        status['check']['drift'] = [{'key': 'planner/objects/missing'}]
        with patch.object(report.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, json.dumps(status), '')):
            observed = report.read_status()
        clear, body = report.summarize(observed)
        self.assertFalse(clear)
        self.assertIn('1 missing/changed', body)
        for code, data in [(4, {'error': 'not configured'}), (1, {'error': 'listing failed'})]:
            with patch.object(report.subprocess, 'run', return_value=subprocess.CompletedProcess([], code, json.dumps(data), '')), self.assertRaises(ValueError):
                report.read_status()

    def test_only_complete_clear_evidence_closes_and_unchanged_body_needs_no_comment(self):
        clear, body = report.summarize(self.status())
        self.assertTrue(clear)
        issue = {'number': 1, 'body': body, 'state': 'OPEN'}
        self.assertEqual(report.issue_changes([issue], clear, body), [('close', None)])
        issue['state'] = 'CLOSED'
        self.assertEqual(report.issue_changes([issue], clear, body), [])
        self.assertEqual(report.issue_changes([], clear, body), [])
        for cause in ('code_changed', 'not_applied', 'input_changed', 'blocked', 'stale'):
            status = self.status()
            status['products'][0]['layers'][0].update(state=cause, reason='specific selected change')
            clear, body = report.summarize(status)
            self.assertFalse(clear)
            self.assertIn(cause, body)
            self.assertEqual(report.issue_changes([], clear, body), [('create', body)])
            opened = {'body': body, 'state': 'OPEN'}
            self.assertEqual(report.issue_changes([opened], clear, body), [])
            opened['state'] = 'CLOSED'
            self.assertEqual(report.issue_changes([opened], clear, body), [('reopen', None)])
        for key in ('vps', 'check', 'products'):
            status = self.status()
            status.pop(key)
            self.assertFalse(report.summarize(status)[0])
        status = self.status()
        status['vps']['services'][0]['ready'] = False
        self.assertFalse(report.summarize(status)[0])
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            report.issue_changes([issue, issue], False, body)
