"""Verify and publish immutable regional planner releases."""

from contextlib import closing
import json
from pathlib import Path
import shutil
import sqlite3
import tempfile

from . import planner_cleanup as cleanup, planner_maps as maps, r2
from .planner_runtime import DATA_LAYERS, digest, encoded, public_metadata, read_url, release, storage_files


def search_metadata(database, full=False):
    with closing(sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)) as db:
        metadata = {k: json.loads(v) for k, v in db.execute("SELECT key,value FROM metadata")}
        if metadata.get("schema") != 5:
            raise ValueError(f"Rebuild search package {database}: incompatible schema.")
        try:
            db.execute('SELECT rowid FROM addresses INDEXED BY address_cells LIMIT 0')
        except sqlite3.Error as error:
            raise ValueError(f"Rebuild search package {database}: missing address index.") from error
        if full and db.execute("PRAGMA quick_check").fetchone() != ("ok",):
            raise ValueError("Search database failed verification")
    return metadata


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
    # route-server --verify below checks the package format.
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
    # A reused routing component does not build route-server, so the seal builds the verifier it runs.
    maps.run("cargo", "build", "--locked", "--release", "-p", "route-server", cwd=maps.ROOT)
    maps.run(maps.ROOT / "target/release/route-server", data / "routing", "--verify")
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
                "attribution": routing["attribution"], "terrain_attribution": map_manifest["terrain_attribution"],
                "terrain_bounds": map_manifest["terrain_bounds"], "sources": provenance,
                "device_catalog_source": device_catalog, "files": files,
                "probe": provenance["probe"],
                "source_files": {p.relative_to(data).as_posix(): {"bytes": p.stat().st_size, "sha256": digest(p)}
                                 for p in sorted((data / "sources").iterdir()) if p.is_file()}}
    (data / "release.json").write_bytes(encoded(document))
    return release(data)


def endpoints(identity, document, name, public, tiles, api):
    """The catalogue entry of a grid release, which the web planner reads as its config at page load."""
    tile_prefix = f"{tiles}/releases/{identity}"
    service = f"{api}/planner-api/releases/{identity}"
    return {"id": identity, "manifest": f"{public}/planner/releases/{identity}/release.json", "region": document["region"],
            "name": name, "device_catalog": tile_prefix + "/device/catalog.json", "routing": service + "/routing",
            "search": service + "/search", "basemap": tile_prefix + "/basemap.json", "places": tile_prefix + "/places.json",
            "overlays": tile_prefix + "/overlays.json",
            "attribution": document["attribution"],
            "terrain": tile_prefix + "/terrain/{z}/{x}/{y}.webp",
            "layers": {layer: f"{tile_prefix}/{layer}.json" for layer in DATA_LAYERS if f"maps/{layer}.json" in document["files"]},
            "glyphs": tile_prefix + "/maps/assets/fonts/{fontstack}/{range}.pbf",
            "sprites": tile_prefix + "/maps/assets/sprites/v4", "bounds": document["bounds"],
            "terrain_attribution": document["terrain_attribution"],
            "routes": tile_prefix + "/routes/tiles/{cell}.json"}


def grid_release(data):
    """The identity and verified manifest of a grid release; only grid releases go online."""
    identity, document = release(data)
    if not document.get("grid"):
        raise ValueError("Online services serve grid releases only. Run obc planner grid first.")
    return identity, document


def publish(args):
    identity, document = grid_release(args.data_dir)
    files = storage_files(document)
    for name, data in public_metadata(document).items():
        path = args.data_dir / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        files[name] = {"bytes": len(data)}
    source_files = document.get("source_files", {})
    count = len(files) + len(source_files) + 1
    size = sum(item["bytes"] for item in {**files, **source_files}.values())
    print(f"Release {identity}\n{document['region']}: {count} objects, {size / 1e9:.2f} GB")
    print(f"R2 storage ceiling for this release: ${size / 1e9 * .015:.2f}/month before the free allowance.")
    print(f"Initial object writes: approximately ${count / 1e6 * 4.5:.3f} before the free allowance.")
    print("Publication stages data. Run deploy and finalize to complete the rollout.")
    if not args.apply:
        return
    remote = r2.bucket_remote()
    cleanup.before_publish(remote, identity)
    prefix = f"planner/releases/{identity}"
    with tempfile.TemporaryDirectory(prefix="planner-upload-") as directory:
        if source_files:
            source_listing = Path(directory) / "sources.txt"
            source_listing.write_text("\n".join(Path(name).name for name in source_files) + "\n")
            options = ["--files-from", str(source_listing)]
            r2.run_rclone(["copy", str(args.data_dir / "sources"), f"{remote.path}/planner/sources", *options,
                           "--immutable", "--checksum", "--transfers", "2", "--header-upload", "Cache-Control: public,max-age=31536000,immutable"], remote.env)
            r2.run_rclone(["check", str(args.data_dir / "sources"), f"{remote.path}/planner/sources", *options,
                           "--one-way", "--download", "--checkers", "2"], remote.env)
        listing = Path(directory) / "files.txt"
        listing.write_text("\n".join(files) + "\n")
        r2.run_rclone(["copy", str(args.data_dir), f"{remote.path}/{prefix}", "--files-from", str(listing),
                       "--immutable", "--checksum", "--transfers", "4", "--s3-upload-concurrency", "2",
                       "--header-upload", "Cache-Control: public,max-age=31536000,immutable"], remote.env)
        rows = json.loads(r2.run_rclone(["lsjson", f"{remote.path}/{prefix}", "--recursive", "--files-only", "--no-modtime", "--no-mimetype"], remote.env, capture=True))
        sizes = {row["Path"]: row["Size"] for row in rows}
        if any(sizes.get(name) != item["bytes"] for name, item in files.items()):
            raise ValueError("Remote release is incomplete; catalogue is unchanged")
        r2.run_rclone(["check", str(args.data_dir), f"{remote.path}/{prefix}", "--files-from", str(listing),
                       "--one-way", "--download", "--checkers", "2"], remote.env)
        r2.run_rclone(["copyto", str(args.data_dir / "release.json"), f"{remote.path}/{prefix}/release.json",
                       "--immutable", "--checksum", "--header-upload", "Content-Type: application/json",
                       "--header-upload", "Cache-Control: public,max-age=31536000,immutable"], remote.env)
    print(f"Published {args.public_url}/planner/releases/{identity}/release.json")
