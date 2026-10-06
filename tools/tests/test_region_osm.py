"""Area unions retain unique objects and the highest supplied duplicate version."""

import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

from tools import region_osm


class RegionOsm(unittest.TestCase):
    @unittest.skipUnless(shutil.which(os.environ.get("OBC_OSMIUM", "osmium")) or os.environ.get("CI"),
                         "Prepared Osmium is unavailable")
    def test_different_dates_union_objects_deterministically_without_duplicate_ids(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, _ = region_osm.probe()
            objects = [
                '<node id="1" version="1" timestamp="2026-01-01T00:00:00Z" lat="47" lon="7"/>'
                '<node id="2" version="1" timestamp="2026-01-01T00:00:00Z" lat="47" lon="8"/>'
                '<way id="10" version="1" timestamp="2026-01-01T00:00:00Z"><nd ref="1"/><nd ref="2"/></way>',
                '<node id="1" version="2" timestamp="2026-02-01T00:00:00Z" lat="48" lon="7"/>'
                '<node id="3" version="1" timestamp="2026-02-01T00:00:00Z" lat="48" lon="8"/>'
                '<way id="11" version="1" timestamp="2026-02-01T00:00:00Z"><nd ref="1"/><nd ref="3"/></way>',
            ]
            inputs = []
            for number, body in enumerate(objects):
                source = root / f"{number}.osm"
                source.write_text(f'<osm version="0.6">{body}</osm>')
                output = root / f"{number}.osm.pbf"
                region_osm.run(binary, "cat", source, "-o", output)
                inputs.append(output)
            first, second = root / "first.osm.pbf", root / "second.osm.pbf"
            region_osm.union(binary, inputs, first)
            region_osm.union(binary, list(reversed(inputs)), second)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            lines = region_osm.run(binary, "cat", first, "-f", "opl").decode().splitlines()
            self.assertEqual([line.split()[0] for line in lines], ["n1", "n2", "n3", "w10", "w11"])
            self.assertTrue(lines[0].startswith("n1 v2 "))
            self.assertIn("y48", lines[0])
            info = json.loads(region_osm.run(binary, "fileinfo", "--extended", "--json", first))
            self.assertFalse(info["data"]["multiple_versions"])

    def test_changed_prepared_tool_refuses_before_reading_area_bytes(self):
        request = {"options": {"osmium": {"sha256": "old", "version": "old"}}}
        with patch.object(region_osm, "probe", return_value=(Path("osmium"), {"sha256": "new"})), \
                patch.object(region_osm, "union") as union:
            with self.assertRaisesRegex(ValueError, "Prepared Osmium changed"):
                region_osm.step(request)
            union.assert_not_called()
