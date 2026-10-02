"""Install a verified planner release and activate its public catalogue."""

import json
from pathlib import Path
import re
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request
from urllib.parse import urlsplit

try:
    from . import planner_maps as maps, planner_release as releases, planner_sources as sources, r2
except ImportError:
    import planner_maps as maps, planner_release as releases, planner_sources as sources, r2


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
    if not args.host or not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.@:-]*", args.host):
        raise ValueError("Provide --host USER@HOST")
    origin = urlsplit(args.site_origin)
    if origin.scheme != "https" or not origin.netloc or origin.path or origin.query or origin.fragment or any(c.isspace() for c in args.site_origin):
        raise ValueError("Provide an HTTPS --site-origin with no path")
    if args.api_url != "https://releases.openbikecomputer.com":
        raise ValueError("Configure the Caddy API virtual host before changing --api-url")
    identity, document = releases.release(args.data_dir)
    active = releases.endpoints(identity, document, args.public_url, args.tiles_url, args.api_url)
    published = releases.read_url(active["manifest"])
    if published != document:
        raise ValueError("Publish this release before deployment")
    try:
        current = releases.read_url(args.public_url + "/planner/catalog.json")
    except HTTPError as error:
        if error.code != 404: raise
        current = {"format": 1, "active": None, "previous": None}
    old = current["active"]
    if old is not None and (not isinstance(old, dict) or type(old.get("slot")) is not int or old["slot"] not in (0, 1)):
        raise ValueError("The active catalogue entry needs slot 0 or 1; catalogue is unchanged")
    slot = 0 if old is None else 1 - old["slot"]
    if old and old["id"] == identity:
        slot = old["slot"]
    route_port, search_port = [(8787, 8786), (8785, 8784)][slot]
    base = f"/opt/obc-planner/releases/{identity}"
    print(f"Install {identity} on {args.host}; routing :{route_port}, search :{search_port}.")
    if not args.apply: return
    ssh(args.host, f"mkdir -p {base}/routing {base}/search/data/model {base}/search/node_modules /opt/obc-planner/source /etc/caddy/planner")
    tracked = maps.run("git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", cwd=maps.ROOT, capture_output=True)
    maps.run("rsync", "-az", "--from0", "--files-from=-", str(maps.ROOT) + "/",
             f"{args.host}:/opt/obc-planner/source/", input=tracked.stdout)
    if document.get("grid"):
        try: from . import planner_offline
        except ImportError: import planner_offline
        runtime = args.data_dir / "runtime"
        # The route server serves no overlays; phones read their overlay cells from the object pool.
        planner_offline.materialize(args.data_dir, runtime, ("routing/", "search/", "offline/"), skip=("routing/layers",))
        maps.run("rsync", "-az", str(runtime / "routing") + "/", f"{args.host}:{base}/routing/")
        maps.run("rsync", "-az", str(runtime / "search") + "/", f"{args.host}:{base}/search/data/")
    else:
        maps.run("rsync", "-az", str(args.data_dir / "routing") + "/", f"{args.host}:{base}/routing/")
        maps.run("rsync", "-az", str(args.data_dir / "search" / (document["region"] + ".sqlite")), f"{args.host}:{base}/search/data/")
        maps.run("rsync", "-az", str(args.data_dir / "search/model") + "/", f"{args.host}:{base}/search/data/model/")
    search_prefix = b"apps/planner-search/"
    search_files = b"\0".join(path[len(search_prefix):] for path in tracked.stdout.split(b"\0")
                              if path.startswith(search_prefix)) + b"\0"
    maps.run("rsync", "-az", "--from0", "--files-from=-", str(maps.ROOT / "apps/planner-search") + "/",
             f"{args.host}:{base}/search/", input=search_files)
    maps.run("rsync", "-az", str(maps.ROOT / "apps/planner-search/node_modules") + "/", f"{args.host}:{base}/search/node_modules/")
    maps.run("rsync", "-az", str(maps.ROOT / "fixtures/sources/route-import/komoot-schwarzwald.gpx"), f"{args.host}:{base}/search/sample.gpx")
    ssh(args.host, f"""cd /opt/obc-planner/source
/root/.cargo/bin/cargo build --locked --release -p route-server -j 2
mkdir -p {base}/bin
cp target/release/route-server {base}/bin/.route-server.next
mv {base}/bin/.route-server.next {base}/bin/route-server
{base}/bin/route-server {base}/routing --verify
python3 -m venv {base}/search/.venv
{base}/search/.venv/bin/pip install -q -r {base}/search/requirements.txt
chmod -R a+rX {base}
""")
    units = {
        "routing": service(f"{base}/bin/route-server {base}/routing",
                           {"ROUTE_LISTEN": f"127.0.0.1:{route_port}", "ROUTE_WORKERS": "2", "ROUTE_ORIGIN": args.site_origin}, "2048M"),
        "search": service(f"/usr/local/bin/node {base}/search/server.mjs",
                          {"OBC_SEARCH_PORT": str(search_port), "OBC_SEARCH_DATA": base + "/search/data",
                           "OBC_SEARCH_PYTHON": base + "/search/.venv/bin/python", "OBC_SEARCH_REGIONS": document["region"],
                           "OBC_SEARCH_SAMPLE": base + "/search/sample.gpx",
                           "OBC_SEARCH_ORIGINS": args.site_origin, "OBC_QUERY_ROUTER": f"http://127.0.0.1:{route_port}"}, "768M"),
    }
    for name, contents in units.items():
        ssh(args.host, f"cat > /etc/systemd/system/obc-planner-{name}-{slot}.service <<'UNIT'\n{contents}UNIT\n")
    ssh(args.host, f"systemctl daemon-reload\nsystemctl enable obc-planner-routing-{slot} obc-planner-search-{slot}\nsystemctl restart obc-planner-routing-{slot} obc-planner-search-{slot}\n")
    # Keep the previous slot and its Caddy route for clients that still have its page open.
    path = f"/planner-api/releases/{identity}"
    caddy = f"""handle_path {path}/routing/* {{
    reverse_proxy 127.0.0.1:{route_port}
}}
handle_path {path}/search/* {{
    rewrite * /api/planner-search{{path}}
    reverse_proxy 127.0.0.1:{search_port}
}}
"""
    ssh(args.host, f"cat > /etc/caddy/planner/slot-{slot}.caddy <<'CONFIG'\n{caddy}CONFIG\n")
    ssh(args.host, r"""python3 - <<'PY'
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
    if document.get("grid"):
        try: from . import planner_downloads_deploy
        except ImportError: import planner_downloads_deploy
        from types import SimpleNamespace
        planner_downloads_deploy.install(SimpleNamespace(host=args.host, source=args.data_dir,
            max_cache_bytes=256 * 1024 * 1024, public_url=args.public_url, apply=True))
    active["slot"] = slot
    catalog = {"format": 1, "active": active, "previous": old if old and old["id"] != identity else current["previous"]}
    activate(args.public_url, catalog)
    print("Rollout is incomplete. After Deploy site succeeds, run: obc planner finalize --apply")


def verify_services(active, document, origin):
    deadline = time.monotonic() + 120
    while True:
        try:
            region = releases.read_url(active["routing"] + "/v1/region")
            status = releases.read_url(active["search"] + "/status")
            if region["package"] == document["routing_package"] and status["parser"]["ready"] and status["regions"][0]["id"] == document["region"]:
                break
        except (OSError, ValueError, KeyError): pass
        if time.monotonic() >= deadline: raise ValueError("Services did not become ready; catalogue is unchanged")
        time.sleep(1)
    with sources.open_url(Request(active["basemap"], headers={"Origin": origin})) as response:
        if response.headers.get("Access-Control-Allow-Origin") != "*": raise ValueError("Tile CORS is absent")
        tilejson = json.load(response)
        if tilejson["maxzoom"] != 14: raise ValueError("Basemap is incomplete")
    with sources.open_url(active["places"]) as response:
        if not json.load(response).get("tiles"): raise ValueError("Rider places are absent")
    with sources.open_url(active["overlays"]) as response:
        if json.load(response).get("routing_package") != document["routing_package"]:
            raise ValueError("Overlay tiles use another routing package")
    probe = document["probe"]
    import math
    z = 12
    x = int((probe["points"][0][0] + 180) / 360 * (1 << z))
    lat = math.radians(probe["points"][0][1])
    y = int((1 - math.asinh(math.tan(lat)) / math.pi) / 2 * (1 << z))
    for url in [tilejson["tiles"][0].replace("{z}", "12").replace("{x}", str(x)).replace("{y}", str(y)),
                active["terrain"].replace("{z}", "12").replace("{x}", str(x)).replace("{y}", str(y)),
                active["sprites"] + "/light@2x.json", active["sprites"] + "/light@2x.png",
                active["glyphs"].replace("{fontstack}", "Noto%20Sans%20Regular").replace("{range}", "0-255")]:
        with sources.open_url(url) as response:
            if response.status != 200 or not response.read(): raise ValueError("Regional map tiles or style assets are absent")
    with sources.open_url(active["search"] + "/sample") as response:
        points = json.load(response)["coordinates"]
        if len(points) < 2: raise ValueError("Example route is absent")
    def post(url, body):
        with sources.open_url(Request(url, data=releases.encoded(body), headers={"Origin": origin, "Content-Type": "application/json"})) as response:
            if response.headers.get("Access-Control-Allow-Origin") not in {origin, "*"}:
                raise ValueError("Service CORS is absent")
            return json.load(response)
    answer = post(active["search"] + "/query", {"q": probe["query"], "region": document["region"], "view": probe["view"]})
    if not answer.get("results"): raise ValueError("Place search has no regional results")
    answer = post(active["search"] + "/query", {"q": "show " + probe["query"], "submitted": True,
                  "region": document["region"], "view": probe["view"]})
    if not answer.get("results") or answer.get("canRetry") or answer.get("parserMs", 0) <= 0:
        raise ValueError("Smart search did not run model inference")
    route = post(active["routing"] + "/v1/route", {"points": probe["points"], "profile": "touring"})
    if not route.get("routes"): raise ValueError("Regional routing did not return a route")


def activate(public_url, catalog):
    remote = r2.bucket_remote()
    with tempfile.TemporaryDirectory(prefix="planner-activate-") as directory:
        path = Path(directory) / "catalog.json"
        path.write_bytes(releases.encoded(catalog))
        r2.run_rclone(["copyto", str(path), f"{remote.path}/planner/catalog.json", "--header-upload", "Content-Type: application/json",
                       "--header-upload", "Cache-Control: public,max-age=30,must-revalidate"], remote.env)
    print(f"Active catalogue: {public_url}/planner/catalog.json")


def rollback(args):
    catalog = releases.read_url(args.public_url + "/planner/catalog.json")
    previous = catalog.get("previous")
    if not previous: raise ValueError("No previous planner release")
    document = releases.read_url(previous["manifest"])
    verify_services(previous, document, args.site_origin)
    print(f"Restore release {previous['id']}. Rebuild the site after activation.")
    if args.apply:
        activate(args.public_url, {"format": 1, "active": previous, "previous": catalog["active"]})
