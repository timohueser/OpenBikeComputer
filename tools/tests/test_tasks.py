"""Task discovery and help use the same native parser as recipe execution."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from tools import tasks


SCRIPT = Path(__file__).parents[1] / "tasks.py"
REAL_JUSTFILE = SCRIPT.parent / "justfile"


class TasksTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.justfile = self.root / "justfile"
        (self.root / "imported.just").write_text(
            "[doc('Run the imported task.')]\n[group('run')]\nimported:\n    @echo ignored\n"
        )
        self.justfile.write_text("""import 'imported.just'

[group('agent')]
agent-task:
    @echo ignored

[doc('Flash the board.')]
[group('device')]
flash: imported
    @touch executed

[private]
default:
    @echo ignored
""")

    def cli(self, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--justfile", str(self.justfile), *args],
            cwd=self.root, capture_output=True, text=True,
        )

    def test_native_metadata_includes_imports_and_docs_but_not_private_recipes(self):
        found = tasks.load(self.justfile)
        self.assertEqual(set(found), {"imported", "flash", "agent-task"})
        self.assertEqual(found["imported"], tasks.Task("run", "Run the imported task."))
        self.assertEqual(found["flash"], tasks.Task("device", "Flash the board."))
        self.assertFalse((self.root / "executed").exists())

    def test_listing_and_completion_preserve_agent_scopes(self):
        for flags, expected in [
            ((), {"imported", "flash"}),
            (("--agent",), {"agent-task"}),
            (("--all",), {"imported", "flash", "agent-task"}),
        ]:
            with self.subTest(flags=flags):
                result = self.cli("--names", *flags)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(set(result.stdout.split()), expected)
        listed = self.cli()
        self.assertIn("Run the imported task.", listed.stdout)
        self.assertIn("agent tasks: obc --agent", listed.stdout)
        self.assertNotIn("agent-task", listed.stdout)

    def test_help_shows_native_recipe_without_executing_it(self):
        result = self.cli("--task", "flash")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Flash the board.", result.stdout)
        self.assertIn("flash: imported", result.stdout)
        self.assertFalse((self.root / "executed").exists())
        self.assertNotEqual(self.cli("--task", "missing").returncode, 0)

    def test_real_tasks_have_sentence_summaries(self):
        for name, task in tasks.load(REAL_JUSTFILE).items():
            with self.subTest(task=name):
                self.assertRegex(task.doc, r"^[A-Z]")
                self.assertFalse(task.doc.startswith("Args"))


if __name__ == "__main__":
    unittest.main()
