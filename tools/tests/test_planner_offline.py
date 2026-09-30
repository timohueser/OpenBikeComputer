"""Only a complete verified runtime release can become the active local package."""

import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import planner_offline as offline, planner_release as releases


def release(root, files):
    root.mkdir()
    entries = {}
    for name, data in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        entries[name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    (root / "release.json").write_bytes(releases.encoded({
        "format": 1, "region": "test", "files": entries,
        "source_files": {"sources/absent.pbf": {"bytes": 1, "sha256": "0" * 64}},
    }))


class OfflinePlanner(unittest.TestCase):
    def test_exact_package_roundtrip_deduplicates_and_reuses_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, bundle, local = (root / name for name in ("source", "bundle", "local"))
            files = {"routing/pages.bin": bytes(range(256)), "routing/empty.bin": b"",
                     "maps/a.json": b"content" * 1000, "maps/b.json": b"content" * 1000}
            release(source, files)
            packed = offline.pack(source, bundle)
            self.assertEqual(packed["objects"], 3)
            self.assertEqual(packed["transfer_bytes"], sum(p.stat().st_size for p in bundle.rglob("*") if p.is_file()))
            checked = offline.verify_bundle(bundle)
            self.assertEqual(checked["verified_objects"], 3)
            self.assertEqual(checked["verified_files"], len(files))
            with patch.object(offline, "stored_bytes", wraps=offline.stored_bytes) as scans:
                installed = offline.install(bundle, local)
            self.assertLessEqual(scans.call_count, 2)
            active = json.loads((local / "active.json").read_bytes())["release"]
            for name, data in files.items():
                self.assertEqual((local / "releases" / active / name).read_bytes(), data)
            self.assertEqual((local / "releases" / active / "release.json").read_bytes(), (source / "release.json").read_bytes())
            self.assertEqual(installed["downloaded_bytes"], packed["transfer_bytes"])
            self.assertGreaterEqual(installed["peak_install_bytes"], installed["stored_bytes"])
            with patch.object(offline, "fetch_object", side_effect=AssertionError("Object was fetched twice")):
                repeated = offline.install(bundle, local)
            self.assertEqual(repeated["added_stored_bytes"], 0)
            self.assertEqual(repeated["stored_bytes"], installed["stored_bytes"])

    def test_peak_accounts_for_resume_stale_decode_hardlinks_and_atomic_update(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            local = root / "local"
            release(root / "old", {"old.bin": b"old release"})
            offline.pack(root / "old", root / "old-bundle")
            old = offline.install(root / "old-bundle", local)["release"]
            release(root / "new", {"shared.bin": b"old release", "plain.bin": b"new bytes",
                                   "data.json": b"new compressed content" * 1000})
            offline.pack(root / "new", root / "new-bundle")
            manifest = json.loads((root / "new-bundle/bundle.json").read_bytes())
            entry = manifest["files"]["data.json"]
            transport = entry["transport"]
            partial = local / "downloads" / transport["sha256"]
            partial.write_bytes((root / "new-bundle/objects" / transport["sha256"]).read_bytes()[:3])
            os.link(partial, local / "downloads/retained-prefix")
            (local / "objects" / (entry["sha256"] + ".part")).write_bytes(b"stale decode" * 10)

            def actual_bytes():
                inodes = { (s.st_dev, s.st_ino): s.st_size for path in local.rglob("*")
                           if path.is_file() for s in [path.stat()] }
                return sum(inodes.values())

            observed = [actual_bytes()]
            fsync = os.fsync

            def sample(fd):
                observed.append(actual_bytes())
                return fsync(fd)

            with patch.object(offline.os, "fsync", side_effect=sample):
                result = offline.install(root / "new-bundle", local)
            self.assertEqual(result["retained_before_bytes"], observed[0])
            self.assertEqual(result["stored_bytes"], actual_bytes())
            self.assertEqual(result["peak_install_bytes"], max(observed))
            self.assertEqual(result["peak_added_bytes"], max(observed) - observed[0])
            self.assertEqual((local / "releases" / old / "old.bin").read_bytes(), b"old release")

    def test_update_keeps_previous_release_and_does_not_activate_corrupt_or_missing_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            local = root / "local"
            release(root / "old", {"data.bin": b"old"})
            offline.pack(root / "old", root / "old-bundle")
            old = offline.install(root / "old-bundle", local)["release"]
            active = (local / "active.json").read_bytes()
            release(root / "new", {"data.bin": b"new", "shared.bin": b"old"})
            offline.pack(root / "new", root / "new-bundle")
            new_hash = hashlib.sha256(b"new").hexdigest()
            path = root / "new-bundle/objects" / new_hash
            path.unlink()
            with self.assertRaises(FileNotFoundError): offline.install(root / "new-bundle", local)
            self.assertEqual((local / "active.json").read_bytes(), active)
            path.write_bytes(b"bad")
            with self.assertRaisesRegex(ValueError, "Checksum"): offline.verify_bundle(root / "new-bundle")
            with self.assertRaisesRegex(ValueError, "Checksum"): offline.install(root / "new-bundle", local)
            self.assertEqual((local / "active.json").read_bytes(), active)
            path.write_bytes(b"new")
            new = offline.install(root / "new-bundle", local)["release"]
            self.assertNotEqual(old, new)
            self.assertEqual((local / "releases" / old / "data.bin").read_bytes(), b"old")
            self.assertEqual(len(list((local / "objects").iterdir())), 2)

    def test_interrupted_download_keeps_partial_bytes_and_resumes(self):
        class Interrupted(io.BytesIO):
            def read(self, _size):
                if self.tell(): raise OSError("interrupted")
                return super().read(3)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, bundle, local = (root / name for name in ("source", "bundle", "local"))
            data = b"complete bytes"
            release(source, {"data.bin": data})
            offline.pack(source, bundle)
            transport = json.loads((bundle / "bundle.json").read_bytes())["files"]["data.bin"]["transport"]
            partial = root / "partial"
            response = Interrupted(data)
            response.status, response.headers = 200, {}
            with patch.object(offline.runtime, "open_url", return_value=response):
                with self.assertRaisesRegex(OSError, "interrupted"):
                    offline.fetch_object("https://maps.example/bundle", transport, partial)
            self.assertEqual(partial.read_bytes(), data[:3])
            self.assertEqual(offline.fetch_object(bundle, transport, partial), len(data) - 3)
            self.assertEqual(partial.read_bytes(), data)
            self.assertFalse((local / "active.json").exists())

    def test_http_resume_requires_the_requested_range_and_can_restart(self):
        data = b"abcdef"
        transport = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "encoding": "identity"}
        with tempfile.TemporaryDirectory() as temporary:
            partial = Path(temporary) / "part"
            for status, content_range, payload, transferred in [(206, "bytes 3-5/6", b"def", 3), (200, None, data, 6)]:
                partial.write_bytes(b"abc")
                response = io.BytesIO(payload)
                response.status, response.headers = status, {"Content-Range": content_range}
                with patch.object(offline.runtime, "open_url", return_value=response) as request:
                    self.assertEqual(offline.fetch_object("https://maps.example/bundle", transport, partial), transferred)
                    self.assertEqual(request.call_args.args[0].get_header("Range"), "bytes=3-")
                self.assertEqual(partial.read_bytes(), data)
            partial.write_bytes(b"abc")
            response = io.BytesIO(b"def")
            response.status, response.headers = 206, {"Content-Range": "bytes 2-4/6"}
            with patch.object(offline.runtime, "open_url", return_value=response):
                with self.assertRaisesRegex(ValueError, "range"):
                    offline.fetch_object("https://maps.example/bundle", transport, partial)
            self.assertEqual(partial.read_bytes(), b"abc")

    def test_manifest_omission_and_path_escape_fail_before_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            release(root / "source", {"data.bin": b"payload"})
            offline.pack(root / "source", root / "bundle")
            path = root / "bundle/bundle.json"
            manifest = json.loads(path.read_bytes())
            manifest["files"].clear()
            path.write_bytes(releases.encoded(manifest))
            with self.assertRaisesRegex(ValueError, "complete"):
                offline.install(root / "bundle", root / "local")
            self.assertFalse((root / "local").exists())
            release_path = root / "bundle/release.json"
            document = json.loads(release_path.read_bytes())
            entry = document["files"].pop("data.bin")
            document["files"]["../outside"] = entry
            release_path.write_bytes(releases.encoded(document))
            manifest["release"] = offline.item(release_path)
            manifest["files"] = {"../outside": {**entry, "transport": {**entry, "encoding": "identity"}}}
            path.write_bytes(releases.encoded(manifest))
            with self.assertRaisesRegex(ValueError, "path"):
                offline.install(root / "bundle", root / "local")
            self.assertFalse((root / "outside").exists())


if __name__ == "__main__":
    unittest.main()
