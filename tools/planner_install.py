"""Stage and probe immutable planner service slots. Traffic activation is separate."""

import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from urllib.request import Request, urlopen
from urllib.parse import urlsplit

from . import planner_offline as offline, planner_runtime as runtime

SERVICES = {"routing": (8787, 8785), "search": (8786, 8784), "downloads": (8790, 8791)}
BASE = Path("/opt/obc-planner/services")
UNITS = Path("/etc/systemd/system")


def run(argv):
    result = subprocess.run(argv, capture_output=True, text=True, check=False)
    if result.returncode:
        raise ValueError(f"Service tool failed: {argv[0]}: {result.stderr.strip()}")
    return result.stdout.strip()


def identity(value):
    if not isinstance(value, str) or not re.fullmatch(r"[a-f0-9]{64}", value):
        raise ValueError("Invalid service identity")
    return value


def installed(value):
    if set(value) != {"service", "id", "slot"} or value["service"] not in SERVICES or type(value["slot"]) is not int or value["slot"] not in (0, 1):
        raise ValueError("Invalid service slot")
    identity(value["id"])
    return value


def destination(value, base):
    value = installed(value)
    return base / value["service"] / value["id"]


def unit(value):
    return f"obc-planner-{value['service']}-{value['slot']}.service"


def host():
    machine = {"x86_64": "x86_64", "aarch64": "aarch64"}.get(platform.machine())
    libc, version = platform.libc_ver()
    if platform.system() != "Linux" or not machine or libc != "glibc":
        raise ValueError("Planner services require a configured GNU Linux host")
    return {"triple": machine + "-unknown-linux-gnu", "glibc": version,
            "python": platform.python_version(), "node": run(["node", "--version"]).removeprefix("v") if shutil.which("node") else None}


def inspect(base=BASE, execute=run):
    result = {"host": host(), "installed": []}
    for name in SERVICES:
        for slot in (0, 1):
            value = {"service": name, "slot": slot}
            directory = execute(["systemctl", "show", unit(value), "--property=WorkingDirectory", "--value"])
            if not directory: continue
            path = Path(directory)
            if path.name != "code" or path.parent.parent != base / name:
                raise ValueError("Unknown service slot ownership")
            value["id"] = identity(path.parent.name)
            result["installed"].append(value)
    return result


def checked(root, item):
    name = runtime.relative_path(item["path"])
    path = root / name
    if not path.resolve().is_relative_to(root.resolve()) or path.stat().st_size != item["size"] or runtime.digest(path) != item["sha256"]:
        raise ValueError(f"Installer checksum mismatch: {name}")
    return path


def members(archive):
    result = {}
    for entry in archive:
        path = runtime.relative_path(entry.name)
        if not entry.isfile() or str(path) in result:
            raise ValueError("Runtime archive needs unique regular files")
        result[str(path)] = entry
    return result


def extract(archive, output):
    with tarfile.open(archive, "r:gz") as bundle:
        entries = members(bundle)
        for name, entry in entries.items():
            path = output / name
            path.parent.mkdir(parents=True, exist_ok=True)
            with bundle.extractfile(entry) as source, path.open("xb") as target:
                shutil.copyfileobj(source, target)
            path.chmod(entry.mode & 0o755)


def verify_code(archive, code):
    with tarfile.open(archive, "r:gz") as bundle:
        entries = members(bundle)
        actual = {path.relative_to(code).as_posix() for path in code.rglob("*") if path.is_file() or path.is_symlink()}
        if actual != entries.keys(): raise ValueError("Installed runtime file set differs")
        for name, entry in entries.items():
            path = code / name
            with bundle.extractfile(entry) as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            if path.is_symlink() or path.stat().st_size != entry.size or runtime.digest(path) != digest or path.stat().st_mode & 0o777 != entry.mode & 0o755:
                raise ValueError(f"Installed runtime differs: {name}")


def prerequisites(descriptor):
    actual, target = host(), descriptor["target"]
    version = lambda value: tuple(map(int, value.split(".")))
    name = descriptor["service"]
    if actual["triple"] != target["triple"] or version(actual["glibc"]) < version(target["glibc"]) or name != "routing" and actual["python"] != target["python"] or name == "search" and actual["node"] != target["node"]:
        raise ValueError("Host prerequisites differ from the runtime target")
    for library in descriptor["libraries"]:
        try: ctypes.CDLL(library)
        except OSError as error: raise ValueError(f"Missing runtime library: {library}") from error


