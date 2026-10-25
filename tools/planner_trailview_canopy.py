"""The TrailView canopy tiles of a region: `trailview-canopy.pmtiles`, as `specs/trailview-tiles.md`
defines it.

The `obc data` step `planner/trailview-canopy` reads the zoom 12 tiles of the prepared source
`forest` in its snapshot."""

import argparse
import io
from pathlib import Path
import re

import numpy as np

from . import step_request
from .trailview import tiles

FORMAT = 1
ZOOM, SIZE = 12, 256
NODATA = 65535
# The code of a cell that no reference knows: its cover bits, 15, are no cover of a forest cell.
UNKNOWN = 0xFFFF
# The lowest stems per hectare of the classes 1 to 7. Class 0 has fewer than 50.
STEMS = (50, 100, 150, 200, 300, 400, 600)


def encode(bands):
    """The 16-bit code of each cell of the reference bands height (dm), cover (%), stems per hectare
    and conifer share (%): `UNKNOWN` for a cell that no reference knows, 0 for one that is not a
    forest cell, else height in 0.5 m steps (bits 0–6), cover in 10 % steps (bits 7–10), the stems
    class (bits 11–13) and the leaf type (bits 14–15: 0 unknown, 1 broadleaved, 2 coniferous, 3
    mixed)."""
    height, cover, stems, conifer = bands.astype(np.int64)
    forest = (height != NODATA) & (height >= 30) & (cover != NODATA) & (cover >= 5)
    code = (np.clip(np.rint(height / 5), 1, 127).astype(np.int64)
            | np.clip(np.rint(cover / 10), 1, 10).astype(np.int64) << 7
            | np.searchsorted(STEMS, stems, side="right") << 11
            | np.select([conifer == NODATA, conifer <= 25, conifer >= 75], [0, 1, 2], 3) << 14)
    return np.select([height == NODATA, forest], [UNKNOWN, code], 0).astype(np.uint16)


def webp(code):
    """The lossless WebP of a tile of codes: the low byte in red, the high byte in green."""
    from PIL import Image

    rgb = np.zeros((*code.shape, 3), np.uint8)
    rgb[..., 0], rgb[..., 1] = code & 0xFF, code >> 8
    out = io.BytesIO()
    Image.fromarray(rgb, "RGB").save(out, "WEBP", lossless=True, method=6, quality=75, exact=True)
    return out.getvalue()


def bands(path):
    """The bands of the forest tile `path`."""
    import rasterio

    with rasterio.open(path) as tile:
        if tile.count != 4 or tile.shape != (SIZE, SIZE):
            raise ValueError(f"{path}: a forest tile has 4 bands of {SIZE} × {SIZE}")
        return tile.read()


def bake(found, bounds, attribution, output):
    """Write `trailview-canopy.pmtiles` in the folder `output` from `found`, the forest tiles
    ({x, y: path}), and return its tile bytes. A tile that the forest lacks stays out."""
    from pmtiles.tile import TileType

    if not found:
        raise ValueError("The forest has no tile in the region: the canopy layer has no tile")
    sizes = tiles.pyramid(
        bounds, [ZOOM], lambda z, x, y: encode(bands(found[x, y])) if (x, y) in found else None, None,
        lambda code: {"trailview-canopy": (TileType.WEBP, webp(code))}, output,
        {"canopy_format": FORMAT, "attribution": attribution})
    return sizes["trailview-canopy"][ZOOM]


def step():
    request = step_request.read()
    options = request["options"]
    found = {}
    for name, path in step_request.files(request, "forest").items():
        x, y = map(int, re.fullmatch(r"12/(\d+)/(\d+)\.tif", name).groups())
        found[x, y] = path
    size = bake(found, options["bounds"], options["attribution"], Path(request["output"]))
    step_request.metrics(request, {"bytes": size, "bytes_per_km2": round(size / tiles.km2(options["bounds"]), 1)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step()


if __name__ == "__main__":
    main()
