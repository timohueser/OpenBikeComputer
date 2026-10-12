"""Local planner data refuses incomplete coverage and another region."""

import argparse
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import planner, planner_maps as maps


class PlannerTests(unittest.TestCase):
    def test_freiburg_package_cannot_pass_as_the_region(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "routing").mkdir()
            (root / "routing/manifest.json").write_text(json.dumps({
                "region": "freiburg", "bounds": [7.5, 47.7, 8.5, 48.4],
            }))
            with patch.object(maps, "check_bundle", return_value={"bounds": [5.95, 45.8, 10.5, 49.85]}):
                with self.assertRaisesRegex(ValueError, f"route package must cover {planner.REGION}"):
                    planner.verify(argparse.Namespace(data_dir=root, region=planner.REGION,
                                                      bounds=[5.95, 45.8, 10.5, 49.85]))

    def test_setup_gets_a_fresh_directory_and_refuses_another_region(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "data"
            contents = []
            with patch.dict(planner.COMMANDS, setup=lambda args: contents.extend(args.data_dir.glob("*"))):
                planner.main(["setup", "--data-dir", str(root)])
            self.assertEqual(contents, [])
            (root / "routing").mkdir(parents=True)
            (root / "routing/manifest.json").write_text(json.dumps({"region": "freiburg"}))
            with patch.dict(planner.COMMANDS, setup=lambda args: self.fail("setup ran")), patch("sys.stderr"):
                with self.assertRaises(SystemExit):
                    planner.main(["setup", "--data-dir", str(root)])


if __name__ == "__main__":
    unittest.main()