def documents(directory, expected):
    release = json.loads(checked(directory, {**expected["release"], "path": "release.json"}).read_bytes())
    descriptor = json.loads(checked(directory, {**expected["runtime"], "path": "runtime.json"}).read_bytes())
    if descriptor["format"] != 1 or descriptor["service"] != expected["installed"]["service"]:
        raise ValueError("Runtime descriptor service differs")
    payload = descriptor["payload"]
    checked(directory, {"path": "runtime.tar.gz", "size": payload["bytes"], "sha256": payload["sha256"]})
    verify_code(directory / "runtime.tar.gz", directory / "code")
    prefix = {"routing": "routing/", "search": "search/", "downloads": "offline/"}[descriptor["service"]]
    for name, item in release["files"].items():
        if name.startswith(prefix):
            checked(directory / "data", {"path": name, "size": item["bytes"], "sha256": item["sha256"]})
    return release, descriptor


def configuration(value, directory, release, objects_url, site_origin):
    name, port = value["service"], SERVICES[value["service"]][value["slot"]]
    data, code = directory / "data", directory / "code"
    environment = {"OBC_PLANNER_SERVICE_ID": value["id"]}
    if name == "routing":
        command = [str(code / "bin/route-server"), str(data / "routing")]
        environment.update(ROUTE_LISTEN=f"127.0.0.1:{port}", ROUTE_WORKERS="2", ROUTE_ORIGIN=site_origin)
    else:
        python = str(Path(sys.executable).resolve())
        wrapper = directory / "python"
        wrapper.write_text(f"#!/bin/sh\nexport PYTHONPATH={shlex.quote(str(code / 'python'))}\nexec {shlex.quote(python)} -S \"$@\"\n")
        wrapper.chmod(0o755)
        if name == "search":
            command = [str(Path(shutil.which("node")).resolve()), str(code / "server.mjs")]
            environment.update(OBC_SEARCH_PORT=str(port), OBC_SEARCH_DATA=str(data / "search"), OBC_SEARCH_PYTHON=str(wrapper), OBC_SEARCH_REGIONS=release["region"], OBC_SEARCH_ORIGINS=site_origin)
        else:
            command = [python, "-S", "-m", "tools.planner_downloads", "--source", str(data / "offline"), "--cache", f"/var/lib/obc-planner-downloads-{value['slot']}/selections", "--max-cache-bytes", str(256 * 1024 * 1024), "--port", str(port), "--objects-url", objects_url]
            environment["PYTHONPATH"] = str(code)
    return command, environment


