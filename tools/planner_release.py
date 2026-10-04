"""Verify and publish immutable regional planner releases."""

import json
from contextlib import closing
import argparse
import os
from pathlib import Path
import signal
import sqlite3
import tempfile
import subprocess

try:
    from . import planner_maps as maps, planner_sources as sources, r2
    from .planner_runtime import DATA_LAYERS, encoded, release, storage_files, public_metadata
except ImportError:
    import planner_maps as maps, planner_sources as sources, r2
    from planner_runtime import DATA_LAYERS, encoded, release, storage_files, public_metadata


def read_url(url):
    with sources.open_url(url) as response:
        return json.load(response)


def search_metadata(database, full=False):
    with closing(sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)) as db:
        metadata = {k: json.loads(v) for k, v in db.execute("SELECT key,value FROM metadata")}
        if metadata.get("schema") != 3:
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
    routing = json.loads((data / "routing/manifest.json").read_bytes())
    if routing["format"] != 7 or routing["region"] != region:
        raise ValueError("Build a packed routing package for this region")
    with sqlite3.connect(f"{(data / 'routing/overlays.sqlite').as_uri()}?mode=ro", uri=True) as db:
        if db.execute("SELECT package FROM metadata").fetchone() != (sources.digest(data / "routing/manifest.json"),):
            raise ValueError("Overlay index uses another routing package")
        if db.execute("PRAGMA quick_check").fetchone() != ("ok",):
            raise ValueError("Overlay index failed verification")
    if archive_metadata(data / "maps/overlays.pmtiles").get("routing_package") != sources.digest(data / "routing/manifest.json"):
        raise ValueError("Overlay tiles use another routing package")
    maps.DATA = data / "maps"
    map_manifest = maps.check_bundle(full=True)
    database = data / "search" / f"{region}.sqlite"
    metadata = search_metadata(database, full=True)
    osm = map_manifest["osm_sha256"]
    if osm not in routing["source_sha256"] or metadata.get("osm_sha256") != osm:
        raise ValueError("Maps, routing, and search must use one OSM snapshot")
    if map_manifest["bounds"] != routing["bounds"] or metadata.get("bounds") != routing["bounds"]:
        raise ValueError("Planner package bounds differ")
    if not set(routing["source_sha256"][1:]) <= set(map_manifest["terrain_sources"]):
        raise ValueError("Maps and routing must use the same terrain inputs")
    maps.run(maps.ROOT / "target/release/route-server", data / "routing", "--verify")
    device = read_url(device_catalog)
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
    files["maps/manifest.json"] = {"bytes": (data / "maps/manifest.json").stat().st_size, "sha256": sources.digest(data / "maps/manifest.json")}
    for part in ["routing", "search/model", "device"]:
        for path in sorted((data / part).rglob("*")):
            if path.is_file():
                files[path.relative_to(data).as_posix()] = {"bytes": path.stat().st_size, "sha256": sources.digest(path)}
    files[database.relative_to(data).as_posix()] = {"bytes": database.stat().st_size, "sha256": sources.digest(database)}
    document = {"format": 1, "region": region, "bounds": routing["bounds"], "osm_sha256": osm,
                "routing_package": sources.digest(data / "routing/manifest.json"), "profiles": sorted(routing["metrics"]),
                "attribution": routing["attribution"], "terrain_attribution": map_manifest["terrain_attribution"],
                "terrain_bounds": map_manifest["terrain_bounds"], "sources": provenance,
                "device_catalog_source": device_catalog, "files": files,
                "probe": provenance["probe"],
                "source_files": {p.relative_to(data).as_posix(): {"bytes": p.stat().st_size, "sha256": sources.digest(p)}
                                 for p in sorted((data / "sources").iterdir()) if p.is_file()}}
    (data / "release.json").write_bytes(encoded(document))
    return release(data)


def endpoints(identity, document, public, tiles, api):
    prefix = f"{public}/planner/releases/{identity}"
    tile_prefix = f"{tiles}/releases/{identity}"
    assets = tile_prefix if document.get("grid") else prefix
    service = f"{api}/planner-api/releases/{identity}"
    return {"id": identity, "manifest": prefix + "/release.json", "region": document["region"],
            "device_catalog": assets + "/device/catalog.json", "routing": service + "/routing",
            "search": service + "/search", "basemap": tile_prefix + "/basemap.json", "places": tile_prefix + "/places.json",
            "overlays": tile_prefix + "/overlays.json",
            "attribution": document["attribution"],
            "terrain": tile_prefix + "/terrain/{z}/{x}/{y}.webp",
            **{layer: f"{tile_prefix}/{layer}.json" for layer in DATA_LAYERS
               if {f"maps/{layer}.json", f"maps/{layer}.pmtiles"} & document["files"].keys()},
            "glyphs": assets + "/maps/assets/fonts/{fontstack}/{range}.pbf",
            "sprites": assets + "/maps/assets/sprites/v4", "bounds": document["bounds"],
            "terrain_attribution": document["terrain_attribution"]}


def publish(args):
    identity, document = release(args.data_dir)
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
    print("Publication stages data. Run deploy, Deploy site, and finalize to complete the rollout.")
    if not args.apply:
        return
    remote = r2.bucket_remote()
    try: from .planner_cleanup import before_publish
    except ImportError: from planner_cleanup import before_publish
    before_publish(remote, identity)
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


