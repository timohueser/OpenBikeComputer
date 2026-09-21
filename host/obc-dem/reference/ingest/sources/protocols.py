"""The request protocols more than one country speaks.

A live service answers one box at a time and caps the pixels it will return, so a box
becomes a grid of requests and each answer is written to the work directory as it arrived.
Nothing is merged and nothing is reprojected: the tail takes any CRS and any dtype.

A country is then a row in the registry. Only France needs code of its own, because IGN
serves raw float32 over WMS and the GeoTIFF has to be assembled from it.
"""

import math
import os
import re
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from rasterio.crs import CRS
from rasterio.warp import transform_bounds

from ..lattice import Refuse, WGS84
from .base import Source, http_get, redact

# One request per sub-box. A failed 4000 x 4000 request wastes far more time than four
# 2000 x 2000 ones, and every service here caps the pixels it will answer with.
MAX_PIXELS = 2000

# Metres per degree of latitude. The longitude span is this times the cosine of the
# latitude, which is only used to size a request, never to place a pixel.
METRES_PER_DEGREE = 111320.0

# Requests of one box in flight at once. Four is well under what a public WCS rate-limits
# and enough that a slow edge request no longer serialises the box behind it.
PARALLEL_REQUESTS = 4


def spans(bbox, epsg=None) -> tuple[float, float]:
    """How many metres wide and tall a box is, measured where the request will be stated.

    A projected grid stretches, and by more than a rounding error: EPSG:3979 over Montréal
    is 9 % longer than the great-circle span. Sizing a request from the sphere and then
    stating it on the grid is what made an earlier version ask for 1.0956 m pixels of a
    1 m product, so the split and the output size are both measured here.
    """

    if epsg in (None, 4326):
        west, south, east, north = bbox
        lat = (south + north) / 2
        return ((east - west) * METRES_PER_DEGREE * math.cos(math.radians(lat)),
                (north - south) * METRES_PER_DEGREE)
    _, (span_x, span_y) = projected_box(bbox, epsg)
    return span_x, span_y


def request_boxes(bbox, resolution_m: float, epsg=None):
    """A WGS84 box cut into requests no larger than `MAX_PIXELS` on a side."""

    west, south, east, north = bbox
    span_x, span_y = spans(bbox, epsg)
    nx = max(1, math.ceil(span_x / resolution_m / MAX_PIXELS))
    ny = max(1, math.ceil(span_y / resolution_m / MAX_PIXELS))
    for i in range(ny):
        for j in range(nx):
            yield (west + (east - west) * j / nx, south + (north - south) * i / ny,
                   west + (east - west) * (j + 1) / nx, south + (north - south) * (i + 1) / ny)


def pixels(count: float, what: str) -> int:
    """The output size of one request, which the split has already kept inside the cap.

    Over the cap is a refusal and not a smaller number: asking a service for fewer pixels
    than the box holds gets a coarser raster back, and a quietly coarser reference is the
    one failure a reference archive must not have.
    """

    size = max(1, round(count))
    if size > MAX_PIXELS:
        raise Refuse(f"{what}: a request of {size} pixels is over the {MAX_PIXELS} cap, so "
                     "the box was not split small enough; this is a bug in request_boxes")
    return size


def output_size(box, resolution_m: float, what: str, epsg=None) -> tuple[int, int]:
    """The output size of a request, at the product's own step."""

    span_x, span_y = spans(box, epsg)
    return pixels(span_x / resolution_m, what), pixels(span_y / resolution_m, what)


def envelope_of(describe: str, axes, what: str):
    """The `gml:lowerCorner`/`upperCorner` of a `DescribeCoverage` answer, on the row's axes.

    The corners are in the order `axisLabels` states, which is the coverage's and not
    necessarily the row's, so each axis is looked up by its label.
    """

    labels = re.search(r'axisLabels="([^"]+)"', describe)
    lower = re.search(r"<gml:lowerCorner>([^<]+)<", describe)
    upper = re.search(r"<gml:upperCorner>([^<]+)<", describe)
    if not (labels and lower and upper):
        raise Refuse(redact(f"{what}: no envelope in the answer: {describe[:200]}"))
    order = labels.group(1).split()
    try:
        lo = dict(zip(order, (float(v) for v in lower.group(1).split())))
        hi = dict(zip(order, (float(v) for v in upper.group(1).split())))
        ax, ay = axes
        return lo[ax], lo[ay], hi[ax], hi[ay]
    except (KeyError, ValueError) as exc:
        raise Refuse(f"{what}: the envelope's axes {order} are not the row's {list(axes)}") from exc


def projected_box(box, epsg: int):
    """A WGS84 box as the service's own grid: the enclosing rectangle, and its span.

    The edges are densified before they are transformed, the same way `pool.read_source`
    densifies a raster's bounds. A projected grid's axes are not the box's — EPSG:3979
    turns a European box inside out — and its edges are curves, so the four corners alone
    miss the bulge between them, which is 130 m for an Austrian box on EPSG:3035.
    """

    lo_x, lo_y, hi_x, hi_y = transform_bounds(WGS84, CRS.from_epsg(epsg), *box, densify_pts=21)
    return (lo_x, lo_y, hi_x, hi_y), (hi_x - lo_x, hi_y - lo_y)


