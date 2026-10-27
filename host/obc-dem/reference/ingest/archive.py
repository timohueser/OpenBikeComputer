"""The archive: tiles, the priority fold, and the deliveries on disk."""

import hashlib
from pathlib import Path

import numpy as np
import rasterio
from rasterio.warp import transform_bounds

from .lattice import MEAN_NODATA, NODATA, Refuse, TILE_PX, WGS84, check_world, covering_window, tile_window
from .pool import VOID, open_raster, pool_onto_lattice, read_source, to_decimetres, to_int16
from .sources.base import READABLE, Source, digest_of, placed, unpack


# The mean archive of a version: the same layout under `mean/`, `int32` decimetres.
MEAN = "mean"

# Priority, finest and best-maintained national product first. Where two sources cover the
# same pixel, the one earlier in this list keeps it. Keys with no adapter yet are listed so
# the ranking does not move when an adapter lands.
PRIORITY = (
    "dk", "nl", "it-tn",
    "de-nw", "de-he", "de-ni", "de-by", "de-sn", "de-th", "de-mv", "de-st", "de-rp", "de-bw",
    "fr", "at", "no", "uk", "gb-sct", "se", "sk", "pl", "us", "ca", "nz",
    "ch", "fi", "cz", "it-bz", "es", "au", "it", "is",
)

def priority_rank(key: str) -> int:
    """Lower is better. An unlisted key ranks last, so it never displaces a listed one."""

    return PRIORITY.index(key) if key in PRIORITY else len(PRIORITY)


def write_tile(path: Path, ti: int, tj: int, tile) -> None:
    """An `int16` tile of the maximum, or an `int32` tile of the mean with the horizontal predictor."""

    mean = tile.dtype == np.int32
    path.parent.mkdir(parents=True, exist_ok=True)
    profile = {
        "driver": "GTiff",
        "height": TILE_PX,
        "width": TILE_PX,
        "count": 1,
        "dtype": str(tile.dtype),
        "nodata": MEAN_NODATA if mean else NODATA,
        "crs": WGS84,
        "transform": tile_window(ti, tj).transform,
        "compress": "deflate",
        "tiled": True,
        "blockxsize": 256,
        "blockysize": 256,
        **({"predictor": 2} if mean else {}),
    }
    with rasterio.open(path, "w", **profile) as dst:
        dst.write(tile, 1)
        dst.update_tags(AREA_OR_POINT="Area")


def fold_tiles(existing, incoming, held: set[str], key: str, nodata: int):
    """The tile of a version of `terrain`: the tile that `key` pooled, folded into the tile of the
    version before, whose pixels come from the sources `held`.

    `key` keeps the pixels where it has a height when it ranks better than every other source in
    `held`, also over an older survey of its own; else it only fills the gaps. A pixel does not
    record its source, so a tile that a better and a worse source share keeps the better one's
    pixels where both have one, whichever came first.
    """

    if all(priority_rank(key) < priority_rank(other) for other in held - {key}):
        return np.where(incoming != nodata, incoming, existing)
    return np.where(existing != nodata, existing, incoming)


def pool_tile(rasters: list[Path], source: Source, ti: int, tj: int):
    """One tile from every raster that reaches it: the maximum as `int16` metres and the mean as
    `int32` decimetres, or `None` where no centre lands in the tile.

    A lattice pixel at the edge of two source rasters takes centres from both, and a mean of two
    means is not the mean, so the tile is pooled whole, from all its rasters at once.
    """

    tile = tile_window(ti, tj)
    top = tile.row0 + TILE_PX
    maximum = np.full((TILE_PX, TILE_PX), VOID, dtype="float32")
    total = np.zeros((TILE_PX, TILE_PX), dtype="float64")
    count = np.zeros((TILE_PX, TILE_PX), dtype="int64")
    for path in rasters:
        values, src_transform, src_crs, bounds, _ = read_source(path, source.grid_crs(), source.void)
        check_world(bounds, str(path))
        window = covering_window(bounds, pad=1)
        row0, row1 = max(window.row0, tile.row0), min(window.row0 + window.rows, top)
        col0, col1 = max(window.col0, tile.col0), min(window.col0 + window.cols, tile.col0 + TILE_PX)
        if row1 <= row0 or col1 <= col0:
            continue
        high, sums, counts, _ = pool_onto_lattice(values, src_transform, src_crs, window)
        upper = window.row0 + window.rows
        part = np.s_[upper - row1:upper - row0, col0 - window.col0:col1 - window.col0]
        into = np.s_[top - row1:top - row0, col0 - tile.col0:col1 - tile.col0]
        np.maximum(maximum[into], high[part], out=maximum[into])
        total[into] += sums[part]
        count[into] += counts[part]
    if not count.any():
        return None
    return to_int16(maximum), to_decimetres(total, count)


def opened_as(archive: Path, directory: Path) -> Path:
    """Where one delivered archive is unpacked: its own place, and its own bytes.

    The name is the path inside the delivery plus the digest of the file, so two orders
    that the portal both called `order.zip` are two directories and the second one is
    ingested. Nothing else distinguishes them: a portal re-issues a name freely.
    """

    inside = archive.relative_to(directory).as_posix().replace("/", "_")
    return Path("delivery") / f"{inside}-{digest_of(archive)[:12]}.d"


def local_rasters(source: Source, directory: Path, bbox, into: Path) -> list[Path]:
    """The rasters of a delivery in `directory` that touch the box, in any CRS.

    A portal delivers a zip as often as a bare raster, and an ESRI ASCII grid arrives
    without the CRS the tail needs to place it, so the delivery is opened and placed
    first. Everything the tool writes goes into `into`, the work directory: the delivery
    directory is left exactly as the portal left it.

    A zip is unpacked under its own path and the digest of its bytes, so the work of a run
    is reused and a **re-issued** delivery under the same file name is a different
    directory and reaches the archive. Keying on the name alone is what would silently
    ingest the old tiles again.
    """

    delivered = sorted(path for path in directory.rglob("*")
                       if path.suffix.lower() in READABLE | {".zip"})
    if not delivered:
        raise Refuse(f"{directory}: holds no .tif, .asc, .xyz or .zip, so it is not what "
                     f"`{source.key}` is delivered as; README.md names the format")
    paths = []
    for path in delivered:
        if path.suffix.lower() == ".zip":
            paths.extend(unpack(path, into / opened_as(path, directory), READABLE))
        else:
            paths.append(path)
    keep = []
    for path in paths:
        path = placed(path, source, into)
        with open_raster(path) as src:
            crs = src.crs or source.grid_crs()
            if crs is None:
                raise Refuse(f"{path}: the raster has no CRS, so it cannot be placed")
            west, south, east, north = transform_bounds(crs, WGS84, *src.bounds)
        if not (east < bbox[0] or west > bbox[2] or north < bbox[1] or south > bbox[3]):
            keep.append(path)
    return keep


def tile_digest(path: Path) -> str:
    """The sha256 of a tile's pixels: row-major, little-endian, in the dtype of the tile.

    Not of the file. Two GDAL or zlib builds deflate the same pixels into different bytes,
    and the terrain bakery uses this digest as a skip key, so it has to mean "these heights"
    and not "this compressed stream".
    """

    with open_raster(path) as src:
        pixels = src.read(1)
    little = pixels.astype(pixels.dtype.newbyteorder("<"))
    return hashlib.sha256(np.ascontiguousarray(little).tobytes()).hexdigest()
