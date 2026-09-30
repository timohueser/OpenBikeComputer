"""Prepare a regional planner release from pinned OSM and terrain inputs."""

import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import sys
import tempfile
from urllib.request import urlopen

try:
    from . import planner_maps as maps, planner_sources as sources, planner_release as releases
except ImportError:
    import planner_maps as maps, planner_sources as sources, planner_release as releases


def recipe(path):
    document = json.loads(path.read_text())
    if document["format"] != 1 or not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", document["region"]):
        raise ValueError("Invalid region recipe")
    if document["country"] != "DE":
        raise ValueError("Routing has German access defaults. Add and verify each country's access policy before extending coverage.")
    maps.bounds(",".join(map(str, document["bounds"])))
    if not re.fullmatch(r"[a-f0-9]{64}", document["osm"]["sha256"]):
        raise ValueError("Pin the OSM SHA-256 in the recipe")
    return document


def link(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.link(source, destination)
    except OSError:
        shutil.copyfile(source, destination)


def inputs(osm, config, cache):
    import hashlib
    bounds = config["bounds"]
    key = hashlib.sha256(releases.encoded({"osm": sources.digest(osm), "bounds": bounds,
                                         "protomaps": sources.PROTOMAPS, "photon": sources.PHOTON_SHA,
                                         "auxiliary": config.get("auxiliary", {})})).hexdigest()
    root = cache / key
    manifest = root / "inputs.json"
    if not manifest.exists():
        with tempfile.TemporaryDirectory(prefix=".sources-", dir=cache) as directory:
            stage = Path(directory)
            basemap = sources.basemap(osm, stage / "basemap.pmtiles", bounds, cache, config.get("auxiliary"))
            search = sources.search_dump(osm, stage / "search.jsonl.zst", cache)
            document = {"osm_sha256": sources.digest(osm), "bounds": bounds, "basemap": basemap, "search": search,
                        "files": {name: sources.digest(stage / name) for name in ["basemap.pmtiles", "search.jsonl.zst"]}}
            (stage / "inputs.json").write_bytes(releases.encoded(document))
            stage.rename(root)
    return root


def prepare(args):
    config = recipe(args.recipe)
    data = args.data_dir
    if (data / "release.json").exists():
        identity, existing = releases.release(data)
        if existing["sources"]["recipe_sha256"] != sources.digest(args.recipe):
            raise ValueError("Existing release uses another recipe; choose a fresh data directory")
        print(f"Verified existing release {identity}. Choose a fresh data directory to build another release.")
        return
    cache = args.source_cache
    cache.mkdir(parents=True, exist_ok=True)
    osm = args.osm or sources.download(config["osm"]["url"], cache / (config["osm"]["sha256"] + ".osm.pbf"), config["osm"]["sha256"])
    if sources.digest(osm) != config["osm"]["sha256"]:
        raise ValueError("OSM input does not match the recipe")
    source = args.inputs or inputs(osm, config, cache)
    provenance = json.loads((source / "inputs.json").read_bytes())
    if provenance["osm_sha256"] != config["osm"]["sha256"] or provenance["bounds"] != config["bounds"]:
        raise ValueError("Prepared inputs do not match the region recipe")
    if provenance["basemap"]["protomaps_commit"] != sources.PROTOMAPS or provenance["search"]["photon_sha256"] != sources.PHOTON_SHA:
        raise ValueError("Prepared inputs use another source builder")
    for name, item in config["auxiliary"].items():
        if provenance["basemap"]["auxiliary"][name]["sha256"] != item["sha256"]:
            raise ValueError(f"Prepared basemap uses another source: {name}")
    if set(provenance["files"]) != {"basemap.pmtiles", "search.jsonl.zst"}:
        raise ValueError("Prepared input manifest is incomplete")
    for name, checksum in provenance["files"].items():
        if not (source / name).resolve().is_relative_to(source.resolve()) or sources.digest(source / name) != checksum:
            raise ValueError(f"Prepared input checksum mismatch: {name}")
    maps.run("cargo", "build", "--release", "-p", "route-build", "-p", "route-server", "-p", "obc-dem",
             "--features", "route-build/planner-dem", cwd=maps.ROOT)
    data.mkdir(parents=True, exist_ok=True)
    maps.run(sys.executable, maps.ROOT / "apps/planner-search/setup.py", "--data-dir", data / "search")
    routing = data / "routing"
    bounds = config["bounds"]
    terrain_bounds = maps.terrain_bounds(bounds)
    maps.run(maps.ROOT / "target/release/obc-dem", "fetch", "--bbox",
             ",".join(map(str, [terrain_bounds[1], terrain_bounds[0], terrain_bounds[3], terrain_bounds[2]])), "--out", args.dem_dir)
    terrain = config["terrain"]
    if terrain.get("reference_index_sha256"):
        if not args.reference or sources.digest(args.reference / "index.json") != terrain["reference_index_sha256"]:
            raise ValueError("Use the curated terrain archive pinned by this recipe with --reference")
    for name, checksum in terrain["dem"].items():
        if Path(name).name != name or sources.digest(args.dem_dir / name) != checksum:
            raise ValueError(f"DEM input does not match the recipe: {name}")
    reference = ["--reference", args.reference] if args.reference else []
    if not routing.exists():
        with tempfile.TemporaryDirectory(prefix=".routing-", dir=data) as directory:
            stage = Path(directory) / "routing"
            maps.run(maps.ROOT / "target/release/route-build", osm, "--output", stage, "--region", config["region"],
                     "--country", config["country"], "--bounds", ",".join(map(str, bounds)), "--profiles", "all",
                     "--dem", args.dem_dir, *reference)
            stage.rename(routing)
    maps.run(maps.ROOT / "target/release/route-server", routing, "--build-overlays")
    if not (data / "maps").exists():
        with tempfile.TemporaryDirectory(prefix=".maps-", dir=data) as directory:
            stage = Path(directory)
            link(source / "basemap.pmtiles", stage / "basemap.pmtiles")
            maps.run(maps.ROOT / "target/release/planner-dem", "--dem", args.dem_dir, *reference,
                     "--bounds", ",".join(map(str, terrain_bounds)), "--output", stage / "terrain.mbtiles")
            maps.run(args.pmtiles, "convert", stage / "terrain.mbtiles", stage / "terrain.pmtiles")
            with sqlite3.connect(stage / "terrain.mbtiles") as db:
                attribution = db.execute("SELECT value FROM metadata WHERE name='attribution'").fetchone()[0]
                terrain_sources = json.loads(db.execute("SELECT value FROM metadata WHERE name='source_sha256'").fetchone()[0])
            (stage / "terrain.mbtiles").unlink()
            for name, kind, zoom in [("basemap", "mvt", 14), ("terrain", "webp", 12)]:
                maps.verify_archive(args.pmtiles, stage / f"{name}.pmtiles", kind, zoom)
            with urlopen(maps.ASSETS_URL, timeout=120) as response:
                maps.install_assets(response.read(), stage / "assets")
            with urlopen(maps.SPRITES_LICENSE_URL, timeout=30) as response:
                (stage / "assets/sprites/LICENSE.txt").write_bytes(response.read())
            manifest = {"bounds": bounds, "terrain_bounds": terrain_bounds, "osm_sha256": config["osm"]["sha256"],
                        "terrain_attribution": attribution, "terrain_sources": terrain_sources,
                        "files": {p.relative_to(stage).as_posix():
                        {"bytes": p.stat().st_size, "sha256": sources.digest(p)} for p in sorted(stage.rglob("*")) if p.is_file()}}
            (stage / "manifest.json").write_bytes(releases.encoded(manifest))
            stage.rename(data / "maps")
    database = data / "search" / (config["region"] + ".sqlite")
    if not database.exists():
        with tempfile.TemporaryDirectory(prefix=".search-", dir=data) as directory:
            maps.run(maps.ROOT / "apps/planner-search/.venv/bin/python", maps.ROOT / "apps/planner-search/build.py",
                     source / "search.jsonl.zst", "--output", directory, "--region", config["region"],
                     "--bounds", ",".join(map(str, bounds)), "--countries", config["country"], "--osm-sha256", config["osm"]["sha256"])
            Path(directory, database.name).rename(database)
    mirror = data / "sources"
    mirror.mkdir(exist_ok=True)
    name = config["osm"]["sha256"] + ".osm.pbf"
    if not (mirror / name).exists(): link(osm, mirror / name)
    for name, item in config.get("auxiliary", {}).items():
        cached = sources.download(item["url"], cache / "auxiliary" / name, item["sha256"])
        destination = mirror / (item["sha256"] + "." + name)
        if not destination.exists(): link(cached, destination)
    provenance.update(recipe_sha256=sources.digest(args.recipe), osm=config["osm"],
                      probe=config["probe"],
                      tools={name: sources.digest(maps.ROOT / "target/release" / name)
                             for name in ["route-build", "planner-dem"]})
    identity, _ = releases.seal(data, config["region"], args.device_catalog, provenance)
    print(f"Prepared release {identity}")