def raster_bytes(url: str, what: str) -> bytes:
    """One raster from a service, or a refusal that quotes what came back instead.

    Every service here answers an error as an XML document with a 200, so the only way to
    tell a raster from a service that has moved or is out of coverage is the TIFF magic.
    `what` is the source and the box, and it is what a refusal names: a keyed service
    reads its token out of the URL, so the URL is not something to put in a message.
    """

    body = http_get(url, what=what)
    if body[:4] not in (b"II*\x00", b"MM\x00*", b"II+\x00", b"MM\x00+"):
        # These services answer an error with a 200 and an XML document that quotes the
        # request, token and all, so the body is redacted like any other message.
        text = body[:400].decode("utf-8", "replace").replace("\n", " ").strip()
        raise Refuse(redact(f"{what}: the service did not answer with a TIFF but with: {text}"))
    return body


class TiledService(Source):
    """A service that answers one box at a time, cached per request in the work directory."""

    #: These services read a key out of the query, which `request` appends.
    credential_style = "query"

    #: The grid a request is stated in, when it is not degrees. The split is measured
    #: there, so a request can never overrun the pixel cap and come back coarsened.
    epsg = None

    def fetch(self, bbox, workdir) -> list[Path]:
        workdir.mkdir(parents=True, exist_ok=True)
        boxes = list(request_boxes(bbox, self.resolution_m, self.epsg))

        def one(box) -> Path | None:
            name = f"{self.key}_{box[0]:.5f}_{box[1]:.5f}_{box[2]:.5f}_{box[3]:.5f}.tif"
            path = workdir / name
            if path.exists():
                return path
            body = self.request(box)
            if body is None:
                return None
            # A half-written file in the cache would look complete to the next run, so
            # the bytes land beside the name and are moved onto it at the end.
            part = path.with_name(name + ".part")
            part.write_bytes(body)
            os.replace(part, path)
            return path

        # The requests of one box are independent, and a server spends most of a request
        # rendering it: the LGL WCS takes 85 s for a box on its coverage edge and 4 s for
        # one inside, so a country run is the server's time, and the pool shares it.
        with ThreadPoolExecutor(max_workers=PARALLEL_REQUESTS) as pool:
            paths = list(pool.map(one, boxes))
        for i, (box, path) in enumerate(zip(boxes, paths), 1):
            note = "" if path else ": outside the coverage"
            print(f"  fetch [{i}/{len(boxes)}] {box[0]:.4f},{box[1]:.4f} → {box[2]:.4f},{box[3]:.4f}{note}",
                  flush=True)
        return [path for path in paths if path]

    def request(self, box) -> bytes | None:
        """One raster, asked for as the protocol states it and as the portal lets it be, or
        `None` for a box the service holds nothing of.

        A portal behind an account reads its key out of the query, so the credential is
        appended here rather than inside every protocol's `url`: the request a keyed WCS
        sends is the request the keyless one sends, plus one parameter.
        """

        url = self.url(box)
        if url is None:
            return None
        credential = self.credential.query() if self.credential else ""
        return raster_bytes(url + credential, f"{self.key} {box}")

    def url(self, box) -> str | None:
        raise NotImplementedError


class ArcGisSource(TiledService):
    """An ArcGIS ImageServer `exportImage`, which resamples to the size it is asked for."""

    def __init__(self, *args, url, **kw):
        super().__init__(*args, **kw)
        self.service = url

    def url(self, box) -> str:
        px, py = output_size(box, self.resolution_m, f"{self.key} {box}")
        query = urllib.parse.urlencode({
            "bbox": ",".join(f"{value}" for value in box), "bboxSR": 4326, "size": f"{px},{py}",
            "imageSR": 4326, "format": "tiff", "pixelType": "F32",
            "interpolation": "RSP_BilinearInterpolation", "f": "image",
        })
        return f"{self.service}?{query}"


