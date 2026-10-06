"""Install a verified grid release on the VPS, activate its public catalogue, and retire the old slot."""

import json
from pathlib import Path
import re
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request
from urllib.parse import urlsplit

from . import planner_geo as geo, planner_maps as maps, planner_offline as offline, planner_prepare, planner_release as releases, r2
from .planner_runtime import DATA_LAYERS, encoded, open_url, read_url, refuse_applied

RELEASES = "/opt/obc-planner/releases"
SOURCE = "/opt/obc-planner/source"
DOWNLOAD_CACHE = "/var/lib/obc-planner-downloads"


def checked_host(host):
    if not host or not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.@:-]*", host):
        raise ValueError("Provide --host USER@HOST")
    return host


def slot(entry):
    if not isinstance(entry, dict) or type(entry.get("slot")) is not int or entry["slot"] not in (0, 1):
        raise ValueError("The active catalogue entry needs slot 0 or 1; catalogue is unchanged")
    return entry["slot"]


def ssh(host, script):
    maps.run("ssh", "-o", "BatchMode=yes", host, "bash -se", input=script.encode())


def service(command, environment, memory):
    if any('\n' in value or '\r' in value or '"' in value for value in [command, *environment.values()]):
        raise ValueError("Invalid service configuration")
    return f"""[Unit]
Description=OpenBikeComputer planner
After=network.target
[Service]
DynamicUser=yes
ExecStart={command}
{''.join('Environment=' + key + '=' + value + chr(10) for key, value in environment.items())}Restart=on-failure
RestartSec=3
MemoryMax={memory}
CPUQuota=200%
TasksMax=64
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
[Install]
WantedBy=multi-user.target
"""


def deploy(args):
    host = checked_host(args.host)
    origin = urlsplit(args.site_origin)
    if origin.scheme != "https" or not origin.netloc or origin.path or origin.query or origin.fragment or any(c.isspace() for c in args.site_origin):
        raise ValueError("Provide an HTTPS --site-origin with no path")
    if args.api_url != "https://releases.openbikecomputer.com":
        raise ValueError("Configure the Caddy API virtual host before changing --api-url")
    identity, document = releases.grid_release(args.data_dir, include_sources=False)
    recipe = planner_prepare.recipe(args.recipe)
    if recipe["region"] != document["region"]:
        raise ValueError(f"Pass --region {document['region']} or its --recipe for this release")
    active = releases.endpoints(identity, document, recipe["name"], args.public_url, args.tiles_url, args.api_url)
    if read_url(active["manifest"]) != document:
        raise ValueError("Publish this release before deployment")
    try:
        current = read_url(args.public_url + "/planner/catalog.json")
    except HTTPError as error:
        if error.code != 404: raise
        current = {"format": 1, "active": None, "previous": None}
    refuse_applied(current)
    old = current["active"]
    target = 0 if old is None else slot(old)
    # A new release goes into the other slot; the same release restarts in place.
    if old is not None and old["id"] != identity: target = 1 - target
    route_port, search_port = [(8787, 8786), (8785, 8784)][target]
    base = f"{RELEASES}/{identity}"
    print(f"Install {identity} on {host}; routing :{route_port}, search :{search_port}.")
    if not args.apply: return
    install(host, args.data_dir, document, base)
    units = {
        "routing": service(f"{base}/bin/route-server {base}/routing",
                           {"ROUTE_LISTEN": f"127.0.0.1:{route_port}", "ROUTE_WORKERS": "2", "ROUTE_ORIGIN": args.site_origin}, "2048M"),
        "search": service(f"/usr/local/bin/node {base}/search/server.mjs",
                          {"OBC_SEARCH_PORT": str(search_port), "OBC_SEARCH_DATA": base + "/search/data",
                           "OBC_SEARCH_PYTHON": base + "/search/.venv/bin/python", "OBC_SEARCH_REGIONS": document["region"],
                           "OBC_SEARCH_ORIGINS": args.site_origin}, "768M"),
    }
    ssh(host, "".join(f"cat > /etc/systemd/system/obc-planner-{name}-{target}.service <<'UNIT'\n{contents}UNIT\n"
                      for name, contents in units.items()) +
        f"systemctl daemon-reload\nsystemctl enable obc-planner-routing-{target} obc-planner-search-{target}\n"
        f"systemctl restart obc-planner-routing-{target} obc-planner-search-{target}\n")
    # The previous slot keeps its Caddy route for open pages until finalize retires it.
    path = f"/planner-api/releases/{identity}"
    caddy = f"""handle_path {path}/routing/* {{
    reverse_proxy 127.0.0.1:{route_port}
}}
handle_path {path}/search/* {{
    rewrite * /api/planner-search{{path}}
    reverse_proxy 127.0.0.1:{search_port}
}}
"""
    ssh(host, f"cat > /etc/caddy/planner/slot-{target}.caddy <<'CONFIG'\n{caddy}CONFIG\n" + r"""python3 - <<'PY'
from pathlib import Path
p=Path('/etc/caddy/Caddyfile')
text=p.read_text()
line='    import /etc/caddy/planner/*.caddy\n'
if line not in text:
    marker='releases.openbikecomputer.com {\n'
    if marker not in text: raise SystemExit('Add the planner import to the API virtual host.')
    p.with_suffix('.before-planner').write_text(text)
    p.write_text(text.replace(marker,marker+line,1))
PY
caddy validate --config /etc/caddy/Caddyfile
systemctl reload caddy
""")
    verify_services(active, document, args.site_origin)
    # The one download service switches to the release that the catalogue activates next.
    switch_downloads(host, identity, base, args.public_url)
    active["slot"] = target
    activate(args.public_url, {"format": 1, "active": active,
                               "previous": old if old and old["id"] != identity else current["previous"]})
    print(f"Rollout is incomplete. Run: obc planner finalize --host {host} --apply")


