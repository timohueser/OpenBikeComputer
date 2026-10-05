"""Prepare independent planner components and compose a coherent regional release."""

from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
import json
import math
from pathlib import Path
import shutil
import sqlite3
import sys
import tempfile
import threading

from . import data_registry, planner_components as components, planner_maps as maps, planner_sources as sources
from . import planner_prepare as preparation, planner_release as releases
from .planner_runtime import open_url


SEARCH = maps.ROOT / "apps/planner-search"
# Producers that bake at the same time. Most leave cores idle in long single-threaded steps; two at a
# time keeps the two largest, the search database and the basemap heap, within 16 GB of memory.
CONCURRENT = 2
# Terrain and routing fetch into one DEM directory.
DEM_FETCH = threading.Lock()


def source_basemap(stage, osm, config, cache, prepared):
    if prepared:
        supplied = json.loads((prepared / "inputs.json").read_bytes())["basemap"]
        if supplied["protomaps_commit"] != sources.PROTOMAPS:
            raise ValueError("Prepared basemap uses another source builder")
        for name, item in config.get("auxiliary", {}).items():
            if supplied["auxiliary"][name]["sha256"] != item["sha256"]:
                raise ValueError(f"Prepared basemap uses another source: {name}")
        preparation.link(prepared / "basemap.pmtiles", stage / "basemap.pmtiles")
        info = json.loads((prepared / "inputs.json").read_bytes())["basemap"]
    else:
        info = sources.basemap(osm(), stage / "basemap.pmtiles", config["bounds"], cache, config.get("auxiliary"))
    (stage / "provenance.json").write_bytes(releases.encoded(info))


def source_search(stage, osm, cache, prepared):
    if prepared:
        if json.loads((prepared / "inputs.json").read_bytes())["search"]["photon_sha256"] != sources.PHOTON_SHA:
            raise ValueError("Prepared search uses another source builder")
        preparation.link(prepared / "search.jsonl.zst", stage / "search.jsonl.zst")
        info = json.loads((prepared / "inputs.json").read_bytes())["search"]
    else:
        info = sources.search_dump(osm(), stage / "search.jsonl.zst", cache)
    (stage / "provenance.json").write_bytes(releases.encoded(info))


def source_records(stage, search):
    maps.run("uv", "run", "--with-requirements", SEARCH / "requirements-build.txt", "python", SEARCH / "split.py",
             search / "search.jsonl.zst", stage, cwd=maps.ROOT)


def build_search(stage, records, config, component):
    maps.run("uv", "run", "--with-requirements", SEARCH / "requirements-build.txt", "python", SEARCH / "build.py",
             records / f"{component}.jsonl.zst", "--component", component, "--output", stage,
             "--region", config["region"], "--bounds", ",".join(map(str, config["bounds"])),
             "--countries", ",".join(config["countries"]), "--osm-sha256", config["osm"]["sha256"],
             "--time-zone", config["time_zone"], cwd=maps.ROOT)
    releases.search_metadata(stage / f"{config['region']}.sqlite", full=True)
    (stage / "regions.geojson").unlink(missing_ok=True)


def build_basemap(stage, source):
    preparation.link(source / "basemap.pmtiles", stage / "basemap.pmtiles")


def build_places(stage, basemap):
    maps.places_archive(basemap / "basemap.pmtiles", stage / "places.pmtiles")


def build_assets(stage):
    with open_url(maps.ASSETS_URL, timeout=120) as response:
        maps.install_assets(response.read(), stage / "assets")
    (stage / "assets/sprites/LICENSE.txt").write_bytes(data_registry.fetch("tangrams-icons")[0].read_bytes())


def terrain_coverage(config):
    bounds = config["bounds"]
    coverage = maps.terrain_bounds(bounds)
    if "sun" in config:
        latitude = config["sun"].get("distance_m", 30000) / 110000
        longitude = latitude / math.cos(math.radians(max(abs(bounds[1]), abs(bounds[3]))))
        coverage = [min(coverage[0], bounds[0] - longitude), min(coverage[1], bounds[1] - latitude),
                    max(coverage[2], bounds[2] + longitude), max(coverage[3], bounds[3] + latitude)]
    return coverage


