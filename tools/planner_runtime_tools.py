"""Finite native providers for the three planner runtime builders."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import sysconfig
import zlib


def environment():
    for name in ("LD_PRELOAD", "LD_AUDIT", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH"):
        if os.environ.get(name):
            raise ValueError(f"Native runtime does not support {name}; remove the injection override")
    names = {"PATH", "HOME", "TMPDIR", "CARGO_HOME", "CARGO_BUILD_JOBS", "RUSTUP_HOME",
             "UV_CACHE_DIR", "npm_config_cache", "LD_LIBRARY_PATH"}
    result = {name: value for name, value in os.environ.items() if name in names}
    result.update(RUSTUP_AUTO_INSTALL="0", UV_OFFLINE="1", UV_PYTHON_DOWNLOADS="never",
                  PYTHONDONTWRITEBYTECODE="1", LC_ALL="C")
    return result


def file(path):
    path = Path(path).resolve(strict=True)
    with path.open("rb") as stream:
        sha = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"path": str(path), "sha256": sha}


def executable(name):
    path = shutil.which(name, path=environment().get("PATH", ""))
    if path is None:
        raise ValueError(f"Prepare native runtime executable: {name}")
    value = file(path)
    with Path(value["path"]).open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            raise ValueError(f"Native runtime {name} needs a Linux executable, without a wrapper")
    return value


def python_files(readelf):
    result = {"python/executable": file(sys.executable)}
    stdlib = Path(sysconfig.get_path("stdlib")).resolve(strict=True)
    # Standard CPython caches must match their source files; this is not loaded-code attestation.
    for module in tuple(sys.modules.values()):
        origin = getattr(module, "__file__", None)
        frozen = getattr(getattr(module, "__spec__", None), "origin", None) in {"frozen", "built-in"}
        if origin and not frozen:
            path = Path(origin).resolve(strict=True)
            if path.is_relative_to(stdlib):
                result[f"python/module/{path.relative_to(stdlib)}"] = file(path)
    libraries = {"libpython": set(), "libz": set()}
    for line in Path("/proc/self/maps").read_text(encoding="utf-8").splitlines():
        fields = line.split(maxsplit=5)
        if len(fields) == 6 and fields[5].startswith("/"):
            path = Path(fields[5])
            for role, prefix in [("libpython", "libpython"), ("libz", "libz.so")]:
                if path.name.startswith(prefix):
                    libraries[role].add(path)
    # Only the already imported zlib extension can require this additional provider.
    origin = getattr(zlib, "__file__", None)
    dynamic = subprocess.run([readelf, "-d", origin or sys.executable], env=environment(),
                             capture_output=True, check=True, text=True).stdout
    needs_z = "[libz.so" in dynamic
    for role, paths in libraries.items():
        if len(paths) > 1 or not paths and (role == "libpython" and sysconfig.get_config_var("Py_ENABLE_SHARED") or role == "libz" and needs_z):
            raise ValueError(f"Native CPython needs an unambiguous loaded {role}")
        if paths:
            result[f"python/{role}"] = file(next(iter(paths)))
    return result


def npm_files(cli):
    cli = Path(cli).resolve(strict=True)
    root = cli.parent.parent
    if cli.name != "npm-cli.js" or json.loads((root / "package.json").read_text(encoding="utf-8")).get("name") != "npm":
        raise ValueError("Native npm needs its self-contained npm-cli.js package")
    files = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink() or not path.resolve().is_relative_to(root):
            raise ValueError("Native npm needs regular implementation files inside its package")
        if path.is_file():
            files[f"npm/{path.relative_to(root)}"] = file(path)
    return files


def providers(service):
    commands = {}
    files = {}
    readelf = executable("readelf")
    commands["readelf"] = readelf["path"]
    files["readelf"] = readelf
    files.update(python_files(readelf["path"]))
    commands["python"] = files["python/executable"]["path"]
    uv = executable("uv")
    commands["uv"] = uv["path"]
    files["uv"] = uv
    if service == "search":
        selected = executable("node")
        commands["node"] = selected["path"]
        files["node"] = selected
        npm = shutil.which("npm", path=environment().get("PATH", ""))
        if npm is None:
            raise ValueError("Prepare the native bundled npm implementation")
        commands["npm"] = str(Path(npm).resolve(strict=True))
        files.update(npm_files(commands["npm"]))
    return {"commands": commands, "files": files}


def check(binding):
    for expected in binding["files"].values():
        if file(expected["path"]) != expected:
            raise ValueError("Native runtime provider changed; prepare a new plan")
    for path, sha in binding.get("rust", {}).get("configuration", {}).items():
        actual = file(path)["sha256"] if Path(path).is_file() else None
        if actual != sha:
            raise ValueError("Native routing configuration changed; prepare a new plan")
    for path, sha in binding.get("rust", {}).get("files", {}).items():
        if file(path)["sha256"] != sha:
            raise ValueError("Native routing provider changed; prepare a new plan")


def npm(binding, *args):
    return [binding["commands"]["node"], "--no-global-search-paths", binding["commands"]["npm"],
            *args, "--userconfig=/dev/null", "--globalconfig=/dev/null"]
