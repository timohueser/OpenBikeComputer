"""Cache independent grid producers and compose their immutable object references."""

from contextlib import closing
import json
from pathlib import Path
import sqlite3

from . import planner_blocks as blocks, planner_components as components, planner_offline as offline
from . import planner_runtime as runtime, planner_geo as geo, planner_maps as maps, planner_prepare as preparation


def partition_maps(stage, source, kind):
    blocks.map_tiles(source / "maps", stage / "tiles", [kind])
    return {f"maps/tiles/{kind}/{path.name}": path for path in sorted((stage / "tiles" / kind).glob("*.pmtiles"))}


def partition_search(stage, source, lookup, name, bounds, metadata):
    database = stage / name
    blocks.search_shard(source, lookup, database, bounds, metadata)
    return {name: database}


def partition_routing(stage, source, selection):
    selected = stage / "cells.json"
    selected.write_bytes(runtime.encoded(selection))
    maps.run("cargo", "build", "--locked", "--release", "-p", "route-build", "--bin", "route-blocks", cwd=maps.ROOT)
    maps.run(maps.ROOT / "target/release/route-blocks", source / "routing", stage / "routing", "--cells", selected)
    selected.unlink()
    routing = stage / "routing"
    graph = json.loads((routing / "blocks.json").read_bytes())
    catalog = json.loads((routing / "catalog.json").read_bytes())
    files = {"routing/blocks.json": routing / "blocks.json"}
    for archive in graph["archives"]:
        for filename in ("pages.bin", "pages.idx"):
            files[f"routing/packs/{archive}/{filename}"] = routing / "packs" / archive / filename
    for cell in catalog["cells"]:
        files[f"offline/routing-cells/{cell['id']}.json"] = routing / cell["manifest"]
    (stage / "routing-info.json").write_bytes(runtime.encoded({"graph": graph, "catalog": catalog}))
    return files


def partition_routes(stage, catalog, names):
    files = {}
    for name, document in blocks.route_tiles(catalog, names).items():
        filename = f"routes/tiles/{name}.json"
        path = stage / filename
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(runtime.encoded(document))
        files[filename] = path
    return files


def joined_fonts(stage, source, release):
    files = blocks.offline_fonts(source, release, stage)
    aliases = {name: f"offline/fonts/{path.name}" for path, names in files.items() for name in names}
    (stage / "aliases.json").write_bytes(runtime.encoded(aliases))
    return {f"offline/fonts/{path.name}": path for path in files}


