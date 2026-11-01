"""The reference archive tool: the `ingest.py` steps of `obc data source prepare terrain`.

The package split is internal: `import ingest` is the tool, and this is its surface.
"""

from .archive import (MEAN, PRIORITY, local_rasters, pixel_digest, pool_tile, priority_rank, tile_digest,
                      write_raster, write_tile)
from .cli import main
from .lattice import (BLOCK_PX, BLOCK_TILES, DEGREE, GRID_ORIGIN, MEAN_NODATA, NODATA, Refuse, SNAP, STEP, TILE,
                      TILE_PX, WGS84, WORLD, Window, block_path, block_window, check_world, covering_window,
                      in_block, pixel_index, tile_bounds, tile_id, tile_index, tile_path, tile_window, udeg_ceil,
                      udeg_floor)
from .pool import (PLAUSIBLE_M, VOID, lattice_indices, open_raster, pool_onto_lattice,
                   read_source, source_envelope, source_xy, to_int16)
from .sources import SOURCES, Credential, Source

__all__ = [
    "BLOCK_PX", "BLOCK_TILES", "Credential", "DEGREE", "GRID_ORIGIN", "MEAN", "MEAN_NODATA", "NODATA", "PLAUSIBLE_M",
    "PRIORITY", "Refuse", "SNAP", "SOURCES", "STEP", "Source", "TILE", "TILE_PX", "VOID", "WGS84", "WORLD", "Window",
    "block_path", "block_window", "check_world", "covering_window", "in_block", "lattice_indices", "local_rasters",
    "main", "open_raster", "pixel_digest", "pixel_index", "pool_onto_lattice", "pool_tile", "priority_rank",
    "read_source", "source_envelope", "source_xy", "tile_bounds", "tile_digest", "tile_id", "tile_index", "tile_path",
    "tile_window", "to_int16", "udeg_ceil", "udeg_floor", "write_raster", "write_tile",
]