def terrain_inputs(args, config, bounds=None):
    bounds = bounds or maps.terrain_bounds(config["bounds"])
    with DEM_FETCH:
        maps.run(maps.ROOT / "target/release/obc-dem", "fetch", "--bbox",
                 ",".join(map(str, [bounds[1], bounds[0], bounds[3], bounds[2]])), "--out", args.dem_dir)
    terrain = config["terrain"]
    if terrain.get("reference_index_sha256"):
        if not args.reference or sources.digest(args.reference / "index.json") != terrain["reference_index_sha256"]:
            raise ValueError("Use the curated terrain archive pinned by this recipe with --reference")
    for name, checksum in terrain["dem"].items():
        if Path(name).name != name or sources.digest(args.dem_dir / name) != checksum:
            raise ValueError(f"DEM input does not match the recipe: {name}")
    return ["--reference", args.reference] if terrain.get("reference_index_sha256") else []


def build_terrain(stage, args, config):
    maps.run("cargo", "build", "--locked", "--release", "-p", "obc-dem", cwd=maps.ROOT)
    maps.run("cargo", "build", "--locked", "--release", "-p", "route-build",
             "--bin", "planner-dem", "--features", "route-build/planner-dem", cwd=maps.ROOT)
    bounds = terrain_coverage(config)
    reference = terrain_inputs(args, config, bounds)
    maps.run(maps.ROOT / "target/release/planner-dem", "--dem", args.dem_dir, *reference,
             "--bounds", ",".join(map(str, bounds)), "--output", stage / "terrain.mbtiles")
    maps.run(args.pmtiles, "convert", stage / "terrain.mbtiles", stage / "terrain.pmtiles")
    maps.compact_archive(stage / "terrain.pmtiles", stage / "compact.pmtiles", bounds if "sun" in config else config["bounds"], terrain=True)
    (stage / "compact.pmtiles").replace(stage / "terrain.pmtiles")
    with sqlite3.connect(stage / "terrain.mbtiles") as db:
        info = {"terrain_attribution": db.execute("SELECT value FROM metadata WHERE name='attribution'").fetchone()[0],
                "terrain_sources": json.loads(db.execute("SELECT value FROM metadata WHERE name='source_sha256'").fetchone()[0])}
    (stage / "terrain.mbtiles").unlink()
    (stage / "provenance.json").write_bytes(releases.encoded(info))
    maps.verify_archive(args.pmtiles, stage / "terrain.pmtiles", "webp", 12)


def build_routing(stage, osm, args, config):
    maps.run("cargo", "build", "--locked", "--release", "-p", "route-build", "-p", "obc-dem",
             "--features", "route-build/planner-dem", cwd=maps.ROOT)
    reference = terrain_inputs(args, config)
    routing = stage / "routing"
    maps.run(maps.ROOT / "target/release/route-build", osm(), "--output", routing, "--region", config["region"],
             "--country", config["access"], "--bounds", ",".join(map(str, config["bounds"])),
             "--profiles", ",".join(config["profiles"]), "--countries", ",".join(config["countries"]),
             "--dem", args.dem_dir, *reference)
    for path in routing.iterdir(): path.rename(stage / path.name)
    routing.rmdir()


def build_overlays(stage, routing):
    maps.overlays_archive(routing / "overlays.sqlite", stage / "overlays.pmtiles")


def layer_options(config, name):
    """The sunlight index evaluates local clock times in the region's time zone."""
    return {**config[name], "time_zone": config["time_zone"]} if name == "sun" else config[name]


def build_layer(stage, config, name, terrain=None):
    options = [item for key, value in layer_options(config, name).items() for item in (f"--{key.replace('_', '-')}", str(value))]
    maps.run("uv", "run", "--with-requirements", maps.ROOT / f"tools/requirements-planner-{name}.txt",
             "python", "-m", f"tools.planner_{name}", config["region"], "--bounds", ",".join(map(str, config["bounds"])),
             *options, *(["--terrain", terrain / "terrain.pmtiles"] if name == "sun" else []), "--output", stage / f"{name}.pmtiles", cwd=maps.ROOT)
    if not releases.archive_metadata(stage / f"{name}.pmtiles").get(releases.DATA_LAYERS[name]):
        raise ValueError(f"Incomplete data layer: {name}")


def build_model(stage):
    maps.run(sys.executable, SEARCH / "setup.py", "--data-dir", stage)
    shutil.rmtree(stage / "__pycache__", ignore_errors=True)
    (stage / "query-parser-v2-int8.tar.gz").unlink(missing_ok=True)


def credits(*ids):
    """The registry credits a component writes. data/sources.toml is in no hashed path, so they are its inputs."""
    return {"credits": {id: data_registry.SOURCES[id]["attribution"] for id in ids}}


