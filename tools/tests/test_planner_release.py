"""Incomplete releases cannot become public planner data."""

import argparse
import hashlib
import json
from pathlib import Path
import tempfile
import sqlite3
import unittest
from unittest.mock import patch

from tools import planner_release as release, planner_prepare, planner_deploy, r2


class ReleaseTests(unittest.TestCase):
    def test_search_validation_rejects_old_or_missing_address_indexes_before_preparation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            database = root / 'search/baden-wuerttemberg.sqlite'
            database.parent.mkdir()
            with sqlite3.connect(database) as db:
                db.execute('CREATE TABLE metadata(key TEXT,value TEXT)')
                db.execute("INSERT INTO metadata VALUES('schema','1')")
            for schema in [1, 2, 3]:
                with sqlite3.connect(database) as db:
                    db.execute("UPDATE metadata SET value=? WHERE key='schema'", (str(schema),))
                with self.assertRaisesRegex(ValueError, 'Rebuild search package'):
                    release.search_metadata(database)
                with patch.object(planner_prepare.maps, 'run') as run:
                    with self.assertRaisesRegex(ValueError, 'Rebuild search package'):
                        planner_prepare.prepare(argparse.Namespace(data_dir=root, recipe=release.maps.ROOT / 'tools/planner-regions/baden-wuerttemberg.json'))
                    run.assert_not_called()
            with sqlite3.connect(database) as db:
                db.execute('CREATE TABLE addresses(lat REAL,lon REAL)')
                db.execute('CREATE INDEX address_cells ON addresses(CAST((lat+90)*200 AS INTEGER)*72001+CAST((lon+180)*200 AS INTEGER))')
            self.assertEqual(release.search_metadata(database, full=True)['schema'], 3)

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

    def test_incomplete_upload_does_not_publish_the_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = argparse.Namespace(data_dir=root, apply=True, public_url="https://maps.example")
            document = {"region": "test", "files": {"page.bin": {"bytes": 10, "sha256": "a" * 64}}}
            calls = []
            def transfer(command, *_args, **_kwargs):
                calls.append(command)
                return "[]" if command[0] == "lsjson" else ""
            with patch.object(release, "release", return_value=("a" * 64, document)), \
                 patch.object(r2, "bucket_remote", return_value=r2.Remote("test:bucket", {})), \
                 patch.object(r2, "run_rclone", side_effect=transfer):
                with self.assertRaisesRegex(ValueError, "incomplete"):
                    release.publish(args)
            self.assertEqual([command[0] for command in calls], ["copy", "lsjson"])

    def test_site_configuration_uses_one_release_and_rejects_line_injection(self):
        active = release.endpoints("a" * 64, {"region": "test", "bounds": [1, 2, 3, 4],
                                   "attribution": "OSM", "terrain_attribution": "Terrain"}, "https://maps.example", "https://tiles.example", "https://api.example")
        env = release.vite_environment(active)
        for key in ["VITE_PLANNER_TILEJSON_URL", "VITE_PLANNER_SEARCH_URL", "VITE_CATALOG_URL"]:
            self.assertIn("a" * 64, env[key])
        active["terrain_attribution"] = "Terrain\nOTHER=value"
        with self.assertRaisesRegex(ValueError, "configuration"): release.vite_environment(active)

    def test_region_recipe_refuses_unsupported_country_defaults(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "region.json"
            path.write_text(json.dumps({"format": 1, "region": "test", "country": "FR"}))
            with self.assertRaisesRegex(ValueError, "German access defaults"):
                planner_prepare.recipe(path)

    def test_recipe_names_explicit_profiles_and_rejects_invalid_selection(self):
        config = json.loads((release.maps.ROOT / "tools/planner-regions/baden-wuerttemberg.json").read_bytes())
        expected = {bike + suffix for bike in ["touring", "road", "gravel", "mtb", "hiking"]
                    for suffix in ["", "/less-climbing", "/shorter"]}
        self.assertEqual(set(config["profiles"]), expected)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "region.json"
            for profiles in [[], ["all"], ["road", "road"], ["mtb/quieter"], [{}]]:
                path.write_text(json.dumps({**config, "profiles": profiles}))
                with self.assertRaisesRegex(ValueError, "unique routing profile"):
                    planner_prepare.recipe(path)

    def test_failed_service_probe_does_not_activate_a_release(self):
        args = argparse.Namespace(host="root@vps.example", data_dir=Path("/release"), apply=True,
                                  site_origin="https://site.example", public_url="https://maps.example",
                                  tiles_url="https://tiles.example", api_url="https://releases.openbikecomputer.com")
        document = {"region": "test", "bounds": [1, 2, 3, 4], "attribution": "OSM", "terrain_attribution": "Terrain"}
        with patch.object(release, "release", return_value=("a" * 64, document)), \
             patch.object(release, "read_url", side_effect=[document, {"active": None, "previous": None}]), \
             patch.object(planner_deploy, "ssh") as ssh, patch.object(planner_deploy.maps, "run"), \
             patch.object(planner_deploy, "verify_services", side_effect=ValueError("Tiles unavailable")), \
             patch.object(planner_deploy, "activate") as activate:
            with self.assertRaisesRegex(ValueError, "Tiles unavailable"):
                planner_deploy.deploy(args)
            activate.assert_not_called()
            for call in ssh.call_args_list:
                script = call.args[1]
                if script.startswith("python3 - <<'PY'\n"):
                    compile(script.split("\n", 1)[1].split("\nPY\n", 1)[0], "remote Caddy setup", "exec")

    def test_missing_active_slot_cannot_restart_live_services(self):
        args = argparse.Namespace(host="root@vps.example", data_dir=Path("/release"), apply=True,
                                  site_origin="https://site.example", public_url="https://maps.example",
                                  tiles_url="https://tiles.example", api_url="https://releases.openbikecomputer.com")
        document = {"region": "test", "bounds": [1, 2, 3, 4], "attribution": "OSM", "terrain_attribution": "Terrain"}
        current = {"active": {"id": "b" * 64}, "previous": None}
        with patch.object(release, "release", return_value=("a" * 64, document)), \
             patch.object(release, "read_url", side_effect=[document, current]), \
             patch.object(planner_deploy, "ssh") as ssh, patch.object(planner_deploy.maps, "run") as run:
            with self.assertRaisesRegex(ValueError, "needs slot"):
                planner_deploy.deploy(args)
            ssh.assert_not_called()
            run.assert_not_called()


if __name__ == "__main__": unittest.main()
