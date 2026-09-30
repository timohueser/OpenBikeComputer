"""Finish a verified planner rollout with one regional dataset in R2."""

import hashlib
from datetime import datetime
from html.parser import HTMLParser
import json
from pathlib import Path, PurePosixPath
import re
import tempfile
from urllib.parse import urljoin, urlsplit

try:
    from . import planner_deploy as deploy, planner_release as releases, planner_sources as sources, r2
except ImportError:
    import planner_deploy as deploy, planner_release as releases, planner_sources as sources, r2


def catalog(remote):
    return json.loads(r2.run_rclone(["cat", remote.path + "/planner/catalog.json"], remote.env, capture=True))


def active_id(current):
    if not isinstance(current, dict) or current.get("format") != 1:
        raise ValueError("Invalid planner catalogue")
    active = current.get("active")
    if active is not None and (not isinstance(active, dict) or not isinstance(active.get("id"), str) or
                               not re.fullmatch(r"[a-f0-9]{64}", active["id"])):
        raise ValueError("Invalid active planner release")
    return active["id"] if active else None


def before_publish(remote, identity):
    with tempfile.TemporaryDirectory() as directory:
        path = r2.fetch_optional(remote, "planner/catalog.json", Path(directory) / "catalog.json")
        current = json.loads(path.read_bytes()) if path else {"format": 1, "active": None}
        active = active_id(current)
    rows = json.loads(r2.run_rclone(["lsjson", remote.path + "/planner/releases", "--dirs-only"], remote.env, capture=True))
    if any(row["Path"].rstrip("/") not in {active, identity} for row in rows):
        raise ValueError("Finish the active deployment with obc planner finalize --apply before publishing another release.")


class Modules(HTMLParser):
    def __init__(self):
        super().__init__()
        self.urls = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "script" and attrs.get("type") == "module" and attrs.get("src"):
            self.urls.append(attrs["src"])


def verify_site(active, origin):
    address = urlsplit(origin)
    if address.scheme != "https" or not address.netloc or address.path or address.query or address.fragment:
        raise ValueError("Provide an HTTPS site origin with no path")
    page = origin + "/plan/"
    modules = Modules()
    with sources.open_url(page) as response:
        modules.feed(response.read().decode())
    scripts = []
    for path in modules.urls:
        url = urljoin(page, path)
        module = urlsplit(url)
        if (module.scheme, module.netloc) != (address.scheme, address.netloc):
            raise ValueError("Planner module uses another origin; cleanup is blocked.")
        with sources.open_url(url) as response:
            scripts.append(response.read().decode())
    code = "\n".join(scripts)
    if not all(active[key] in code for key in ["basemap", "terrain", "routing", "search"]):
        raise ValueError("Deploy site must serve the active planner release before cleanup.")


def manifest(remote, entry):
    identity = active_id({"format": 1, "active": entry})
    prefix = "planner/releases/" + identity + "/"
    raw = r2.run_rclone(["cat", remote.path + "/" + prefix + "release.json"], remote.env, capture=True)
    if hashlib.sha256(raw.encode()).hexdigest() != identity:
        raise ValueError("Active release manifest hash differs; cleanup is blocked.")
    document = json.loads(raw)
    if document.get("format") != 1 or document.get("region") != entry["region"] or not document.get("files"):
        raise ValueError("Invalid active release manifest")
    return document, prefix


def referenced_keys(document, prefix):
    keep = {prefix + "release.json"}
    for section in ["files", "source_files"]:
        for name in document.get(section, {}):
            path = PurePosixPath(name)
            if path.is_absolute() or ".." in path.parts or path.as_posix() != name or "\\" in name:
                raise ValueError("Invalid active release file")
            if section == "source_files" and (len(path.parts) != 2 or path.parts[0] != "sources"):
                raise ValueError("Invalid active source mirror")
            keep.add(prefix + name if section == "files" else "planner/sources/" + path.name)
    return keep


def plan(remote, current):
    if active_id(current) is None:
        raise ValueError("Deploy a planner release before cleanup.")
    document, prefix = manifest(remote, current["active"])
    keep = referenced_keys(document, prefix)
    rows = json.loads(r2.run_rclone(["lsjson", remote.path + "/planner", "--recursive", "--files-only"], remote.env, capture=True))
    found = {"planner/" + row["Path"]: row for row in rows}
    for key in found:
        if key.startswith(("planner/releases/", "planner/sources/")) and (
                PurePosixPath(key).as_posix() != key or ".." in PurePosixPath(key).parts or "\n" in key or "\r" in key):
            raise ValueError("Invalid planner object key; cleanup is blocked.")
    if prefix + "release.json" not in found:
        raise ValueError("Active release manifest is absent")
    for section in ["files", "source_files"]:
        for name, item in document.get(section, {}).items():
            key = prefix + name if section == "files" else "planner/sources/" + PurePosixPath(name).name
            if key not in found or found[key]["Size"] != item["bytes"]:
                raise ValueError("Active release is incomplete; cleanup is blocked.")
    stale = [r2.Target(key, row["Size"], row["ModTime"]) for key, row in found.items()
             if key.startswith(("planner/releases/", "planner/sources/")) and not key.startswith(prefix) and key not in keep]
    published = datetime.fromisoformat(found[prefix + "release.json"]["ModTime"])
    newer = [item for item in stale if datetime.fromisoformat(item.modified) > published]
    if newer:
        previous = current.get("previous")
        abandoned = referenced_keys(*manifest(remote, previous)) if previous else set()
        if any(item.key not in abandoned for item in newer):
            raise ValueError("Another planner upload is pending; finish it before cleanup.")
    return document, sorted(stale, key=lambda item: item.key)


def finalize(args):
    # GOVERNS: specs/planner-release.md
    # RULE: Remove inactive planner data only after the live services and web planner use the active release.
    remote = r2.bucket_remote()
    current = catalog(remote)
    document, stale = plan(remote, current)
    print(f"Keep planner release {current['active']['id']}.")
    print(f"Remove {len(stale)} inactive planner objects, {sum(item.bytes for item in stale) / 1e9:.2f} GB.")
    print("Device cells and terrain reference objects stay outside this cleanup.")
    if not args.apply:
        return
    deploy.verify_services(current["active"], document, args.site_origin)
    verify_site(current["active"], args.site_origin)
    if catalog(remote) != current:
        raise ValueError("Planner catalogue changed; repeat cleanup.")
    with tempfile.TemporaryDirectory() as directory:
        staging = Path(directory)
        if stale:
            r2.append_log(remote, staging, stale, "Finalize active planner release " + current["active"]["id"])
            listing = staging / "remove.txt"
            # Keep manifests until their data is removed so a failed rollback cleanup can retry.
            for manifests in [False, True]:
                keys = [item.key for item in stale if item.key.endswith("/release.json") == manifests]
                if keys:
                    listing.write_text("".join(key + "\n" for key in keys))
                    r2.run_rclone(["delete", remote.path, "--files-from-raw", str(listing)], remote.env)
    if plan(remote, current)[1]:
        raise ValueError("Inactive planner objects remain; repeat cleanup.")
    if catalog(remote) != current:
        raise ValueError("Planner catalogue changed; repeat cleanup.")
    deploy.activate(args.public_url, {**current, "previous": None})
    print("Planner rollout complete. R2 contains one regional planner dataset.")
