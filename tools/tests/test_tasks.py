from pathlib import Path
import tempfile
import unittest

from tools import tasks


JUSTFILE = """
# Run the simulator.
[group('run')]
sim *args:
    @echo sim

# Flash the board.
[group('device')]
flash:
    @echo flash

# Line budget.
[group('agent')]
loc-ledger:
    @echo ledger

# Ungrouped for now.
stray:
    @echo stray

[private]
default:
    @echo default
"""


class TasksTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.justfile = Path(self.scratch.name) / "justfile"
        self.justfile.write_text(JUSTFILE, encoding="utf-8")
        self.tasks = tasks.load(self.justfile)
        self.addCleanup(self.scratch.cleanup)

    def test_load_reads_the_group_and_doc_and_drops_private_tasks(self):
        self.assertEqual(self.tasks["sim"], ("run", "Run the simulator."))
        self.assertEqual(self.tasks["loc-ledger"][0], "agent")
        self.assertEqual(self.tasks["stray"][0], "")
        self.assertNotIn("default", self.tasks)

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