def layer_credits(config, name):
    """The sources whose credit a data layer writes: the snow layer credits its chosen source only."""
    if name == "snow":
        return ["modis-snow", "hansen-gfc"] if config["snow"]["source"] == "nasa-modis" else ["hr-wsi"]
    return ["era5-land"] if name == "climate" else []


def specifications(config, prepared=None):
    bounds = config["bounds"]
    osm = config["osm"]["sha256"]
    result = {}
    def add(name, function, inputs, options=None, dependencies=(), paths=(), functions=()):
        references = {dependency: components.identity(result[dependency]) for dependency in dependencies}
        result[name] = components.specification(name, components.implementation([function, *functions], paths),
            {**inputs, **references}, options or {}, bounds, dependencies)
    supplied = json.loads((prepared / "inputs.json").read_bytes()) if prepared else None
    if supplied and (supplied["osm_sha256"] != osm or supplied["bounds"] != bounds):
        raise ValueError("Prepared inputs do not match the region recipe")
    # A pin read from data/env/live.toml is in no hashed path, so it is an input of the component it changes.
    add("source-basemap", source_basemap, {"osm": osm, "protomaps": sources.PROTOMAPS, "archive": sources.PROTO_SHA,
                                           "planetiler": maps.PINS["planetiler"]},
        config.get("auxiliary", {}), functions=[sources.basemap, sources.download])
    add("source-search", source_search, {"osm": osm, "nominatim": sources.NOMINATIM, "photon": sources.PHOTON_SHA},
        functions=[sources.search_dump, sources.download])
    # Of the query contract, records.py reads only the data kinds.
    data_kinds = sorted(json.loads((SEARCH / "query/contract.json").read_bytes())["data"])
    add("source-records", source_records, {"data_kinds": data_kinds}, dependencies=["source-search"],
        paths=[SEARCH / "split.py", SEARCH / "records.py", SEARCH / "requirements-build.txt"])
    common = [SEARCH / path for path in ["build.py", "writer.py", "records.py", "storage.py", "index.py", "schema.sql", "indexes.sql", "web/address-terms.json", "requirements-build.txt"]]
    for component in ["pois", "addresses"]:
        add(component, build_search, {"osm": osm, **credits("osm-planet")}, {"region": config["region"], "countries": config["countries"],
            "time_zone": config["time_zone"], "component": component, "schema": 5},
            ["source-records"], [*common, SEARCH / f"{component}.py"])
    add("basemap", build_basemap, {}, dependencies=["source-basemap"])
    map_requirements = maps.ROOT / "tools/requirements-planner-maps.txt"
    add("places", build_places, {}, dependencies=["basemap"], paths=[maps.ROOT / path for path in
        ("tools/planner_maps.py", "tools/planner_mvt.py", "tools/planner_places.py", "tools/requirements-planner-maps.txt", "builder/app/src/lib/planner/poi-kinds.json")])
    rust_manifests = [maps.ROOT / path for path in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "host/route-build/Cargo.toml", "host/route-engine/Cargo.toml", "host/obc-dem/Cargo.toml"]]
    elevation_paths = [*components.rust_sources("host/obc-dem"), maps.ROOT / "host/route-build/src/obc_terrain.rs"]
    elevation = {"sources": config["terrain"], "producer": components.implementation(paths=elevation_paths)}
    terrain_paths = [*rust_manifests, *elevation_paths, map_requirements, maps.ROOT / "tools/planner_map_archive.py",
                     *[maps.ROOT / path for path in ["host/route-build/src/obc_terrain.rs", "host/route-build/src/bin/planner-dem.rs", "host/route-engine/src/model.rs"]]]
    add("terrain", build_terrain, {"elevation": elevation, **credits("copernicus-glo-30")}, {"terrain_bounds": terrain_coverage(config)}, paths=terrain_paths,
        functions=[terrain_inputs, terrain_coverage, maps.compact_archive, maps.verify_archive])
    routing_paths = components.rust_sources("host/route-build")
    add("routing", build_routing, {"osm": osm, "elevation": elevation, **credits("osm-planet", "copernicus-glo-30")}, {"region": config["region"], "access": config["access"], "countries": config["countries"], "profiles": config["profiles"]}, paths=routing_paths,
        functions=[terrain_inputs])
    add("overlays", build_overlays, credits("osm-planet"), dependencies=["routing"], paths=[maps.ROOT / path for path in
        ("tools/planner_maps.py", "tools/planner_mvt.py", "tools/planner_overlays.py", "tools/requirements-planner-maps.txt")])
    add("assets", build_assets, {"assets": maps.ASSETS_URL, "tangrams-icons": maps.PINS["tangrams-icons"]}, paths=[maps.ROOT / "tools/planner_maps.py"])
    add("model", build_model, {}, paths=[SEARCH / "setup.py", SEARCH / "query/artifacts.py", SEARCH / "query/schema.py"])
    for name in releases.DATA_LAYERS:
        if name in config:
            paths = [maps.ROOT / f"tools/planner_{name}.py", maps.ROOT / f"tools/requirements-planner-{name}.txt"]
            if name == "sun": paths.extend(maps.ROOT / path for path in ("tools/planner_sun_horizons.py", "tools/planner_map_archive.py"))
            inputs = {"hansen-gfc": maps.PINS["hansen-gfc"]} if name == "snow" else {}
            inputs.update(credits(*layer_credits(config, name)))
            add(name, build_layer, inputs, layer_options(config, name), dependencies=["terrain"] if name == "sun" else [], paths=paths)
    return result


