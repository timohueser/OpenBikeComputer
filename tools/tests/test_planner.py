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
    def test_interrupted_routing_setup_leaves_no_partial_package(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "maps").mkdir()
            source = root / "bw.osm.pbf"
            source.write_bytes(b"input")
            args = argparse.Namespace(data_dir=root, pmtiles="pmtiles", osm=source, region=planner.REGION,
                                      bounds=maps.bounds(maps.BW_BOUNDS),
                                      dem_dir=root / "dem", reference=None)

            def run(*command):
                if str(command[0]).endswith("/route-build"):
                    output = Path(command[command.index("--output") + 1])
                    output.with_suffix(".building-test").mkdir()
                    raise KeyboardInterrupt

            with patch.object(planner, "run", side_effect=run), \
                 patch.object(planner.shutil, "which", return_value="tool"), \
                 patch.object(maps, "DATA", root / "maps"):
                with self.assertRaises(KeyboardInterrupt):
                    planner.setup(args)
            self.assertEqual({p.name for p in root.iterdir()}, {"maps", "bw.osm.pbf"})

    def test_freiburg_package_cannot_pass_as_baden_wuerttemberg(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "routing").mkdir()
            (root / "routing/manifest.json").write_text(json.dumps({
                "region": "freiburg", "bounds": [7.5, 47.7, 8.5, 48.4],
            }))
            with patch.object(maps, "check_bundle", return_value={"bounds": maps.bounds(maps.BW_BOUNDS)}):
                with self.assertRaisesRegex(ValueError, "route package must cover baden-wuerttemberg"):
                    planner.verify(argparse.Namespace(data_dir=root, region=planner.REGION,
                                                      bounds=maps.bounds(maps.BW_BOUNDS)))

    def test_test_region_setup_refuses_another_region_folder_and_bw_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "routing").mkdir()
            (root / "routing/manifest.json").write_text(json.dumps({"region": planner.REGION}))
            for extra in [["--data-dir", str(root)], ["--data-dir", str(root / "engadin"), "--osm", "bw.osm.pbf"]]:
                with self.subTest(extra=extra), patch.object(sys, "argv", ["planner", "setup", "--region", "engadin", *extra]), \
                     patch.object(planner, "setup") as setup, patch("sys.stderr"):
                    with self.assertRaises(SystemExit):
                        planner.main()
                    setup.assert_not_called()

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
