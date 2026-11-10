"""The request protocols more than one country speaks.

A live service answers one box at a time and caps the pixels it will return, so a box
becomes a grid of requests and each answer is written to the work directory as it arrived.
Nothing is merged and nothing is reprojected: the tail takes any CRS and any dtype.

A country is then a row in the registry. Only France needs code of its own, because IGN
serves raw float32 over WMS and the GeoTIFF has to be assembled from it.
"""

import json
import math
import os
import re
import threading
import urllib.error
import urllib.parse
from pathlib import Path

import rasterio
from rasterio.crs import CRS
from rasterio.warp import transform_bounds

from ..lattice import Refuse, WGS84
from ..pool import read_source
from .base import MOST_MISSING, TIFF_MAGIC, Source, decode_tiff, each, http_get, redact

# One request per sub-box. A failed 4000 x 4000 request wastes far more time than four
# 2000 x 2000 ones, and every service here caps the pixels it will answer with.
MAX_PIXELS = 2000

# Metres per degree of latitude. The longitude span is this times the cosine of the
# latitude, which is only used to size a request, never to place a pixel.
METRES_PER_DEGREE = 111320.0

# Requests of one block in flight at once, chosen by the owner after the Environment Agency blocked
# a prepare that ran 4: Defra asks bulk downloads for one at a time. A server sees this times the
# blocks of its source that run at once, the `fetch.at_once` of the terrain prepare.
PARALLEL_REQUESTS = 1


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


def native_envelope(describe: str, epsg: int, what: str):
    """The `gml:Envelope` of a WCS 1.0.0 `DescribeCoverage` answer that is on `epsg`.

    A coverage can state its envelope in several systems, Piedmont's in EPSG:4326 first,
    and a server may write the code in lower case, as Denmark's does.
    """

    for srs, body in re.findall(r'<gml:Envelope[^>]*srsName="([^"]+)"[^>]*>(.*?)</gml:Envelope>',
                                describe, re.S):
        if srs.upper() != f"EPSG:{epsg}":
            continue
        corners = re.findall(r"<gml:pos[^>]*>([^<]+)<", body)
        try:
            (lo_x, lo_y), (hi_x, hi_y) = (tuple(float(v) for v in c.split()) for c in corners)
        except ValueError as exc:
            raise Refuse(f"{what}: the EPSG:{epsg} envelope is not two corners: {body[:200]}") from exc
        return lo_x, lo_y, hi_x, hi_y
    raise Refuse(redact(f"{what}: no EPSG:{epsg} envelope in the answer: {describe[:200]}"))


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

    body = http_get(url, what=what, check=decode_tiff)
    if body[:4] not in TIFF_MAGIC:
        # These services answer an error with a 200 and an XML document that quotes the
        # request, token and all, so the body is redacted like any other message.
        text = body[:400].decode("utf-8", "replace").replace("\n", " ").strip()
        raise Refuse(redact(f"{what}: the service did not answer with a TIFF but with: {text}"))
    return body


class NotMirrored(Refuse):
    """A mirrored fetch that lacks answers, after it named them in `wanted/`."""


def label(box) -> str:
    """A box in degrees as the ledger names it: west, south, east, north."""

    return ",".join(f"{value:.5f}" for value in box)


