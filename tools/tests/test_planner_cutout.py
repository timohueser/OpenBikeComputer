"""A geographic cutout retains complete map dependencies and overlay features."""

import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from tools import planner_cutout as cutout, planner_sources as sources


class PlannerCutout(unittest.TestCase):
    def test_overlay_selection_retains_whole_crossing_features_and_properties(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = (Path(temporary) / name for name in ("source.sqlite", "target.sqlite"))
            with sqlite3.connect(source) as db:
                db.executescript("""PRAGMA user_version=2;
                    CREATE TABLE metadata(package TEXT,coverage TEXT);
                    INSERT INTO metadata VALUES('original','[-2,-2,4,4]');
                    CREATE TABLE geometries(id INTEGER PRIMARY KEY,way INTEGER,points INTEGER,coordinates BLOB);
                    CREATE TABLE attributes(id INTEGER PRIMARY KEY,properties TEXT);
                    CREATE TABLE routes(id INTEGER PRIMARY KEY,properties TEXT);
                    CREATE TABLE features(id INTEGER PRIMARY KEY,kind TEXT,geometry INTEGER,attributes INTEGER);
                    CREATE VIRTUAL TABLE bounds USING rtree(id,west,east,south,north,facet_min,facet_max);""")
                for identity, west, east in [(1, -1, 2), (2, 2, 3), (3, 1, 2)]:
                    db.execute("INSERT INTO geometries VALUES(?,?,?,?)", (identity, identity * 100, 2, bytes([identity, 0, 255])))
                    db.execute("INSERT INTO routes VALUES(?,?)", (identity, json.dumps({"id": identity, "name": "A route", "website": "https://example.com"})))
                    db.execute("INSERT INTO attributes VALUES(?,?)", (identity, json.dumps({"routes": [identity]})))
                    db.execute("INSERT INTO features VALUES(?,?,?,?)", (identity, "cycling", identity, identity))
                    db.execute("INSERT INTO bounds VALUES(?,?,?,?,?,?,?)", (identity, west, east, 0, 1, 8, 8))
            original = sources.digest(source)
            result = cutout.overlays(source, target, [0, 0, 1, 1], [0.1, 0.1, 0.9, 0.9], "new")
            self.assertEqual(result["features"], 2)
            self.assertEqual(sources.digest(source), original)
            with sqlite3.connect(source) as before, sqlite3.connect(target) as after:
                self.assertEqual(after.execute('PRAGMA user_version').fetchone(), (2,))
                self.assertEqual(after.execute("SELECT * FROM features ORDER BY id").fetchall(),
                                 before.execute("SELECT * FROM features WHERE id IN (1,3) ORDER BY id").fetchall())
                self.assertEqual(after.execute("SELECT * FROM bounds ORDER BY id").fetchall(),
                                 before.execute("SELECT * FROM bounds WHERE id IN (1,3) ORDER BY id").fetchall())
                for name in ('geometries', 'attributes', 'routes'):
                    self.assertEqual(after.execute(f"SELECT * FROM {name} ORDER BY id").fetchall(),
                                     before.execute(f"SELECT * FROM {name} WHERE id IN (1,3) ORDER BY id").fetchall())
                package, coverage = after.execute("SELECT * FROM metadata").fetchone()
                self.assertEqual(package, "new")
                self.assertEqual(json.loads(coverage), [0.1, 0.1, 0.9, 0.9])

    def test_invalid_region_cannot_escape_the_staging_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "output"
            with self.assertRaisesRegex(ValueError, "region"):
                cutout.prepare(Path(temporary) / "absent", target, [0, 0, 1, 1], "../../escape")
            self.assertFalse(target.exists())


if __name__ == "__main__":
    unittest.main()
