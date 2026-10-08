"""Build planner services on the VPS and probe them before catalog publication."""

import argparse
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
import time
import tomllib
from urllib.request import Request, urlopen

from . import planner_offline as offline, planner_runtime as runtime

PORTS = {"routing": 8787, "search": 8786, "downloads": 8790}
UNITS = Path("/etc/systemd/system")
CADDY = Path("/etc/caddy/Caddyfile")
ROUTES = Path("/etc/caddy/planner")
PREFIXES = ("routing/", "search/", "offline/")


def run(argv, **kwargs):
    return subprocess.run(argv, check=True, text=True, **kwargs)


def unit(name):
    return f"obc-data-planner-{name}.service"


def configuration(name, directory, document, origins, release, node, python):
    source, data = directory / "source", directory / "data"
    env = {"PYTHONDONTWRITEBYTECODE": "1"}
    if name == "routing":
        command = [directory / "planner-service", data / "routing"]
        env.update(ROUTE_LISTEN=f"127.0.0.1:{PORTS[name]}", ROUTE_WORKERS="2", ROUTE_ORIGIN=origins["site_origin"])
    elif name == "search":
        command = [node, source / "planner/search/server.mjs"]
        env.update(OBC_SEARCH_PORT=str(PORTS[name]), OBC_SEARCH_DATA=str(data / "search"),
                   OBC_SEARCH_PYTHON=str(python), OBC_SEARCH_REGIONS=document["region"], OBC_SEARCH_ORIGINS=origins["site_origin"])
    else:
        command = [python, "-m", "tools.planner_downloads", "--source", data / "offline",
                   "--cache", "/var/lib/obc-data-planner-downloads/selections", "--max-cache-bytes", str(256 * 1024 * 1024),
                   "--port", str(PORTS[name]), "--objects-url", origins["objects_origin"] + "/planner/objects",
                   "--public-url", f"{origins['api_origin']}/planner-api/releases/{release}/downloads"]
    # systemd quotes each token; these values contain no specifiers or shell expansions.
    quote = lambda value: json.dumps(str(value)).replace("%", "%%").replace("$", "$$")
    settings = "".join(f"Environment={quote(key + '=' + value)}\n" for key, value in env.items())
    state = "StateDirectory=obc-data-planner-downloads\n" if name == "downloads" else ""
    return (f"[Unit]\nDescription=Planner {name}\nAfter=network.target\n\n[Service]\n"
            f"DynamicUser=yes\nWorkingDirectory={source}\nExecStart={' '.join(map(quote, command))}\n{settings}{state}"
            "Restart=on-failure\nRestartSec=2\nNoNewPrivileges=yes\nProtectSystem=strict\nProtectHome=yes\nPrivateTmp=yes\n"
            "\n[Install]\nWantedBy=multi-user.target\n")


def routes(release):
    result = ""
    for name, port in PORTS.items():
        rewrite = "    rewrite * /api/planner-search{path}\n" if name == "search" else ""
        result += f"handle_path /planner-api/releases/{release}/{name}/* {{\n{rewrite}    reverse_proxy 127.0.0.1:{port}\n}}\n"
    return result + "handle_path /planner-offline/* {\n    reverse_proxy 127.0.0.1:8790\n}\n"


def expected(document):
    files = document["files"]
    return {
        "routing": files["routing/blocks.json"]["sha256"],
        "search": {"region": document["region"], "grid": files[f"search/{document['region']}.grid.json"]["sha256"],
                   "model": {name: files[f"search/model/{name}"]["sha256"] for name in ("labels.json", "tokenizer.json", "model.int8.onnx")}},
        "downloads": files["offline/catalog.json"]["sha256"],
    }


def probe(document, origins, release, public=False, read=None):
    wanted = expected(document)
    def fetch(name, path):
        url = (f"{origins['api_origin']}/planner-api/releases/{release}/{name}" if public else f"http://127.0.0.1:{PORTS[name]}")
        if name == "search" and not public:
            path = "/api/planner-search" + path
        request = Request(url + path, headers={"Origin": origins["site_origin"], "User-Agent": "OpenBikeComputer/1.0"})
        with urlopen(request, timeout=5) as response:
            if name != "downloads" and response.headers.get("Access-Control-Allow-Origin") != origins["site_origin"]:
                raise ValueError("Planner service does not allow the site origin")
            return json.load(response)
    read = read or fetch
    if read("routing", "/v1/region")["package"] != wanted["routing"]:
        raise ValueError("Routing opened another package")
    search = read("search", "/status")
    regions = search["regions"]
    if (not search["parser"]["ready"] or len(regions) != 1 or regions[0]["id"] != wanted["search"]["region"]
            or regions[0]["grid"] != wanted["search"]["grid"] or search["parser"]["model"] != wanted["search"]["model"]):
        raise ValueError("Search opened another grid or model")
    if read("downloads", "/catalog")["sha256"] != wanted["downloads"]:
        raise ValueError("Downloads opened another catalog")


def ready(document, origins, release, public=False):
    deadline = time.monotonic() + 120
    while True:
        try:
            probe(document, origins, release, public)
            return
        except (OSError, ValueError, KeyError):
            if time.monotonic() >= deadline:
                raise
            time.sleep(1)