def stage(request, base=BASE, units=UNITS, execute=run):
    value = installed(request["installed"])
    source, directory = Path(request["source"]), destination(value, base)
    origin = urlsplit(request["objects_url"])
    if origin.scheme != "https" or not origin.netloc or origin.query or origin.fragment:
        raise ValueError("Provide an HTTPS shared object pool URL")
    site = urlsplit(request["site_origin"])
    if site.scheme != "https" or not site.netloc or site.path or site.query or site.fragment:
        raise ValueError("Provide an HTTPS site origin without a path")
    release = json.loads(checked(source, request["release"]).read_bytes())
    descriptor = json.loads(checked(source, request["runtime"]).read_bytes())
    if descriptor["service"] != value["service"] or descriptor["format"] != 1:
        raise ValueError("Runtime descriptor service differs")
    prerequisites(descriptor)
    if not directory.exists():
        directory.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".stage-", dir=directory.parent) as temporary:
            pending = Path(temporary) / "service"
            pending.mkdir()
            archive = descriptor["payload"]
            original = checked(source, {"path": "objects/" + identity(archive["sha256"]), "size": archive["bytes"], "sha256": archive["sha256"]})
            shutil.copyfile(original, pending / "runtime.tar.gz")
            (pending / "code").mkdir()
            extract(pending / "runtime.tar.gz", pending / "code")
            prefix = {"routing": "routing/", "search": "search/", "downloads": "offline/"}[value["service"]]
            for name in release["files"]: runtime.relative_path(name)
            offline.materialize(source, pending / "data", release, (prefix,))
            shutil.copyfile(checked(source, request["release"]), pending / "release.json")
            shutil.copyfile(checked(source, request["runtime"]), pending / "runtime.json")
            pending.rename(directory)
    stored = {"installed": value, "release": {"size": (directory / "release.json").stat().st_size, "sha256": runtime.digest(directory / "release.json")},
              "runtime": {"size": (directory / "runtime.json").stat().st_size, "sha256": runtime.digest(directory / "runtime.json")}}
    actual, actual_runtime = documents(directory, stored)
    prefix = {"routing": "routing/", "search": "search/", "downloads": "offline/"}[value["service"]]
    selected = lambda body: {name: item for name, item in body["files"].items() if name.startswith(prefix)}
    if actual_runtime != descriptor or actual["region"] != release["region"] or selected(actual) != selected(release):
        raise ValueError("Installed service identity has different runtime or data")
    command, environment = configuration(value, directory, release, request["objects_url"], request["site_origin"])
    environment.update(OBC_PLANNER_RELEASE_SHA=stored["release"]["sha256"], OBC_PLANNER_RUNTIME_SHA=stored["runtime"]["sha256"])
    if any(any(c in argument for c in '\r\n"') for argument in [*command, *environment.values(), str(directory)]):
        raise ValueError("Invalid service configuration")
    contents = "[Unit]\nDescription=OpenBikeComputer planner\n[Service]\n" + f"WorkingDirectory={directory / 'code'}\nExecStart={shlex.join(command)}\n"
    contents += "".join(f"Environment={key}={item}\n" for key, item in environment.items())
    contents += "Restart=on-failure\nDynamicUser=yes\nNoNewPrivileges=yes\nPrivateTmp=yes\nProtectHome=yes\nProtectSystem=strict\nCPUQuota=200%\nTasksMax=64\n"
    contents += f"MemoryMax={2048 if value['service'] == 'routing' else 768 if value['service'] == 'search' else 256}M\n"
    if value["service"] == "downloads": contents += f"StateDirectory=obc-planner-downloads-{value['slot']}\n"
    units.mkdir(parents=True, exist_ok=True)
    (units / unit(value)).write_text(contents)
    execute(["systemctl", "daemon-reload"])
    execute(["systemctl", "restart", unit(value)])


def probe(value, base=BASE, execute=run, read=None):
    value = installed(value)
    directory = destination(value, base)
    environment = dict(item.split("=", 1) for item in shlex.split(execute(["systemctl", "show", unit(value), "--property=Environment", "--value"])))
    if execute(["systemctl", "show", unit(value), "--property=WorkingDirectory", "--value"]) != str(directory / "code"):
        raise ValueError("Service slot belongs to another identity")
    expected = {"installed": value,
                "release": {"size": (directory / "release.json").stat().st_size, "sha256": identity(environment["OBC_PLANNER_RELEASE_SHA"])},
                "runtime": {"size": (directory / "runtime.json").stat().st_size, "sha256": identity(environment["OBC_PLANNER_RUNTIME_SHA"])}}
    release, descriptor = documents(directory, expected)
    prerequisites(descriptor)
    name, port = value["service"], SERVICES[value["service"]][value["slot"]]
    waiting = read is None
    if waiting:
        def read(path):
            origin = environment.get("ROUTE_ORIGIN", environment.get("OBC_SEARCH_ORIGINS"))
            request = Request(f"http://127.0.0.1:{port}{path}", headers={"Origin": origin} if origin else {})
            with urlopen(request, timeout=5) as response:
                if origin and response.headers.get("Access-Control-Allow-Origin") != origin:
                    raise ValueError("Service origin readiness differs")
                return json.load(response)
    deadline = time.monotonic() + 120
    while True:
        try:
            if name == "routing": return {"service": name, "package": read("/v1/region")["package"]}
            if name == "downloads": return {"service": name, "catalog": read("/catalog")["sha256"]}
            status = read("/api/planner-search/status")
            if not status["parser"]["ready"] or len(status["regions"]) != 1 or status["regions"][0]["id"] != release["region"]:
                raise ValueError("Search service is not ready for this region")
            return {"service": name, "grid": status["regions"][0]["grid"], "model": status["parser"]["model"]}
        except (OSError, ValueError, KeyError):
            if not waiting or time.monotonic() >= deadline: raise
            time.sleep(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("inspect", "stage", "probe"))
    args = parser.parse_args()
    request = None if args.command == "inspect" else json.load(sys.stdin)
    result = inspect() if args.command == "inspect" else stage(request) if args.command == "stage" else probe(request)
    print(json.dumps(result))


if __name__ == "__main__": main()
