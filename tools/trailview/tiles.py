"""The Web Mercator tiles of the TrailView raster layers, as specs/trailview-tiles.md defines them: the
grid, the pyramid and the archive."""

import io
import math
import os
import tempfile

import numpy as np

from .. import planner_geo as geo, step_pool

TILE = 512
# The number that the readers of the archives check: `trailview_format` in their metadata.
FORMAT = 1
# Each tile of this zoom, and the tiles below it, bake in one worker.
CHUNK_ZOOM = 10
EARTH_M = 2 * math.pi * 6378137.0


def transform(z, x, y):
    """The affine transform of tile z/x/y in EPSG:3857 metres, row 0 north."""
    from affine import Affine

    size = EARTH_M / (1 << z)
    return Affine(size / TILE, 0, -EARTH_M / 2 + x * size, 0, -size / TILE, EARTH_M / 2 - y * size)


def texel_m(z, y):
    """The ground size of a texel of tile row y at zoom z, in metres, at the middle of the tile."""
    lat = (geo.tile_bounds(z, 0, y)[1] + geo.tile_bounds(z, 0, y)[3]) / 2
    return EARTH_M * math.cos(math.radians(lat)) / (1 << z) / TILE


def covered(bounds, z):
    """The tiles x, y of zoom z that the bounds overlap."""
    west, south, east, north = bounds
    x0, y0 = geo.mercator(west, north, z)
    x1, y1 = geo.mercator(east, south, z)
    last = (1 << z) - 1
    return [(x, y) for x in range(int(x0), min(math.ceil(x1), last + 1)) for y in range(int(y0), min(math.ceil(y1), last + 1))]


def inside(bounds, z, x, y):
    """Per texel of tile z/x/y, whether its centre lies in the bounds."""
    west, south, east, north = bounds
    n = (1 << z) * TILE
    cols = (x * TILE + np.arange(TILE) + 0.5) / n * 360 - 180
    rows = np.degrees(np.arctan(np.sinh(np.pi * (1 - 2 * (y * TILE + np.arange(TILE) + 0.5) / n))))
    return ((rows >= south) & (rows <= north))[:, None] & ((cols >= west) & (cols <= east))[None, :]


def mode(children, no_data=None):
    """A tile from its four children {(dx, dy): array or None}: each texel takes the most common of its
    four, the first in reading order on a tie. A missing child is `no_data`, which counts only where all
    four are; without `no_data`, a missing child is 0."""
    mosaic = np.full((2 * TILE, 2 * TILE), 0 if no_data is None else no_data, np.uint8)
    for (dx, dy), child in children.items():
        if child is not None:
            mosaic[dy * TILE:(dy + 1) * TILE, dx * TILE:(dx + 1) * TILE] = child
    four = np.stack([mosaic[0::2, 0::2], mosaic[0::2, 1::2], mosaic[1::2, 0::2], mosaic[1::2, 1::2]])
    counts = (four[:, None] == four[None, :]).sum(axis=1)
    if no_data is not None:
        counts[four == no_data] = 0
    return np.take_along_axis(four, counts.argmax(axis=0)[None], axis=0)[0]


def lossless(texels):
    """One byte per texel as lossless WebP: the decoded red channel holds the byte."""
    from PIL import Image
    from pmtiles.tile import TileType

    out = io.BytesIO()
    Image.fromarray(texels, "L").save(out, "WEBP", lossless=True, exact=True)
    return TileType.WEBP, out.getvalue()


_job = None


def _chunk(x, y):
    """The encoded tiles of the chunk tile x/y and below it, and its texels, in a worker process."""
    job, found = _job, {}

    def below(z, x, y):
        if not intersects(job["bounds"], z, x, y):
            return None
        if z == job["top"]:
            texels = job["bake"](z, x, y)
        else:
            texels = job["parent"]({(dx, dy): below(z + 1, 2 * x + dx, 2 * y + dy) for dx in (0, 1) for dy in (0, 1)})
        keep(found, job, z, x, y, texels)
        return texels

    return found, below(job["chunk"], x, y)


