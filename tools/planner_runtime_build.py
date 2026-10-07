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
import tomllib

ROOT = Path(__file__).resolve().parents[1]
if sys.flags.isolated:
    sys.path.insert(0, str(ROOT))
from tools import planner_runtime_tools as runtime_tools, step_request
if sys.flags.isolated:
    sys.path.remove(str(ROOT))
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
                  "planner_maps.py", "planner_runtime.py", "planner_offline.py", "planner_install.py", "planner_activation.py"]


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


def native(service, wanted, bind=True):
    if platform.system() != "Linux" or platform.machine() != TRIPLES[wanted["triple"]][0]:
        raise ValueError("Native builder must run on the configured Linux target; select a prepared container")
    libc, release = platform.libc_ver()
    if libc != "glibc" or version(release) > version(wanted["glibc"]):
        raise ValueError("Builder glibc exceeds the configured runtime baseline")
    tools = {"kind": "native", "glibc": release}
    providers = runtime_tools.providers(service) if bind else None
    env = runtime_tools.environment()
    if service == "routing":
        tools["release_profile"] = release_profile()
    else:
        tools["python"] = {"version": ".".join(map(str, sys.version_info[:3])),
                           "implementation": sys.implementation.name}
        if tools["python"]["version"] != wanted["python"] or tools["python"]["implementation"] != "cpython":
            raise ValueError("Prepared CPython differs from the configured runtime version")
        if service == "search":
            node = providers["commands"]["node"] if providers else "node"
            tools["node"] = run([node, "--version"], env=env).strip().removeprefix("v")
            if tools["node"] != wanted["node"]:
                raise ValueError("Prepared Node differs from the configured runtime version")
    if providers:
        tools["providers"] = providers
    return tools


def execution_builder(service, wanted):
    result = native(service, wanted)
    if service == "routing":
        worker = os.environ.get("OBC_PLANNER_RUNTIME_WORKER")
        if not worker or not Path(worker).is_absolute():
            raise ValueError("Start native routing through the checked obc data worker")
        result["providers"]["rust"] = json.loads(run([worker, "--planner-runtime-routing"]))
    return result


def execution_digest(builder):
    providers = builder["providers"]
    document = {"tools": {name: value["sha256"] for name, value in providers["files"].items()},
                "rust": providers.get("rust", {}).get("identity"),
                "settings": {name: value for name, value in builder.items() if name not in {"providers", "execution"}},
                "policy": "isolated-no-site-utf8-empty-npm-config-no-global-paths-no-uv-config"}
    return hashlib.sha256(json.dumps(document, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


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
    result = {"kind": "container", "image": image}
    if service == "routing":
        result["release_profile"] = release_profile()
    return result


def release_profile():
    profile = tomllib.loads((ROOT / "Cargo.toml").read_text()).get("profile", {}).get("release", {})
    return hashlib.sha256(encoded(profile)).hexdigest()


def local_docker():
    override = os.environ.get("DOCKER_HOST")
    endpoint = json.loads(run(["docker", "context", "inspect", "--format", "{{json .Endpoints.docker.Host}}"])).strip()
    if not endpoint.startswith("unix://") or override and not override.startswith("unix://"):
        raise ValueError("Planner runtime containers require a local Docker socket; remote builders need owner authorization")


def code_paths(service):
    if service == "routing":
        return [name for name in ("LICENSE", "THIRD-PARTY.md", "rust-toolchain.toml", ".cargo/config.toml") if (ROOT / name).is_file()]
    if service == "downloads":
        return ["LICENSE", *(f"tools/{name}" for name in DOWNLOAD_FILES)]
    source = ROOT / "planner/search"
    names = [*SERVICE_FILES, "package.json", "package-lock.json"]
    names.extend(path.name for path in sorted(source.glob("LICENSE.*")))
    return ["LICENSE", *(f"planner/search/{name}" for name in names)]


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


def service_files():
    root = ROOT / "planner/search"
    names = [*SERVICE_FILES, *(path.name for path in sorted(root.glob("LICENSE.*")))]
    return sorted(set(filter(None, run(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *names], cwd=root).split("\0"))))


def copy_service(root, output, files):
    if files != sorted(set(files)):
        raise ValueError("Service source files must be sorted and unique")
    for name in files:
        if Path(name).is_absolute() or Path(name).as_posix() != name or ".." in Path(name).parts:
            raise ValueError("Invalid service source file")
        source, dest = root / name, output / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, dest)
    (output / "package.json").write_bytes(encoded({"type": "module"}))