class Wcs20Source(TiledService):
    """WCS 2.0.1 `GetCoverage`.

    WCS 2.0 answers at the coverage's native step unless it is asked otherwise, and
    half-metre LiDAR overruns every server's size cap, so the output size is stated where
    the server accepts it. Several coverages refuse `scalesize` with `ScaleAxisUndefined`
    however the axes are named, and those rows set `scale=False` and take the native step;
    `request_boxes` already keeps a box inside the pixel cap at that step.

    The axis labels are the coverage's own: `x`/`y` or `E`/`N` on a projected grid,
    `long`/`lat` on a geographic one, and a server refuses a label it does not know.

    A subset outside the coverage's envelope is refused as `InvalidSubsetting`, not answered
    void, so `fetch` reads the envelope out of `DescribeCoverage` once and every request is
    clipped to it. A country box then runs past the state's border without a refusal, and a
    box wholly outside is not asked for at all.
    """

    def __init__(self, *args, url, coverage, epsg, axes, scale=True, **kw):
        super().__init__(*args, **kw)
        self.service, self.coverage, self.epsg, self.axes = url, coverage, epsg, axes
        self.scale = scale
        #: `(lo_x, lo_y, hi_x, hi_y)` on the coverage's own grid, or `None` before `fetch`.
        self.envelope = None

    def fetch(self, bbox, workdir) -> list[Path]:
        if self.envelope is None:
            self.envelope = self.describe()
        return super().fetch(bbox, workdir)

    def describe(self):
        """The coverage's envelope, as `DescribeCoverage` states it, on the row's axes."""

        what = f"{self.key} DescribeCoverage"
        body = http_get(f"{self.service}?service=WCS&version=2.0.1&request=DescribeCoverage"
                        f"&coverageId={self.coverage}", what=what).decode("utf-8", "replace")
        return envelope_of(body, self.axes, what)

    def url(self, box) -> str | None:
        if self.epsg == 4326:
            lo_x, lo_y, hi_x, hi_y = box
        else:
            (lo_x, lo_y, hi_x, hi_y), _ = projected_box(box, self.epsg)
        px, py = output_size(box, self.resolution_m, f"{self.key} {box}", self.epsg)
        if self.envelope is not None:
            ex0, ey0, ex1, ey1 = self.envelope
            clip = (max(lo_x, ex0), max(lo_y, ey0), min(hi_x, ex1), min(hi_y, ey1))
            if clip[0] >= clip[2] or clip[1] >= clip[3]:
                return None
            # The output size shrinks with the box, so the step the server answers at stays
            # the product's own: a request that kept its size over a smaller box is finer
            # than the product, which is a resample.
            px = max(1, round(px * (clip[2] - clip[0]) / (hi_x - lo_x)))
            py = max(1, round(py * (clip[3] - clip[1]) / (hi_y - lo_y)))
            lo_x, lo_y, hi_x, hi_y = clip
        ax, ay = self.axes
        query = (f"{self.service}?service=WCS&version=2.0.1&request=GetCoverage"
                 f"&coverageId={self.coverage}"
                 f"&subset={ax}({lo_x},{hi_x})&subset={ay}({lo_y},{hi_y})")
        if self.scale:
            query += f"&scalesize={ax}({px}),{ay}({py})"
        return query + "&format=image/tiff"


class Wcs10Source(TiledService):
    """WCS 1.0.0 `GetCoverage`, which states the grid as a bbox plus a width and height.

    `image_format` is the format name the coverage's own capabilities offer, because 1.0.0
    names a format with the server's word for it rather than a media type: Kartverket
    answers to `GeoTIFF` and Denmark's MapServer to `GTiff`.
    """

    def __init__(self, *args, url, coverage, epsg, image_format="GeoTIFF", **kw):
        super().__init__(*args, **kw)
        self.service, self.coverage, self.epsg = url, coverage, epsg
        self.image_format = image_format

    def url(self, box) -> str:
        (lo_x, lo_y, hi_x, hi_y), _ = projected_box(box, self.epsg)
        px, py = output_size(box, self.resolution_m, f"{self.key} {box}", self.epsg)
        return (f"{self.service}?service=WCS&version=1.0.0&request=GetCoverage"
                f"&coverage={self.coverage}&crs=EPSG:{self.epsg}"
                f"&bbox={lo_x},{lo_y},{hi_x},{hi_y}&width={px}&height={py}"
                f"&format={self.image_format}")


class Wcs11Source(TiledService):
    """WCS 1.1.1 `GetCoverage`, which states the grid itself rather than an output size.

    A 1.1.1 server answers one pixel if it is not given the grid, so the origin and the
    offsets are always in the request. The origin is the grid's first pixel centre, which
    is the north-west corner, and the offsets step east and south from it.
    """

    def __init__(self, *args, url, coverage, epsg, **kw):
        super().__init__(*args, **kw)
        self.service, self.coverage, self.epsg = url, coverage, epsg

    def url(self, box) -> str:
        (lo_x, lo_y, hi_x, hi_y), _ = projected_box(box, self.epsg)
        # The product's own step, never coarser: `output_size` refuses a box the split
        # left too large rather than letting the service answer at a coarser step.
        output_size(box, self.resolution_m, f"{self.key} {box}", self.epsg)
        step = self.resolution_m
        crs = f"urn:ogc:def:crs:EPSG::{self.epsg}"
        return (f"{self.service}?service=WCS&version=1.1.1&request=GetCoverage"
                f"&identifier={self.coverage}&format=image/geotiff"
                f"&boundingbox={lo_x},{lo_y},{hi_x},{hi_y},{crs}"
                f"&gridbasecrs={crs}"
                f"&gridcs=urn:ogc:def:cs:OGC:0.0:Grid2dSquareCS"
                f"&gridtype=urn:ogc:def:method:WCS:1.1:2dSimpleGrid"
                f"&gridorigin={lo_x},{hi_y}&gridoffsets={step},-{step}")