def execute(args, config, cache, specs, active):
    """Bake each component once its dependencies are built, CONCURRENT at a time, in the order of `specs`."""
    built, download = {}, threading.Lock()
    def osm():
        with download:
            path = args.osm or sources.download(config["osm"]["url"], cache.root / "downloads" / (config["osm"]["sha256"] + ".osm.pbf"), config["osm"]["sha256"])
        if sources.digest(path) != config["osm"]["sha256"]: raise ValueError("OSM input does not match the recipe")
        return path
    callbacks = {
        "source-basemap": lambda stage: source_basemap(stage, osm, config, cache.root / "downloads", args.inputs),
        "source-search": lambda stage: source_search(stage, osm, cache.root / "downloads", args.inputs),
        "source-records": lambda stage: source_records(stage, built["source-search"][0]),
        **{name: (lambda stage, name=name: build_search(stage, built["source-records"][0], config, name)) for name in ["pois", "addresses"]},
        "basemap": lambda stage: build_basemap(stage, built["source-basemap"][0]),
        "places": lambda stage: build_places(stage, built["basemap"][0]),
        "terrain": lambda stage: build_terrain(stage, args, config),
        "routing": lambda stage: build_routing(stage, osm, args, config),
        "overlays": lambda stage: build_overlays(stage, built["routing"][0]),
        "assets": build_assets, "model": build_model,
        **{name: (lambda stage, name=name: build_layer(stage, config, name, built["terrain"][0] if name == "sun" else None)) for name in releases.DATA_LAYERS if name in config}}
    def produce(name):
        if name in active and args.inputs and name in ("source-basemap", "source-search"):
            supplied = json.loads((args.inputs / "inputs.json").read_bytes())
            filename = "basemap.pmtiles" if name == "source-basemap" else "search.jsonl.zst"
            if sources.digest(args.inputs / filename) != supplied["files"][filename]:
                raise ValueError(f"Prepared input checksum mismatch: {filename}")
        return cache.build(specs[name], callbacks[name]) if name in active else cache.read(specs[name])
    pending, running = list(specs), {}
    with ThreadPoolExecutor(CONCURRENT) as pool:
        try:
            while pending or running:
                for name in [name for name in pending if set(specs[name]["dependencies"]) <= set(built)][:CONCURRENT - len(running)]:
                    pending.remove(name)
                    running[pool.submit(produce, name)] = name
                if not running: raise ValueError("Component dependencies form a cycle")
                done, _ = wait(running, return_when=FIRST_COMPLETED)
                for future in done:
                    built[running.pop(future)] = future.result()
        except BaseException:
            # The other producer stops at its next process. A repeated interrupt must not end the wait.
            maps.STOPPING.set()
            while not all(future.done() for future in running):
                try:
                    maps.stop_running()
                    wait(running, timeout=1)
                except KeyboardInterrupt:
                    pass
            maps.STOPPING.clear()
            raise
    return built


