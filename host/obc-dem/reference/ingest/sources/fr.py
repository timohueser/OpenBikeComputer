"""France: RGE ALTI, served by IGN as raw float32 over WMS.

IGN publishes the high-resolution elevation grid through `image/x-bil;bits=32`, which is
the band and nothing else: no header, no CRS, no nodata. So this is the one adapter that
assembles its own GeoTIFF, from the size and the box it asked for.
"""

import urllib.parse

import numpy as np
import rasterio
from rasterio.io import MemoryFile

from ..lattice import Refuse
from .protocols import TiledService, output_size
from .base import http_get

# The sentinel IGN puts where RGE ALTI has no ground. The tail drops anything below
# −500 m anyway; this is here so the assembled raster declares a nodata of its own.
IGN_VOID = -99999.0


class BilWmsSource(TiledService):
    """A WMS that answers with the band alone, so the GeoTIFF is built around it."""

    def __init__(self, *args, url, layer, **kw):
        super().__init__(*args, **kw)
        self.service, self.layer = url, layer

    def url(self, box) -> str:
        px, py = output_size(box, self.resolution_m, f"{self.key} {box}")
        west, south, east, north = box
        # WMS 1.3.0 with a geographic CRS states the box latitude first.
        query = urllib.parse.urlencode({
            "SERVICE": "WMS", "VERSION": "1.3.0", "REQUEST": "GetMap", "LAYERS": self.layer,
            "STYLES": "", "CRS": "EPSG:4326", "BBOX": f"{south},{west},{north},{east}",
            "WIDTH": px, "HEIGHT": py, "FORMAT": "image/x-bil;bits=32",
        })
        return f"{self.service}?{query}"

    def request(self, box) -> bytes:
        px, py = output_size(box, self.resolution_m, f"{self.key} {box}")
        raw = http_get(self.url(box), what=f"{self.key} {box}")
        if len(raw) != px * py * 4:
            text = raw[:400].decode("utf-8", "replace").replace("\n", " ").strip()
            raise Refuse(f"{self.key} {box}: asked for {px}x{py} float32 and got "
                         f"{len(raw)} bytes: {text}")
        band = np.frombuffer(raw, dtype="<f4").reshape(py, px)
        west, south, east, north = box
        profile = {
            "driver": "GTiff", "height": py, "width": px, "count": 1, "dtype": "float32",
            "crs": "EPSG:4326", "nodata": IGN_VOID,
            "transform": rasterio.transform.from_bounds(west, south, east, north, px, py),
        }
        with MemoryFile() as memory:
            with memory.open(**profile) as dst:
                dst.write(band, 1)
            return memory.read()


FR = BilWmsSource(
    "fr", "France", "RGE ALTI 1 m", 1.0,
    "Licence Ouverte / Open Licence", "© IGN", "NGF-IGN69", (-5.3, 41.3, 9.6, 51.1),
    url="https://data.geopf.fr/wms-r/wms",
    layer="ELEVATION.ELEVATIONGRIDCOVERAGE.HIGHRES",
)
