"""New endpoints serve checked candidates before their pointer can switch."""

from pathlib import Path
import hashlib
import io
import platform
import subprocess
import sys
import tarfile
import tempfile
import unittest

from tools import planner_activation as activation, planner_install as install


class PlannerActivation(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.routes, self.units, self.base = [self.root / name for name in ("routes", "units", "services")]
        self.config = self.root / "Caddyfile"
        self.config.write_text("api.example {\n}\n")
        self.stages = []
        for name in install.SERVICES:
            candidate = {"service": name, "id": "a" * 64, "api_origin": "https://api.example",
                         "site_origin": "https://site.example", "objects_url": "https://objects.example/planner/objects",
                         "expected": {"service": name, "opened": name}}
            self.stages.append({"candidate": candidate, "installed": {
                "service": name, "id": candidate["id"], "binding": install.binding(candidate), "slot": 1}})
        self.commands, self.probes = [], []

    def execute(self, command):
        self.commands.append(command)
        return ""

    def probe(self, stage, **kwargs):
        public = "read" in kwargs
        if public:
            self.assertIn(["systemctl", "reload", "caddy"], self.commands)
        self.probes.append((stage["installed"]["service"], public))
        return stage["candidate"]["expected"]

    def test_activation_retains_old_pinned_routes_and_checks_new_routes_after_reload(self):
        self.routes.mkdir()
        old = self.routes / ("routing-" + "b" * 64 + ".caddy")
        old.write_text("old binding")
        result = activation.activate({"stages": self.stages}, self.routes, self.config, self.execute, self.probe)
        self.assertEqual(result, [stage["candidate"]["expected"] for stage in self.stages])
        self.assertEqual(self.probes, [(name, public) for public in (False, True) for name in install.SERVICES])
        self.assertEqual(old.read_text(), "old binding")
        for stage in self.stages:
            value = stage["installed"]
            contents = (self.routes / f"{value['service']}-{value['binding']}.caddy").read_text()
            self.assertIn(f"/planner-api/services/{value['binding']}/{value['service']}/*", contents)
            self.assertIn(str(install.SERVICES[value['service']][1]), contents)
        self.assertIn("8791", (self.routes / "offline.caddy").read_text())
        self.assertFalse(any("disable" in command for command in self.commands))

    def test_reload_failure_is_not_a_successful_activation_and_invalid_origins_do_not_mutate(self):
        def failed(command):
            self.execute(command)
            if command == ["systemctl", "reload", "caddy"]:
                raise ValueError("reload interrupted")
        with self.assertRaisesRegex(ValueError, "reload interrupted"):
            activation.activate({"stages": self.stages}, self.routes, self.config, failed, self.probe)
        self.assertFalse(any(public for _, public in self.probes))
        self.commands.clear()
        self.stages[0]["candidate"]["api_origin"] = "https://api.example\nunsafe"
        with self.assertRaises(ValueError):
            activation.activate({"stages": self.stages}, self.routes, self.config, self.execute, self.probe)
        self.assertEqual(self.commands, [])

    def test_retirement_checks_exact_owned_slot_and_never_stops_the_current_slot(self):
        old = {**self.stages[0]["installed"], "slot": 0, "id": "b" * 64, "binding": "c" * 64}
        self.units.mkdir()
        self.routes.mkdir()
        directory = install.destination(old, self.base)
        (directory / "code").mkdir(parents=True)
        (self.units / install.unit(old)).write_text("owned unit")
        (self.routes / f"routing-{old['binding']}.caddy").write_text("old binding")
        def execute(command):
            self.execute(command)
            if "--property=WorkingDirectory" in command: return str(directory / "code")
            if "--property=Environment" in command: return "OBC_PLANNER_BINDING=" + old['binding']
            return ""
        current = [stage["installed"] for stage in self.stages]
        activation.retire({"previous": [old], "current": current}, self.routes, self.base, self.units, self.config, execute)
        self.assertIn(["systemctl", "disable", "--now", install.unit(old)], self.commands)
        self.assertFalse(directory.exists())
        self.commands.clear()
        with self.assertRaisesRegex(ValueError, "current service slot"):
            activation.retire({"previous": [{**old, "slot": 1}], "current": current},
                              self.routes, self.base, self.units, self.config, execute)
        self.assertEqual(self.commands, [])

    def test_bootstrap_checks_the_archive_before_loading_any_helper_code(self):
        script = Path(__file__).resolve().parents[2] / "host/obc-data/src/vps/bootstrap.py"
        archive = self.root / "helper.tar.gz"
        for name, kind in [("tools/planner_activation.py", tarfile.REGTYPE), ("../outside", tarfile.REGTYPE),
                           ("tools/planner_activation.py", tarfile.SYMTYPE)]:
            with self.subTest(name=name, kind=kind):
                with tarfile.open(archive, "w:gz") as bundle:
                    for path, mode in [("tools/planner_install.py", tarfile.REGTYPE), (name, kind)]:
                        entry = tarfile.TarInfo(path)
                        entry.type, entry.linkname = mode, "/outside"
                        body = b"raise AssertionError('bootstrap must not execute helper code')\n"
                        entry.size = len(body) if mode == tarfile.REGTYPE else 0
                        bundle.addfile(entry, io.BytesIO(body) if mode == tarfile.REGTYPE else None)
                digest = hashlib.sha256(archive.read_bytes()).hexdigest()
                output = self.root / ("extracted-" + str(len(list(self.root.glob('extracted-*')))))
                wrong = subprocess.run([sys.executable, "-S", str(script), str(archive), "0" * 64,
                                        str(output), platform.python_version()], capture_output=True)
                self.assertNotEqual(wrong.returncode, 0)
                self.assertFalse(output.exists(), "bad digest cannot even extract the installer")
                result = subprocess.run([sys.executable, "-S", str(script), str(archive), digest,
                                         str(output), platform.python_version()], capture_output=True)
                self.assertEqual(result.returncode == 0, kind == tarfile.REGTYPE and name == "tools/planner_activation.py")
                self.assertFalse((self.root.parent / "outside").exists())