def install(host, data, document, base):
    """Copy the checkout, the release's runtime files and the search app, then build the route server."""
    runtime = data / "runtime"
    offline.materialize(data, runtime, document, ("routing/", "search/", "offline/"))
    ssh(host, f"mkdir -p {base}/routing {base}/search/data {base}/search/node_modules {base}/offline {SOURCE} /etc/caddy/planner")
    tracked = maps.run("git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", cwd=maps.ROOT, capture_output=True).stdout
    maps.run("rsync", "-az", "--from0", "--files-from=-", str(maps.ROOT) + "/", f"{host}:{SOURCE}/", input=tracked)
    for part, destination in [("routing", "routing"), ("search", "search/data"), ("offline", "offline")]:
        maps.run("rsync", "-az", str(runtime / part) + "/", f"{host}:{base}/{destination}/")
    search_prefix = b"apps/planner-search/"
    search_files = b"\0".join(path[len(search_prefix):] for path in tracked.split(b"\0") if path.startswith(search_prefix)) + b"\0"
    maps.run("rsync", "-az", "--from0", "--files-from=-", str(maps.ROOT / "apps/planner-search") + "/",
             f"{host}:{base}/search/", input=search_files)
    maps.run("rsync", "-az", str(maps.ROOT / "apps/planner-search/node_modules") + "/", f"{host}:{base}/search/node_modules/")
    ssh(host, f"""cd {SOURCE}
/root/.cargo/bin/cargo build --locked --release -p route-server -j 2
mkdir -p {base}/bin
cp target/release/route-server {base}/bin/.route-server.next
mv {base}/bin/.route-server.next {base}/bin/route-server
{base}/bin/route-server {base}/routing --verify
UV_PROJECT_ENVIRONMENT={base}/search/.venv uv sync --locked --group search-runtime --project {SOURCE}
id obc-planner-downloads >/dev/null 2>&1 || useradd --system --home-dir {DOWNLOAD_CACHE} --shell /usr/sbin/nologin obc-planner-downloads
install -d -o obc-planner-downloads -g obc-planner-downloads {DOWNLOAD_CACHE} /var/cache/obc-planner-downloads
chmod -R a+rX {base}
""")


def switch_downloads(host, identity, base, public_url):
    """Serve download selections of this release. Their payloads stream from its R2 object pool."""
    unit = service(
        f"/usr/bin/python3 -m tools.planner_downloads --source {base}/offline --cache {DOWNLOAD_CACHE}/selections "
        f"--max-cache-bytes {256 * 1024 * 1024} --objects-url {public_url}/planner/objects "
        "--public-url https://releases.openbikecomputer.com/planner-offline",
        {"PATH": "/usr/local/bin:/usr/bin:/bin"}, "256M")
    unit = unit.replace("DynamicUser=yes", f"User=obc-planner-downloads\nWorkingDirectory={SOURCE}\nStateDirectory=obc-planner-downloads\nCacheDirectory=obc-planner-downloads")
    config = "handle_path /planner-offline/* {\n    reverse_proxy 127.0.0.1:8790\n}\n"
    # Check the loopback service before its route goes through the planner import.
    ssh(host, f"""cat > /etc/systemd/system/obc-planner-downloads.service <<'UNIT'
{unit}UNIT
systemctl daemon-reload
systemctl enable obc-planner-downloads
systemctl restart obc-planner-downloads
for attempt in $(seq 1 30); do curl -fsS http://127.0.0.1:8790/catalog >/dev/null && break; sleep 1; done
curl -fsS http://127.0.0.1:8790/catalog >/dev/null
cat > /etc/caddy/planner/offline.caddy <<'CONFIG'
{config}CONFIG
caddy validate --config /etc/caddy/Caddyfile
systemctl reload caddy
""")


