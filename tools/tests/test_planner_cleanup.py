"""Planner finalization protects the live dataset and rejects incomplete rollouts."""

import argparse
import hashlib
from io import BytesIO
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import planner_cleanup as cleanup, planner_release as release, r2


class CleanupTests(unittest.TestCase):
    def setUp(self):
        self.document = {"format": 1, "region": "test", "files": {"maps/basemap.pmtiles": {"bytes": 8}},
                         "source_files": {"sources/current.pbf": {"bytes": 7}}}
        self.raw = release.encoded(self.document).decode()
        self.identity = hashlib.sha256(self.raw.encode()).hexdigest()
        self.active = {"id": self.identity, "region": "test", **{key: "https://maps.example/" + self.identity + "/" + key
                       for key in ["basemap", "terrain", "routing", "search"]}}
        self.current = {"format": 1, "active": self.active, "previous": {"id": "b" * 64}}
        self.remote = r2.Remote("test:bucket", {})
        self.prefix = "releases/" + self.identity + "/"
        self.rows = [{"Path": key, "Size": size, "ModTime": "2000-01-01T00:00:00Z"} for key, size in [
            (self.prefix + "release.json", len(self.raw)), (self.prefix + "maps/basemap.pmtiles", 8),
            (self.prefix + "extra.bin", 2), ("sources/current.pbf", 7), ("sources/unused.pbf", 4),
            ("sources/current.pbf ", 1),
            ("releases/" + "b" * 64 + "/routing/pages.bin", 100), ("catalog.json", 3), ("other/reference", 10)]]

    def transfer(self, command, *_args, **_kwargs):
        return self.raw if command[0] == "cat" else json.dumps(self.rows)

    def test_plan_keeps_active_objects_and_shared_sources(self):
        with patch.object(r2, "run_rclone", side_effect=self.transfer):
            document, stale = cleanup.plan(self.remote, self.current)
        self.assertEqual(document, self.document)
        self.assertEqual({item.key for item in stale}, {"planner/sources/unused.pbf", "planner/sources/current.pbf ",
                         "planner/releases/" + "b" * 64 + "/routing/pages.bin"})

    def test_rollback_removes_known_newer_release_but_blocks_unknown_uploads(self):
        previous_document = {**self.document, "files": {"routing/pages.bin": {"bytes": 100}},
                             "source_files": {**self.document["source_files"], "sources/unused.pbf": {"bytes": 4}}}
        previous_raw = release.encoded(previous_document).decode()
        previous_id = hashlib.sha256(previous_raw.encode()).hexdigest()
        self.current["previous"] = {"id": previous_id, "region": "test"}
        self.rows = [dict(row, Path=row["Path"].replace("b" * 64, previous_id), ModTime="2000-01-02T00:00:00Z")
                     if row["Path"].startswith("releases/" + "b" * 64) or row["Path"] == "sources/unused.pbf"
                     else row for row in self.rows]
        self.rows.append({"Path": "releases/" + previous_id + "/release.json", "Size": len(previous_raw),
                          "ModTime": "2000-01-02T00:00:00Z"})
        def transfer(command, *_args, **_kwargs):
            if command[0] == "cat" and previous_id in command[1]: return previous_raw
            return self.transfer(command)
        with patch.object(r2, "run_rclone", side_effect=transfer):
            _, stale = cleanup.plan(self.remote, self.current)
            self.assertIn("planner/sources/unused.pbf", {item.key for item in stale})
            self.assertIn("planner/releases/" + previous_id + "/release.json", {item.key for item in stale})
            self.rows.append({"Path": "releases/" + "c" * 64 + "/pending.bin", "Size": 1,
                              "ModTime": "2000-01-02T00:00:00Z"})
            with self.assertRaisesRegex(ValueError, "upload is pending"):
                cleanup.plan(self.remote, self.current)

    def test_incomplete_active_data_and_newer_uploads_block_cleanup(self):
        for mutation in ["missing", "newer", "invalid_path", "wrong_hash", "invalid_key"]:
            with self.subTest(mutation=mutation):
                old_rows, old_raw = self.rows.copy(), self.raw
                if mutation == "missing": self.rows = [row for row in self.rows if not row["Path"].endswith("basemap.pmtiles")]
                if mutation == "newer": self.rows = [dict(row, ModTime="2000-01-02T00:00:00Z") if row["Path"] == "sources/unused.pbf" else row for row in self.rows]
                if mutation == "invalid_key": self.rows = self.rows + [{"Path": "sources/unused\n" + self.prefix + "maps/basemap.pmtiles", "Size": 1, "ModTime": "2000-01-01T00:00:00Z"}]
                current = self.current
                if mutation == "invalid_path":
                    self.raw = release.encoded({**self.document, "files": {"../cell-catalog/catalog.json": {"bytes": 3}}}).decode()
                    current = {**self.current, "active": {**self.active, "id": hashlib.sha256(self.raw.encode()).hexdigest()}}
                if mutation == "wrong_hash": self.raw = self.raw + " "
                with patch.object(r2, "run_rclone", side_effect=self.transfer):
                    with self.assertRaises(ValueError): cleanup.plan(self.remote, current)
                self.rows, self.raw = old_rows, old_raw

    def test_site_must_reference_active_endpoints(self):
        for active in [True, False]:
            code = "\n".join(self.active[key] for key in ["basemap", "terrain", "routing", "search"]) if active else "old release"
            with patch.object(cleanup.sources, "open_url", side_effect=[
                    BytesIO(b'<script type="module" src="./assets/planner.js"></script>'), BytesIO(code.encode())]):
                if active: cleanup.verify_site(self.active, "https://site.example")
                else:
                    with self.assertRaisesRegex(ValueError, "Deploy site"): cleanup.verify_site(self.active, "https://site.example")

    def test_apply_rechecks_catalog_and_deletes_only_planned_objects(self):
        stale = [r2.Target(key, 100, "2000-01-01T00:00:00Z") for key in [
            "planner/releases/" + "b" * 64 + "/old.bin", "planner/sources/current.pbf ",
            "planner/releases/" + "b" * 64 + "/release.json"]]
        for changed in [False, True]:
            with self.subTest(changed=changed), patch.object(r2, "bucket_remote", return_value=self.remote), \
                 patch.object(cleanup, "catalog", side_effect=[self.current, {**self.current, "active": {}} if changed else self.current, self.current]), \
                 patch.object(cleanup, "plan", side_effect=[(self.document, stale), (self.document, [])]), \
                 patch.object(cleanup.deploy, "verify_services"), patch.object(cleanup, "verify_site"), \
                 patch.object(r2, "append_log") as log, patch.object(cleanup.deploy, "activate") as activate:
                removed = []
                def remove(command, *_args, **_kwargs):
                    self.assertEqual(command[:2], ["delete", self.remote.path])
                    removed.extend(Path(command[command.index("--files-from-raw") + 1]).read_text().splitlines())
                with patch.object(r2, "run_rclone", side_effect=remove):
                    args = argparse.Namespace(apply=True, site_origin="https://site.example", public_url="https://maps.example")
                    if changed:
                        with self.assertRaisesRegex(ValueError, "catalogue changed"): cleanup.finalize(args)
                        log.assert_not_called()
                        activate.assert_not_called()
                        self.assertEqual(removed, [])
                    else:
                        cleanup.finalize(args)
                        self.assertEqual(removed, [item.key for item in stale])
                        log.assert_called_once()
                        activate.assert_called_once_with(args.public_url, {**self.current, "previous": None})

    def test_preview_does_not_change_storage_or_probe_services(self):
        with patch.object(r2, "bucket_remote", return_value=self.remote), \
             patch.object(cleanup, "catalog", return_value=self.current), \
             patch.object(cleanup, "plan", return_value=(self.document, [])), \
             patch.object(cleanup.deploy, "verify_services") as verify, \
             patch.object(r2, "append_log") as log, patch.object(cleanup.deploy, "activate") as activate:
            cleanup.finalize(argparse.Namespace(apply=False))
            verify.assert_not_called()
            log.assert_not_called()
            activate.assert_not_called()

    def test_publication_cannot_accumulate_another_inactive_release(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "catalog.json"
            path.write_bytes(release.encoded(self.current))
            for extra in [False, True]:
                rows = [{"Path": self.identity + "/"}, {"Path": "c" * 64 + "/"}]
                if extra: rows.append({"Path": "b" * 64 + "/"})
                with patch.object(r2, "fetch_optional", return_value=path), patch.object(r2, "run_rclone", return_value=json.dumps(rows)):
                    if extra:
                        with self.assertRaisesRegex(ValueError, "finalize"): cleanup.before_publish(self.remote, "c" * 64)
                    else: cleanup.before_publish(self.remote, "c" * 64)


if __name__ == "__main__": unittest.main()