def publish(source, routing, output, cache=None):
    identity, release = runtime.release(source, include_sources=False)
    if (output / "release.json").exists(): raise ValueError("Publication already exists")
    output.mkdir(parents=True, exist_ok=True)
    cache = cache or components.Cache(Path.home() / ".cache/obc/planner/sources")
    files, receipts = {}, {}
    objects = output / "objects"
    objects.mkdir(exist_ok=True)
    coverage = release["bounds"]
    def package(name, inputs, producer, build, bounds=coverage, paths=()):
        if producer in (partition_maps, joined_fonts):
            paths = [*paths, maps.ROOT / "uv.lock"]
        dependency_functions = {partition_maps: [blocks.map_tiles], partition_search: [blocks.search_lookup, blocks.search_shard],
                                joined_fonts: [blocks.offline_fonts, blocks.glyph_ranges, blocks.label_texts],
                                partition_routes: [blocks.route_tiles], partition_routing: []}
        spec = components.specification(name, components.implementation([producer, offline.pack_file, offline.item, offline.verify, *dependency_functions.get(producer, [])], paths), inputs,
                                        {"zoom": blocks.ZOOM, "map_zoom": blocks.MAP_ZOOM, "compressed": sorted(offline.COMPRESSED), "chunk": offline.CHUNK}, bounds)
        def produce(stage):
            original = build(stage)
            entries = {name: offline.pack_file(path, stage / "objects") for name, path in original.items()}
            (stage / "index.json").write_bytes(runtime.encoded(entries))
            for path in set(original.values()):
                if path.is_relative_to(stage): path.unlink()
        root, receipt = cache.build(spec, produce)
        entries = json.loads((root / "index.json").read_bytes())
        for entry in entries.values():
            checksum = entry["transport"]["sha256"]
            if not (objects / checksum).exists(): preparation.link(root / "objects" / checksum, objects / checksum)
        files.update(entries)
        receipts[name] = receipt
        return root, entries
    def metadata(name, value):
        path = output / "building" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(runtime.encoded(value))
        files[name] = offline.pack_file(path, objects)
        return name

    selection = [{"id": name, "bounds": bounds} for name, bounds in blocks.cells(coverage)]
    route_catalog = f"routes/{release['region']}.json"
    package("grid-route-catalog", {"source": release["files"][route_catalog]}, partition_routes,
            lambda stage: partition_routes(stage, source / route_catalog, [cell["id"] for cell in selection]))
    paths = components.rust_sources("host/route-build")
    root, _ = package("grid-routing", {"routing": release["routing_package"]}, partition_routing,
                      lambda stage: partition_routing(stage, source, selection), paths=paths)
    info = json.loads((root / "routing-info.json").read_bytes())
    graph = info["graph"]
    if graph["source"] != release["routing_package"]: raise ValueError("Routing blocks use another release")
    routing_cells = {}
    for cell in info["catalog"]["cells"]:
        name = f"offline/routing-cells/{cell['id']}.json"
        routing_cells[cell["id"]] = {**cell, "manifest": name.removeprefix("offline/"), "sha256": files[name]["sha256"]}

    shared_inputs = {name: entry for name, entry in release["files"].items() if name.startswith("maps/assets/")}
    def copy_files(stage, names): return {name: source / name for name in names}
    _, entries = package("grid-assets", shared_inputs, copy_files, lambda stage: copy_files(stage, shared_inputs))
    shared = {name: name for name in entries}
    font_inputs = {"basemap": release["files"]["maps/basemap.pmtiles"], "places": release["files"]["maps/places.pmtiles"], "routing": release["routing_package"],
                   "assets": shared_inputs, "label_keys": sorted(blocks.LABEL_KEYS)}
    root, _ = package("grid-fonts", font_inputs, joined_fonts, lambda stage: joined_fonts(stage, source, release),
                      paths=[maps.ROOT / "tools/planner_mvt.py"])
    shared.update(json.loads((root / "aliases.json").read_bytes()))
    map_blocks = []
    for kind in blocks.map_kinds(source / "maps"):
        _, entries = package(f"grid-map-{kind}", {"source": release["files"][f"maps/{kind}.pmtiles"]}, partition_maps,
                             lambda stage, kind=kind: partition_maps(stage, source, kind))
        if kind in ("basemap", "places", "overlays", "terrain"):
            for name in entries:
                z, x, y = map(int, Path(name).stem.split("-"))
                map_blocks.append({"kind": kind, "tile": [z,x,y], "bounds": geo.tile_bounds(z,x,y), "files": [name]})
        from pmtiles.reader import Reader, MmapSource
        with (source / "maps" / f"{kind}.pmtiles").open("rb") as stream:
            reader = Reader(MmapSource(stream))
            header, info = reader.header(), reader.metadata()
        if kind == "overlays":
            if info.get("routing_package") != graph["source"]: raise ValueError("Overlay tiles use another routing package")
            info["routing_package"] = files["routing/blocks.json"]["sha256"]
        metadata(f"maps/{kind}.json", {**info, "tilejson": "3.0.0", "minzoom": header["min_zoom"], "maxzoom": header["max_zoom"],
            "bounds": release["terrain_bounds"] if kind == "terrain" else coverage})

    search_inputs = {component: source / "search" / component / f"{release['region']}.sqlite" for component in ("pois", "addresses")}
    from .planner_release import search_metadata
    meta = {component: search_metadata(path) for component, path in search_inputs.items()}
    lookups = {}
    for component, database in search_inputs.items():
        spec = components.specification(f"grid-search-lookup-{component}", components.implementation([blocks.search_lookup]),
            {"source": release["files"][database.relative_to(source).as_posix()]}, {}, coverage)
        root, receipt = cache.build(spec, lambda stage, database=database: blocks.search_lookup(database, stage / "lookup.sqlite"))
        lookups[component] = root / "lookup.sqlite"
        receipts[spec["name"]] = receipt
    search_meta = dict(next(iter(meta.values())))
    search_meta["component"] = "all"
    search_meta["counts"] = {}
    for item in meta.values():
        if item["bounds"] != coverage or item["osm_sha256"] != release["osm_sha256"]:
            raise ValueError("Search component coverage or OSM snapshot differs")
        for kind, count in item["counts"].items(): search_meta["counts"][kind] = search_meta["counts"].get(kind, 0) + count
    geographic, grid_cells = [], []
    for cell in selection:
        name, bounds = cell["id"], cell["bounds"]
        cell_files = []
        for component, database in search_inputs.items():
            filename = f"search/tiles/{component}/{name}.sqlite"
            relative = database.relative_to(source).as_posix()
            package(f"grid-search-{component}-{name}", {"source": release["files"][relative]}, partition_search,
                lambda stage, database=database, filename=filename, component=component: partition_search(stage, database, lookups[component], filename, bounds, meta[component]),
                bounds, paths=[maps.ROOT / "apps/planner-search" / path for path in ("storage.py", "index.py", "schema.sql", "indexes.sql", "web/address-terms.json")])
            cell_files.append(filename)
        geographic.append({**cell, "routing": routing_cells.get(name), "files": [*cell_files, f"routes/tiles/{name}.json"]})
        grid_cells.append({**cell, "files": [filename.removeprefix("search/") for filename in cell_files]})
    metadata(f"search/{release['region']}.grid.json", {"format": 3, "metadata": search_meta, "cells": grid_cells})
    remaining = {name: entry for name, entry in release["files"].items() if name.startswith(("search/model/", "device/"))}
    package("grid-model-device", remaining, copy_files, lambda stage: copy_files(stage, remaining))
    catalog = {"format": 3, "source": identity, "release": {key: value for key, value in release.items() if key not in {"files", "source_files"}},
        "routing_source": graph["source"], "map_blocks": map_blocks, "cells": geographic, "shared": shared,
        "files": dict(files), "zoom": blocks.ZOOM, "map_zoom": blocks.MAP_ZOOM}
    metadata("offline/catalog.json", catalog)
    # Publication uploads the source mirror and finalization keeps the mirror that the active release names.
    for name in release["source_files"]:
        if not (output / name).exists(): preparation.link(source / name, output / name)
    document = {**catalog["release"], "routing_package": files["routing/blocks.json"]["sha256"], "source_files": release["source_files"],
        "grid": {"format": 2, "zoom": blocks.ZOOM, "map_zoom": blocks.MAP_ZOOM}, "files": files,
        "sources": {**release["sources"], "grid_components": receipts}}
    offline.atomic_write(output / "release.json", runtime.encoded(document))
    return catalog
