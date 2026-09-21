from pathlib import Path
import re
import tempfile
import unittest

from tools import tasks


REAL_JUSTFILE = Path(__file__).parents[1] / "justfile"


JUSTFILE = """
set shell := ["bash", "-c"]
lib := justfile_directory() / "helper.sh"
export OBC_ROOT := parent_directory(justfile_directory())

# Run the simulator.
[group('run')]
sim *args:
    @echo sim
    # an indented comment is body text, not a doc
    cd "$OBC_ROOT"; run this:

# The first line of the block.
# Flash the board.
[group('device')]
flash:
    @echo flash

# This comment is broken off by the empty line.

[group('agent')]
loc-ledger:
    @echo ledger

# Ungrouped for now.
stray:
    @echo stray

# Never listed.
[private]
default:
    @echo default
"""


class TasksTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        justfile = Path(scratch.name) / "justfile"
        justfile.write_text(JUSTFILE, encoding="utf-8")
        self.tasks = tasks.load(justfile)

    def test_reads_the_group_and_doc_and_drops_private_tasks(self):
        self.assertEqual(self.tasks["sim"], tasks.Task("run", ("Run the simulator.",)))
        self.assertEqual(self.tasks["loc-ledger"].group, "agent")
        self.assertEqual(self.tasks["stray"].group, "")
        self.assertNotIn("default", self.tasks)

    def test_reads_no_task_from_an_assignment_a_setting_or_a_recipe_body(self):
        self.assertEqual(set(self.tasks), {"sim", "flash", "loc-ledger", "stray"})

    def test_the_last_comment_line_is_the_doc_and_an_empty_line_breaks_the_block(self):
        self.assertEqual(self.tasks["flash"].doc, "Flash the board.")
        self.assertEqual(self.tasks["loc-ledger"].doc, "")

    def test_describe_leads_with_the_summary_and_indents_the_rest_of_the_block(self):
        self.assertEqual(
            tasks.describe("flash", self.tasks["flash"]),
            "obc flash  Flash the board.\n\n  The first line of the block.",
        )
        self.assertEqual(tasks.describe("sim", self.tasks["sim"]), "obc sim  Run the simulator.")

    def test_every_real_task_summary_is_a_sentence_not_a_trailing_fragment(self):
        for name, task in tasks.load(REAL_JUSTFILE).items():
            with self.subTest(task=name):
                self.assertRegex(task.doc, re.compile(r"^[A-Z]"), f"{name}: summary is not a sentence")
                self.assertFalse(task.doc.startswith("Args"), f"{name}: summary is only its arguments")

    def test_a_justfile_with_no_task_is_an_error(self):
        with tempfile.TemporaryDirectory() as scratch:
            empty = Path(scratch) / "justfile"
            empty.write_text("x := 1\n", encoding="utf-8")
            with self.assertRaises(SystemExit):
                tasks.load(empty)

    def test_the_everyday_listing_hides_the_agent_group_and_points_at_it(self):
        text = tasks.render(self.tasks, lambda group: group != tasks.AGENT, "agent tasks: obc --agent")
        self.assertIn("sim", text)
        self.assertNotIn("loc-ledger", text)
        self.assertIn("agent tasks: obc --agent", text)

    def test_the_agent_listing_holds_the_agent_group_alone(self):
        text = tasks.render(self.tasks, lambda group: group == tasks.AGENT)
        self.assertIn("loc-ledger", text)
        self.assertNotIn("flash", text)

    def test_groups_print_in_the_declared_order_and_a_new_group_prints_last(self):
        rows = tasks.grouped(self.tasks, lambda _: True)
        self.assertEqual([group for group, _ in rows], ["run", "device", "agent", ""])


if __name__ == "__main__":
    unittest.main()
