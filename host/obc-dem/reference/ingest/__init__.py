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

from . import publish
from .archive import (PRIORITY, best_contributor, contributors, cut_tile, ingest_raster,
                      load_manifests, local_rasters, manifest_dir, merge_tiles, priority_rank,
                      read_index, rebuild_index, source_facts, tile_digest, tile_problems,
                      write_manifest, write_tile)
from .cli import main, parse_bbox
from .lattice import (DEGREE, GRID_ORIGIN, NODATA, Refuse, SNAP, STEP, TILE, TILE_PX, WGS84,
                      WORLD, Window, box_tiles, check_world, covering_window, pixel_index,
                      tile_id, tile_index, tile_path, tile_window, udeg_ceil, udeg_floor)
from .pool import (PLAUSIBLE_M, VOID, lattice_indices, open_raster, pool_onto_lattice,
                   read_source, source_envelope, source_xy, to_int16)
from .publish import (ARCHIVE_PREFIX, Remote, merge_index, mirror_plan, publish_plan,
                      r2_remote, run_rclone)
from .sources import SOURCES, Source
