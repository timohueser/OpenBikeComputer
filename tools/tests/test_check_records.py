import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "check_records.py"
SPEC = importlib.util.spec_from_file_location("check_records", MODULE_PATH)
records = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = records
SPEC.loader.exec_module(records)


class SignatureTests(unittest.TestCase):
    def test_an_issue_number_is_a_signature_and_a_colour_is_not(self):
        for line in ("fixed in #1234", "(epic #1068 / #1145 §3)", "see issue #904."):
            self.assertTrue(records.ISSUE.search(line), line)
        for line in ('fill:#000', 'stroke="#3d3427"', "&#9654;", "#[cfg(test)]", "a/#123/b", "color=#ffaa00", "P2.00 #1"):
            self.assertFalse(records.ISSUE.search(line), line)

    def test_comment_lines_include_python_docstrings(self):
        text = '"""Module doc\nmore doc (#12)\n"""\nx = 1  # code\n# a comment\n'
        lines = [n for n, _ in records.comment_lines("tool.py", text)]
        self.assertEqual(lines, [1, 2, 3, 5])

    def test_rust_doc_and_line_comments_only(self):
        text = "//! crate\n/// item\nfn f() {}\n// note\nlet s = \"#1234\";\n"
        lines = [n for n, _ in records.comment_lines("a.rs", text)]
        self.assertEqual(lines, [1, 2, 4])


if __name__ == "__main__":
    unittest.main()
