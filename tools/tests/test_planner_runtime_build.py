"""Runtime packaging has explicit target, offline, and archive boundaries."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from tools import planner_runtime_build as runtime


TARGET = {"triple": "x86_64-unknown-linux-gnu", "glibc": "2.31", "node": "24.0.0", "python": "3.12.0"}


class RuntimeBuild(unittest.TestCase):
    def test_target_versions_and_platform_are_explicit(self):
        self.assertEqual(runtime.target(TARGET, "search"), TARGET)
        route = {key: TARGET[key] for key in ("triple", "glibc")}
        self.assertEqual(runtime.target(route, "routing"), route)
        for invalid in [{}, {**TARGET, "triple": "aarch64-apple-darwin"}, {**TARGET, "python": "3.12"},
                        {**TARGET, "node": "latest"}, {**TARGET, "glibc": "2.31.0"}]:
            with self.subTest(target=invalid), self.assertRaises(ValueError):
                runtime.target(invalid, "search")
        with patch.object(runtime.platform, "system", return_value="Darwin"), patch.object(runtime, "run") as run:
            with self.assertRaisesRegex(ValueError, "configured Linux target"):
                runtime.native("routing", route)
            run.assert_not_called()

    def test_archive_preserves_licenses_and_executable_modes_without_host_paths_or_times(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = root / "payload"
            (payload / "bin").mkdir(parents=True)
            executable = payload / "bin/route-server"
            executable.write_bytes(b"authored executable fixture")
            executable.chmod(0o755)
            license = payload / "python/example.dist-info/licenses/LICENSE"
            license.parent.mkdir(parents=True)
            license.write_text("A package license.\n")
            first = runtime.archive(payload, root / "first.tar.gz")
            os.utime(executable, (100, 100))
            second = runtime.archive(payload, root / "second.tar.gz")
            self.assertEqual(first, second)
            with tarfile.open(root / "first.tar.gz") as archive:
                self.assertEqual(archive.getnames(), ["bin/route-server", "python/example.dist-info/licenses/LICENSE"])
                self.assertEqual(archive.getmember("bin/route-server").mode, 0o755)
                self.assertEqual(archive.getmember("bin/route-server").mtime, 0)
                self.assertEqual(archive.extractfile("python/example.dist-info/licenses/LICENSE").read(), b"A package license.\n")
            (payload / "outside").symlink_to(root / "first.tar.gz")
            with self.assertRaisesRegex(ValueError, "regular files"):
                runtime.archive(payload, root / "invalid.tar.gz")

    def test_production_npm_closure_retains_pins_and_skips_absent_optional_peers(self):
        package = {"version": "1.0.0", "resolved": "https://example.org/package.tgz", "integrity": "sha512-pin"}
        tree = {"devDependencies": {"unused": "2.0.0"}, "dependencies": {
            "production": {**package, "dependencies": {"optional-peer": {}, "nested": package}}}}
        self.assertEqual(set(runtime.npm_packages(tree)), {"production@1.0.0", "nested@1.0.0"})
        self.assertEqual(runtime.npm_packages(tree)["nested@1.0.0"]["integrity"], "sha512-pin")
        with self.assertRaisesRegex(ValueError, "Unpinned"):
            runtime.npm_packages({"dependencies": {"production": {"version": "1.0.0"}}})

    def test_archived_download_service_imports_without_a_checkout_or_site_packages(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = root / "payload"
            payload.mkdir()
            runtime.copy_downloads(runtime.ROOT / "tools", payload)
            artifact = root / "downloads.tar.gz"
            runtime.archive(payload, artifact)
            installed = root / "installed"
            with tarfile.open(artifact) as archive:
                archive.extractall(installed, filter="data")
            env = {"PATH": os.environ["PATH"], "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1"}
            ready = subprocess.run([sys.executable, "-S", "-m", "tools.planner_downloads", "--help"],
                                   cwd=installed, env=env, capture_output=True, check=False)
            self.assertEqual(ready.returncode, 0, ready.stderr.decode())
            self.assertIn(b"--objects-url", ready.stdout)
            self.assertFalse((installed / "pyproject.toml").exists())

    def test_elf_architecture_and_required_glibc_are_checked_with_readelf(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "library.so").write_bytes(b"\x7fELF authored metadata fixture")
            def inspect(argv, **kwargs):
                self.assertEqual(kwargs["env"]["LC_ALL"], "C")
                if "-h" in argv:
                    return "Machine: Advanced Micro Devices X86-64\n"
                if "--version-info" in argv:
                    return "Name: GLIBC_2.31\n"
                return "(NEEDED) Shared library: [libc.so.6]\n"
            with patch.object(runtime, "run", side_effect=inspect):
                self.assertEqual(runtime.elf_requirements(root, TARGET), ["libc.so.6"])
            with patch.object(runtime, "run", side_effect=["Machine: AArch64", ""]):
                with self.assertRaisesRegex(ValueError, "architecture"):
                    runtime.elf_requirements(root, TARGET)
            with patch.object(runtime, "run", side_effect=["Machine: Advanced Micro Devices X86-64", "Name: GLIBC_2.34"]):
                with self.assertRaisesRegex(ValueError, "baseline"):
                    runtime.elf_requirements(root, TARGET)

    def test_container_probe_never_pulls_and_refuses_remote_docker(self):
        image = "sha256:" + "a" * 64
        record = [{"Id": image, "Os": "linux", "Architecture": "amd64"}]
        with patch.dict(os.environ, {"DOCKER_HOST": "ssh://remote"}), patch.object(runtime, "run", return_value='"unix:///local.sock"') as run:
            with self.assertRaisesRegex(ValueError, "local Docker socket"):
                runtime.builder("routing", TARGET, "docker:" + image)
            self.assertEqual(run.call_count, 1)
        with patch.dict(os.environ, {}, clear=True), patch.object(runtime, "run", side_effect=['"unix:///local.sock"', json.dumps(record)]) as run:
            self.assertEqual(runtime.builder("routing", TARGET, "docker:" + image), {"kind": "container", "image": image})
            self.assertEqual(run.call_args.args[0], ["docker", "image", "inspect", image])
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "output"
            output.mkdir()
            request = {"output": str(output), "metrics": "unused", "options": {"builder": {"image": image}}}
            with patch.dict(os.environ, {"UV_PYTHON": "/laptop/python", "DOCKER_HOST": "unix:///local.sock", "CARGO_BUILD_JOBS": "2"}, clear=True), patch.object(runtime, "run") as run:
                runtime.container(request)
                argv = run.call_args.args[0]
                self.assertIn("--network=none", argv)
                self.assertIn("--pull=never", argv)
                self.assertNotIn("UV_PYTHON=/laptop/python", argv)
                self.assertIn("CARGO_BUILD_JOBS=2", argv)
                self.assertEqual(json.loads(run.call_args.kwargs["input"])["output"], "/work/output")

    def test_changed_native_toolchain_is_refused_before_build_or_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "output"
            output.mkdir()
            request = {"output": str(output), "options": {"service": "routing", "target": {
                key: TARGET[key] for key in ("triple", "glibc")}, "builder": {"kind": "native", "rustc": "planned"}}}
            with patch.object(runtime, "native", return_value={"kind": "native", "rustc": "changed"}), patch.object(runtime, "run") as run:
                with self.assertRaisesRegex(ValueError, "changed; plan again"):
                    runtime.build(request)
                run.assert_not_called()
                self.assertEqual(list(output.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
