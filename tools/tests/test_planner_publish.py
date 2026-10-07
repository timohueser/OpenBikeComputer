"""Service takeover and readiness use the same release as the pending pointer."""

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import planner_publish as publish, planner_offline as offline


class Publication(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.units, self.routes = self.root / "units", self.root / "routes"
        self.units.mkdir()
        self.routes.mkdir()
        self.origins = {"api_origin": "https://api.example", "site_origin": "https://site.example", "objects_origin": "https://objects.example"}
        self.document = {"region": "test", "files": {}}
        self.events = []
        for name in ("routing/blocks.json", "search/test.grid.json", "offline/catalog.json",
                     *[f"search/model/{name}" for name in ("labels.json", "tokenizer.json", "model.int8.onnx")]):
            original = self.root / "original"
            original.unlink(missing_ok=True)
            original.write_bytes(json.dumps({"name": name}).encode())
            self.document["files"][name] = offline.pack_file(original, self.root / "payload/objects")
        for name in ("obc-planner-routing-0", "obc-planner-routing-1", "obc-planner-search-0", "obc-planner-search-1", "obc-planner-downloads"):
            (self.units / f"{name}.service").write_text("old")
        (self.routes / "slot-0.caddy").write_text("old route")
        (self.routes / "slot-1.caddy").write_text("old route")
        (self.routes / "offline.caddy").write_text("old route")

    def install(self, check):
        original = offline.materialize
        def materialize(*args):
            self.events.append("copy")
            original(*args)
        with patch.object(offline, "materialize", side_effect=materialize):
            publish.install(self.root, self.document, self.origins, "a" * 64, "/usr/bin/node", self.root / "venv/bin/python",
                            execute=lambda argv: self.events.append(argv), units=self.units, routes_dir=self.routes,
                            caddy=self.root / "Caddyfile", check=check)

    def test_takeover_stops_both_slots_then_copies_starts_and_probes(self):
        self.install(lambda *args, **kwargs: self.events.append("public" if kwargs.get("public") else "local"))
        self.assertEqual([event[3] for event in self.events[:5]],
                         ["obc-planner-routing-0.service", "obc-planner-routing-1.service", "obc-planner-search-0.service", "obc-planner-search-1.service", "obc-planner-downloads.service"])
        self.assertEqual(self.events[5], "copy")
        self.assertLess(self.events.index(["chmod", "-R", "a+rX", str(self.root)]), self.events.index("local"))
        self.assertLess(self.events.index(["systemctl", "reload", "caddy"]), self.events.index(["systemctl", "enable", "--now", *[publish.unit(name) for name in publish.PORTS]]))
        self.assertEqual(self.events[-1], "public")
        self.assertEqual([path.name for path in self.routes.iterdir()], ["live.caddy"])
        self.assertIn("/planner-api/releases/" + "a" * 64, (self.routes / "live.caddy").read_text())
        for name in publish.PORTS:
            self.assertIn("DynamicUser=yes", (self.units / publish.unit(name)).read_text())
        for name, entry in self.document["files"].items():
            offline.verify(self.root / "data" / name, entry)

    def test_failed_local_probe_stops_before_public_readiness(self):
        def fail(*args):
            raise ValueError("wrong data")
        with self.assertRaisesRegex(ValueError, "wrong data"):
            self.install(fail)
        self.assertFalse((self.routes / "slot-1.caddy").exists())
        self.assertIn(["systemctl", "reload", "caddy"], self.events)

    def test_probe_checks_the_open_routing_grid_model_and_download_catalog(self):
        expected = publish.expected(self.document)
        answers = {"routing": {"package": expected["routing"]}, "downloads": {"sha256": expected["downloads"]},
                   "search": {"regions": [{"id": "test", "grid": expected["search"]["grid"]}],
                              "parser": {"ready": True, "model": expected["search"]["model"]}}}
        read = lambda name, path: answers[name]
        publish.probe(self.document, self.origins, "a" * 64, read=read)
        for name, key in (("routing", "package"), ("downloads", "sha256")):
            previous = answers[name][key]
            answers[name][key] = "wrong"
            with self.assertRaises(ValueError):
                publish.probe(self.document, self.origins, "a" * 64, read=read)
            answers[name][key] = previous
        answers["search"]["parser"]["model"] = {}
        with self.assertRaisesRegex(ValueError, "grid or model"):
            publish.probe(self.document, self.origins, "a" * 64, read=read)


if __name__ == "__main__":
    unittest.main()
