"""Slovakia: ÚGKK's DMR 5.0, the national 1 m ALS terrain model, on a keyless INSPIRE WCS 2.0.1.

This is the one row whose service answers ellipsoidal heights, GRS80 heights tens of metres
above sea level. The adapter moves each answer to Baltic 1957 normal heights with GKÚ's quasigeoid,
the grid PROJ publishes, before the answer is written, so the tail sees orthometric metres.
"""

from pathlib import Path

import numpy as np
import rasterio
from rasterio.warp import Resampling, reproject

from ..lattice import Refuse
from .base import http_download
from .protocols import Wcs20Source, projected_box


def minus_geoid(body: bytes, grid: Path) -> bytes:
    """One GeoTIFF answer with the geoid height of each pixel taken off, voids kept."""

    with rasterio.MemoryFile(body) as memory, memory.open() as src:
        heights, profile = src.read(1), src.profile
    geoid = np.zeros_like(heights)
    with rasterio.open(grid) as model:
        reproject(rasterio.band(model, 1), geoid, dst_transform=profile["transform"],
                  dst_crs=profile["crs"], resampling=Resampling.bilinear)
    ground = heights != profile["nodata"]
    heights[ground] -= geoid[ground]
    with rasterio.MemoryFile() as memory:
        with memory.open(**profile) as dst:
            dst.write(heights, 1)
        return memory.read()


class QuasigeoidWcs(Wcs20Source):
    """A WCS of ellipsoidal heights, moved onto a height system by a PROJ geoid grid."""

    def __init__(self, *args, geoid, **kw):
        super().__init__(*args, **kw)
        self.geoid = geoid
        self.grid = None

    def fetch(self, bbox, workdir):
        # Once, before the requests run in parallel.
        self.grid = http_download(self.geoid, workdir / Path(self.geoid).name)
        return super().fetch(bbox, workdir)

    def describe(self):
        # EPSG:3046 is the northing-first UTM 34N, and GeoServer writes the envelope's corners
        # in that order under the labels `E N`, so the parsed corners are swapped back.
        # A server that one day writes them in label order would clip every box to nothing, so an
        # envelope that misses the row's extent is refused.
        lo_n, lo_e, hi_n, hi_e = super().describe()
        (x0, y0, x1, y1), _ = projected_box(self.extent, self.epsg)
        if not (lo_e < x1 and hi_e > x0 and lo_n < y1 and hi_n > y0):
            raise Refuse(f"{self.key} DescribeCoverage: the envelope {(lo_e, lo_n, hi_e, hi_n)} does not "
                         "meet the row's extent; the server no longer states it northing first")
        return lo_e, lo_n, hi_e, hi_n

    def request(self, box):
        body = super().request(box)
        return None if body is None else minus_geoid(body, self.grid)


SK = QuasigeoidWcs(
    "sk", "Slovakia", "DMR 5.0 1 m (ÚGKK SR)", 1.0,
    "CC BY 4.0",
    "Bpv (Baltic 1957 normal heights), converted by GKÚ's quasigeoid",
    url="https://inspirews.skgeodesy.sk/geoserver/el/wcs",
    compress=True,
    coverage="el__EL.GridCoverage",
    epsg=3046,
    axes=("E", "N"),
    # The coverage answers `ScaleAxisUndefined` to `scalesize`, so it is read at its own 1 m.
    scale=False,
    geoid="https://cdn.proj.org/sk_gku_Slovakia_ETRS89h_to_Baltic1957.tif",
)
