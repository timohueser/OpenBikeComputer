from contextlib import redirect_stdout
import io
from pathlib import Path
import tempfile
import unittest

from tools import docs_copy


def page(copy: str, body: str) -> str:
    return f"---\ntitle: Test\ndescription: Test page.\ncopy: {copy}\n---\n\n{body}\n"


class DocsCopyTests(unittest.TestCase):
    def test_status_reports_each_state_and_pending_review(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            (root / "ai.md").write_text(page("ai", "# Draft\n\nDraft copy."), encoding="utf-8")
            (root / "human.md").write_text(
                page(
                    "human",
                    "# Finished\n\n<!-- copy-review:\nThe behavior changed.\n-->\n\nHuman copy.",
                ),
                encoding="utf-8",
            )
            (root / "mixed.md").write_text(
                page(
                    "mixed",
                    "# Partial\n\n<!-- human-copy:start -->\nHuman copy.\n<!-- human-copy:end -->",
                ),
                encoding="utf-8",
            )

            output = io.StringIO()
            with redirect_stdout(output):
                result = docs_copy.main(["status", "--content", str(root)])

            self.assertEqual(result, 0)
            self.assertIn("Human:   1", output.getvalue())
            self.assertIn("Mixed:   1 (1 human-owned blocks)", output.getvalue())
            self.assertIn("AI draft: 1", output.getvalue())
            self.assertIn("human.md:9 — Finished", output.getvalue())
            self.assertIn("The behavior changed.", output.getvalue())

    def test_every_page_declares_copy_ownership(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            (root / "missing.md").write_text("---\ntitle: Missing\n---\n", encoding="utf-8")

            pages, errors = docs_copy.scan(root)

            self.assertEqual(pages, [])
            self.assertEqual(errors, ["missing.md: front matter needs copy: ai|mixed|human"])

    def test_mixed_copy_requires_balanced_nonempty_human_blocks(self):
        cases = {
            "none.md": (page("mixed", "# None\n\nDraft copy."), "needs at least one"),
            "open.md": (page("mixed", "# Open\n\n<!-- human-copy:start -->\nText."), "has no end"),
            "empty.md": (
                page("mixed", "# Empty\n\n<!-- human-copy:start -->\n<!-- human-copy:end -->"),
                "is empty",
            ),
        }
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            for name, (content, _expected) in cases.items():
                (root / name).write_text(content, encoding="utf-8")

            _pages, errors = docs_copy.scan(root)

            for name, (_content, expected) in cases.items():
                self.assertTrue(any(error.startswith(name) and expected in error for error in errors), errors)


if __name__ == "__main__":
    unittest.main()
