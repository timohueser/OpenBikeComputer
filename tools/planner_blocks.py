"""Build the canonical planner grid from one verified regional release."""
import argparse
from contextlib import closing
import json
import sqlite3
from pathlib import Path
import subprocess

from . import planner_runtime
from .planner_grid_maps import MAP_ZOOM, map_kinds, map_tiles
from .planner_grid import ZOOM, tile, intersects, cells
from .planner_grid_search import search_lookup, search_shard
from .planner_grid_fonts import LABEL_KEYS, label_texts, glyph_ranges, offline_fonts


def stage_sqlite(path, build):
    if path.exists(): return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".partial")
    temporary.unlink(missing_ok=True)
    try:
        build(temporary)
        with closing(sqlite3.connect(temporary)) as db:
            if db.execute("PRAGMA quick_check").fetchone() != ("ok",):
                raise ValueError("Grid database failed verification")
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def route_tiles(catalog, names):
    """Split a region route catalog into the documents of the grid cells `names`."""
    document = json.loads(catalog.read_bytes())
    if document["format"] != 1: raise ValueError("Unsupported route catalog")
    tiles = {name: [] for name in names}
    for record in sorted(document["routes"], key=lambda record: record["id"]):
        # A cell outside the grid has no file.
        for cell in record["cells"]:
            if cell in tiles: tiles[cell].append(record)
    return {name: {"format": 1, "routes": routes} for name, routes in tiles.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--source-cache", type=Path, default=Path.home() / ".cache/obc/planner/sources")
    args = parser.parse_args()
    try:
        prepare(args.source, args.output, args.source_cache)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"planner grid: {error}\n")


def prepare(source, output, cache_root=None):
    """Build the canonical grid from one verified regional bake."""
    from .planner_components import Cache
    from .planner_grid_components import publish
    publish(source, None, output, Cache(cache_root) if cache_root else None)
    identity, _ = planner_runtime.release(output, include_sources=False)
    print(f"Prepared grid release {identity}")


if __name__ == "__main__": main()
