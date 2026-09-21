"""The archive lattice: integer microdegrees, windows, tiles, and the one refusal.

All lattice arithmetic is exact integer microdegrees. Degrees appear only in the GeoTIFF
transform, which is built from these integers and read back into them.
"""

from dataclasses import dataclass
from pathlib import Path

import numpy as np
from rasterio.crs import CRS
from rasterio.transform import Affine


GRID_ORIGIN = -268435456  # −2^28 µdeg, the OBCT grid origin on both axes
STEP = 1 << 6  # µdeg per pixel: about 7.1 m of latitude
TILE = 1 << 16  # µdeg per tile side
TILE_PX = TILE // STEP  # 1024 pixels per tile side
NODATA = -32768
DEGREE = 1_000_000  # µdeg per degree

# The world box, in µdeg. A tile outside it is a bug in the caller, not a coverage edge.
WORLD = (-180 * DEGREE, -90 * DEGREE, 180 * DEGREE, 90 * DEGREE)

WGS84 = CRS.from_epsg(4326)
# A microdegree of tolerance, in microdegrees: 1e-12 degrees, far below a micrometre on the
# ground and far above the rounding error of a transform or a projection. It is what makes a
# coordinate that is mathematically on a lattice line land on the side the half-open square
# says, rather than on the side the last floating-point bit says.
SNAP = 1e-6

def pixel_index(udeg: int) -> int:
    """The lattice index of the pixel that contains a microdegree coordinate."""

    return (udeg - GRID_ORIGIN) // STEP


def udeg_floor(degrees: float) -> int:
    """The microdegree a coordinate is in."""

    return int(np.floor(degrees * DEGREE + SNAP))


def udeg_ceil(degrees: float) -> int:
    """The first microdegree at or after a coordinate."""

    return int(np.ceil(degrees * DEGREE - SNAP))


def tile_index(pixel: int) -> int:
    return pixel // TILE_PX


def tile_id(ti: int, tj: int) -> str:
    return f"{ti:04d}/{tj:04d}"


def tile_path(root: Path, ti: int, tj: int) -> Path:
    return root / "16" / f"{ti:04d}" / f"{tj:04d}.tif"


@dataclass(frozen=True)
class Window:
    """A rectangle of lattice pixels. Rows count north from the grid origin."""

    row0: int
    col0: int
    rows: int
    cols: int

    @property
    def transform(self) -> Affine:
        """The exact lattice transform, north-up: row 0 of the raster is the northernmost."""

        lon0 = (GRID_ORIGIN + self.col0 * STEP) / DEGREE
        lat_top = (GRID_ORIGIN + (self.row0 + self.rows) * STEP) / DEGREE
        return Affine(STEP / DEGREE, 0, lon0, 0, -STEP / DEGREE, lat_top)

    def tiles(self):
        for ti in range(tile_index(self.row0), tile_index(self.row0 + self.rows - 1) + 1):
            for tj in range(tile_index(self.col0), tile_index(self.col0 + self.cols - 1) + 1):
                yield ti, tj


def covering_window(bounds: tuple[float, float, float, float], pad: int = 0) -> Window:
    """The smallest lattice window that covers a degree box, with `pad` pixels of margin.

    An ingest pads by one pixel. The pooling below drops any centre that lands outside the
    window, and a reprojected boundary is an approximation of a curve, so the margin is
    what makes that drop a safety net rather than a way to lose an edge pixel.
    """

    west, south, east, north = bounds
    col0 = pixel_index(udeg_floor(west)) - pad
    col1 = pixel_index(udeg_ceil(east) - 1) + 1 + pad
    row0 = pixel_index(udeg_floor(south)) - pad
    row1 = pixel_index(udeg_ceil(north) - 1) + 1 + pad
    return Window(row0, col0, max(row1 - row0, 1), max(col1 - col0, 1))


def tile_window(ti: int, tj: int) -> Window:
    return Window(ti * TILE_PX, tj * TILE_PX, TILE_PX, TILE_PX)


class Refuse(Exception):
    """A condition the tool refuses to guess about, reported to the caller by name."""


def check_world(bounds: tuple[float, float, float, float], what: str) -> None:
    """Refuse a box outside the world or across the antimeridian.

    The lattice has no wrap. A box whose east edge is west of its west edge crosses the
    antimeridian, and one beyond ±180° or ±90° would land on tiles that cannot exist, so
    both are refused here rather than producing tiles nobody can read.
    """

    west, south, east, north = bounds
    if east <= west or north <= south:
        raise Refuse(
            f"{what}: {west},{south},{east},{north} is empty or crosses the antimeridian; "
            "the archive lattice does not wrap, so split the box at ±180°"
        )
    box = (udeg_floor(west), udeg_floor(south), udeg_ceil(east), udeg_ceil(north))
    if box[0] < WORLD[0] or box[1] < WORLD[1] or box[2] > WORLD[2] or box[3] > WORLD[3]:
        raise Refuse(f"{what}: {west},{south},{east},{north} reaches outside the world box")

def box_tiles(bbox, halo: int = 1) -> list[str]:
    """The tiles a box needs, with a ring of `halo` tiles around it.

    The baker reads a cell's nodes plus a two-node halo, and a node at the edge of a cell
    takes its pixels from the tile next door, so the box alone is one tile short on every
    side.
    """

    window = covering_window(bbox)
    # The core is the tiles the box's own pixels are in — a box on tile lines is exactly its
    # own tiles, not one more — and the ring goes around that.
    rows = range(max(tile_index(window.row0) - halo, 0), tile_index(window.row0 + window.rows - 1) + halo + 1)
    cols = range(max(tile_index(window.col0) - halo, 0), tile_index(window.col0 + window.cols - 1) + halo + 1)
    return [tile_id(ti, tj) for ti in rows for tj in cols]