def copy_downloads(root, output):
    (output / "tools").mkdir()
    for name in DOWNLOAD_FILES:
        shutil.copyfile(root / name, output / "tools" / name)


def routing_notices(triple):
    heading = f"## Linux routing service (`planner-service`, `{triple}`)\n"
    document = (ROOT / "THIRD-PARTY.md").read_text()
    if heading not in document:
        raise ValueError("Regenerate the Linux routing notices with obc licenses")
    return heading + document.split(heading, 1)[1].split("\n## ", 1)[0]


def elf_requirements(directory, wanted, readelf="readelf"):
    libraries, provided = set(), set()
    for path in sorted(directory.rglob("*")):
        if not path.is_file():
            continue
        with path.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                continue
        env = runtime_tools.environment()
        header = run([readelf, "-h", str(path)], env=env)
        if TRIPLES[wanted["triple"]][2] not in header:
            raise ValueError(f"Runtime ELF architecture differs: {path.name}")
        versions = run([readelf, "--version-info", str(path)], env=env)
        if any(version(item) > version(wanted["glibc"]) for item in re.findall(r"\bGLIBC_(\d+\.\d+)\b", versions)):
            raise ValueError(f"Runtime ELF exceeds the glibc baseline: {path.name}")
        dynamic = run([readelf, "-d", str(path)], env=env)
        libraries.update(re.findall(r"\(NEEDED\).*?\[(.*?)\]", dynamic))
        provided.update(re.findall(r"\(SONAME\).*?\[(.*?)\]", dynamic))
        provided.add(path.name)
    return sorted(libraries - provided)


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
    if os.environ.get("OBC_BAKE_BUDGETED") == "1":
        raise ValueError("Budgeted Linux bakes need OBC_PLANNER_RUNTIME_BUILDER=native for new runtimes; "
                         "reuse a verified artifact or prepare the native builder")
    output = Path(request["output"])
    child = {**request, "output": "/work/output", "metrics": "/work/metrics.json"}
    argv = ["docker", "run", "--interactive", "--rm", "--pull=never", "--network=none", "--read-only", "--tmpfs", "/tmp",
            "--user", f"{os.getuid()}:{os.getgid()}", "--mount", f"type=bind,source={ROOT},target=/src,readonly",
            "--mount", f"type=bind,source={output.parent},target=/work", "--workdir", "/src"]
    if "CARGO_BUILD_JOBS" in os.environ:
        argv += ["--env", f"CARGO_BUILD_JOBS={os.environ['CARGO_BUILD_JOBS']}"]
    for name, dest in [("CARGO_HOME", "/cache/cargo"), ("UV_CACHE_DIR", "/cache/uv"), ("npm_config_cache", "/cache/npm")]:
        value = os.environ.get(f"OBC_PLANNER_RUNTIME_{name.upper()}")
        if value:
            source = Path(value).resolve(strict=True)
            argv += ["--mount", f"type=bind,source={source},target={dest}", "--env", f"{name}={dest}"]
    argv += [request["options"]["builder"]["image"], "python3", "-I", "-S", "-X", "utf8", "/src/tools/planner_runtime_build.py", "--step", "--inside"]
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
    actual = native(service, wanted, bind=False) if inside else execution_builder(service, wanted)
    providers = actual.get("providers")
    if not inside and fingerprint != {"kind":"native", "execution":execution_digest(actual)} or service == "routing" and inside and actual.get("release_profile") != fingerprint.get("release_profile"):
        raise ValueError("Prepared runtime tools changed; plan again")
    if providers:
        runtime_tools.check(providers)
    commands = providers["commands"] if providers else {}
    output = Path(request["output"])
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        work = Path(temporary)
        payload = work / "payload"
        payload.mkdir()
        shutil.copyfile(ROOT / "LICENSE", payload / "LICENSE")
        env = runtime_tools.environment()
        packages = {}
        if service == "routing":
            env.pop("LD_LIBRARY_PATH", None)
            env["CARGO_TARGET_DIR"] = str(work / "target")
            flags = [f"--remap-path-prefix={ROOT}=/src", f"--remap-path-prefix={work}=/build"]
            cargo = "cargo"
            if providers:
                rust = providers["rust"]["executables"]
                cargo = rust["cargo"]
                env["RUSTC"] = rust["rustc"]
                flags.append(f"-Clinker={rust['cc']}")
            env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
            run([cargo, "build", "--release", "--locked", "--offline", "-p", "planner-service", "--bin", "planner-service", "--target", wanted["triple"]], env=env)
            (payload / "bin").mkdir()
            shutil.copyfile(work / "target" / wanted["triple"] / "release/planner-service", payload / "bin/planner-service")
            (payload / "bin/planner-service").chmod(0o755)
            (payload / "THIRD-PARTY.md").write_text(routing_notices(wanted["triple"]))
        elif service == "search":
            source = ROOT / "planner/search"
            app = work / "dependencies"
            app.mkdir()
            for name in ("package.json", "package-lock.json"):
                shutil.copyfile(source / name, app / name)
            npm = lambda *args: runtime_tools.npm(providers, *args) if providers else ["npm", *args]
            run(npm("ci", "--offline", "--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund", "--bin-links=false"), cwd=app, env=env)
            packages = npm_packages(json.loads(run(npm("ls", "--all", "--omit=dev", "--json", "--long"), cwd=app, env=env)))
            shutil.copytree(app / "node_modules", payload / "node_modules")
            copy_service(source, payload, options["files"])
            requirements = work / "requirements.txt"
            uv = commands.get("uv", "uv")
            run([uv, "--no-config", "export", "--locked", "--offline", "--no-default-groups", "--group", "search-runtime", "--no-emit-project", "--no-header", "--no-annotate", "--output-file", str(requirements)], env=env)
            python = commands.get("python", sys.executable)
            run([uv, "--no-config", "pip", "install", "--offline", "--no-python-downloads", "--python", python, "--target", str(payload / "python"), "--no-deps", "--require-hashes", "--only-binary=:all:", "-r", str(requirements)], env=env)
            if (payload / "python/bin").exists():
                shutil.rmtree(payload / "python/bin")
            shutil.copyfile(requirements, payload / "requirements.txt")
            run([python, "-I", "-S", "-X", "utf8", "-c", f"import sys; sys.path.insert(0, {str(payload / 'python')!r}); import numpy,onnxruntime,tokenizers,rapidfuzz,snowballstemmer,yaml"], env=env)
        else:
            copy_downloads(ROOT / "tools", payload)
            python = commands.get("python", sys.executable)
            for module in ("planner_downloads", "planner_install"):
                run([python, "-I", "-S", "-X", "utf8", "-c", f"import sys,runpy; sys.path.insert(0, {str(payload)!r}); runpy.run_module('tools.{module}', run_name='__main__')", "--help"], cwd=payload, env=env)
        if providers:
            runtime_tools.check(providers)
        if (native(service, wanted, bind=False) if inside else execution_builder(service, wanted)) != actual:
            raise ValueError("Prepared runtime tools changed during the build; plan again")
        libraries = elf_requirements(payload, wanted, commands.get("readelf", "readelf"))
        artifact = output / f"{service}.tar.gz"
        stored = archive(payload, artifact)
        if providers:
            runtime_tools.check(providers)
            if execution_builder(service, wanted) != actual:
                raise ValueError("Prepared runtime tools changed during packaging; plan again")
    document = {"format": 1, "service": service, "target": wanted,
                "payload": {"path": artifact.name, **stored}, "libraries": libraries,
                "entry": {"routing": "bin/planner-service", "search": "server.mjs", "downloads": "tools.planner_downloads"}[service]}
    (output / "runtime.json").write_bytes(encoded(document))
    step_request.metrics(request, {"bytes": stored["bytes"], "npm_packages": len(packages)})


def main():
    if not sys.flags.isolated or not sys.flags.no_site or not sys.flags.utf8_mode:
        raise ValueError("Run the runtime adapter with python -I -S -X utf8 tools/planner_runtime_build.py")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true")
    parser.add_argument("--inside", action="store_true")
    parser.add_argument("--probe", choices=("routing", "search", "downloads"))
    args = parser.parse_args()
    if args.probe:
        wanted = target(json.load(sys.stdin), args.probe)
        print(json.dumps({"builder": builder(args.probe, wanted, os.environ.get("OBC_PLANNER_RUNTIME_BUILDER", "native")),
                          "paths": code_paths(args.probe), "files": service_files() if args.probe == "search" else []}, sort_keys=True))
    elif args.step:
        build(step_request.read(), args.inside)
    else:
        parser.error("Select --probe or --step")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        raise SystemExit(str(error)) from error
