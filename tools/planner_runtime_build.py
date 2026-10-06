"""Build stored Linux planner runtimes with prepared offline tools."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

from tools import step_request

ROOT = Path(__file__).resolve().parents[1]
TRIPLES = {"x86_64-unknown-linux-gnu": ("x86_64", "amd64", "Advanced Micro Devices X86-64"),
           "aarch64-unknown-linux-gnu": ("aarch64", "arm64", "AArch64")}
SERVICE_FILES = [
    "server.mjs", "parser.mjs", "runtime.mjs", "query.mjs", "validation.mjs", "origins.mjs",
    "installation.mjs", "cells.mjs", "federation.mjs", "hours.mjs", "calendar-date.mjs",
    "resolver.mjs", "local-query.mjs", "web", "query/contract.json", "query/runtime.py",
    "query/artifacts.py", "query/prediction.py", "query/decode.py", "query/schema.py",
    "query/words.py", "query/lexicon.py", "query/lexicon",
]
DOWNLOAD_FILES = ["planner_downloads.py", "planner_grid.py", "planner_geo.py", "planner_map_archive.py",
                  "planner_maps.py", "planner_runtime.py"]


def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def version(value):
    if not isinstance(value, str) or not re.fullmatch(r"\d+\.\d+(?:\.\d+)?", value):
        raise ValueError("Runtime versions must be exact numeric versions")
    return tuple(map(int, value.split(".")))


def target(value, service):
    fields = {"triple", "glibc"} | ({"python"} if service != "routing" else set()) | ({"node"} if service == "search" else set())
    if not isinstance(value, dict) or set(value) != fields or not isinstance(value["triple"], str) or value["triple"] not in TRIPLES:
        raise ValueError("Planner runtime target fields or Linux triple differ")
    if len(version(value["glibc"])) != 2:
        raise ValueError("Use glibc major.minor")
    if service != "routing":
        if len(version(value["python"])) != 3 or service == "search" and len(version(value["node"])) != 3:
            raise ValueError("Use exact Node and Python major.minor.patch")
        if version(value["python"])[:2] < (3, 12) or service == "search" and version(value["node"])[0] < 24:
            raise ValueError("Planner search needs Node 24 or later and Python 3.12 or later")
    return value


def run(argv, *, cwd=ROOT, env=None, input=None):
    result = subprocess.run(argv, cwd=cwd, env=env, input=input, capture_output=True, check=False)
    if result.returncode:
        raise ValueError(f"Prepared runtime tool failed: {' '.join(map(str, argv))}: "
                         f"{result.stderr.decode(errors='replace').strip()}")
    return result.stdout.decode()


def native(service, wanted):
    if platform.system() != "Linux" or platform.machine() != TRIPLES[wanted["triple"]][0]:
        raise ValueError("Native builder must run on the configured Linux target; select a prepared container")
    libc, release = platform.libc_ver()
    if libc != "glibc" or version(release) > version(wanted["glibc"]):
        raise ValueError("Builder glibc exceeds the configured runtime baseline")
    tools = {"kind": "native", "glibc": release}
    if service == "routing":
        tools["rustc"] = run(["rustc", "--version", "--verbose"], env={**os.environ, "RUSTUP_AUTO_INSTALL": "0"}).strip()
        tools["cargo"] = run(["cargo", "--version"], env={**os.environ, "RUSTUP_AUTO_INSTALL": "0"}).strip()
        tools["cc"] = run(["cc", "--version"]).splitlines()[0]
        home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
        tools["cargo_config"] = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                                 for path in (home / "config", home / "config.toml") if path.is_file()}
        if f"host: {wanted['triple']}" not in tools["rustc"]:
            raise ValueError("Rust compiler host differs from the configured runtime target")
    else:
        python = os.environ.get("OBC_PLANNER_RUNTIME_PYTHON", "python3")
        tools["python"] = json.loads(run([python, "-c", "import json,sys,sysconfig; print(json.dumps({'version':'.'.join(map(str,sys.version_info[:3])),'abi':sysconfig.get_config_var('SOABI'),'implementation':sys.implementation.name}))"]))
        if service == "search":
            tools["node"] = run(["node", "--version"]).strip().removeprefix("v")
            tools["npm"] = run(["npm", "--version"]).strip()
            tools["uv"] = run(["uv", "--version"]).strip()
        if tools["python"]["version"] != wanted["python"] or tools["python"]["implementation"] != "cpython" or service == "search" and tools["node"] != wanted["node"]:
            raise ValueError("Prepared Node or CPython differs from the configured runtime version")
    return tools


def builder(service, wanted, choice):
    if choice == "native":
        return native(service, wanted)
    if not re.fullmatch(r"docker:sha256:[a-f0-9]{64}", choice):
        raise ValueError("Select native or docker:sha256:IMAGE with OBC_PLANNER_RUNTIME_BUILDER")
    local_docker()
    image = choice.removeprefix("docker:")
    info, = json.loads(run(["docker", "image", "inspect", image]))
    if info["Id"] != image or info["Os"] != "linux" or info["Architecture"] != TRIPLES[wanted["triple"]][1]:
        raise ValueError("Prepared container image differs from the configured Linux target")
    if service == "routing":
        cache = os.environ.get("OBC_PLANNER_RUNTIME_CARGO_HOME")
        if cache and any((Path(cache) / name).exists() for name in ("config", "config.toml")):
            raise ValueError("Container Cargo cache has external configuration; use a cache without config files")
    return {"kind": "container", "image": image}


def local_docker():
    override = os.environ.get("DOCKER_HOST")
    endpoint = json.loads(run(["docker", "context", "inspect", "--format", "{{json .Endpoints.docker.Host}}"])).strip()
    if not endpoint.startswith("unix://") or override and not override.startswith("unix://"):
        raise ValueError("Planner runtime containers require a local Docker socket; remote builders need owner authorization")


def code_paths(service):
    if service == "routing":
        return [name for name in ("LICENSE", "rust-toolchain.toml", ".cargo/config.toml") if (ROOT / name).is_file()]
    if service == "downloads":
        return ["LICENSE", *(f"tools/{name}" for name in DOWNLOAD_FILES)]
    source = ROOT / "apps/planner-search"
    names = [*SERVICE_FILES, "package.json", "package-lock.json"]
    names.extend(path.name for path in sorted(source.glob("LICENSE.*")))
    return ["LICENSE", *(f"apps/planner-search/{name}" for name in names)]


def npm_packages(tree):
    selected = {}
    for name, item in tree.get("dependencies", {}).items():
        if not item.get("version"):
            continue
        if not item.get("integrity") or not item.get("resolved"):
            raise ValueError(f"Unpinned production npm package: {name}")
        key = f"{name}@{item['version']}"
        record = {key: {field: item[field] for field in ("version", "resolved", "integrity")}}
        record.update(npm_packages(item))
        for key, value in record.items():
            if key in selected and selected[key] != value:
                raise ValueError("Conflicting production npm package")
            selected[key] = value
    return selected


def copy_service(root, output):
    for name in SERVICE_FILES:
        source, dest = root / name, output / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            shutil.copytree(source, dest, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        else:
            shutil.copyfile(source, dest)
    for source in root.glob("LICENSE.*"):
        shutil.copyfile(source, output / source.name)
    (output / "package.json").write_bytes(encoded({"type": "module"}))


def copy_downloads(root, output):
    (output / "tools").mkdir()
    for name in DOWNLOAD_FILES:
        shutil.copyfile(root / name, output / "tools" / name)


def elf_requirements(directory, wanted):
    libraries = set()
    for path in sorted(directory.rglob("*")):
        if not path.is_file():
            continue
        with path.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                continue
        env = {**os.environ, "LC_ALL": "C"}
        header = run(["readelf", "-h", str(path)], env=env)
        if TRIPLES[wanted["triple"]][2] not in header:
            raise ValueError(f"Runtime ELF architecture differs: {path.name}")
        versions = run(["readelf", "--version-info", str(path)], env=env)
        if any(version(item) > version(wanted["glibc"]) for item in re.findall(r"\bGLIBC_(\d+\.\d+)\b", versions)):
            raise ValueError(f"Runtime ELF exceeds the glibc baseline: {path.name}")
        dynamic = run(["readelf", "-d", str(path)], env=env)
        libraries.update(re.findall(r"\(NEEDED\).*?\[(.*?)\]", dynamic))
    return sorted(libraries)


def archive(directory, output):
    with output.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as stream, tarfile.open(fileobj=stream, mode="w|") as tar:
        for path in sorted(directory.rglob("*")):
            if path.is_symlink():
                raise ValueError("Runtime archives contain only regular files and directories")
            if not path.is_file():
                continue
            info = tar.gettarinfo(str(path), str(path.relative_to(directory)))
            info.uid = info.gid = info.mtime = 0
            info.uname = info.gname = ""
            info.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
            with path.open("rb") as body:
                tar.addfile(info, body)
    with output.open("rb") as stream:
        return {"sha256": hashlib.file_digest(stream, "sha256").hexdigest(), "bytes": output.stat().st_size}


def container(request):
    output = Path(request["output"])
    child = {**request, "output": "/work/output", "metrics": "/work/metrics.json"}
    argv = ["docker", "run", "--rm", "--pull=never", "--network=none", "--read-only", "--tmpfs", "/tmp",
            "--user", f"{os.getuid()}:{os.getgid()}", "--mount", f"type=bind,source={ROOT},target=/src,readonly",
            "--mount", f"type=bind,source={output.parent},target=/work", "--workdir", "/src"]
    if "CARGO_BUILD_JOBS" in os.environ:
        argv += ["--env", f"CARGO_BUILD_JOBS={os.environ['CARGO_BUILD_JOBS']}"]
    for name, dest in [("CARGO_HOME", "/cache/cargo"), ("UV_CACHE_DIR", "/cache/uv"), ("npm_config_cache", "/cache/npm")]:
        value = os.environ.get(f"OBC_PLANNER_RUNTIME_{name.upper()}")
        if value:
            source = Path(value).resolve(strict=True)
            argv += ["--mount", f"type=bind,source={source},target={dest}", "--env", f"{name}={dest}"]
    argv += [request["options"]["builder"]["image"], "python3", "-m", "tools.planner_runtime_build", "--step", "--inside"]
    run(argv, input=encoded(child))


def build(request, inside=False):
    options = request["options"]
    service = options["service"]
    if service not in {"routing", "search", "downloads"}:
        raise ValueError("Unknown planner runtime service")
    wanted = target(options["target"], service)
    fingerprint = options["builder"]
    if fingerprint["kind"] == "container" and not inside:
        if builder(service, wanted, "docker:" + fingerprint["image"]) != fingerprint:
            raise ValueError("Prepared runtime builder changed; plan again")
        container(request)
        return
    actual = native(service, wanted)
    if not inside and actual != fingerprint:
        raise ValueError("Prepared runtime tools changed; plan again")
    output = Path(request["output"])
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        work = Path(temporary)
        payload = work / "payload"
        payload.mkdir()
        shutil.copyfile(ROOT / "LICENSE", payload / "LICENSE")
        env = {key: value for key, value in os.environ.items() if key in {"PATH", "HOME", "TMPDIR", "CARGO_HOME", "CARGO_BUILD_JOBS", "RUSTUP_HOME", "UV_CACHE_DIR", "npm_config_cache"}}
        env.update({"RUSTUP_AUTO_INSTALL": "0", "UV_OFFLINE": "1", "UV_PYTHON_DOWNLOADS": "never", "PYTHONDONTWRITEBYTECODE": "1", "LC_ALL": "C"})
        packages = {}
        if service == "routing":
            env["CARGO_TARGET_DIR"] = str(work / "target")
            env["RUSTFLAGS"] = f"--remap-path-prefix={ROOT}=/src --remap-path-prefix={work}=/build"
            run(["cargo", "build", "--release", "--locked", "--offline", "-p", "route-server", "--bin", "route-server", "--target", wanted["triple"]], env=env)
            (payload / "bin").mkdir()
            shutil.copyfile(work / "target" / wanted["triple"] / "release/route-server", payload / "bin/route-server")
            (payload / "bin/route-server").chmod(0o755)
        elif service == "search":
            source = ROOT / "apps/planner-search"
            app = work / "dependencies"
            app.mkdir()
            for name in ("package.json", "package-lock.json"):
                shutil.copyfile(source / name, app / name)
            run(["npm", "ci", "--offline", "--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund", "--bin-links=false"], cwd=app, env=env)
            packages = npm_packages(json.loads(run(["npm", "ls", "--all", "--omit=dev", "--json", "--long"], cwd=app, env=env)))
            shutil.copytree(app / "node_modules", payload / "node_modules")
            copy_service(source, payload)
            requirements = work / "requirements.txt"
            run(["uv", "export", "--locked", "--offline", "--no-default-groups", "--group", "search-runtime", "--no-emit-project", "--no-header", "--no-annotate", "--output-file", str(requirements)], env=env)
            python = os.environ.get("OBC_PLANNER_RUNTIME_PYTHON", "python3")
            run(["uv", "pip", "install", "--offline", "--no-python-downloads", "--python", python, "--target", str(payload / "python"), "--no-deps", "--require-hashes", "--only-binary=:all:", "-r", str(requirements)], env=env)
            if (payload / "python/bin").exists():
                shutil.rmtree(payload / "python/bin")
            shutil.copyfile(requirements, payload / "requirements.txt")
            run([python, "-c", "import numpy,onnxruntime,tokenizers,rapidfuzz,snowballstemmer,yaml"], env={**env, "PYTHONPATH": str(payload / "python")})
        else:
            copy_downloads(ROOT / "tools", payload)
            python = os.environ.get("OBC_PLANNER_RUNTIME_PYTHON", "python3")
            run([python, "-S", "-m", "tools.planner_downloads", "--help"], cwd=payload, env=env)
        if native(service, wanted) != actual:
            raise ValueError("Prepared runtime tools changed during the build; plan again")
        libraries = elf_requirements(payload, wanted)
        artifact = output / f"{service}.tar.gz"
        stored = archive(payload, artifact)
    document = {"format": 1, "service": service, "target": wanted,
                "payload": {"path": artifact.name, **stored}, "libraries": libraries,
                "entry": {"routing": "bin/route-server", "search": "server.mjs", "downloads": "tools.planner_downloads"}[service]}
    (output / "runtime.json").write_bytes(encoded(document))
    step_request.metrics(request, {"bytes": stored["bytes"], "npm_packages": len(packages)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true")
    parser.add_argument("--inside", action="store_true")
    parser.add_argument("--probe", choices=("routing", "search", "downloads"))
    args = parser.parse_args()
    if args.probe:
        wanted = target(json.load(sys.stdin), args.probe)
        print(json.dumps({"builder": builder(args.probe, wanted, os.environ.get("OBC_PLANNER_RUNTIME_BUILDER", "native")),
                          "paths": code_paths(args.probe)}, sort_keys=True))
    elif args.step:
        build(step_request.read(), args.inside)
    else:
        parser.error("Select --probe or --step")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        raise SystemExit(str(error)) from error
