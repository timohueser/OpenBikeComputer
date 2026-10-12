"""Verify and seal regional planner releases."""

import json
import shutil
import sqlite3

from . import data_registry, planner_maps as maps
from .planner_grid_search import search_metadata
from .planner_runtime import digest, encoded, read_url, release


def archive_metadata(path):
    """The JSON metadata of a PMTiles v3 archive."""
    import gzip
    import struct
    with path.open("rb") as stream:
        header = stream.read(127)
        offset, length = struct.unpack_from("<QQ", header, 24)
        stream.seek(offset)
        data = stream.read(length)
    return json.loads(gzip.decompress(data) if header[97] == 2 else data)


def seal(data, region, device_catalog, provenance):
    # planner-service --verify below checks the package format.
    routing = json.loads((data / "routing/manifest.json").read_bytes())
    if routing["region"] != region:
        raise ValueError("Build the routing package for this region")
    with sqlite3.connect(f"{(data / 'routing/overlays.sqlite').as_uri()}?mode=ro", uri=True) as db:
        if db.execute("SELECT package FROM metadata").fetchone() != (digest(data / "routing/manifest.json"),):
            raise ValueError("Overlay index uses another routing package")
        if db.execute("PRAGMA quick_check").fetchone() != ("ok",):
            raise ValueError("Overlay index failed verification")
    if archive_metadata(data / "maps/overlays.pmtiles").get("routing_package") != digest(data / "routing/manifest.json"):
        raise ValueError("Overlay tiles use another routing package")
    map_manifest = maps.check_bundle(data / "maps", full=True)
    databases = [data / "search" / component / f"{region}.sqlite" for component in ("pois", "addresses")]
    if not any(path.exists() for path in databases):
        databases = [data / "search" / f"{region}.sqlite"]
    search = [search_metadata(database, full=True) for database in databases]
    metadata = search[0]
    if any(item.get("osm_sha256") != metadata.get("osm_sha256") or item.get("bounds") != metadata.get("bounds") for item in search):
        raise ValueError("Search components must use one OSM snapshot and coverage")
    if not metadata.get("time_zone") or any(item.get("time_zone") != metadata["time_zone"] for item in search):
        raise ValueError("Search components must name one region time zone")
    if len(databases) == 2 and [item.get("component") for item in search] != ["pois", "addresses"]:
        raise ValueError("Search component ownership differs")
    osm = map_manifest["osm_sha256"]
    if osm not in routing["source_sha256"] or metadata.get("osm_sha256") != osm:
        raise ValueError("Maps, routing, and search must use one OSM snapshot")
    if map_manifest["bounds"] != routing["bounds"] or metadata.get("bounds") != routing["bounds"]:
        raise ValueError("Planner package bounds differ")
    if not set(routing["source_sha256"][1:]) <= set(map_manifest["terrain_sources"]):
        raise ValueError("Maps and routing must use the same terrain inputs")
    if "sun.pmtiles" in map_manifest["files"]:
        if archive_metadata(data / "maps/sun.pmtiles").get("terrain_sha256") != map_manifest["files"]["terrain.pmtiles"]["sha256"]:
            raise ValueError("Sunlight index uses another terrain archive")
    # A reused routing component does not build planner-service, so the seal builds the verifier it runs.
    maps.run("cargo", "build", "--locked", "--release", "-p", "planner-service", cwd=maps.ROOT)
    maps.run(maps.ROOT / "target/release/planner-service", data / "routing", "--verify")
    device = json.loads((data / "device/catalog.json").read_bytes()) if (data / "device/catalog.json").exists() else read_url(device_catalog)
    # Catalogue file references remain at their original content-addressed URLs.
    from urllib.parse import urljoin
    def absolute(value):
        if isinstance(value, list): return [absolute(item) for item in value]
        if isinstance(value, dict):
            return {key: urljoin(device_catalog, item) if (key == "url" or key.endswith("_url")) and isinstance(item, str)
                    else absolute(item) for key, item in value.items()}
        return value
    (data / "device").mkdir(exist_ok=True)
    (data / "device/catalog.json").write_bytes(encoded(absolute(device)))
    # The release carries the verified map bundle, which includes each data layer that the recipe asks for.
    files = {f"maps/{name}": item for name, item in map_manifest["files"].items()}
    files["maps/manifest.json"] = {"bytes": (data / "maps/manifest.json").stat().st_size, "sha256": digest(data / "maps/manifest.json")}
    # The routing step bakes the route catalog beside the route package; the release ships it in `routes/`.
    catalog = data / "routes" / f"{region}.json"
    catalog.parent.mkdir(exist_ok=True)
    baked = data / "routing/route-catalog.json"
    if not baked.is_file(): raise ValueError("Missing route catalog; run obc planner prepare")
    shutil.copyfile(baked, catalog)
    for part in ["routing", "routes", "search/model", "device"]:
        for path in sorted((data / part).rglob("*")):
            if path.is_file() and path != baked:
                files[path.relative_to(data).as_posix()] = {"bytes": path.stat().st_size, "sha256": digest(path)}
    for database in databases:
        files[database.relative_to(data).as_posix()] = {"bytes": database.stat().st_size, "sha256": digest(database)}
    document = {"format": 1, "region": region, "bounds": routing["bounds"], "osm_sha256": osm,
                "routing_package": digest(data / "routing/manifest.json"), "profiles": sorted(routing["metrics"]),
                "attribution": data_registry.attribution("osm-planet"),
                "landcover_attribution": data_registry.attribution("daylight-landcover"), "terrain_attribution": map_manifest["terrain_attribution"],
                "terrain_bounds": map_manifest["terrain_bounds"], "sources": provenance,
                "device_catalog_source": device_catalog, "files": files,
                "probe": provenance["probe"],
                "source_files": {p.relative_to(data).as_posix(): {"bytes": p.stat().st_size, "sha256": digest(p)}
                                 for p in sorted((data / "sources").iterdir()) if p.is_file()}}
    (data / "release.json").write_bytes(encoded(document))
    return release(data)
