import tempfile
import unittest
from pathlib import Path

from tools import step_request


class StepRequestTests(unittest.TestCase):
    def test_a_view_links_each_file_by_its_path(self):
        with tempfile.TemporaryDirectory() as temp:
            object_ = Path(temp, "object")
            object_.write_text("tile")
            view = step_request.view({"tiles/a.pbf": str(object_)}, Path(temp, "view"))
            self.assertEqual(Path(view, "tiles/a.pbf").read_text(), "tile")
            self.assertTrue(Path(view, "tiles/a.pbf").is_symlink())
            with self.assertRaisesRegex(ValueError, "not a relative path"):
                step_request.view({"../a.pbf": str(object_)}, Path(temp, "other"))


if __name__ == "__main__":
    unittest.main()
