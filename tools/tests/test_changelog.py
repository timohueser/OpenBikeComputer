import importlib.util
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).parents[1] / "changelog.py"
SPEC = importlib.util.spec_from_file_location("changelog", MODULE_PATH)
changelog = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = changelog
SPEC.loader.exec_module(changelog)

MERGED = [("2026-09", 12, "Second change"), ("2026-08", 11, "First change")]


class CheckTests(unittest.TestCase):
    def check(self, content):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "CHANGELOG.md"
            path.write_text(content)
            with mock.patch.object(changelog, "entries", return_value=MERGED), \
                 mock.patch.object(changelog, "CHANGELOG", path), \
                 mock.patch.object(sys, "argv", ["changelog.py", "--check"]), \
                 redirect_stdout(io.StringIO()):
                return changelog.main()

    def test_a_stale_file_passes(self):
        stale = changelog.render(MERGED[1:])
        self.assertEqual(self.check(stale), 0)
        self.assertEqual(self.check(changelog.render(MERGED)), 0)

    def test_a_hand_written_line_fails(self):
        edited = changelog.render(MERGED).replace("Second change", "Second change, measured at 3 MB/s")
        self.assertEqual(self.check(edited), 1)
        self.assertEqual(self.check(changelog.render(MERGED) + "\nMeasured by hand.\n"), 1)


if __name__ == "__main__":
    unittest.main()
