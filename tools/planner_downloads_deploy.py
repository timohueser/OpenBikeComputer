"""Install a published offline block catalog beside the online planner."""

import argparse
import json
from pathlib import Path
import re

try:
    from . import planner_deploy as deploy, planner_maps as maps, planner_runtime, planner_offline
except ImportError:
    import planner_deploy as deploy, planner_maps as maps, planner_runtime, planner_offline


def install(args):
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.@:-]*", args.host) or args.max_cache_bytes <= 0:
        raise ValueError("Provide a valid SSH host and a positive cache budget.")
    identity, publication = planner_runtime.release(args.source, include_sources=False)
    if publication.get("grid", {}).get("format") != 2:
        raise ValueError("Provide a complete offline block publication.")
    base = "/opt/obc-planner/source"
    source = f"/opt/obc-planner/releases/{identity}/offline"
    cache = "/var/lib/obc-planner-downloads"
    print(f"Install offline preparation for release {identity} on {args.host}.")
    if not args.apply:
        return
    deploy.ssh(args.host, f"mkdir -p {source} {base} /etc/caddy/planner")
    files = maps.run("git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", cwd=maps.ROOT, capture_output=True).stdout
    maps.run("rsync", "-az", "--from0", "--files-from=-", str(maps.ROOT) + "/", f"{args.host}:{base}/", input=files)
    planner_offline.materialize(args.source, args.source / "runtime", ("offline/",))
    maps.run("rsync", "-a", str(args.source / "runtime/offline") + "/", f"{args.host}:{source}/")
    deploy.ssh(args.host, f"""id obc-planner-downloads >/dev/null 2>&1 || useradd --system --home-dir {cache} --shell /usr/sbin/nologin obc-planner-downloads
install -d -o obc-planner-downloads -g obc-planner-downloads {cache} /var/cache/obc-planner-downloads
chmod -R a+rX {source}
""")
    unit = deploy.service(
        f"/usr/bin/python3 -m tools.planner_downloads --source {source} --cache {cache}/selections --max-cache-bytes {args.max_cache_bytes} --objects-url {args.public_url}/planner/releases/{identity}/objects",
        {"PATH": "/usr/local/bin:/usr/bin:/bin"}, "256M")
    unit = unit.replace("DynamicUser=yes", f"User=obc-planner-downloads\nWorkingDirectory={base}\nStateDirectory=obc-planner-downloads\nCacheDirectory=obc-planner-downloads")
    config = "handle_path /planner-offline/* {\n    reverse_proxy 127.0.0.1:8790\n}\n"
    deploy.ssh(args.host, f"cat > /etc/systemd/system/obc-planner-downloads.service <<'UNIT'\n{unit}UNIT\nsystemctl daemon-reload\nsystemctl enable --now obc-planner-downloads\nsystemctl restart obc-planner-downloads\n")
    # Validate the loopback service before exposing its route through the existing planner import.
    deploy.ssh(args.host, "for attempt in $(seq 1 30); do curl -fsS http://127.0.0.1:8790/catalog >/dev/null && break; sleep 1; done\ncurl -fsS http://127.0.0.1:8790/catalog >/dev/null\n")
    deploy.ssh(args.host, f"cat > /etc/caddy/planner/offline.caddy <<'CONFIG'\n{config}CONFIG\ncaddy validate --config /etc/caddy/Caddyfile\nsystemctl reload caddy\n")
    print("Offline downloads: https://releases.openbikecomputer.com/planner-offline/catalog")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--public-url", default="https://maps.openbikecomputer.com")
    parser.add_argument("--max-cache-bytes", required=True, type=int)
    parser.add_argument("--apply", action="store_true")
    install(parser.parse_args())


if __name__ == "__main__":
    main()
