"""A product published as cloud-optimised GeoTIFFs too large to download.

Austria publishes its national 1 m model as 50 km squares, and one square is 7.7 GB. The
file is a COG and the server answers byte ranges, so the window the box needs is read over
HTTP and written out as a small raster. Nothing else is transferred.

The box is split first, so one read is never larger than the pixel cap: a country-scale
box would otherwise ask for the whole square in memory. Each cut is cached under the
square **and** the sub-box it came from, because two boxes in one 50 km square are the
normal case and a cache keyed on the square alone would hand the second one the first
one's heights. A cut that is already there answers before the network is asked anything,
so a second run of the same box makes no request at all.

This is the third publication model in the registry, beside a service that answers a box
and a file small enough to fetch whole.
"""

import urllib.request
from pathlib import Path

import rasterio
from rasterio.errors import RasterioIOError
from rasterio.windows import Window, from_bounds

from ..lattice import Refuse
from .base import HTTP_TIMEOUT, Source, with_retry
from .grid import grid_squares
from .protocols import projected_box, request_boxes

# What keeps a remote open to one range request: GDAL must not list the directory, and it
# must not go looking for sidecar files next to the object.
REMOTE = {
    "GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR",
    "CPL_VSIL_CURL_ALLOWED_EXTENSIONS": ".tif,.tiff",
    "GDAL_HTTP_MAX_RETRY": "3",
    "GDAL_HTTP_RETRY_DELAY": "2",
}


class CogWindows(Source):
    """Remote COGs; a box is the window read out of each one it touches.

    `urls` is the whole adapter: which published objects a box needs. Everything after
    that — whether a square exists, the window, the cache — is shared.
    """

    def __init__(self, *args, base, epsg, **kw):
        super().__init__(*args, **kw)
        self.base, self.epsg = base, epsg

    def urls(self, bbox) -> list[tuple[str, str]]:
        raise NotImplementedError

    def published(self, url: str) -> bool:
        """Whether a square exists at all, from one ranged byte.

        A square outside the country is not published and answers 404, which is a coverage
        edge. Everything else — a 500, a reset, a truncated range — is a fault, and a fault
        must not become a silent hole in the archive, so only the 404 is absence.
        """

        request = urllib.request.Request(url, headers={"Range": "bytes=0-0"})

        def once():
            with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
                response.read(1)
            return True

        return bool(with_retry(once, url, absent=(404,)))

    def fetch(self, bbox, workdir) -> list[Path]:
        workdir.mkdir(parents=True, exist_ok=True)
        boxes = list(request_boxes(bbox, self.resolution_m, self.epsg))
        known: dict[str, bool] = {}
        paths, absent = [], 0
        with rasterio.Env(**REMOTE):
            for i, box in enumerate(boxes, 1):
                (lo_x, lo_y, hi_x, hi_y), _ = projected_box(box, self.epsg)
                for name, url in self.urls(box):
                    cut = workdir / (f"{Path(name).stem}_{round(lo_x)}_{round(lo_y)}"
                                     f"_{round(hi_x)}_{round(hi_y)}.tif")
                    if cut.exists():
                        paths.append(cut)  # the cache answers before the network is asked
                        continue
                    if name not in known:
                        known[name] = self.published(url)
                    if not known[name]:
                        absent += 1
                        continue
                    if self.window(url, (lo_x, lo_y, hi_x, hi_y), cut) is None:
                        absent += 1
                        continue
                    paths.append(cut)
                print(f"  fetch [{i}/{len(boxes)}] {box[0]:.4f},{box[1]:.4f} → "
                      f"{box[2]:.4f},{box[3]:.4f}")
        # A box no published square covers is a coverage edge, not a fault: a per-tile run
        # over a country's box meets one at every corner of the country.
        if absent:
            print(f"  {absent} read(s) found no published square for their part of the box")
        return paths

    def window(self, url: str, box, path: Path):
        """The box's window out of one remote COG, written beside the others."""

        lo_x, lo_y, hi_x, hi_y = box
        try:
            with rasterio.open(url) as src:
                # One pixel of margin, clipped to the square, because the lattice pooling
                # drops a centre that lands outside the window it was given and the box
                # edge does not sit on a pixel line.
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
        except RasterioIOError as error:
            raise Refuse(f"{url}: the square is published but its window cannot be read "
                         f"({error})") from error
        part = path.with_name(path.name + ".part")
        with rasterio.open(part, "w", **profile) as dst:
            dst.write(values, 1)
        part.replace(path)
        return path


class CogGrid(CogWindows):
    """COGs named after the square of a national metre grid they cover."""

    def __init__(self, *args, name, tile_m, **kw):
        super().__init__(*args, **kw)
        self.name, self.tile_m = name, tile_m

    def urls(self, bbox) -> list[tuple[str, str]]:
        return [(name, self.base + name)
                for name in (self.name.format(east=east, north=north)
                             for east, north in grid_squares(bbox, self.epsg, self.tile_m))]
