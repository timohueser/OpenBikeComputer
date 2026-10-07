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

    def test_search_copies_only_identity_covered_files_without_container_git(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app = root / "apps/planner-search"
            (app / "web/vendor").mkdir(parents=True)
            (app / ".gitignore").write_text("web/vendor/\n")
            (app / "server.mjs").write_text("export const service = true;\n")
            (app / "web/view.mjs").write_text("export const view = true;\n")
            (app / "web/vendor/local.mjs").write_text("ignored host content\n")
            subprocess.run(["git", "init", "--quiet"], cwd=app, check=True, capture_output=True)
            with patch.object(runtime, "ROOT", root):
                files = runtime.service_files()
            self.assertEqual(files, ["server.mjs", "web/view.mjs"])
            payload = root / "payload"
            payload.mkdir()
            with patch.object(runtime, "run", side_effect=AssertionError("container copy must not use Git")):
                runtime.copy_service(app, payload, files)
            self.assertEqual((payload / "web/view.mjs").read_bytes(), (app / "web/view.mjs").read_bytes())
            self.assertFalse((payload / "web/vendor").exists())

    def test_routing_profile_and_selected_notices_ignore_unrelated_content(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = root / "Cargo.toml"
            manifest.write_text('[profile.release]\nopt-level = 3\n')
            heading = "## Linux routing service (`route-server`, `x86_64-unknown-linux-gnu`)\n"
            notices = root / "THIRD-PARTY.md"
            notices.write_text(heading + "\nHTTP library licence.\n\n## Other artifact\n\nOther licence.\n")
            with patch.object(runtime, "ROOT", root):
                first = runtime.release_profile()
                selected = runtime.routing_notices(TARGET["triple"])
                manifest.write_text('[profile.release]\nopt-level = 3\n[workspace.dependencies]\nunrelated = "2"\n')
                self.assertEqual(runtime.release_profile(), first)
                notices.write_text(heading + "\nHTTP library licence.\n\n## Other artifact\n\nChanged licence.\n")
                self.assertEqual(runtime.routing_notices(TARGET["triple"]), selected)
                manifest.write_text('[profile.release]\nopt-level = 2\n')
                self.assertNotEqual(runtime.release_profile(), first)
                with self.assertRaisesRegex(ValueError, "Regenerate"):
                    runtime.routing_notices("aarch64-unknown-linux-gnu")

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
            for module, option in [("planner_downloads", b"--objects-url"), ("planner_install", b"stage")]:
                ready = subprocess.run([sys.executable, "-S", "-m", f"tools.{module}", "--help"],
                                       cwd=installed, env={**env, "PYTHONPATH": str(installed)}, capture_output=True, check=False)
                self.assertEqual(ready.returncode, 0, ready.stderr.decode())
                self.assertIn(option, ready.stdout)
            activation = subprocess.run([sys.executable, "-S", "-c", "from tools import planner_activation"],
                                        cwd=installed, env={**env, "PYTHONPATH": str(installed)}, capture_output=True)
            self.assertEqual(activation.returncode, 0, activation.stderr.decode())
            self.assertFalse((installed / "pyproject.toml").exists())

    def test_elf_architecture_and_required_glibc_are_checked_with_readelf(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "library.so").write_bytes(b"\x7fELF authored metadata fixture")
            bundled = root / "numpy.libs/openblas.so"
            bundled.parent.mkdir()
            bundled.write_bytes(b"\x7fELF authored bundled library")
            def inspect(argv, **kwargs):
                self.assertEqual(kwargs["env"]["LC_ALL"], "C")
                if "-h" in argv:
                    return "Machine: Advanced Micro Devices X86-64\n"
                if "--version-info" in argv:
                    return "Name: GLIBC_2.31\n"
                if str(bundled) == argv[-1]:
                    return "(SONAME) Library soname: [libopenblas-hash.so]\n(NEEDED) Shared library: [libc.so.6]\n"
                return "(NEEDED) Shared library: [libc.so.6]\n(NEEDED) Shared library: [libopenblas-hash.so]\n"
            with patch.object(runtime, "run", side_effect=inspect):
                self.assertEqual(runtime.elf_requirements(root, TARGET), ["libc.so.6"])
                bundled.unlink()
                self.assertEqual(runtime.elf_requirements(root, TARGET), ["libc.so.6", "libopenblas-hash.so"])
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
            self.assertEqual(runtime.builder("routing", TARGET, "docker:" + image), {"kind": "container", "image": image, "release_profile": runtime.release_profile()})
            self.assertEqual(run.call_args.args[0], ["docker", "image", "inspect", image])
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "output"
            output.mkdir()
            request = {"output": str(output), "metrics": "unused", "options": {"builder": {"image": image}}}
            with patch.dict(os.environ, {"UV_PYTHON": "/laptop/python", "DOCKER_HOST": "unix:///local.sock", "CARGO_BUILD_JOBS": "2"}, clear=True), patch.object(runtime, "run") as run:
                runtime.container(request)
                argv = run.call_args.args[0]
                self.assertIn("--network=none", argv)
                self.assertIn("--interactive", argv)
                self.assertIn("--pull=never", argv)
                self.assertNotIn("UV_PYTHON=/laptop/python", argv)
                self.assertIn("CARGO_BUILD_JOBS=2", argv)
                self.assertEqual(argv[-9:], [image, "python3", "-I", "-S", "-X", "utf8", "/src/tools/planner_runtime_build.py", "--step", "--inside"])
                self.assertFalse(any("OBC_PLANNER_RUNTIME_WORKER=" in value for value in argv))
                self.assertEqual(json.loads(run.call_args.kwargs["input"])["output"], "/work/output")

    def test_container_routing_build_never_resolves_host_native_providers(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "LICENSE").write_text("Licence")
            output = root / "output"
            output.mkdir()
            request = {"output":str(output), "metrics":str(root / "metrics.json"), "options":{
                "service":"routing", "target":{key:TARGET[key] for key in ("triple", "glibc")},
                "builder":{"kind":"container", "image":"sha256:" + "a" * 64, "release_profile":"selected"}}}
            def build(argv, **kwargs):
                self.assertEqual(argv[0], "cargo")
                self.assertEqual(argv[1:5], ["build", "--release", "--locked", "--offline"])
                self.assertNotIn("RUSTC", kwargs["env"])
                executable = Path(kwargs["env"]["CARGO_TARGET_DIR"]) / TARGET["triple"] / "release/route-server"
                executable.parent.mkdir(parents=True)
                executable.write_bytes(b"authored image-built fixture")
                return ""
            with patch.object(runtime, "ROOT", root), patch.object(runtime, "native", return_value={"kind":"native", "release_profile":"selected"}) as native, \
                 patch.object(runtime, "execution_builder", side_effect=AssertionError("no host providers inside image")), \
                 patch.object(runtime, "run", side_effect=build), patch.object(runtime, "routing_notices", return_value="Selected licence"):
                runtime.build(request, inside=True)
                self.assertTrue(all(call.kwargs == {"bind":False} for call in native.call_args_list))
            self.assertEqual(json.loads((output / "runtime.json").read_text())["service"], "routing")

    def test_native_runtime_environment_preserves_only_declared_settings(self):
        with patch.dict(os.environ, {"PATH":"/selected/bin", "LD_LIBRARY_PATH":"/selected/libs",
             "RUSTFLAGS":"operator flags", "CC":"operator cc", "NODE_OPTIONS":"operator hook",
             "UV_CONFIG_FILE":"operator config", "PYTHONPATH":"operator modules"}, clear=True):
            env = runtime.runtime_tools.environment()
            self.assertEqual(env["LD_LIBRARY_PATH"], "/selected/libs")
            self.assertTrue({"RUSTFLAGS", "CC", "NODE_OPTIONS", "UV_CONFIG_FILE", "PYTHONPATH"}.isdisjoint(env))
            self.assertEqual(env["LC_ALL"], "C")
            with patch.dict(os.environ, {"LD_PRELOAD":"/operator/preload"}):
                with self.assertRaisesRegex(ValueError, "LD_PRELOAD"):
                    runtime.runtime_tools.environment()

    def test_routing_selector_uses_the_retained_worker_without_building_another(self):
        providers = {"commands":{}, "files":{}}
        actual = {"kind":"native", "providers":providers}
        with patch.dict(os.environ, {"OBC_PLANNER_RUNTIME_WORKER":"/retained/worker"}), \
             patch.object(runtime, "native", return_value=actual), patch.object(runtime, "run", return_value='{"identity":"checked"}') as run:
            self.assertEqual(runtime.execution_builder("routing", TARGET)["providers"]["rust"], {"identity":"checked"})
            self.assertEqual(run.call_args.args[0], ["/retained/worker", "--planner-runtime-routing"])
            self.assertNotIn("env", run.call_args.kwargs, "selector retains the worker validation environment")
        with patch.dict(os.environ, {}, clear=True), patch.object(runtime, "native", return_value=actual), patch.object(runtime, "run") as run:
            with self.assertRaisesRegex(ValueError, "checked obc data worker"):
                runtime.execution_builder("routing", TARGET)
            run.assert_not_called()

    def test_native_npm_binds_its_nested_implementation_and_ignores_operator_config(self):
        tools = runtime.runtime_tools
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "npm"
            (root / "bin").mkdir(parents=True)
            (root / "node_modules/dependency").mkdir(parents=True)
            (root / "package.json").write_text('{"name":"npm"}')
            cli = root / "bin/npm-cli.js"
            cli.write_text("require('../lib/cli.js');")
            dependency = root / "node_modules/dependency/index.js"
            dependency.write_text("module.exports = 1;")
            first = tools.npm_files(cli)
            dependency.write_text("module.exports = 2;")
            second = tools.npm_files(cli)
            self.assertNotEqual(first["npm/node_modules/dependency/index.js"]["sha256"],
                                second["npm/node_modules/dependency/index.js"]["sha256"])
            binding = {"commands":{"node":"/selected/node", "npm":str(cli)}}
            with patch.dict(os.environ, {"npm_config_userconfig":"/operator/config", "NODE_OPTIONS":"--require=/operator/hook"}):
                argv = tools.npm(binding, "ci", "--offline")
                self.assertEqual(argv[:3], ["/selected/node", "--no-global-search-paths", str(cli)])
                self.assertEqual(argv[-2:], ["--userconfig=/dev/null", "--globalconfig=/dev/null"])
                self.assertNotIn("NODE_OPTIONS", tools.environment())
                self.assertNotIn("npm_config_userconfig", tools.environment())
            dependency.unlink()
            dependency.symlink_to(cli)
            with self.assertRaisesRegex(ValueError, "regular implementation"):
                tools.npm_files(cli)

    def test_native_download_commands_and_provider_checks_span_archive_creation(self):
        tools = runtime.runtime_tools
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "tools").mkdir()
            (root / "LICENSE").write_text("Licence")
            for name in ("planner_downloads.py", "planner_install.py"):
                (root / "tools" / name).write_text("authored service fixture")
            provider = root / "python"
            provider.write_bytes(b"selected interpreter fixture")
            binding = {"commands":{"python":str(provider), "readelf":"/selected/readelf"},
                       "files":{"python/executable":tools.file(provider)}}
            builder = {"kind":"native", "providers":binding}
            output = root / "output"
            output.mkdir()
            request = {"output":str(output), "metrics":str(root / "metrics.json"), "options":{
                "service":"downloads", "target":{key:TARGET[key] for key in ("triple", "glibc", "python")},
                "builder":{"kind":"native", "execution":runtime.execution_digest(builder)}}}
            def command(argv, **kwargs):
                self.assertEqual(argv[:6], [str(provider), "-I", "-S", "-X", "utf8", "-c"])
                self.assertNotIn("PYTHONPATH", kwargs["env"])
                self.assertNotIn("PYTHONHOME", kwargs["env"])
                return ""
            archive = runtime.archive
            def replacing(payload, artifact):
                result = archive(payload, artifact)
                provider.write_bytes(b"replacement after packaging")
                return result
            with patch.object(runtime, "ROOT", root), patch.object(runtime, "DOWNLOAD_FILES", ["planner_downloads.py", "planner_install.py"]), \
                 patch.object(runtime, "execution_builder", return_value=builder), patch.object(runtime, "run", side_effect=command) as run, \
                 patch.object(runtime, "archive", side_effect=replacing):
                with self.assertRaisesRegex(ValueError, "provider changed"):
                    runtime.build(request)
                self.assertEqual(run.call_count, 2)
            self.assertFalse((output / "runtime.json").exists())
            self.assertFalse((root / "metrics.json").exists())

    def test_cpython_binds_only_selected_modules_and_required_loaded_libraries(self):
        from types import SimpleNamespace
        tools = runtime.runtime_tools
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stdlib = root / "stdlib"
            (stdlib / "lib-dynload").mkdir(parents=True)
            module = stdlib / "json.py"
            extension = stdlib / "lib-dynload/zlib.so"
            outside = root / "operator.py"
            python = root / "python"
            libpython, libz = root / "libpython3.12.so", root / "libz.so.1"
            for path in (module, extension, outside, python, libpython, libz):
                path.write_bytes(path.name.encode())
            modules = {"json":SimpleNamespace(__file__=str(module)), "zlib":SimpleNamespace(__file__=str(extension)),
                       "operator":SimpleNamespace(__file__=str(outside)),
                       "frozen":SimpleNamespace(__file__=str(outside), __spec__=SimpleNamespace(origin="frozen"))}
            maps = "".join(f"0000-1000 r--p 00000000 00:00 0 {path}\n" for path in (libpython, libz))
            read_text = Path.read_text
            def read(path, **kwargs):
                return maps if path == Path("/proc/self/maps") else read_text(path, **kwargs)
            with patch.object(tools.sys, "modules", modules), patch.object(tools.sys, "executable", str(python)), \
                 patch.object(tools.sysconfig, "get_path", return_value=str(stdlib)), \
                 patch.object(tools.sysconfig, "get_config_var", return_value=1), patch.object(tools.zlib, "__file__", str(extension), create=True), \
                 patch.object(Path, "read_text", read), patch.object(tools.subprocess, "run", return_value=SimpleNamespace(stdout="(NEEDED) [libz.so.1]")) as run:
                selected = tools.python_files("/selected/readelf")
                self.assertEqual(set(selected), {"python/executable", "python/module/json.py", "python/zlib-extension", "python/libpython", "python/libz"})
                self.assertEqual(run.call_args.args[0], ["/selected/readelf", "-d", str(extension)])
                external = root / "external-zlib.so"
                external.write_bytes(extension.read_bytes())
                extension.unlink()
                extension.symlink_to(external)
                redirected = tools.python_files("/selected/readelf")
                self.assertEqual(redirected["python/zlib-extension"], tools.file(external))
                old_digest = runtime.execution_digest({"kind":"native", "providers":{"files":redirected}})
                external.write_bytes(b"replacement of only the external zlib extension")
                changed = tools.python_files("/selected/readelf")
                self.assertNotEqual(runtime.execution_digest({"kind":"native", "providers":{"files":changed}}), old_digest)
                with self.assertRaisesRegex(ValueError, "provider changed"):
                    tools.check({"files":redirected})
                maps = maps.splitlines()[0] + "\n"
                with self.assertRaisesRegex(ValueError, "loaded libz"):
                    tools.python_files("/selected/readelf")


    def test_changed_native_toolchain_is_refused_before_build_or_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "output"
            output.mkdir()
            request = {"output": str(output), "options": {"service": "routing", "target": {
                key: TARGET[key] for key in ("triple", "glibc")}, "builder": {"kind": "native", "execution": "a" * 64}}}
            with patch.object(runtime, "execution_builder", return_value={"kind":"native", "providers":{"files":{"rustc":{"sha256":"b" * 64}}}}), patch.object(runtime, "run") as run:
                with self.assertRaisesRegex(ValueError, "changed; plan again"):
                    runtime.build(request)
                run.assert_not_called()
                self.assertEqual(list(output.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