def vite_environment(active):
    values = {"VITE_PLANNER_TILEJSON_URL": active["basemap"], "VITE_PLANNER_PLACES_URL": active["places"],
              "VITE_PLANNER_OVERLAYS_URL": active["overlays"],
              "VITE_PLANNER_DEM_URL": active["terrain"],
              **{f"VITE_PLANNER_{layer.upper()}_URL": active.get(layer, "") for layer in DATA_LAYERS},
              "VITE_PLANNER_ROUTING_URL": active["routing"], "VITE_PLANNER_SEARCH_URL": active["search"],
              "VITE_PLANNER_GLYPHS_URL": active["glyphs"], "VITE_PLANNER_SPRITES_URL": active["sprites"],
              "VITE_PLANNER_MAP_BOUNDS": ",".join(map(str, active["bounds"])),
              "VITE_PLANNER_TERRAIN_ATTRIBUTION": active["terrain_attribution"],
              "VITE_PLANNER_SEARCH_REGIONS": active["region"], "VITE_CATALOG_URL": active["device_catalog"]}
    if any("\n" in value or "\r" in value for value in values.values()):
        raise ValueError("Invalid planner configuration")
    return values


def site_config(catalog_url, destination):
    catalog = read_url(catalog_url)
    if catalog["format"] != 1:
        raise ValueError("Unsupported planner catalogue")
    destination.write_text("".join(f"{key}={value}\n" for key, value in vite_environment(catalog["active"]).items()))


def main(argv=None):
    def stop(_signum, _frame): raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "grid", "publish", "deploy", "rollback", "finalize", "site-config"])
    parser.add_argument("--input-release", type=Path, help="Verified regional bake to partition with grid")
    parser.add_argument("--data-dir", type=Path, default=os.environ.get("OBC_PLANNER_RELEASE", str(Path.home() / ".cache/obc/planner/bw-online")))
    parser.add_argument("--recipe", type=Path, default=maps.ROOT / "tools/planner-regions/baden-wuerttemberg-switzerland.json")
    parser.add_argument("--source-cache", type=Path, default=Path.home() / ".cache/obc/planner/sources")
    parser.add_argument("--osm", type=Path)
    parser.add_argument("--inputs", type=Path, help="Verified source-builder output directory")
    parser.add_argument("--dem-dir", type=Path, default=Path.home() / ".cache/obcm/dem")
    reference = os.environ.get("OBC_REFERENCE_ARCHIVE")
    if not reference and (Path.home() / "obc-reference/index.json").is_file(): reference = str(Path.home() / "obc-reference")
    parser.add_argument("--reference", type=Path, default=reference)
    parser.add_argument("--pmtiles", default=os.environ.get("PMTILES", "pmtiles"))
    parser.add_argument("--device-catalog", default=os.environ.get("OBC_CATALOG_URL", "https://maps.openbikecomputer.com/cell-catalog/catalog.json"))
    parser.add_argument("--public-url", default="https://maps.openbikecomputer.com")
    parser.add_argument("--tiles-url", default="https://tiles.openbikecomputer.com")
    parser.add_argument("--api-url", default="https://releases.openbikecomputer.com")
    parser.add_argument("--site-origin", default="https://openbikecomputer.com")
    parser.add_argument("--host", default=os.environ.get("OBC_PLANNER_HOST"))
    parser.add_argument("--apply", action="store_true", help="Upload or install the previewed release")
    parser.add_argument("--catalog", default=os.environ.get("OBC_PLANNER_CATALOG_URL", "https://maps.openbikecomputer.com/planner/catalog.json"))
    parser.add_argument("--output", type=Path, help="Output environment file for site-config")
    args = parser.parse_args(argv)
    for name in ["data_dir", "input_release", "recipe", "source_cache", "osm", "inputs", "dem_dir", "reference", "output"]:
        value = getattr(args, name)
        if value is not None: setattr(args, name, value.expanduser().resolve())
    try:
        if args.command == "prepare":
            try: from .planner_prepare import prepare
            except ImportError: from planner_prepare import prepare
            prepare(args)
        elif args.command == "grid":
            if not args.input_release: raise ValueError("Provide --input-release for grid publication")
            try:
                maps.run("uv", "run", "--with-requirements", maps.ROOT / "tools/requirements-planner-maps.txt",
                         "python", maps.ROOT / "tools/planner_blocks.py", args.input_release, args.data_dir, cwd=maps.ROOT)
            except subprocess.CalledProcessError as error:
                raise ValueError(f"grid step failed with exit status {error.returncode}") from None
        elif args.command == "publish": publish(args)
        elif args.command in {"deploy", "rollback"}:
            try: from . import planner_deploy
            except ImportError: import planner_deploy
            getattr(planner_deploy, args.command)(args)
        elif args.command == "finalize":
            try: from .planner_cleanup import finalize
            except ImportError: from planner_cleanup import finalize
            finalize(args)
        else:
            if not args.output: raise ValueError("Provide --output for site-config")
            site_config(args.catalog, args.output)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError, r2.Refuse) as error:
        parser.exit(1, f"planner release: {error}\n")
    except KeyboardInterrupt:
        parser.exit(130, "Planner release interrupted. Check the active catalogue before retrying.\n")