def intersects(bounds, z, x, y):
    w, s, e, n = geo.tile_bounds(z, x, y)
    return w < bounds[2] and e > bounds[0] and s < bounds[3] and n > bounds[1]


def keep(found, job, z, x, y, texels):
    from pmtiles.tile import zxy_to_tileid

    if texels is not None and z >= job["bottom"]:
        for name, data in job["encode"](texels).items():
            found[name, zxy_to_tileid(z, x, y)] = data


def pyramid(bounds, zooms, bake, parent, encode, output, metadata, workers=None):
    """Write an archive `<name>.pmtiles` in the folder `output` for each name that `encode` gives, with
    the tiles over `bounds` at `zooms`, and return the bytes per archive and zoom. `bake(z, x, y)` gives
    the texels of a tile of the last zoom; `parent(children)` those of a coarser tile from its four
    children {(dx, dy): texels or None}; `encode(texels)` the archives' bytes {name: (tile type,
    bytes)}, without an archive where the tile has no data. The tiles below each tile of the first zoom,
    or of CHUNK_ZOOM when the first zoom is 0, bake in a worker of `step_pool.pool`."""
    from concurrent.futures import as_completed
    from pmtiles.tile import Compression, tileid_to_zxy
    from pmtiles.writer import Writer

    global _job
    bottom, top = zooms[0], zooms[-1]
    chunk = bottom or min(CHUNK_ZOOM, top)
    _job = {"bounds": bounds, "top": top, "bottom": bottom, "chunk": chunk, "bake": bake, "parent": parent, "encode": encode}
    found, texels = {}, {}
    try:
        pool = step_pool.pool(workers)
        try:
            futures = {pool.submit(_chunk, x, y): (x, y) for x, y in covered(bounds, chunk) if intersects(bounds, chunk, x, y)}
            for done in as_completed(futures):
                tiles, body = done.result()
                found.update(tiles)
                if bottom < chunk:
                    texels[futures[done]] = body
        finally:
            pool.shutdown(cancel_futures=True)

        def above(z, x, y):
            if z == chunk:
                return texels.pop((x, y), None)
            if not intersects(bounds, z, x, y):
                return None
            body = parent({(dx, dy): above(z + 1, 2 * x + dx, 2 * y + dy) for dx in (0, 1) for dy in (0, 1)})
            keep(found, _job, z, x, y, body)
            return body

        if bottom < chunk:
            above(0, 0, 0)
    finally:
        _job = None
    e7 = lambda value: round(value * 1e7)
    west, south, east, north = bounds
    sizes = {}
    output.mkdir(parents=True, exist_ok=True)
    for name in sorted({name for name, _ in found}):
        ids = sorted(tile_id for archive, tile_id in found if archive == name)
        with tempfile.NamedTemporaryFile(dir=output, prefix=f".{name}-", delete=False) as stream:
            try:
                writer = Writer(stream)
                for tile_id in ids:
                    tile_type, data = found[name, tile_id]
                    z = tileid_to_zxy(tile_id)[0]
                    sizes.setdefault(name, {})[z] = sizes.get(name, {}).get(z, 0) + len(data)
                    writer.write_tile(tile_id, data)
                writer.finalize({
                    "tile_type": tile_type, "tile_compression": Compression.NONE,
                    "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
                    "center_zoom": top, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2),
                }, {**metadata, "archive": name})
                stream.flush()
                os.replace(stream.name, output / f"{name}.pmtiles")
            except BaseException:
                os.unlink(stream.name)
                raise
    return sizes


def km2(bounds):
    """The area of the bounds in square kilometres."""
    west, south, east, north = bounds
    return (6371.0088 ** 2) * math.radians(east - west) * (math.sin(math.radians(north)) - math.sin(math.radians(south)))
