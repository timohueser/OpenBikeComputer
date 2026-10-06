"""Stored services stage without a source checkout or public traffic writes."""

import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from tools import planner_install as install, planner_runtime as runtime, planner_offline as offline


HOST = {"triple": "x86_64-unknown-linux-gnu", "glibc": "2.36", "python": "3.12.0", "node": "24.0.0"}


class PlannerInstall(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source, self.base, self.units = [self.root / name for name in ("source", "services", "units")]
        (self.source / "objects").mkdir(parents=True)
        payload = self.root / "runtime.tar.gz"
        with tarfile.open(payload, "w:gz") as archive:
            body = b"print('stored runtime')\n"
            entry = tarfile.TarInfo("entry.py")
            entry.size, entry.mode = len(body), 0o644
            archive.addfile(entry, io.BytesIO(body))
        sha = runtime.digest(payload)
        (self.source / "objects" / sha).write_bytes(payload.read_bytes())
        self.descriptor = {"format": 1, "service": "downloads", "target": {key: HOST[key] for key in ("triple", "glibc", "python")},
                           "payload": {"path": "downloads.tar.gz", "sha256": sha, "bytes": payload.stat().st_size}, "libraries": []}
        data = self.root / "catalog.json"
        data.write_bytes(b'{"format":3}')
        self.document = {"format": 1, "region": "test", "files": {"offline/catalog.json": offline.pack_file(data, self.source / "objects")}}
        self.value = {"service": "downloads", "id": "a" * 64, "slot": 1}
        self.request = {"installed": self.value, "source": str(self.source), "objects_url": "https://maps.openbikecomputer.com/planner/objects", "site_origin": "https://openbikecomputer.com"}
        self.write_metadata()
        self.commands = []

    def write_metadata(self):
        for field, name, document in [("release", "release.json", self.document), ("runtime", "runtime/downloads.json", self.descriptor)]:
            path = self.source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(runtime.encoded(document))
            self.request[field] = {"path": name, "size": path.stat().st_size, "sha256": runtime.digest(path)}

    def execute(self, command):
        self.commands.append(command)
        if "--property=WorkingDirectory" in command:
            return str(install.destination(self.value, self.base) / "code")
        if "--property=Environment" in command:
            lines = (self.units / install.unit(self.value)).read_text().splitlines()
            return " ".join(line.removeprefix("Environment=") for line in lines if line.startswith("Environment="))
        return ""

    def stage(self):
        with patch.object(install, "host", return_value=HOST):
            install.stage(self.request, self.base, self.units, self.execute)

    def test_candidate_materializes_verified_bytes_and_probes_actual_opened_identity(self):
        self.stage()
        directory = install.destination(self.value, self.base)
        self.assertEqual((directory / "data/offline/catalog.json").read_bytes(), b'{"format":3}')
        self.assertTrue((directory / "code/entry.py").is_file())
        self.assertEqual([command for command in self.commands if command[:2] == ["systemctl", "restart"]], [["systemctl", "restart", "obc-planner-downloads-1.service"]])
        self.assertFalse(any("caddy" in command or "enable" in command for command in self.commands))
        contents = (self.units / install.unit(self.value)).read_text()
        self.assertIn(" -S -m tools.planner_downloads", contents)
        self.assertIn("DynamicUser=yes", contents)
        with patch.object(install, "host", return_value=HOST):
            actual = install.probe(self.value, self.base, self.execute, lambda _: {"sha256": "b" * 64})
        self.assertEqual(actual, {"service": "downloads", "catalog": "b" * 64}, "readiness must report opened data, not requested id")
        (directory / "data/offline/catalog.json").write_bytes(b'corrupt')
        with patch.object(install, "host", return_value=HOST), self.assertRaisesRegex(ValueError, "checksum"):
            install.probe(self.value, self.base, self.execute, lambda _: self.fail("corrupt data must not be accepted"))

    def test_wrong_target_and_archive_traversal_fail_before_any_slot_restart(self):
        with patch.object(install, "host", return_value={**HOST, "python": "3.13.0"}), self.assertRaisesRegex(ValueError, "prerequisites"):
            install.stage(self.request, self.base, self.units, self.execute)
        self.assertEqual(self.commands, [])
        for name, kind in [("../escape", tarfile.REGTYPE), ("link", tarfile.SYMTYPE)]:
            with self.subTest(name=name):
                payload = self.root / "invalid.tar.gz"
                with tarfile.open(payload, "w:gz") as archive:
                    entry = tarfile.TarInfo(name)
                    entry.type, entry.linkname = kind, "/outside"
                    archive.addfile(entry)
                sha = runtime.digest(payload)
                (self.source / "objects" / sha).write_bytes(payload.read_bytes())
                self.descriptor["payload"].update(sha256=sha, bytes=payload.stat().st_size)
                self.write_metadata()
                with patch.object(install, "host", return_value=HOST), self.assertRaises(ValueError):
                    install.stage(self.request, self.base, self.units, self.execute)
                self.assertEqual(self.commands, [])
                self.assertFalse(install.destination(self.value, self.base).exists())

    def test_same_service_data_accepts_an_index_change_but_never_different_immutable_data(self):
        self.stage()
        self.document["files"]["maps/optional"] = {"bytes": 1, "sha256": "c" * 64}
        self.write_metadata()
        self.stage()
        self.document["files"]["offline/catalog.json"]["sha256"] = "d" * 64
        self.write_metadata()
        previous = len(self.commands)
        with self.assertRaisesRegex(ValueError, "different runtime or data"):
            self.stage()
        self.assertEqual(len(self.commands), previous)


if __name__ == "__main__": unittest.main()
