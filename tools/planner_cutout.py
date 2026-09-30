"""Prepare a complete offline planner release for a requested bounding box."""

import argparse
from contextlib import closing
import json
from pathlib import Path
import re
import shutil
import sqlite3
import sys
import tempfile
import time

try:
    from . import planner_maps as maps, planner_prepare as preparation, planner_release as releases, planner_sources as sources
except ImportError:
    import planner_maps as maps, planner_prepare as preparation, planner_release as releases, planner_sources as sources


def overlays(source, destination, selection, coverage, package):
    """Keep complete intersecting features, including every route membership."""
    with closing(sqlite3.connect(destination, uri=True)) as db:
        db.execute("ATTACH DATABASE ? AS original", (source.resolve().as_uri() + "?mode=ro",))
        db.execute(f"PRAGMA user_version={db.execute('PRAGMA original.user_version').fetchone()[0]}")
        for name in ("metadata", "geometries", "attributes", "routes", "features", "bounds"):
            schema = db.execute("SELECT sql FROM original.sqlite_schema WHERE type='table' AND name=?", (name,)).fetchone()
            if not schema:
                raise ValueError(f"Missing overlay table: {name}")
            db.execute(schema[0])
        db.execute("CREATE TEMP TABLE selected(id INTEGER PRIMARY KEY)")
        west, south, east, north = selection
        db.execute("INSERT INTO selected SELECT id FROM original.bounds WHERE west<=? AND east>=? AND south<=? AND north>=?",
                   (east, west, north, south))
        for name in ("features", "bounds"):
            db.execute(f"INSERT INTO {name} SELECT * FROM original.{name} WHERE id IN selected")
            if db.execute(f"SELECT * FROM {name} EXCEPT SELECT * FROM original.{name} WHERE id IN selected").fetchone():
                raise ValueError("Overlay extraction changed a feature")
        for name, selection in (("geometries", "SELECT DISTINCT geometry FROM features"),
                                ("attributes", "SELECT DISTINCT attributes FROM features"),
                                ("routes", "SELECT DISTINCT value FROM attributes,json_each(properties,'$.routes')")):
            db.execute(f"INSERT INTO {name} SELECT * FROM original.{name} WHERE id IN ({selection})")
            if db.execute(f"SELECT * FROM {name} EXCEPT SELECT * FROM original.{name} WHERE id IN ({selection})").fetchone():
                raise ValueError("Overlay extraction changed a dependency")
            if db.execute(f"SELECT count(*) FROM ({selection})").fetchone()[0] != db.execute(f"SELECT count(*) FROM {name}").fetchone()[0]:
                raise ValueError("Overlay extraction is missing a dependency")
        count = db.execute("SELECT count(*) FROM features").fetchone()[0]
        if count != db.execute("SELECT count(*) FROM selected").fetchone()[0]:
            raise ValueError("Overlay extraction is incomplete")
        db.execute("INSERT INTO metadata VALUES (?,?)", (package, json.dumps(coverage)))
        db.commit()
        if db.execute("PRAGMA quick_check").fetchone() != ("ok",):
            raise ValueError("Overlay extraction failed verification")
    return {"features": count, "bytes": destination.stat().st_size}


def prepare(source, destination, bounds, region):
    started = time.monotonic()
    if destination.exists():
        raise ValueError("Output already exists; choose a fresh directory")
    if not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", region):
        raise ValueError("Invalid region ID")
    maps.bounds(",".join(map(str, bounds)))
    source_id, original = releases.release(source, include_sources=False)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".cutout-", dir=destination.parent) as temporary:
        stage = Path(temporary)
        built = maps.run(maps.ROOT / "target/release/route-extract", source / "routing",
                         "--output", stage / "routing", "--region", region,
                         "--bounds", ",".join(map(str, bounds)), "--runtime", capture_output=True, text=True)
        routing = json.loads(built.stdout)
        geometry = routing["geometry_bounds"]
        envelope = [min(bounds[0], geometry[0]), min(bounds[1], geometry[1]),
                    max(bounds[2], geometry[2]), max(bounds[3], geometry[3])]
        (stage / "maps").mkdir()
        for name in ("basemap", "terrain"):
            maps.compact_archive(source / "maps" / f"{name}.pmtiles", stage / "maps" / f"{name}.pmtiles",
                                 envelope, terrain=name == "terrain", recompress=False)
        for name in ("maps/assets", "search/model", "device"):
            shutil.copytree(source / name, stage / name, copy_function=lambda a, b: preparation.link(Path(a), Path(b)))
        database = stage / "search" / f"{region}.sqlite"
        maps.run(sys.executable, maps.ROOT / "apps/planner-search/extract.py",
                 source / "search" / f"{original['region']}.sqlite", database,
                 "--bounds", ",".join(map(str, bounds)))
        package = sources.digest(stage / "routing/manifest.json")
        overlay = overlays(source / "routing/overlays.sqlite", stage / "routing/overlays.sqlite", envelope, bounds, package)
        map_manifest = json.loads((source / "maps/manifest.json").read_bytes())
        map_manifest.update(bounds=bounds, terrain_bounds=maps.terrain_bounds(envelope), geometry_bounds=envelope)
        map_manifest["files"] = {
            path.relative_to(stage / "maps").as_posix(): {"bytes": path.stat().st_size, "sha256": sources.digest(path)}
            for path in sorted((stage / "maps").rglob("*")) if path.is_file()}
        (stage / "maps/manifest.json").write_bytes(releases.encoded(map_manifest))
        graph = json.loads((stage / "routing/manifest.json").read_bytes())
        search = releases.search_metadata(database, full=True)
        if graph["bounds"] != bounds or search["bounds"] != bounds or sorted(graph["metrics"]) != original["profiles"]:
            raise ValueError("Extracted planner coverage or profiles differ")
        if graph["source_sha256"][0] != search["osm_sha256"] or search["osm_sha256"] != map_manifest["osm_sha256"]:
            raise ValueError("Extracted planner sources differ")
        document = {**original, "region": region, "bounds": bounds, "routing_package": package, "source_files": {},
                    "terrain_bounds": map_manifest["terrain_bounds"],
                    "sources": {**original["sources"], "extraction": {"source_release": source_id, "geometry_bounds": envelope}}}
        document.pop("probe", None)
        document["files"] = {path.relative_to(stage).as_posix(): {"bytes": path.stat().st_size, "sha256": sources.digest(path)}
                             for path in sorted(stage.rglob("*")) if path.is_file()}
        (stage / "release.json").write_bytes(releases.encoded(document))
        identity, _ = releases.release(stage, include_sources=False)
        maps.run(maps.ROOT / "target/release/route-server", stage / "routing", "--verify")
        stage.rename(destination)
    return {"release": identity, "bounds": bounds, "geometry_bounds": envelope,
            "routing": routing, "overlays": overlay,
            "runtime_bytes": sum(entry["bytes"] for entry in document["files"].values()),
            "prepare_seconds": time.monotonic() - started}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--bbox", type=maps.bounds, required=True)
    parser.add_argument("--region", required=True)
    args = parser.parse_args()
    print(json.dumps(prepare(args.source, args.output, args.bbox, args.region), sort_keys=True))


if __name__ == "__main__":
    main()
