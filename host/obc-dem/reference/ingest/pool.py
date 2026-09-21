"""Read a source raster, and pool its pixel centres onto the lattice.

This is where every adapter's rasters become the same thing: float32 heights with one void
convention, placed by the contract's rule, then `int16` metres.
"""

from pathlib import Path

import numpy as np
import rasterio
from pyproj import Transformer
from rasterio.errors import RasterioIOError
from rasterio.transform import Affine
from rasterio.warp import transform_bounds

from .lattice import DEGREE, NODATA, Refuse, SNAP, WGS84, Window, pixel_index


# Absence while pooling. Every real height is far above it, so a lattice pixel that no
# source pixel centre reached still reads as absent after the maximum.
VOID = -1e30
POOL_BLOCK = 256  # source rows per coordinate transform

# The range a real orthometric height is in. A value outside it is a sentinel that the
# source did not declare — −9999 is the common one — and not ground.
PLAUSIBLE_M = (-500.0, 9000.0)

def open_raster(path: Path):
    """`rasterio.open`, with a refusal that names the file and where to delete it from."""

    try:
        return rasterio.open(path)
    except RasterioIOError as exc:
        raise Refuse(f"{path}: cannot be opened ({exc}); delete it from {path.parent} and run again") from exc


def source_xy(transform: Affine, rows, cols):
    """Source-CRS coordinates of pixel centres. Rotation and shear fall out of the affine."""

    return (transform.c + transform.a * cols + transform.b * rows,
            transform.f + transform.d * cols + transform.e * rows)


def source_envelope(transform: Affine, width: int, height: int):
    """The axis-aligned box of a raster's four corners, in its own CRS."""

    cols = np.array([0.0, width, 0.0, width])
    rows = np.array([0.0, 0.0, height, height])
    x, y = source_xy(transform, rows, cols)
    return float(x.min()), float(y.min()), float(x.max()), float(y.max())


def read_source(path: Path):
    """One raster as float32 heights with voids marked, plus its CRS, transform and box.

    A void arrives as a declared sentinel, as a non-finite value, or undeclared. All three
    become `VOID` here, so the pooling below has one convention and no adapter has to know
    what its service sends. Anything outside `PLAUSIBLE_M` is an undeclared sentinel.

    The fraction of the raster that is void comes back with it, because that number is how a
    source that answered with a mostly empty raster is noticed.
    """

    with open_raster(path) as src:
        if src.crs is None:
            raise Refuse(f"{path}: the raster has no CRS, so it cannot be placed")
        if src.scales != (1.0,) or src.offsets != (0.0,):
            raise Refuse(
                f"{path}: the band has scale {src.scales} and offset {src.offsets}, but the tail "
                "reads raw metres; undo them with `gdal_translate -unscale` first"
            )
        values = src.read(1).astype("float32")
        void = ~np.isfinite(values) | (values < PLAUSIBLE_M[0]) | (values > PLAUSIBLE_M[1])
        if src.nodata is not None and np.isfinite(src.nodata):
            void |= values == np.float32(src.nodata)
        values[void] = VOID
        envelope = source_envelope(src.transform, src.width, src.height)
        bounds = transform_bounds(src.crs, WGS84, *envelope)
        return values, src.transform, src.crs, bounds, float(void.mean())


def lattice_indices(transform: Affine, src_crs, rows, cols, to_wgs84):
    """The lattice row and column of each source pixel centre.

    A source that is already in degrees can be placed exactly: twice a pixel centre in
    microdegrees is a whole number when twice the transform's terms are, so a centre that
    sits on a lattice line falls on the side the half-open square says and not on the side
    the last floating-point bit says. That case is common — a national product in EPSG:4326
    on a round step — and it is the one a projection cannot round-trip.

    Every other CRS goes through the projection, and `SNAP` does the same job there.
    """

    if src_crs == WGS84:
        terms = [transform.a * DEGREE, transform.b * DEGREE, 2 * transform.c * DEGREE,
                 transform.d * DEGREE, transform.e * DEGREE, 2 * transform.f * DEGREE]
        if all(abs(term - round(term)) < SNAP for term in terms):
            a, b, twice_c, d, e, twice_f = (round(term) for term in terms)
            odd_col, odd_row = 2 * cols + 1, 2 * rows + 1
            twice_lon = twice_c + a * odd_col + b * odd_row
            twice_lat = twice_f + d * odd_col + e * odd_row
            return pixel_index(twice_lat // 2), pixel_index(twice_lon // 2)
    x, y = source_xy(transform, rows + 0.5, cols + 0.5)
    lon, lat = to_wgs84.transform(x, y)
    return (pixel_index(np.floor(lat * DEGREE + SNAP).astype("int64")),
            pixel_index(np.floor(lon * DEGREE + SNAP).astype("int64")))


def pool_onto_lattice(values, src_transform, src_crs, window: Window):
    """The contract's pooling rule, done directly.

    Every source pixel goes to the one lattice pixel that contains its centre, and a
    lattice pixel keeps the maximum of the pixels that reached it. `Resampling.max` cannot
    do this: it pools by area overlap, so a source pixel that merely touches a lattice
    pixel raises it. Over the Engelberg box that read 51.7 % of the pixels too high and
    none too low, by up to 286 m, it filled 2 659 pixels no source centre reached, and it
    spread a one-pixel tower over two to four archive pixels.

    A centre is a point, so a rotated or sheared source needs no special case, and a source
    coarser than the lattice simply reaches fewer lattice pixels: the pooling never invents
    ground where no centre landed.
    """

    dest = np.full(window.rows * window.cols, VOID, dtype="float32")
    to_wgs84 = Transformer.from_crs(src_crs, WGS84, always_xy=True)
    top = window.row0 + window.rows
    dropped = 0
    for start in range(0, values.shape[0], POOL_BLOCK):
        band = values[start:start + POOL_BLOCK]
        rows, cols = np.nonzero(band > VOID / 2)
        if rows.size == 0:
            continue
        row, col = lattice_indices(src_transform, src_crs, rows + start, cols, to_wgs84)
        inside = ((row >= window.row0) & (row < top)
                  & (col >= window.col0) & (col < window.col0 + window.cols))
        dropped += int(rows.size - inside.sum())
        flat = (top - 1 - row[inside]) * window.cols + (col[inside] - window.col0)
        np.maximum.at(dest, flat, band[rows[inside], cols[inside]])
    return dest.reshape(window.rows, window.cols), dropped


def to_int16(values):
    """Metres as `int16`, rounded half away from zero, with every void at `NODATA`."""

    valid = np.isfinite(values) & (values > VOID / 2)
    finite = np.where(valid, values, 0.0).astype("float64")
    rounded = np.clip(np.sign(finite) * np.floor(np.abs(finite) + 0.5), NODATA + 1, 32767)
    out = np.full(values.shape, NODATA, dtype="int16")
    out[valid] = rounded[valid].astype("int16")
    return out