def retire(host, active):
    """Stop the inactive slot, remove its Caddy route, and delete every release directory except the active one."""
    identity, number = active["id"], 1 - slot(active)
    if not re.fullmatch(r"[a-f0-9]{64}", identity): raise ValueError("Invalid active planner release")
    ssh(host, f"""for name in routing search; do
  unit=obc-planner-$name-{number}
  if [ -e /etc/systemd/system/$unit.service ]; then systemctl disable --now $unit; rm /etc/systemd/system/$unit.service; fi
done
systemctl daemon-reload
rm -f /etc/caddy/planner/slot-{number}.caddy
caddy validate --config /etc/caddy/Caddyfile
systemctl reload caddy
find {RELEASES} -mindepth 1 -maxdepth 1 ! -name {identity} -exec rm -rf {{}} +
""")


def verify_services(active, document, origin):
    deadline = time.monotonic() + 120
    while True:
        try:
            region = read_url(active["routing"] + "/v1/region")
            status = read_url(active["search"] + "/status")
            if region["package"] == document["routing_package"] and status["parser"]["ready"] and status["regions"][0]["id"] == document["region"]:
                break
        except (OSError, ValueError, KeyError): pass
        if time.monotonic() >= deadline: raise ValueError("Services did not become ready; catalogue is unchanged")
        time.sleep(1)
    with open_url(Request(active["basemap"], headers={"Origin": origin})) as response:
        if response.headers.get("Access-Control-Allow-Origin") != "*": raise ValueError("Tile CORS is absent")
        tilejson = json.load(response)
        if tilejson["maxzoom"] != 14: raise ValueError("Basemap is incomplete")
    if not read_url(active["places"]).get("tiles"): raise ValueError("Rider places are absent")
    if read_url(active["overlays"]).get("routing_package") != document["routing_package"]:
        raise ValueError("Overlay tiles use another routing package")
    for layer, url in active["layers"].items():
        if not read_url(url).get(DATA_LAYERS[layer]): raise ValueError(f"Data layer {layer} is absent")
    probe = document["probe"]
    x, y = map(int, geo.mercator(*probe["points"][0], 12))
    for url in [tilejson["tiles"][0].replace("{z}", "12").replace("{x}", str(x)).replace("{y}", str(y)),
                active["terrain"].replace("{z}", "12").replace("{x}", str(x)).replace("{y}", str(y)),
                active["sprites"] + "/light@2x.json", active["sprites"] + "/light@2x.png",
                active["glyphs"].replace("{fontstack}", "Noto%20Sans%20Regular").replace("{range}", "0-255")]:
        with open_url(url) as response:
            if response.status != 200 or not response.read(): raise ValueError("Regional map tiles or style assets are absent")
    if "routes" in active:
        with open_url(active["routes"].replace("{cell}", f"9-{x >> 3}-{y >> 3}")) as response:
            if response.status != 200 or json.load(response).get("format") != 1: raise ValueError("Route catalog cell is absent")
    def post(url, body):
        with open_url(Request(url, data=encoded(body), headers={"Origin": origin, "Content-Type": "application/json"})) as response:
            if response.headers.get("Access-Control-Allow-Origin") not in {origin, "*"}:
                raise ValueError("Service CORS is absent")
            return json.load(response)
    answer = post(active["search"] + "/query", {"q": probe["query"], "view": probe["view"]})
    if not answer.get("results"): raise ValueError("Place search has no regional results")
    answer = post(active["search"] + "/query", {"q": "show " + probe["query"], "view": probe["view"]})
    if not answer.get("results") or answer.get("canRetry") or answer.get("parserMs", 0) <= 0:
        raise ValueError("Smart search did not run model inference")
    route = post(active["routing"] + "/v1/route", {"points": probe["points"], "profile": "touring"})
    if not route.get("routes"): raise ValueError("Regional routing did not return a route")


def activate(public_url, catalog):
    remote = r2.bucket_remote()
    with tempfile.TemporaryDirectory(prefix="planner-activate-") as directory:
        # An apply can switch the catalogue while a deploy runs: read the bucket, not the CDN.
        current = r2.fetch_optional(remote, "planner/catalog.json", Path(directory) / "current.json")
        if current:
            refuse_applied(json.loads(current.read_bytes()))
        path = Path(directory) / "catalog.json"
        path.write_bytes(encoded(catalog))
        r2.run_rclone(["copyto", str(path), f"{remote.path}/planner/catalog.json", "--header-upload", "Content-Type: application/json",
                       "--header-upload", "Cache-Control: public,max-age=30,must-revalidate"], remote.env)
    print(f"Active catalogue: {public_url}/planner/catalog.json")
