"""Incomplete releases cannot become public planner data."""

import argparse
from contextlib import closing
import hashlib
import json
from pathlib import Path
import re
import tempfile
import sqlite3
import unittest
from unittest.mock import patch

from tools import planner_bake, planner_release as release, planner_prepare


class ReleaseTests(unittest.TestCase):
    def test_search_validation_rejects_old_or_missing_address_indexes_before_preparation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            database = root / 'search/baden-wuerttemberg-switzerland.sqlite'
            database.parent.mkdir()
            with sqlite3.connect(database) as db:
                db.execute('CREATE TABLE metadata(key TEXT,value TEXT)')
                db.execute("INSERT INTO metadata VALUES('schema','1')")
            for schema in [1, 2, 3, 4, 5, 6]:
                with sqlite3.connect(database) as db:
                    db.execute("UPDATE metadata SET value=? WHERE key='schema'", (str(schema),))
                with self.assertRaisesRegex(ValueError, 'Rebuild search package'):
                    release.search_metadata(database)
                with patch.object(planner_bake.maps, 'run') as run:
                    with self.assertRaisesRegex(ValueError, 'Rebuild search package'):
                        planner_bake.prepare(argparse.Namespace(data_dir=root, recipe=release.maps.ROOT / 'tools/planner-regions/baden-wuerttemberg-switzerland.json'))
                    run.assert_not_called()
            with sqlite3.connect(database) as db:
                db.execute('CREATE TABLE addresses(lat REAL,lon REAL)')
                db.execute('CREATE INDEX address_cells ON addresses(CAST((lat+90)*200 AS INTEGER)*72001+CAST((lon+180)*200 AS INTEGER))')
            self.assertEqual(release.search_metadata(database, full=True)['schema'], 6)

    def test_changed_or_missing_bytes_fail_release_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = b"immutable"
            (root / "page.bin").write_bytes(data)
            (root / "release.json").write_bytes(release.encoded({"format": 1, "region": "test", "files": {
                "page.bin": {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}}}))
            identity, _ = release.release(root)
            self.assertEqual(len(identity), 64)
            (root / "page.bin").write_bytes(b"different")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                release.release(root)
            (root / "page.bin").unlink()
            with self.assertRaises(FileNotFoundError): release.release(root)

    def test_seal_accepts_the_routing_format_that_the_engine_writes(self):
        source = (release.maps.ROOT / "planner/router/src/package.rs").read_text()
        engine_format = int(re.search(r"pub const FORMAT: u32 = (\d+);", source)[1])
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory)
            (data / "routing").mkdir()
            (data / "routing/manifest.json").write_text(json.dumps({"format": engine_format, "region": "test"}))
            with closing(sqlite3.connect(data / "routing/overlays.sqlite")) as db:
                db.execute("CREATE TABLE metadata(package TEXT, coverage TEXT)")
                db.execute("INSERT INTO metadata VALUES ('another', '[]')")
                db.commit()
            with self.assertRaisesRegex(ValueError, "for this region"):
                release.seal(data, "other", None, {})
            # The next check after the manifest fails, so the format passed.
            with self.assertRaisesRegex(ValueError, "Overlay index uses another routing package"):
                release.seal(data, "test", None, {})

    def test_interrupted_grid_database_is_rebuilt_before_publication(self):
        from tools.planner_blocks import stage_sqlite
        import sqlite3
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "cell.sqlite"
            def interrupted(path):
                path.write_bytes(b"partial database")
                raise OSError("interrupted")
            with self.assertRaises(OSError): stage_sqlite(target, interrupted)
            self.assertFalse(target.exists())
            self.assertFalse(target.with_suffix(".sqlite.partial").exists())
            def build(path):
                db = sqlite3.connect(path)
                try: db.execute("CREATE TABLE data(id INTEGER)"); db.commit()
                finally: db.close()
            stage_sqlite(target, build)
            self.assertTrue(target.is_file())

    def test_grid_release_uses_one_pool_and_materializes_verified_runtime_files(self):
        from tools import planner_offline as offline, planner_runtime as runtime
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / "places.sqlite"
            raw.write_bytes(b"indexed places" * 1000)
            entry = offline.pack_file(raw, root / "objects")
            self.assertEqual(entry["transport"]["encoding"], "gzip")
            document = {"format": 1, "region": "test", "grid": {"format": 2, "zoom": 9, "map_zoom": 11},
                "files": {"search/tiles/9-1-1.sqlite": entry, "search/tiles/9-1-2.sqlite": entry}}
            (root / "release.json").write_bytes(runtime.encoded(document))
            self.assertEqual(len(runtime.storage_files(document)), 1)
            _, verified = runtime.release(root)
            self.assertEqual(verified, document)
            offline.materialize(root, root / "runtime", verified, ("search/",))
            for name in document["files"]:
                self.assertEqual((root / "runtime" / name).read_bytes(), raw.read_bytes())
            cell = runtime.public_metadata({**document, "files": {"routes/tiles/9-1-1.json": entry}})["public/routes/tiles/9-1-1.json.json"]
            self.assertEqual(json.loads(cell), {**entry["transport"], "decoded_bytes": entry["bytes"]})
            (root / "objects" / entry["transport"]["sha256"]).write_bytes(b"corrupt")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                runtime.release(root)

    def test_each_region_recipe_is_valid_and_named_by_its_file(self):
        for path in (release.maps.ROOT / "tools/planner-regions").glob("*.json"):
            self.assertEqual(planner_prepare.recipe(path)["region"], path.stem)

    def test_recipe_names_explicit_profiles_and_rejects_invalid_selection(self):
        config = json.loads((release.maps.ROOT / "tools/planner-regions/baden-wuerttemberg-switzerland.json").read_bytes())
        expected = {bike + suffix for bike in ["touring", "road", "gravel", "mtb", "hiking"]
                    for suffix in ["", "/less-climbing", "/shorter"]}
        self.assertEqual(set(config["profiles"]), expected)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "region.json"
            for profiles in [[], ["all"], ["road", "road"], ["mtb/quieter"], [{}]]:
                path.write_text(json.dumps({**config, "profiles": profiles}))
                with self.assertRaisesRegex(ValueError, "unique routing profile"):
                    planner_prepare.recipe(path)


if __name__ == "__main__": unittest.main()
