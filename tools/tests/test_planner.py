"""The local launcher rejects incomplete coverage and cleans up a failed start."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools import planner, planner_maps as maps


class PlannerTests(unittest.TestCase):
    def test_freiburg_package_cannot_pass_as_baden_wuerttemberg(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "routing").mkdir()
            (root / "routing/manifest.json").write_text(json.dumps({
                "region": "freiburg", "bounds": [7.5, 47.7, 8.5, 48.4],
            }))
            with patch.object(maps, "check_bundle", return_value={"bounds": maps.bounds(maps.BW_BOUNDS)}):
                with self.assertRaisesRegex(ValueError, "route package must cover Baden-Württemberg"):
                    planner.verify(argparse.Namespace(data_dir=root))

    def test_a_failed_start_stops_services_already_started(self):
        children = []
        real_popen = subprocess.Popen

        def launch(*args, **kwargs):
            if children:
                raise FileNotFoundError("missing second service")
            child = real_popen(*args, **kwargs)
            children.append(child)
            return child

        commands = [([sys.executable, "-c", "import time; time.sleep(60)"], planner.ROOT)] * 2
        with patch.object(maps.subprocess, "Popen", side_effect=launch):
            with self.assertRaisesRegex(FileNotFoundError, "second service"):
                maps.supervise(commands, os.environ.copy())
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].poll())


if __name__ == "__main__":
    unittest.main()
