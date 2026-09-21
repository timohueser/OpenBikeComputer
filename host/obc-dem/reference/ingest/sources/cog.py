"""A product published as cloud-optimised GeoTIFFs too large to download.

Austria publishes its national 1 m model as 50 km squares, and one square is 6.5 GB. The
file is a COG and the server answers byte ranges, so the window the box needs is read over
HTTP and written out as a small raster. Nothing else is transferred.

This is the third publication model in the registry, beside a service that answers a box
and a file small enough to fetch whole.
"""

from pathlib import Path

import rasterio
from rasterio.errors import RasterioIOError
from rasterio.windows import Window, from_bounds

from ..lattice import Refuse
from .base import Source
from .grid import grid_squares
from .protocols import projected_box

# What keeps a remote open to one range request: GDAL must not list the directory, and it
# must not go looking for sidecar files next to the object.
REMOTE = {
    "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR",
    "CPL_VSIL_CURL_ALLOWED_EXTENSIONS": ".tif,.tiff",
    "GDAL_HTTP_MAX_RETRY": "3",
    "GDAL_HTTP_RETRY_DELAY": "2",
}


class CogGrid(Source):
    """Remote COGs on a metre grid; a box is the window read out of each one it touches."""

    def __init__(self, *args, base, name, epsg, tile_m, **kw):
        super().__init__(*args, **kw)
        self.base, self.name, self.epsg, self.tile_m = base, name, epsg, tile_m

    def urls(self, bbox) -> list[tuple[str, str]]:
        return [(name, self.base + name)
                for name in (self.name.format(east=east, north=north)
                             for east, north in grid_squares(bbox, self.epsg, self.tile_m))]

    def fetch(self, bbox, workdir) -> list[Path]:
        workdir.mkdir(parents=True, exist_ok=True)
        wanted = self.urls(bbox)
        (lo_x, lo_y, hi_x, hi_y), _ = projected_box(bbox, self.epsg)
        paths, absent = [], 0
        with rasterio.Env(**REMOTE):
            for i, (name, url) in enumerate(wanted, 1):
                path = workdir / name
                if path.exists():
                    paths.append(path)
                    print(f"  fetch [{i}/{len(wanted)}] {name} (cached)")
                    continue
                try:
                    cut = self.window(url, (lo_x, lo_y, hi_x, hi_y), path)
                except RasterioIOError:
                    absent += 1
                    continue
                if cut is None:
                    absent += 1
                    continue
                print(f"  fetch [{i}/{len(wanted)}] {name}")
                paths.append(cut)
        if absent:
            print(f"  {absent} of {len(wanted)} square(s) hold nothing for this box")
        if not paths:
            raise Refuse(f"{self.key}: none of the {len(wanted)} square(s) the box needs is published")
        return paths

    def window(self, url: str, box, path: Path):
        """The box's window out of one remote COG, written beside the others."""

        lo_x, lo_y, hi_x, hi_y = box
        with rasterio.open(url) as src:
            # One pixel of margin, clipped to the square, because the lattice pooling
            # drops a centre that lands outside the window it was given and the box edge
            # does not sit on a pixel line.
            asked = from_bounds(lo_x, lo_y, hi_x, hi_y, src.transform)
            col0 = max(0, int(asked.col_off) - 1)
            row0 = max(0, int(asked.row_off) - 1)
            col1 = min(src.width, int(asked.col_off + asked.width) + 2)
            row1 = min(src.height, int(asked.row_off + asked.height) + 2)
            if col1 <= col0 or row1 <= row0:
                return None
            window = Window(col0, row0, col1 - col0, row1 - row0)
            values = src.read(1, window=window)
            profile = {
                **src.profile,
                "height": int(window.height), "width": int(window.width),
                "transform": src.window_transform(window),
                "driver": "GTiff", "compress": "deflate", "tiled": True,
                "blockxsize": 256, "blockysize": 256,
            }
        part = path.with_name(path.name + ".part")
        with rasterio.open(part, "w", **profile) as dst:
            dst.write(values, 1)
        part.replace(path)
        return path