def install(directory, document, origins, release, node, python, execute=run,
            units=UNITS, routes_dir=ROUTES, caddy=CADDY, check=ready):
    # Both slots can still be running when the old publisher hands over.
    legacy = [f"obc-planner-{name}-{slot}.service" for name in ("routing", "search") for slot in (0, 1)]
    legacy += ["obc-planner-downloads.service"]
    for name in legacy + [unit(name) for name in PORTS]:
        if (units / name).exists():
            execute(["systemctl", "disable", "--now", name])
    offline.materialize(directory / "payload", directory / "data", document, PREFIXES)
    execute(["chmod", "-R", "a+rX", str(directory)])
    for name in PORTS:
        (units / unit(name)).write_text(configuration(name, directory, document, origins, release, node, python))
    execute(["systemctl", "daemon-reload"])
    routes_dir.mkdir(parents=True, exist_ok=True)
    for path in routes_dir.glob("*.caddy"):
        path.unlink()
    (routes_dir / "live.caddy").write_text(routes(release))
    execute(["caddy", "validate", "--config", str(caddy)])
    execute(["systemctl", "reload", "caddy"])
    execute(["systemctl", "enable", "--now", *[unit(name) for name in PORTS]])
    check(document, origins, release)
    check(document, origins, release, public=True)


def remote(directory, release, commit):
    source = directory / "source"
    config = tomllib.loads((source / "data/planner-runtime.toml").read_text())
    target, origins = config["target"], config["publication"]
    node = shutil.which("node")
    if not node:
        raise ValueError("Install the configured Node version on the VPS")
    node_version = run([node, "--version"], capture_output=True).stdout.strip().removeprefix("v")
    if (platform.system() != "Linux" or platform.machine() + "-unknown-linux-gnu" != target["triple"]
            or ".".join(platform.python_version_tuple()[:2]) != target["python"]
            or ".".join(node_version.split(".")[:2]) != target["node"]):
        raise ValueError("VPS Python, Node or platform differs from data/planner-runtime.toml")
    config_text = CADDY.read_text()
    if f"import {ROUTES}/*.caddy" not in config_text:
        raise ValueError("Configure the planner route import in the API Caddy virtual host")
    python = directory / "venv/bin/python"
    run([sys.executable, "-m", "venv", str(directory / "venv")])
    run([str(python), "-m", "pip", "install", "--require-hashes", "-r", str(directory / "requirements.txt")])
    run(["npm", "ci", "--omit=dev", "--prefix", str(source / "planner/search")])
    env = {**os.environ, "PATH": str(Path.home() / ".cargo/bin") + os.pathsep + os.environ.get("PATH", ""),
           "CARGO_TARGET_DIR": str(directory / "build"), "CARGO_INCREMENTAL": "0"}
    run(["cargo", "build", "--release", "--locked", "-p", "planner-service", "-j", "2"], cwd=source, env=env)
    shutil.copy2(directory / "build/release/planner-service", directory / "planner-service")
    shutil.rmtree(directory / "build")
    document = json.loads((directory / "payload/release.json").read_bytes())
    install(directory, document, origins, release, node, python)
    (directory / "installed.json").write_text(json.dumps({"commit": commit, "release": release}))


def publish(root, host, commit, release, manifest, store):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+@[A-Za-z0-9.-]+", host):
        raise ValueError("Set OBC_PLANNER_HOST to USER@HOST")
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or not re.fullmatch(r"[0-9a-f]{64}", release):
        raise ValueError("Invalid published commit or release")
    ssh = ["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", host]
    with tempfile.TemporaryDirectory(prefix="publish-", dir=store) as temporary:
        temporary = Path(temporary)
        run(["git", "archive", "--format=tar", "--output", str(temporary / "source.tar"), commit], cwd=root)
        export = temporary / "export"
        export.mkdir()
        with tarfile.open(temporary / "source.tar") as archive:
            for name in ("pyproject.toml", "uv.lock", ".python-version"):
                (export / name).write_bytes(archive.extractfile(name).read())
        run(["uv", "export", "--locked", "--no-default-groups", "--group", "search-runtime", "--no-emit-project",
             "--format", "requirements-txt", "--output-file", str(temporary / "requirements.txt")], cwd=export, stdout=subprocess.DEVNULL)
        document = json.loads(manifest.read_bytes())
        with tarfile.open(temporary / "data.tar", "w") as archive:
            archive.add(manifest, arcname="release.json")
            objects = {}
            for name, item in document["files"].items():
                runtime.relative_path(name)
                if name.startswith(PREFIXES):
                    objects[item["transport"]["sha256"]] = item["transport"]
            for sha, item in objects.items():
                if not re.fullmatch(r"[a-f0-9]{64}", sha):
                    raise ValueError("Invalid planner object identity")
                path = store / "objects" / sha[:2] / sha
                offline.verify(path, item)
                archive.add(path, arcname=f"objects/{sha}")
        directory = run([*ssh, "mkdir -p /opt/obc-planner/published && chmod a+rx /opt/obc-planner /opt/obc-planner/published && mktemp -d /opt/obc-planner/published/run-XXXXXXXX"], capture_output=True).stdout.strip()
        if not re.fullmatch(r"/opt/obc-planner/published/run-[A-Za-z0-9]+", directory):
            raise ValueError("Unexpected VPS staging path")
        run(["scp", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
             *[str(temporary / name) for name in ("source.tar", "data.tar", "requirements.txt")], f"{host}:{directory}/"])
        command = (f"cd {directory} && mkdir source payload && tar -xf source.tar -C source && tar -xf data.tar -C payload"
                   f" && rm source.tar data.tar && cd source && python3 -m tools.planner_publish --remote {directory} --release {release} --commit {commit}")
        run([*ssh, command])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--remote", type=Path)
    parser.add_argument("--host")
    parser.add_argument("--commit", required=True)
    parser.add_argument("--release", required=True)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--store", type=Path)
    args = parser.parse_args()
    if args.remote:
        remote(args.remote, args.release, args.commit)
    else:
        publish(Path.cwd(), args.host, args.commit, args.release, args.manifest, args.store)


if __name__ == "__main__":
    main()
