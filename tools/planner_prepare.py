"""Prepare a regional planner release from pinned OSM and terrain inputs."""

import json
import os
import re
import shutil
from zoneinfo import ZoneInfo

try:
    from . import planner_maps as maps, planner_sources as sources, planner_release as releases
except ImportError:
    import planner_maps as maps, planner_sources as sources, planner_release as releases


def recipe(path):
    document = json.loads(path.read_text())
    if document["format"] != 1 or not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", document["region"]):
        raise ValueError("Invalid region recipe")
    if document["access"] != "DE":
        raise ValueError("Routing has German access defaults. Add and verify each country's access policy before extending coverage.")
    if not document["countries"] or any(not re.fullmatch(r"[A-Z]{2}", code) for code in document["countries"]):
        raise ValueError("Name the region's countries as ISO codes")
    try:
        ZoneInfo(document["time_zone"])
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("Name the region's IANA time zone in the recipe") from error
    maps.bounds(",".join(map(str, document["bounds"])))
    if not re.fullmatch(r"[a-f0-9]{64}", document["osm"]["sha256"]):
        raise ValueError("Pin the OSM SHA-256 in the recipe")
    profiles = document["profiles"]
    # The presets of route-build (`Profile::presets`); it rejects any other ID at bake time.
    if not isinstance(profiles, list) or not profiles or any(
            not isinstance(profile, str) or not re.fullmatch(r"(?:touring|road|gravel|mtb|hiking)(?:/(?:shorter|less-climbing))?", profile)
            for profile in profiles) or len(profiles) != len(set(profiles)):
        raise ValueError("Choose unique routing profile IDs in the recipe")
    return document


def link(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.link(source, destination)
    except OSError:
        shutil.copyfile(source, destination)


def add_map(folder, name):
    manifest = json.loads((folder / "manifest.json").read_bytes())
    manifest["files"][name] = {"bytes": (folder / name).stat().st_size, "sha256": sources.digest(folder / name)}
    (folder / "manifest.json").write_bytes(releases.encoded(manifest))


def prepare(args):
    try:
        from .planner_bake import prepare as bake
    except ImportError:
        from tools.planner_bake import prepare as bake
    return bake(args)