def unreadable(refusal: Refuse) -> bool:
    """Whether a request failed every retry with a 500, which is how a server answers a file it
    cannot read. A 502, 503 or 504 is a gateway in front of a server that is down or busy."""

    fault = refusal.__cause__
    return isinstance(fault, urllib.error.HTTPError) and fault.code == 500


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
        mirror = self.mirror()
        if mirror is not None:
            return self.mirrored(boxes, mirror)

        def one(box) -> Path | None:
            name = self.request_name(box)
            path = workdir / name
            if path.exists():
                return path
            try:
                body = self.request(box)
            except Refuse as refusal:
                if not (unreadable(refusal) and self.reads()):
                    raise
                # An outage that ended during `reads` leaves the box readable, so it is asked again.
                try:
                    body = self.request(box)
                except Refuse as again:
                    if not unreadable(again):
                        raise
                    self.missing.append(label(box))
                    return None
            if body is None:
                return None
            # A half-written file in the cache would look complete to the next run, so
            # the bytes land beside the name and are moved onto it at the end.
            part = path.with_name(name + ".part")
            part.write_bytes(body)
            os.replace(part, path)
            self.remember(box, body)
            return path

        paths = each(one, boxes, PARALLEL_REQUESTS)
        for i, (box, path) in enumerate(zip(boxes, paths), 1):
            note = ("" if path else ": the service is up, but cannot read it" if label(box) in self.missing
                    else ": outside the coverage")
            print(f"  fetch [{i}/{len(boxes)}] {box[0]:.4f},{box[1]:.4f} → {box[2]:.4f},{box[3]:.4f}{note}",
                  flush=True)
        if len(self.missing) > MOST_MISSING:
            raise Refuse(f"{self.key}: the service cannot read {len(self.missing)} boxes of one fetch, "
                         f"such as {self.missing[0]}")
        return [path for path in paths if path]

    def request_name(self, box) -> str:
        """The file that the answer to a box lands in, in the work directory and in a mirror."""

        return f"{self.key}_{box[0]:.5f}_{box[1]:.5f}_{box[2]:.5f}_{box[3]:.5f}.tif"

    def mirror(self) -> Path | None:
        """`<shared>/mirror/<key>`, a folder that another host fills with this row's answers, or
        `None` when there is none.

        Another host can fetch a row at the rate its server allows. While the folder exists, the
        row asks its server nothing: it reads `envelope.json` and each answer by its
        `request_name`, in place. `<name>.unreadable` is an answer the server could not read. A
        fetch that lacks an answer writes a file per lacking answer into `wanted/`, holding the
        box, for the other host to fetch, and raises `NotMirrored`.

        A link to a drive that is not mounted is still a mirror, and refused: the row must not
        fall back to its server.
        """

        if self.shared is None:
            return None
        mirror = self.shared / "mirror" / self.key
        if not os.path.lexists(mirror):
            return None
        if not mirror.is_dir():
            raise Refuse(f"{self.key}: the mirror {mirror} is not a folder that can be read")
        return mirror

    def mirrored(self, boxes, mirror: Path) -> list[Path]:
        """The answers of `boxes` in `mirror`, each a whole TIFF, read where they are."""

        asked = [(box, mirror / self.request_name(box)) for box in boxes if self.url(box) is not None]
        absent = [box for box, path in asked
                  if not (path.exists() or path.with_name(path.name + ".unreadable").exists())]
        if absent:
            (mirror / "wanted").mkdir(exist_ok=True)
            for box in absent:
                want = mirror / "wanted" / self.request_name(box)
                part = want.with_name(f"{want.name}.{os.getpid()}")
                part.write_text(json.dumps(list(box)), encoding="utf-8")
                os.replace(part, want)
            raise NotMirrored(f"{self.key}: {len(absent)} of {len(boxes)} answers are not in the mirror yet")
        paths = []
        for box, path in asked:
            if not path.exists():
                self.missing.append(label(box))
                continue
            with path.open("rb") as handle:
                head = handle.read(4)
            try:
                if head not in TIFF_MAGIC:
                    raise OSError(f"it starts with {head!r}")
                decode_tiff(path)
            except OSError as error:
                raise Refuse(f"{self.key}: {path.name} in the mirror is not a whole TIFF: {error}") from error
            if not self.known().exists():
                self.remember(box, path.read_bytes())
            paths.append(path)
        if len(self.missing) > MOST_MISSING:
            raise Refuse(f"{self.key}: the service cannot read {len(self.missing)} boxes of one fetch, "
                         f"such as {self.missing[0]}")
        return paths

    def holds_heights(self, body: bytes) -> bool:
        """Whether a raster holds a height that the tail does not read as void, so the server read
        a file for it."""

        with rasterio.MemoryFile(body) as memory:
            return read_source(memory.name, self.grid_crs(), self.void)[4] < 1

    def known(self) -> Path:
        """Where the prepare keeps a box of this row that the server read heights for."""

        return self.shared / "read" / self.key

    def remember(self, box, body: bytes) -> None:
        """Keep the first box of the prepare that the server read heights for, as `known`."""

        if self.shared is None or self.known().exists() or not self.holds_heights(body):
            return
        self.known().parent.mkdir(parents=True, exist_ok=True)
        part = self.known().with_name(f"{self.key}.{os.getpid()}.{threading.get_ident()}")
        part.write_text(label(box), encoding="utf-8")
        os.replace(part, self.known())

    def reads(self) -> bool:
        """Whether the server still reads heights for the box of `known`.

        A mosaic can hold a file that its server cannot read, and that box is a hole in its data.
        A server whose storage has gone answers its catalogue but reads no file, so only a read
        of heights tells the two apart. Before the prepare has read one box, nothing does.
        """

        if self.shared is None or not self.known().exists():
            return False
        box = tuple(float(value) for value in self.known().read_text(encoding="utf-8").split(","))
        body = self.request(box)
        return body is not None and self.holds_heights(body)

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
        if self.mirror() is not None:
            raise Refuse(f"{self.key}: a mirrored row asks its server nothing")
        credential = self.credential.query() if self.credential else ""
        return raster_bytes(url + credential, f"{self.key} {box}")

    def url(self, box) -> str | None:
        raise NotImplementedError


