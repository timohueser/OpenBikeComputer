"""Turn a national DTM into reference archive tiles, and move the archive to and from R2.

The archive is one format: max-pooled bare-earth height on the OBCT lattice at 2^6 µdeg,
`int16` metres, one GeoTIFF per 2^16 µdeg tile. Every country branch is an adapter in
`ingest/sources/`, and the tool runs once per source release. The baker knows the tiles
only, and `README.md` beside this file holds the contract both sides read.

    python3 ingest.py ingest ch --bbox 8.30,46.75,8.60,46.95 --archive ref/
    python3 ingest.py check --archive ref/
    python3 ingest.py publish --archive ref/

A source adapter has one job: give the shared tail rasters that cover the box, in any CRS
and any dtype. The tail pools each one onto the lattice by the contract's rule — a lattice
pixel keeps the maximum of the source pixels whose centres lie in it — so a rock tower one
source pixel wide survives the 7 m step, and writes whole tiles.
"""

from . import publish, wizard
from .archive import (PRIORITY, best_contributor, contributors, cut_tile, ingest_raster,
                      load_manifests, local_rasters, manifest_dir, merge_tiles, priority_rank,
                      read_index, rebuild_index, source_facts, tile_digest, tile_problems,
                      write_manifest, write_tile)
from .cli import command_wizard, main, parse_bbox
from .lattice import (DEGREE, GRID_ORIGIN, NODATA, Refuse, SNAP, STEP, TILE, TILE_PX, WGS84,
                      WORLD, Window, box_tiles, check_world, covering_window, pixel_index,
                      tile_id, tile_index, tile_path, tile_window, udeg_ceil, udeg_floor)
from .pool import (PLAUSIBLE_M, VOID, lattice_indices, open_raster, pool_onto_lattice,
                   read_source, source_envelope, source_xy, to_int16)
from .publish import (ARCHIVE_PREFIX, Remote, merge_index, mirror_plan, publish_plan,
                      r2_remote, run_rclone)
from .sources import SOURCES, Credential, Source

# The package split is internal: `import ingest` is the tool, and this is its surface.
__all__ = [
    "ARCHIVE_PREFIX", "Credential", "DEGREE", "GRID_ORIGIN", "NODATA", "PLAUSIBLE_M",
    "PRIORITY", "Refuse",
    "Remote", "SNAP", "SOURCES", "STEP", "Source", "TILE", "TILE_PX", "VOID", "WGS84", "WORLD",
    "Window", "best_contributor", "box_tiles", "check_world", "command_wizard", "contributors",
    "covering_window", "cut_tile", "ingest_raster", "lattice_indices", "load_manifests",
    "local_rasters", "main", "manifest_dir", "merge_index", "merge_tiles", "mirror_plan",
    "open_raster", "parse_bbox", "pixel_index", "pool_onto_lattice", "priority_rank",
    "publish", "publish_plan", "r2_remote", "read_index", "read_source", "rebuild_index",
    "run_rclone", "source_envelope", "source_facts", "source_xy", "tile_digest", "tile_id",
    "tile_index", "tile_path", "tile_problems", "tile_window", "to_int16", "udeg_ceil",
    "udeg_floor", "wizard", "write_manifest", "write_tile"
]
