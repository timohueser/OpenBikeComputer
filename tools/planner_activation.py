"""Activate the three checked planner slots and retire their exact former routes."""

import json
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

from . import planner_install as install

CONFIG = Path("/etc/caddy/Caddyfile")
ROUTES = Path("/etc/caddy/planner/data")


def context(api_origin, config=CONFIG):
    install.origin(api_origin)
    parsed = urlsplit(api_origin)
    marker = parsed.netloc + " {\n"
    text = config.read_text()
    if marker not in text:
        raise ValueError("Configure the planner API virtual host before activation")
    return marker, text


def route(value):
    install.installed(value)
    name = value["service"]
    port = install.SERVICES[name][value["slot"]]
    path = f"/planner-api/services/{value['binding']}/{name}"
    rewrite = "    rewrite * /api/planner-search{path}\n" if name == "search" else ""
    return f"handle_path {path}/* {{\n{rewrite}    reverse_proxy 127.0.0.1:{port}\n}}\n"


def activate(request, routes=ROUTES, config=CONFIG, execute=install.run, probe=install.probe):
    stages = request["stages"]
    if len(stages) != 3 or {stage["installed"]["service"] for stage in stages} != set(install.SERVICES):
        raise ValueError("Activation needs all three service slots")
    api = stages[0]["candidate"]["api_origin"]
    if any(stage["candidate"]["api_origin"] != api for stage in stages):
        raise ValueError("Service API origins differ")
    marker, text = context(api, config)
    for stage in stages:
        if stage["installed"]["binding"] != install.binding(stage["candidate"]):
            raise ValueError("Service endpoint binding differs")
        if probe(stage) != stage["candidate"]["expected"]:
            raise ValueError("Service data readiness differs")
    routes.mkdir(parents=True, exist_ok=True)
    for stage in stages:
        value = stage["installed"]
        (routes / f"{value['service']}-{value['binding']}.caddy").write_text(route(value))
    downloads = next(stage["installed"] for stage in stages if stage["installed"]["service"] == "downloads")
    port = install.SERVICES["downloads"][downloads["slot"]]
    (routes / "offline.caddy").write_text(f"handle_path /planner-offline/* {{\n    reverse_proxy 127.0.0.1:{port}\n}}\n")
    line = f"    import {routes}/*.caddy\n"
    if line not in text:
        config.write_text(text.replace(marker, marker + line, 1))
    execute(["caddy", "validate", "--config", str(config)])
    execute(["systemctl", "reload", "caddy"])
    result = []
    for stage in stages:
        candidate = stage["candidate"]
        url = f"{api}/planner-api/services/{stage['installed']['binding']}/{stage['installed']['service']}"
        def read(path):
            if candidate["service"] == "search":
                path = path.removeprefix("/api/planner-search")
            origin = candidate["site_origin"] if candidate["service"] != "downloads" else None
            request = Request(url + path, headers={"Origin": origin} if origin else {})
            with urlopen(request, timeout=5) as response:
                if origin and response.headers.get("Access-Control-Allow-Origin") != origin:
                    raise ValueError("Public service origin readiness differs")
                return json.load(response)
        ready = probe(stage, read=read)
        if ready != candidate["expected"]:
            raise ValueError("Public service data readiness differs")
        result.append(ready)
    return result


def retire(request, routes=ROUTES, base=install.BASE, units=install.UNITS, config=CONFIG, execute=install.run):
    current = [install.installed(value) for value in request["current"]]
    previous = [install.installed(value) for value in request["previous"]]
    for old in previous:
        if old in current:
            continue
        if any(value["service"] == old["service"] and value["slot"] == old["slot"] for value in current):
            raise ValueError("Retirement would stop a current service slot")
        directory = execute(["systemctl", "show", install.unit(old), "--property=WorkingDirectory", "--value"])
        environment = execute(["systemctl", "show", install.unit(old), "--property=Environment", "--value"])
        import shlex
        values = dict(item.split("=", 1) for item in shlex.split(environment) if "=" in item)
        if Path(directory) != install.destination(old, base) / "code" or values.get("OBC_PLANNER_BINDING") != old["binding"]:
            raise ValueError("Retirement slot ownership differs")
        execute(["systemctl", "disable", "--now", install.unit(old)])
        (units / install.unit(old)).unlink(missing_ok=True)
        (routes / f"{old['service']}-{old['binding']}.caddy").unlink(missing_ok=True)
        if not any(value["service"] == old["service"] and value["id"] == old["id"] for value in current):
            import shutil
            shutil.rmtree(install.destination(old, base))
    execute(["systemctl", "daemon-reload"])
    execute(["caddy", "validate", "--config", str(config)])
    execute(["systemctl", "reload", "caddy"])