class ArcGisSource(TiledService):
    """An ArcGIS ImageServer `exportImage`, which resamples to the size it is asked for.

    `nodata` is for a service that states none: ČÚZK's answers 0 for the land abroad unless it
    is given one, and with one it also keeps the void out of the interpolation at the border.
    `compress` is for a service that answers the same pixels in fewer bytes when asked.
    """

    def __init__(self, *args, url, nodata=None, compress=False, **kw):
        super().__init__(*args, **kw)
        self.service, self.nodata, self.compress = url, nodata, compress

    def url(self, box) -> str:
        px, py = output_size(box, self.resolution_m, f"{self.key} {box}")
        query = urllib.parse.urlencode({
            "bbox": ",".join(f"{value}" for value in box), "bboxSR": 4326, "size": f"{px},{py}",
            "imageSR": 4326, "format": "tiff", "pixelType": "F32",
            "interpolation": "RSP_BilinearInterpolation", "f": "image",
            **({"noData": self.nodata} if self.nodata is not None else {}),
            **({"compression": "LZ77"} if self.compress else {}),
        })
        return f"{self.service}?{query}"


class CoverageService(TiledService):
    """A WCS that refuses a request past its coverage's envelope instead of answering it void.

    `fetch` reads the envelope out of `DescribeCoverage` once, and every request is clipped
    to it. A country box then runs past the coverage's edge without a refusal, and a box
    wholly outside is not asked for at all.
    """

    def __init__(self, *args, url, coverage, epsg, **kw):
        super().__init__(*args, **kw)
        self.service, self.coverage, self.epsg = url, coverage, epsg
        #: `(lo_x, lo_y, hi_x, hi_y)` on the coverage's own grid, or `None` before `fetch`.
        self.envelope = None

    def fetch(self, bbox, workdir) -> list[Path]:
        if self.envelope is None:
            mirror = self.mirror()
            if mirror is None:
                self.envelope = self.describe()
            elif (mirror / "envelope.json").exists():
                envelope = tuple(json.loads((mirror / "envelope.json").read_text(encoding="utf-8")))
                self.envelope = self.meets_extent(envelope, f"{self.key} envelope.json")
            else:
                raise Refuse(f"{self.key}: the mirror holds no envelope.json yet")
        return super().fetch(bbox, workdir)

    def describe(self):
        """The coverage's envelope, as `DescribeCoverage` states it, on the coverage's grid."""

        what = f"{self.key} DescribeCoverage"
        credential = self.credential.query() if self.credential else ""
        body = http_get(f"{self.service}?service=WCS&{self.describe_query()}{credential}",
                        what=what).decode("utf-8", "replace")
        return self.meets_extent(self.read_envelope(body, what), what)

    def meets_extent(self, envelope, what: str):
        """`envelope`, refused when it does not meet the row's extent.

        An envelope read wrong, such as with its axes swapped, would clip every box to nothing,
        and the blocks would be kept as holding no heights.
        """

        lo_x, lo_y, hi_x, hi_y = envelope
        (x0, y0, x1, y1), _ = projected_box(self.extent, self.epsg)
        if not (lo_x < x1 and hi_x > x0 and lo_y < y1 and hi_y > y0):
            raise Refuse(f"{what}: the envelope {envelope} does not meet the row's extent")
        return envelope

    def describe_query(self) -> str:
        raise NotImplementedError

    def read_envelope(self, describe: str, what: str):
        raise NotImplementedError

    def grid_box(self, box):
        """A request's box and output size on the coverage's grid, clipped to the envelope,
        or `None` when the box is wholly outside it."""

        if self.epsg == 4326:
            lo_x, lo_y, hi_x, hi_y = box
        else:
            (lo_x, lo_y, hi_x, hi_y), _ = projected_box(box, self.epsg)
        px, py = output_size(box, self.resolution_m, f"{self.key} {box}", self.epsg)
        if self.envelope is None:
            return (lo_x, lo_y, hi_x, hi_y), (px, py)
        ex0, ey0, ex1, ey1 = self.envelope
        clip = (max(lo_x, ex0), max(lo_y, ey0), min(hi_x, ex1), min(hi_y, ey1))
        if clip[0] >= clip[2] or clip[1] >= clip[3]:
            return None
        # The output size shrinks with the box, so the step the server answers at stays
        # the product's own: a request that kept its size over a smaller box is finer
        # than the product, which is a resample.
        px = max(1, round(px * (clip[2] - clip[0]) / (hi_x - lo_x)))
        py = max(1, round(py * (clip[3] - clip[1]) / (hi_y - lo_y)))
        return clip, (px, py)


