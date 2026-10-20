"""The TrailView canopy tiles of a region: `trailview-canopy.pmtiles`, as `specs/trailview-tiles.md`
defines it.

The `obc data` step `planner/trailview-canopy` reads the zoom 12 tiles of the `forest-reference/<key>`
versions in its snapshots, best first: a cell takes the first reference that knows it."""

import argparse
import io
import os
from pathlib import Path
import re
import tempfile

import numpy as np

from . import step_request

FORMAT = 1
ZOOM, SIZE = 12, 256
NODATA = 65535
# The lowest stems per hectare of the classes 1 to 7. Class 0 has fewer than 50.
STEMS = (50, 100, 150, 200, 300, 400, 600)


def encode(bands):
    """The 16-bit code of each cell of the reference bands height (dm), cover (%), stems per hectare
    and conifer share (%): 0 for a cell that is not a forest cell, else height in 0.5 m steps (bits
    0–6), cover in 10 % steps (bits 7–10), the stems class (bits 11–13) and the leaf type (bits
    14–15: 0 unknown, 1 broadleaved, 2 coniferous, 3 mixed)."""
    height, cover, stems, conifer = bands.astype(np.int64)
    forest = (height != NODATA) & (height >= 30) & (cover != NODATA) & (cover >= 5)
    code = (np.clip(np.rint(height / 5), 1, 127).astype(np.int64)
            | np.clip(np.rint(cover / 10), 1, 10).astype(np.int64) << 7
            | np.searchsorted(STEMS, stems, side="right") << 11
            | np.select([conifer == NODATA, conifer <= 25, conifer >= 75], [0, 1, 2], 3) << 14)
    return np.where(forest, code, 0).astype(np.uint16)


def webp(code):
    """The lossless WebP of a tile of codes: the low byte in red, the high byte in green."""
    from PIL import Image

    rgb = np.zeros((*code.shape, 3), np.uint8)
    rgb[..., 0], rgb[..., 1] = code & 0xFF, code >> 8
    out = io.BytesIO()
    Image.fromarray(rgb, "RGB").save(out, "WEBP", lossless=True, method=6, quality=75, exact=True)
    return out.getvalue()


def merged(paths):
    """The bands of one tile from the reference tiles `paths`, best first."""
    import rasterio

    bands = None
    for path in paths:
        with rasterio.open(path) as tile:
            if tile.count != 4 or tile.shape != (SIZE, SIZE):
                raise ValueError(f"{path}: a reference tile has 4 bands of {SIZE} × {SIZE}")
            values = tile.read()
        if bands is None:
            bands = values
        else:
            unknown = bands[0] == NODATA
            bands[:, unknown] = values[:, unknown]
    return bands


def bake(references, bounds, attribution, output):
    """Write `output` from `references`, the reference tiles of each source ({x, y: path}) best
    first; return the tile count. A tile without a forest cell stays out."""
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import Writer

    tiles = {}
    for reference in references:
        for tile, path in reference.items():
            tiles.setdefault(tile, []).append(path)
    bodies = {}
    for (x, y), paths in tiles.items():
        code = encode(merged(paths))
        if code.any():
            bodies[zxy_to_tileid(ZOOM, x, y)] = webp(code)
    if not bodies:
        raise ValueError("No forest cell is in the region: the canopy layer has no tile")
    west, south, east, north = bounds
    e7 = lambda value: round(value * 1e7)
    with tempfile.NamedTemporaryFile(dir=output.parent, prefix=".canopy-", delete=False) as stream:
        try:
            writer = Writer(stream)
            for tile_id in sorted(bodies):
                writer.write_tile(tile_id, bodies[tile_id])
            writer.finalize({
                "tile_type": TileType.WEBP, "tile_compression": Compression.NONE,
                "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
                "center_zoom": ZOOM, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2),
            }, {"canopy_format": FORMAT, "attribution": attribution})
            stream.flush()
            os.replace(stream.name, output)
        except BaseException:
            os.unlink(stream.name)
            raise
    return len(bodies)


def step():
    request = step_request.read()
    options = request["options"]
    references = []
    for source in options["references"]:
        tiles = {}
        for name, path in step_request.files(request, source).items():
            x, y = map(int, re.fullmatch(r"12/(\d+)/(\d+)\.tif", name).groups())
            tiles[x, y] = path
        references.append(tiles)
    output = Path(request["output"]) / "trailview-canopy.pmtiles"
    count = bake(references, options["bounds"], options["attribution"], output)
    step_request.metrics(request, {"tiles": count, "bytes": output.stat().st_size})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step()


if __name__ == "__main__":
    main()
