"""Known Local services retain one owner across immutable view changes."""

import fcntl
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from tools import planner_local as local, planner_local_route as route, planner_runtime as runtime


class Local(unittest.TestCase):
    def test_one_owner_replaces_only_the_changed_running_child_and_drains_on_token_stop(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            lock = directory / "owner.lock"
            binary = b"native route"
            views = []
            for number in range(2):
                view = directory / str(number)
                view.mkdir()
                (view / "route-server").write_bytes(binary)
                value = {"view": str(view), "routing_executable": runtime.digest(view / "route-server"),
                         "fingerprints": {name: number if name == "frontend" else 0
                                          for name in ("routing", "search", "tiles", "frontend")}}
                local.write(view / "service.json", value)
                views.append(value)
            def select(number):
                view = Path(views[number]["view"])
                local.write(directory / "desired.json", {"token": "owner", "code": "supervisor",
                                                          "view": str(view), "sha256": runtime.digest(view / "service.json")})
            select(0)
            made, stopped = [], []
            def spawn(argv, **options):
                with lock.open("a") as other:
                    with self.assertRaises(BlockingIOError):
                        fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)
                child = SimpleNamespace(name=argv[0], poll=lambda: None)
                self.assertTrue(options["start_new_session"])
                made.append(child)
                return child
            def ready(value):
                if value == views[0]: select(1)
                else: local.write(directory / "stop.json", {"token": "owner"})
            recipes = {name: ([name], directory) for name in views[0]["fingerprints"]}
            with patch.object(local, "commands", return_value=(recipes, {})), patch.object(local, "ready", side_effect=ready), \
                 patch.object(local.runtime, "release", return_value=("manifest", {"files": {}})), \
                 patch.object(local.maps, "check_port"), patch.object(local.maps, "stop_process", side_effect=stopped.append), \
                 patch.object(local.subprocess, "Popen", side_effect=spawn), patch.object(local.time, "sleep"):
                local.supervise(directory, "owner", lock)
            self.assertEqual([child.name for child in made], ["routing", "search", "tiles", "frontend", "frontend"])
            self.assertIs(stopped[0], made[3])
            self.assertCountEqual(stopped, made)
            self.assertEqual(local.read(directory / "state.json")["status"], "stopped")
            with lock.open("a") as released:
                fcntl.flock(released, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_readiness_requires_the_actual_opened_region_grid_and_query_model(self):
        value = {"region": "ride", "release": "a" * 64,
                 "expected": {"routing": "routes", "search": "grid", "model": {"labels.json": "model"}}}
        search = {"parser": {"ready": True, "model": {"labels.json": "model"}},
                  "regions": [{"id": "ride", "grid": "grid"}]}
        def opened(url, **_options):
            from io import BytesIO
            return BytesIO(json.dumps({"package": "routes"} if url.endswith("/v1/region") else search).encode())
        with patch.object(local, "urlopen", side_effect=opened):
            local.ready(value)
            search["regions"][0]["id"] = "another-region"
            with self.assertRaisesRegex(ValueError, "another prepared grid"): local.ready(value)
            search["regions"][0]["id"] = "ride"
            search["parser"]["model"] = {"labels.json": "another-model"}
            with self.assertRaisesRegex(ValueError, "another query model"): local.ready(value)

    def test_private_launch_environment_cannot_replace_the_selected_python_runtime(self):
        import hashlib
        import sys
        import sysconfig
        actual = {"implementation": sys.implementation.name, "version": list(sys.version_info[:3]),
                  "abi": sysconfig.get_config_var("SOABI")}
        expected = hashlib.sha256(json.dumps(actual, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        local.check_python(Path(sys._base_executable), expected)
        with self.assertRaisesRegex(ValueError, "selected interpreter"):
            local.check_python(Path(sys._base_executable), "0" * 64)

    def test_native_route_copy_checks_compiled_root_and_code_after_shared_target_replacement(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            binary = root / "compiled"
            binary.write_bytes(b"native artifact")
            request = {"output": str(root / "output"), "options": {"root": str(root), "code": "declared-code"}}
            artifact = json.dumps({"reason": "compiler-artifact", "target": {"name": "route-server"}, "executable": str(binary)})
            def run(argv, **options):
                if argv[0] == "cargo":
                    self.assertIn("--locked", argv)
                    self.assertIn("--offline", argv)
                    self.assertIn("--release", argv)
                    self.assertEqual(options["env"]["OBC_ROUTE_BUILD_CODE"], "declared-code")
                    return SimpleNamespace(returncode=0, stdout=artifact, stderr="")
                return SimpleNamespace(stdout=json.dumps({"root": str(root), "code": "replacement-code"}))
            with patch.object(route.Path, "cwd", return_value=root), patch.object(route.subprocess, "run", side_effect=run):
                with self.assertRaisesRegex(ValueError, "another root or execution identity"): route.step(request)
            self.assertEqual((root / "output/route-server").read_bytes(), binary.read_bytes())
