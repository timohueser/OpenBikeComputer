import importlib.util
import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).parents[1] / "prose.py"
SPEC = importlib.util.spec_from_file_location("prose", MODULE_PATH)
prose = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = prose
SPEC.loader.exec_module(prose)


class WordCountTests(unittest.TestCase):
    def test_only_running_text_counts(self):
        text = (
            "---\ncopy: ai\n---\n"
            "# A heading\n"
            "One two three.\n"
            "| a | table | row |\n"
            "```\ncode code code\n```\n"
            "<!-- four five -->\n"
            "<svg>six</svg>\n"
            "seven eight\n"
        )
        self.assertEqual(prose.words(text), 5)


class UpdateTests(unittest.TestCase):
    def run_update(self, found, base, *paths):
        with tempfile.TemporaryDirectory() as tmp:
            baseline = Path(tmp) / "baseline.json"
            baseline.write_text(json.dumps(base))
            argv = ["prose.py", "--update", *paths]
            with mock.patch.object(prose, "corpus", return_value=found), \
                 mock.patch.object(prose, "BASELINE", baseline), \
                 mock.patch.object(sys, "argv", argv), redirect_stdout(io.StringIO()):
                self.assertEqual(prose.main(), 0)
            return json.loads(baseline.read_text())

    def test_a_shrink_is_recorded_and_growth_is_not(self):
        found = {"a/README.md": ("readme", 500), "b/README.md": ("readme", 950)}
        merged = self.run_update(found, {"a/README.md": 700, "b/README.md": 900})
        self.assertEqual(merged, {"a/README.md": 500, "b/README.md": 900})

    def test_a_new_over_cap_file_needs_to_be_named(self):
        found = {"new/README.md": ("readme", 5000), "small/README.md": ("readme", 100)}
        self.assertEqual(self.run_update(found, {}), {"small/README.md": 100})
        self.assertEqual(
            self.run_update(found, {}, "new/README.md"),
            {"new/README.md": 5000, "small/README.md": 100},
        )


if __name__ == "__main__":
    unittest.main()
