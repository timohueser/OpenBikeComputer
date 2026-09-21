"""The request protocols more than one country speaks.

A live service answers one box at a time and caps the pixels it will return, so a box
becomes a grid of requests and each answer is written to the work directory as it arrived.
Nothing is merged and nothing is reprojected: the tail takes any CRS and any dtype.

A country is then a row in the registry. Only France needs code of its own, because IGN
serves raw float32 over WMS and the GeoTIFF has to be assembled from it.
"""

import math
import os
import urllib.parse
from pathlib import Path

from pyproj import Transformer

from ..lattice import Refuse
from .base import Source, http_get

# One request per sub-box. A failed 4000 x 4000 request wastes far more time than four
# 2000 x 2000 ones, and every service here caps the pixels it will answer with.
MAX_PIXELS = 2000

# Metres per degree of latitude. The longitude span is this times the cosine of the
# latitude, which is only used to size a request, never to place a pixel.
METRES_PER_DEGREE = 111320.0


def request_boxes(bbox, resolution_m: float):
    """A WGS84 box cut into requests no larger than `MAX_PIXELS` on a side."""

    west, south, east, north = bbox
    lat = (south + north) / 2
    span_x = (east - west) * METRES_PER_DEGREE * math.cos(math.radians(lat))
    span_y = (north - south) * METRES_PER_DEGREE
    nx = max(1, math.ceil(span_x / resolution_m / MAX_PIXELS))
    ny = max(1, math.ceil(span_y / resolution_m / MAX_PIXELS))
    for i in range(ny):
        for j in range(nx):
            yield (west + (east - west) * j / nx, south + (north - south) * i / ny,
                   west + (east - west) * (j + 1) / nx, south + (north - south) * (i + 1) / ny)


def clamp(count: float) -> int:
    return max(1, min(int(count), MAX_PIXELS))


def degree_pixels(box, resolution_m: float) -> tuple[int, int]:
    """The output size of a request stated in degrees, at the product's own step."""

    west, south, east, north = box
    lat = (south + north) / 2
    return (clamp((east - west) * METRES_PER_DEGREE * math.cos(math.radians(lat)) / resolution_m),
            clamp((north - south) * METRES_PER_DEGREE / resolution_m))


def projected_box(box, epsg: int):
    """A WGS84 box as the service's own grid: the two opposite corners, and the span."""

    west, south, east, north = box
    transformer = Transformer.from_crs("EPSG:4326", f"EPSG:{epsg}", always_xy=True)
    (lo_x, lo_y), (hi_x, hi_y) = transformer.transform(west, south), transformer.transform(east, north)
    return (lo_x, lo_y, hi_x, hi_y), (hi_x - lo_x, hi_y - lo_y)


def raster_bytes(url: str, what: str) -> bytes:
    """One raster from a service, or a refusal that quotes what came back instead.

    Every service here answers an error as an XML document with a 200, so the only way to
    tell a raster from a service that has moved or is out of coverage is the TIFF magic.
    """

    body = http_get(url)
    if body[:4] not in (b"II*\x00", b"MM\x00*", b"II+\x00", b"MM\x00+"):
        text = body[:400].decode("utf-8", "replace").replace("\n", " ").strip()
        raise Refuse(f"{what}: the service did not answer with a TIFF but with: {text}")
    return body


class TiledService(Source):
    """A service that answers one box at a time, cached per request in the work directory."""

    def fetch(self, bbox, workdir) -> list[Path]:
        workdir.mkdir(parents=True, exist_ok=True)
        boxes = list(request_boxes(bbox, self.resolution_m))
        paths = []
        for i, box in enumerate(boxes, 1):
            name = f"{self.key}_{box[0]:.5f}_{box[1]:.5f}_{box[2]:.5f}_{box[3]:.5f}.tif"
            path = workdir / name
            if not path.exists():
                # A half-written file in the cache would look complete to the next run, so
                # the bytes land beside the name and are moved onto it at the end.
                part = path.with_name(name + ".part")
                part.write_bytes(self.request(box))
                os.replace(part, path)
            print(f"  fetch [{i}/{len(boxes)}] {box[0]:.4f},{box[1]:.4f} → {box[2]:.4f},{box[3]:.4f}")
            paths.append(path)
        return paths

    def request(self, box) -> bytes:
        raise NotImplementedError

    def url(self, box) -> str:
        raise NotImplementedError


class ArcGisSource(TiledService):
    """An ArcGIS ImageServer `exportImage`, which resamples to the size it is asked for."""

    def __init__(self, *args, url, **kw):
        super().__init__(*args, **kw)
        self.service = url

    def url(self, box) -> str:
        px, py = degree_pixels(box, self.resolution_m)
        query = urllib.parse.urlencode({
            "bbox": ",".join(f"{value}" for value in box), "bboxSR": 4326, "size": f"{px},{py}",
            "imageSR": 4326, "format": "tiff", "pixelType": "F32",
            "interpolation": "RSP_BilinearInterpolation", "f": "image",
        })
        return f"{self.service}?{query}"

    def request(self, box) -> bytes:
        return raster_bytes(self.url(box), f"{self.key} {box}")


class Wcs20Source(TiledService):
    """WCS 2.0.1 `GetCoverage`.

    WCS 2.0 answers at the coverage's native step unless it is asked otherwise, and
    half-metre LiDAR overruns every server's size cap, so the output size is always stated.
    The axis labels are the coverage's own: `x`/`y` on a projected grid, `long`/`lat` on a
    geographic one, and a server refuses a subset it does not recognise the label of.
    """

    def __init__(self, *args, url, coverage, epsg, axes, **kw):
        super().__init__(*args, **kw)
        self.service, self.coverage, self.epsg, self.axes = url, coverage, epsg, axes

    def url(self, box) -> str:
        if self.epsg == 4326:
            lo_x, lo_y, hi_x, hi_y = box
            px, py = degree_pixels(box, self.resolution_m)
        else:
            (lo_x, lo_y, hi_x, hi_y), (span_x, span_y) = projected_box(box, self.epsg)
            px, py = clamp(span_x / self.resolution_m), clamp(span_y / self.resolution_m)
        ax, ay = self.axes
        return (f"{self.service}?service=WCS&version=2.0.1&request=GetCoverage"
                f"&coverageId={self.coverage}"
                f"&subset={ax}({lo_x},{hi_x})&subset={ay}({lo_y},{hi_y})"
                f"&scalesize={ax}({px}),{ay}({py})&format=image/tiff")

    def request(self, box) -> bytes:
        return raster_bytes(self.url(box), f"{self.key} {box}")


class Wcs10Source(TiledService):
    """WCS 1.0.0 `GetCoverage`, which states the grid as a bbox plus a width and height."""

    def __init__(self, *args, url, coverage, epsg, **kw):
        super().__init__(*args, **kw)
        self.service, self.coverage, self.epsg = url, coverage, epsg

    def url(self, box) -> str:
        (lo_x, lo_y, hi_x, hi_y), (span_x, span_y) = projected_box(box, self.epsg)
        px, py = clamp(span_x / self.resolution_m), clamp(span_y / self.resolution_m)
        return (f"{self.service}?service=WCS&version=1.0.0&request=GetCoverage"
                f"&coverage={self.coverage}&crs=EPSG:{self.epsg}"
                f"&bbox={lo_x},{lo_y},{hi_x},{hi_y}&width={px}&height={py}&format=GeoTIFF")

    def request(self, box) -> bytes:
        return raster_bytes(self.url(box), f"{self.key} {box}")
