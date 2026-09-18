import importlib.util
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("docs_review", Path(__file__).parents[1] / "docs_review.py")
review = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(review)


class DocsReviewTests(unittest.TestCase):
    def test_source_links_directory_links_and_nearest_readme(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            (root / "firmware/app/src").mkdir(parents=True)
            (root / "docs").mkdir()
            (root / "firmware/app/README.md").write_text("Setup")
            (root / "README.md").write_text("Overview")
            (root / "docs/design.md").write_text(
                "[owner](src:firmware/app/src/removed.rs#owner)\n"
                "[src:firmware/app/src/new.rs]\n"
                "[module](../firmware/app/src/)\n"
                "[external](https://example.com/firmware/other.rs)\n"
            )
            changed = {"firmware/app/src/removed.rs", "firmware/app/src/new.rs", "firmware/other.rs"}
            queue = review.review_queue(root, changed, ["README.md", "docs/design.md"])
            self.assertEqual(queue["docs/design.md"], changed - {"firmware/other.rs"})
            self.assertEqual(queue["firmware/app/README.md"], changed - {"firmware/other.rs"})
            self.assertEqual(queue["README.md"], {"firmware/other.rs"})