class Wcs20Source(CoverageService):
    """WCS 2.0.1 `GetCoverage`.

    WCS 2.0 answers at the coverage's native step unless it is asked otherwise, and
    half-metre LiDAR overruns every server's size cap, so the output size is stated where
    the server accepts it. Several coverages refuse `scalesize` with `ScaleAxisUndefined`
    however the axes are named, and those rows set `scale=False` and take the native step;
    `request_boxes` already keeps a box inside the pixel cap at that step.

    The axis labels are the coverage's own: `x`/`y` or `E`/`N` on a projected grid,
    `long`/`lat` on a geographic one, and a server refuses a label it does not know.
    A subset outside the envelope is refused as `InvalidSubsetting`.

    `compress` is for a server that answers the same pixels in fewer bytes when asked. Some
    servers refuse this parameter, so a row opts in.
    """

    def __init__(self, *args, axes, scale=True, compress=False, **kw):
        super().__init__(*args, **kw)
        self.axes, self.scale, self.compress = axes, scale, compress

    def describe_query(self) -> str:
        return f"version=2.0.1&request=DescribeCoverage&coverageId={self.coverage}"

    def read_envelope(self, describe: str, what: str):
        return envelope_of(describe, self.axes, what)

    def url(self, box) -> str | None:
        grid = self.grid_box(box)
        if grid is None:
            return None
        (lo_x, lo_y, hi_x, hi_y), (px, py) = grid
        ax, ay = self.axes
        query = (f"{self.service}?service=WCS&version=2.0.1&request=GetCoverage"
                 f"&coverageId={self.coverage}"
                 f"&subset={ax}({lo_x},{hi_x})&subset={ay}({lo_y},{hi_y})")
        if self.scale:
            query += f"&scalesize={ax}({px}),{ay}({py})"
        if self.compress:
            query += "&geotiff:compression=Deflate"
        return query + "&format=image/tiff"


class Wcs10Source(CoverageService):
    """WCS 1.0.0 `GetCoverage`, which states the grid as a bbox plus a width and height.

    `image_format` is the format name the coverage's own capabilities offer, because 1.0.0
    names a format with the server's word for it rather than a media type: Kartverket
    answers to `GeoTIFF` and Denmark's MapServer to `GTiff`. A bbox past the envelope is
    refused: Denmark answers `InvalidBoundingBox`, Saxony-Anhalt `InvalidParameterValue`.
    """

    def __init__(self, *args, image_format="GeoTIFF", **kw):
        super().__init__(*args, **kw)
        self.image_format = image_format

    def describe_query(self) -> str:
        return f"version=1.0.0&request=DescribeCoverage&coverage={self.coverage}"

    def read_envelope(self, describe: str, what: str):
        return native_envelope(describe, self.epsg, what)

    def url(self, box) -> str | None:
        grid = self.grid_box(box)
        if grid is None:
            return None
        (lo_x, lo_y, hi_x, hi_y), (px, py) = grid
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