def compose(args, config, built, previous=None):
    data = args.data_dir
    data.parent.mkdir(parents=True, exist_ok=True)
    if data.exists() and any(data.iterdir()):
        raise ValueError("Choose a fresh --data-dir; completed releases are immutable")
    with tempfile.TemporaryDirectory(prefix=".compose-", dir=data.parent) as directory:
        stage = Path(directory)
        def install(root, receipt, prefix, names=None):
            for name in names or receipt["files"]:
                preparation.link(root / name, stage / prefix / name)
        for name in ["basemap", "places", "overlays", "terrain", *[name for name in releases.DATA_LAYERS if name in config]]:
            install(*built[name], "maps", [f"{name}.pmtiles"])
        install(*built["assets"], "maps")
        install(*built["routing"], "routing")
        install(*built["model"], "search")
        for name in ["pois", "addresses"]:
            install(*built[name], f"search/{name}")
        terrain = json.loads((built["terrain"][0] / "provenance.json").read_bytes())
        map_manifest = {"bounds": config["bounds"], "terrain_bounds": terrain_coverage(config),
            "osm_sha256": config["osm"]["sha256"], **terrain,
            "files": {path.relative_to(stage / "maps").as_posix(): entry for name, (_, receipt) in built.items()
                      if name in {"assets", "basemap", "places", "terrain", "overlays", *releases.DATA_LAYERS}
                      for filename, entry in receipt["files"].items() if filename != "provenance.json"
                      for path in [stage / "maps" / filename]}}
        (stage / "maps/manifest.json").write_bytes(releases.encoded(map_manifest))
        (stage / "sources").mkdir()
        if previous:
            for path in (previous / "sources").glob("*"):
                if path.is_file(): preparation.link(path, stage / "sources" / path.name)
        osm_name = config["osm"]["sha256"] + ".osm.pbf"
        osm = args.osm or args.source_cache / "downloads" / osm_name
        if osm.exists() and not (stage / "sources" / osm_name).exists():
            preparation.link(osm, stage / "sources" / osm_name)
        for name, item in config.get("auxiliary", {}).items():
            cached = args.source_cache / "downloads/auxiliary" / name
            destination = stage / "sources" / (item["sha256"] + "." + name)
            if cached.exists() and not destination.exists():
                if sources.digest(cached) != item["sha256"]: raise ValueError(f"Cached auxiliary checksum mismatch: {name}")
                preparation.link(cached, destination)
        receipts = {name: receipt for name, (_, receipt) in built.items()}
        provenance = {"recipe_sha256": sources.digest(args.recipe), "osm": config["osm"], "probe": config["probe"],
                      "components": receipts}
        if previous:
            (stage / "device").mkdir()
            shutil.copyfile(previous / "device/catalog.json", stage / "device/catalog.json")
        identity, _ = releases.seal(stage, config["region"], args.device_catalog, provenance)
        if data.exists(): data.rmdir()
        stage.rename(data)
    return identity


def prepare(args):
    config = preparation.recipe(args.recipe)
    database = args.data_dir / "search" / f"{config['region']}.sqlite"
    if database.exists(): releases.search_metadata(database)
    if not getattr(args, "dry_run", False) and args.data_dir.exists() and any(args.data_dir.iterdir()):
        raise ValueError("Choose a fresh --data-dir; completed releases are immutable")
    cache = components.Cache(args.source_cache)
    previous = None
    if args.input_release:
        if getattr(args, "dry_run", False):
            original = json.loads((args.input_release / "release.json").read_bytes())
        else:
            _, original = releases.release(args.input_release, include_sources=False)
        if original["region"] != config["region"] or original["bounds"] != config["bounds"] or original["osm_sha256"] != config["osm"]["sha256"]:
            raise ValueError("Component updates require the same region, coverage and OSM snapshot")
        previous = original["sources"].get("components")
        if not previous: raise ValueError("The input release has no component receipts; prepare a modular release first")
        if previous["pois"]["spec"]["options"]["countries"] != config["countries"]:
            raise ValueError("Component updates require the same country coverage")
    current = specifications(config, args.inputs)
    if previous and args.component and set(previous) - set(current):
        raise ValueError("Removing a component requires preparation of the complete release")
    specs, active = components.plan(current, args.component, previous)
    if previous and any(previous[name]["spec"] != specs[name] for name in specs if name not in active):
        raise ValueError("Unselected component identity differs")
    plan = [{"component": name, "key": components.identity(spec), "status": cache.status(spec)[0],
             "reason": cache.status(spec)[1], "selected": name in active, "dependencies": spec["dependencies"],
             "coverage": spec["coverage"]} for name, spec in specs.items()]
    print(json.dumps({"cache_root": str(cache.root), "components": plan}, indent=2), flush=True)
    if getattr(args, "dry_run", False): return plan
    built = execute(args, config, cache, specs, active)
    identity = compose(args, config, built, args.input_release)
    print(f"Prepared component release {identity}")
