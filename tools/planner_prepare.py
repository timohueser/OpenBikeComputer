"""Prepare a regional planner release from pinned OSM and terrain inputs."""

import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import sys
import tempfile

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
    maps.bounds(",".join(map(str, document["bounds"])))
    if not re.fullmatch(r"[a-f0-9]{64}", document["osm"]["sha256"]):
        raise ValueError("Pin the OSM SHA-256 in the recipe")
    profiles = document["profiles"]
    if not isinstance(profiles, list) or not profiles or any(
            not isinstance(profile, str) or not re.fullmatch(r"(?:touring|road|gravel|mtb|hiking)(?:/(?:shorter|smoother|less-climbing|quieter))?", profile)
            or profile.endswith("/quieter") and profile != "road/quieter" for profile in profiles) or len(profiles) != len(set(profiles)):
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


def add_layers(data, config):
    """Bake each data layer that the recipe asks for and that the maps folder does not have yet.

    A layer touches only its own archive and its manifest entry. Its recipe fields are the options of
    its bake tool and keys of its archive metadata.
    """
    for layer in releases.DATA_LAYERS:
        if layer not in config or f"{layer}.pmtiles" in json.loads((data / "maps/manifest.json").read_bytes())["files"]:
            continue
        archive = data / "maps" / f"{layer}.pmtiles"
        # A bake with the same recipe fields survives an interrupted run.
        if not archive.exists() or any(releases.archive_metadata(archive).get(key) != value for key, value in config[layer].items()):
            options = [item for key, value in config[layer].items() for item in (f"--{key.replace('_', '-')}", str(value))]
            maps.run("uv", "run", "--with-requirements", maps.ROOT / f"tools/requirements-planner-{layer}.txt", "python", "-m", f"tools.planner_{layer}",
                     config["region"], "--bounds", ",".join(map(str, config["bounds"])), *options, "--output", archive, cwd=maps.ROOT)
        add_map(data / "maps", f"{layer}.pmtiles")


def runtime_routing(routing):
    """Keep compiled routing, overlays and the route catalog; omit their source OSM tables."""
    manifest = json.loads((routing / "manifest.json").read_bytes())
    if not any(table["len"] for table in manifest["osm"].values()):
        return
    with tempfile.TemporaryDirectory(prefix=".runtime-", dir=routing.parent) as directory:
        stage = Path(directory) / "routing"
        maps.run(maps.ROOT / "target/release/route-select", routing, "--output", stage, "--runtime")
        shutil.copyfile(routing / "overlays.sqlite", stage / "overlays.sqlite")
        shutil.copyfile(routing / "route-catalog.json", stage / "route-catalog.json")
        with sqlite3.connect(stage / "overlays.sqlite") as db:
            db.execute("UPDATE metadata SET package=?", (sources.digest(stage / "manifest.json"),))
        maps.run(maps.ROOT / "target/release/route-server", stage, "--verify")
        backup = Path(directory) / "source"
        routing.rename(backup)
        try:
            stage.rename(routing)
        except BaseException:
            backup.rename(routing)
            raise


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
    database = data / "search" / f"{config['region']}.sqlite"
    if database.exists(): releases.search_metadata(database)
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
    for name, item in config.get("auxiliary", {}).items():
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
    # Only a pinned archive enters the bake; an unpinned one would make it irreproducible.
    reference = ["--reference", args.reference] if terrain.get("reference_index_sha256") else []
    if not routing.exists():
        with tempfile.TemporaryDirectory(prefix=".routing-", dir=data) as directory:
            stage = Path(directory) / "routing"
            maps.run(maps.ROOT / "target/release/route-build", osm, "--output", stage, "--region", config["region"],
                     "--country", config["access"], "--bounds", ",".join(map(str, bounds)), "--profiles", ",".join(config["profiles"]),
                     "--dem", args.dem_dir, *reference)
            stage.rename(routing)
    if set(json.loads((routing / "manifest.json").read_bytes())["metrics"]) != set(config["profiles"]):
        raise ValueError("Routing profiles differ from the recipe; choose a fresh package")
    maps.run(maps.ROOT / "target/release/route-server", routing, "--build-overlays")
    maps.run(maps.ROOT / "target/release/route-catalog", routing, "--countries", ",".join(config["countries"]))
    runtime_routing(routing)
    # The map manifest is written last, so it marks a complete maps folder.
    if not (data / "maps/manifest.json").exists():
        with tempfile.TemporaryDirectory(prefix=".maps-", dir=data) as directory:
            stage = Path(directory)
            link(source / "basemap.pmtiles", stage / "basemap.pmtiles")
            maps.places_archive(stage / "basemap.pmtiles", stage / "places.pmtiles")
            maps.overlays_archive(routing / "overlays.sqlite", stage / "overlays.pmtiles")
            maps.run(maps.ROOT / "target/release/planner-dem", "--dem", args.dem_dir, *reference,
                     "--bounds", ",".join(map(str, terrain_bounds)), "--output", stage / "terrain.mbtiles")
            maps.run(args.pmtiles, "convert", stage / "terrain.mbtiles", stage / "terrain.pmtiles")
            maps.compact_archive(stage / "terrain.pmtiles", stage / "compact.pmtiles", bounds, terrain=True)
            (stage / "compact.pmtiles").replace(stage / "terrain.pmtiles")
            with sqlite3.connect(stage / "terrain.mbtiles") as db:
                attribution = db.execute("SELECT value FROM metadata WHERE name='attribution'").fetchone()[0]
                terrain_sources = json.loads(db.execute("SELECT value FROM metadata WHERE name='source_sha256'").fetchone()[0])
            (stage / "terrain.mbtiles").unlink()
            for name, kind, zoom in [("basemap", "mvt", 14), ("terrain", "webp", 12)]:
                maps.verify_archive(args.pmtiles, stage / f"{name}.pmtiles", kind, zoom)
            with sources.open_url(maps.ASSETS_URL, timeout=120) as response:
                maps.install_assets(response.read(), stage / "assets")
            with sources.open_url(maps.SPRITES_LICENSE_URL, timeout=30) as response:
                (stage / "assets/sprites/LICENSE.txt").write_bytes(response.read())
            manifest = {"bounds": bounds, "terrain_bounds": terrain_bounds, "osm_sha256": config["osm"]["sha256"],
                        "terrain_attribution": attribution, "terrain_sources": terrain_sources,
                        "files": {p.relative_to(stage).as_posix():
                        {"bytes": p.stat().st_size, "sha256": sources.digest(p)} for p in sorted(stage.rglob("*")) if p.is_file()}}
            (stage / "manifest.json").write_bytes(releases.encoded(manifest))
            (data / "maps").mkdir(exist_ok=True)
            for path in sorted(stage.iterdir(), key=lambda path: path.name == "manifest.json"):
                path.replace(data / "maps" / path.name)
    elif not (data / "maps/overlays.pmtiles").exists():
        # A routing selection reuses the maps; only the overlay tiles follow the new routing package.
        maps.overlays_archive(routing / "overlays.sqlite", data / "maps/overlays.pmtiles")
        add_map(data / "maps", "overlays.pmtiles")
    add_layers(data, config)
    if not database.exists():
        with tempfile.TemporaryDirectory(prefix=".search-", dir=data) as directory:
            maps.run(maps.ROOT / "apps/planner-search/.venv/bin/python", maps.ROOT / "apps/planner-search/build.py",
                     source / "search.jsonl.zst", "--output", directory, "--region", config["region"],
                     "--bounds", ",".join(map(str, bounds)), "--countries", ",".join(config["countries"]), "--osm-sha256", config["osm"]["sha256"])
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
