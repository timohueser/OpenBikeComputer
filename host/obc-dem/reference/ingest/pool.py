"""Read a source raster, and pool its pixel centres onto the lattice.

This is where every adapter's rasters become the same thing: float32 heights with one void
convention, placed by the contract's rule, then `int16` metres for the maximum and `int32`
decimetres for the mean.
"""

from pathlib import Path

import numpy as np
import rasterio
from pyproj import Transformer
from rasterio.errors import RasterioIOError
from rasterio.transform import Affine
from rasterio.warp import transform_bounds

from .lattice import DEGREE, GRID_ORIGIN, MEAN_NODATA, NODATA, STEP, Refuse, SNAP, WGS84, Window, pixel_index


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


def read_source(path: Path, crs=None, sentinel=None):
    """One raster as float32 heights with voids marked, plus its CRS, transform and box.

    `crs` is the row's grid, for a format that carries none of its own, and `sentinel` the row's
    undeclared void inside the plausible range.

    A void arrives as a declared sentinel, as a non-finite value, or undeclared. All three
    become `VOID` here, so the pooling below has one convention and no adapter has to know
    what its service sends. Anything outside `PLAUSIBLE_M` is an undeclared sentinel.

    The fraction of the raster that is void comes back with it, because that number is how a
    source that answered with a mostly empty raster is noticed.
    """

    with open_raster(path) as src:
        src_crs = src.crs or crs
        if src_crs is None:
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
        if sentinel is not None:
            void |= values == np.float32(sentinel)
        values[void] = VOID
        envelope = source_envelope(src.transform, src.width, src.height)
        bounds = transform_bounds(src_crs, WGS84, *envelope)
        return values, src.transform, src_crs, bounds, float(void.mean())


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

    Every source pixel goes to the one lattice pixel that contains its centre. A lattice pixel
    keeps the maximum of the pixels that reached it, and their sum and count for the mean.
    `Resampling.max` cannot do this: it pools by area overlap, so a source pixel that merely
    touches a lattice pixel raises it. Over the Engelberg box that read 51.7 % of the pixels
    too high and none too low, by up to 286 m, it filled 2 659 pixels no source centre
    reached, and it spread a one-pixel tower over two to four archive pixels.

    A centre is a point, so a rotated or sheared source needs no special case, and a source
    coarser than the lattice reaches only some lattice pixels; `fill_gaps` fills the others.
    Returns the maximum (`VOID` where no centre landed), the
    sum and the count, each in the shape of `window`, and the centres outside the window.
    """

    size = window.rows * window.cols
    dest = np.full(size, VOID, dtype="float32")
    total = np.zeros(size, dtype="float64")
    count = np.zeros(size, dtype="int64")
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
        heights = band[rows[inside], cols[inside]]
        np.maximum.at(dest, flat, heights)
        total += np.bincount(flat, weights=heights, minlength=size)
        count += np.bincount(flat, minlength=size)
    shape = (window.rows, window.cols)
    return dest.reshape(shape), total.reshape(shape), count.reshape(shape), dropped


def fill_gaps(maximum, total, count, rasters, window: Window):
    """Fill each lattice pixel of `window` that no source centre reached and that lies between four
    source centres with a height: the maximum and the mean are their bilinear height, from the
    first of `rasters` (values, transform, CRS) that has the four. The arrays are in the shape of
    `window`, north-up, and change in place."""

    rows, cols = np.nonzero(count == 0)
    lat = (GRID_ORIGIN + (window.row0 + window.rows - 1 - rows) * STEP + STEP // 2) / DEGREE
    lon = (GRID_ORIGIN + (window.col0 + cols) * STEP + STEP // 2) / DEGREE
    empty = np.ones(rows.size, dtype=bool)
    for values, transform, src_crs in rasters:
        if not empty.any():
            break
        x, y = Transformer.from_crs(WGS84, src_crs, always_xy=True).transform(lon[empty], lat[empty])
        col, row = ~transform * (np.asarray(x), np.asarray(y))
        col, row = col - 0.5, row - 0.5  # from the corners of the source pixels to their centres
        c0, r0 = np.floor(col).astype("int64"), np.floor(row).astype("int64")
        inside = (r0 >= 0) & (c0 >= 0) & (r0 + 1 < values.shape[0]) & (c0 + 1 < values.shape[1])
        r0, c0 = np.where(inside, r0, 0), np.where(inside, c0, 0)
        corners = [values[r0 + dr, c0 + dc].astype("float64") for dr in (0, 1) for dc in (0, 1)]
        found = inside & np.all([corner > VOID / 2 for corner in corners], axis=0)
        fy, fx = row - r0, col - c0
        height = ((corners[0] * (1 - fx) + corners[1] * fx) * (1 - fy)
                  + (corners[2] * (1 - fx) + corners[3] * fx) * fy)
        at = np.flatnonzero(empty)[found]
        maximum[rows[at], cols[at]] = height[found]
        total[rows[at], cols[at]] = height[found]
        count[rows[at], cols[at]] = 1
        empty[at] = False


def to_int16(values):
    """Metres as `int16`, rounded half away from zero, with every void at `NODATA`."""

    valid = np.isfinite(values) & (values > VOID / 2)
    finite = np.where(valid, values, 0.0).astype("float64")
    rounded = np.clip(np.sign(finite) * np.floor(np.abs(finite) + 0.5), NODATA + 1, 32767)
    out = np.full(values.shape, NODATA, dtype="int16")
    out[valid] = rounded[valid].astype("int16")
    return out


def to_decimetres(total, count):
    """The mean as `int32` decimetres, rounded half away from zero, with `MEAN_NODATA` where
    no centre landed. A decimetre is the step of the terrain that reads it."""

    valid = count > 0
    mean = np.where(valid, total, 0.0) / np.maximum(count, 1) * 10.0
    out = np.full(total.shape, MEAN_NODATA, dtype="int32")
    out[valid] = (np.sign(mean) * np.floor(np.abs(mean) + 0.5))[valid].astype("int32")
    return out
