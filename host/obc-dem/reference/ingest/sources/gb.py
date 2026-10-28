"""Great Britain: each nation publishes its LiDAR separately.

England is the Environment Agency's LIDAR Composite DTM, over Defra's WCS 2.0.1. The endpoint
carries no survey year, so the coverage id is stable: a UUID and a name, exactly as the
capabilities spell it. The year in the attribution is the composite release that the endpoint
serves, and the agency asks for it verbatim. Check it against the service's metadata when the
composite is re-released.

Scotland is the Scottish Public Sector LiDAR, every open survey of the Remote Sensing Portal.
The portal puts each survey on a public S3 bucket as its own folder of tiled GeoTIFFs in British
National Grid, one per OS grid square: 10 km for Phases I and II, 5 km quarters for Phases III to
VI and the Outer Hebrides, 1 km for the National LiDAR Programme, Orkney and the coast. No survey
covers the whole country.

A file is named after its square, so where a file lies is arithmetic, but which squares a survey
flew is not. The bucket listing is the index: one request per survey and 100 km square, read
once per fetch. The squares are tiled, so only the box's window is read, as for Austria.

The Historic Environment Scotland surveys are left out: most of them are licensed for
non-commercial use only. Where two surveys overlap, both reach the pooling.

Wales is the Welsh Government's national LiDAR of the winters 2020–23, one COG of the whole
nation with range reads. The row reads the Int16 copy of whole metres, 3.6 GB, and not the
float32 copy of 48.6 GB: the Int16 values are the float32 ones rounded to the nearest metre,
so the maximum pool, whole metres too, is the same, and the mean pool of decimetres moves by
the mean of the rounding over a lattice pixel, which is about zero.
"""

import math
import re
import urllib.parse
from pathlib import Path

from ..lattice import Refuse
from .base import http_get
from .cog import CogWindows, OneFile
from .protocols import Wcs20Source, projected_box

# The OS grid letters: `I` is never used. A 100 km square has two of them, the 500 km square
# first, counted from the false origin's square `S`.
LETTERS = "ABCDEFGHJKLMNOPQRSTUVWXYZ"
REFERENCE = re.compile(r"^([A-HJ-Z]{2})((?:\d\d)*)(NE|NW|SE|SW)?$")


def letters_at(east: int, north: int) -> str:
    """The two letters of the 100 km square that holds a point of the grid."""

    row, col = 19 - north // 100_000, east // 100_000
    first = row - row % 5 + (col + 10) // 5
    second = row * 5 % 25 + col % 5
    return LETTERS[first] + LETTERS[second]


def square_of(reference: str) -> tuple[int, int, int]:
    """The south-west corner and the side, in metres, of an OS grid reference such as `NN16`,
    `NN16NE` or `NN1265`."""

    match = REFERENCE.match(reference)
    if not match:
        raise Refuse(f"`{reference}` is not an OS grid reference")
    letters, digits, quarter = match.groups()
    first, second = (LETTERS.index(letter) for letter in letters)
    east = (first % 5 - 2) * 500_000 + second % 5 * 100_000
    north = (3 - first // 5) * 500_000 + (4 - second // 5) * 100_000
    half = len(digits) // 2
    side = 10 ** (5 - half)
    if half:
        east += int(digits[:half]) * side
        north += int(digits[half:]) * side
    if quarter:
        side //= 2
        east += side * (quarter[1] == "E")
        north += side * (quarter[0] == "N")
    return east, north, side


class SurveyListing(CogWindows):
    """The squares of every survey folder that meet the box, as the bucket lists them."""

    def __init__(self, *args, surveys, **kw):
        super().__init__(*args, epsg=27700, **kw)
        self.surveys = surveys
        self.listed: dict[str, list[str]] = {}

    def listing(self, prefix: str) -> list[str]:
        """Every GeoTIFF key under a prefix, following the bucket's continuation tokens."""

        if prefix not in self.listed:
            keys, query = [], {"list-type": "2", "prefix": prefix}
            while True:
                page = http_get(f"{self.base}?{urllib.parse.urlencode(query)}").decode("utf-8")
                keys += [key for key in re.findall(r"<Key>([^<]+)</Key>", page) if key.endswith(".tif")]
                token = re.search(r"<NextContinuationToken>([^<]+)<", page)
                if token is None:
                    break
                query["continuation-token"] = token.group(1)
            self.listed[prefix] = keys
        return self.listed[prefix]

    def urls(self, bbox) -> list[tuple[str, str]]:
        (lo_x, lo_y, hi_x, hi_y), _ = projected_box(bbox, self.epsg)
        hundreds = sorted({letters_at(x, y)
                           for x in range(math.floor(lo_x / 1e5) * 100_000, math.ceil(hi_x), 100_000)
                           for y in range(math.floor(lo_y / 1e5) * 100_000, math.ceil(hi_y), 100_000)})
        wanted = []
        for survey in self.surveys:
            for letters in hundreds:
                for key in self.listing(survey + letters):
                    name = Path(key).name
                    east, north, side = square_of(name.split("_")[0])
                    if east < hi_x and east + side > lo_x and north < hi_y and north + side > lo_y:
                        wanted.append((name, self.base + key))
        return wanted


SCT = SurveyListing(
    "gb-sct", "United Kingdom (Scotland)", "Scottish Public Sector LiDAR DTM 0.5–1 m", 0.5,
    "Open Government Licence v3",
    "Ordnance Datum Newlyn, the islands on their OS local datums",
    base="https://srsp-open-data.s3.eu-west-2.amazonaws.com/",
    surveys=tuple(f"lidar/{folder}/27700/gridded/" for folder in (
        "phase-1/dtm", "phase-2/dtm", "phase-3/dtm", "phase-4/dtm", "phase-5/dtm", "phase-6/dtm",
        "outer-hebrides/2019/dtm/50cm", "orkney-islands-council-23/dtm",
        "coastal/coastal-2025-2026/dtm", "national-lidar-programme/dtm")),
)

ENG = Wcs20Source(
    "gb-eng", "United Kingdom (England)", "EA LIDAR Composite DTM 1 m", 1.0,
    "Open Government Licence v3",
    "Ordnance Datum Newlyn",
    url="https://environment.data.gov.uk/spatialdata/lidar-composite-digital-terrain-model-dtm-1m/wcs",
    coverage="13787b9a-26a4-4775-8523-806d13af58fc__Lidar_Composite_Elevation_DTM_1m",
    epsg=27700,
    axes=("E", "N"),
    scale=False,
)

WLS = OneFile(
    "gb-wls", "United Kingdom (Wales)", "Welsh Government LiDAR DTM 2020–23 1 m", 1.0,
    "Open Government Licence v3",
    "Ordnance Datum Newlyn",
    base="https://dmwproductionblob.blob.core.windows.net/cogs/lidar/wales_dtm_16bit_cog.tif",
    epsg=27700,
)

GB = (ENG, SCT, WLS)
