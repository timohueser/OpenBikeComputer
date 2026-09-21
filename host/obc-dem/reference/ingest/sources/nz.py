"""New Zealand: the LINZ 1 m DEM, one COG per NZTopo50 map sheet, open on S3.

The gap that kept this row from having an adapter was the index: the tiles are named
after the NZTopo50 map sheets and the static STAC carries 424 item links with no box in
the collection, so finding the sheet a box needs looked like a fourth kind of index.

It is not an index at all. NZTopo50 is a regular grid in NZTM2000, 24 km east by 36 km
north, and the constants below place it exactly: sheet `AS21` runs from (1 492 000,
6 198 000) to (1 516 000, 6 234 000), and `BX15` from (1 348 000, 5 154 000). So the sheet
is arithmetic, the same as a German state's kilometre grid, and the LINZ Data Service key
that would fetch the sheet outlines is not needed.

A sheet the survey has not reached is not published and answers 404, which `CogGrid`
counts as a coverage edge. The COGs are LERC-compressed and one sheet at 1 m is 864 M
pixels, so only the window the box needs is read.
"""

import math

from .cog import CogWindows
from .protocols import projected_box

# The sheet grid, in NZTM2000 (EPSG:2193). `SHEET_NORTH` is the north edge of letter-pair
# row 0 (`AA`); the published rows run from `AS` (row 16) to `CJ` (row 56).
SHEET_LETTERS = "ABCDEFGHJKLMNPQRSTUVWXYZ"  # `I` and `O` are never used
SHEET_WEST = 988_000
SHEET_NORTH = 6_810_000
SHEET_EAST_M = 24_000
SHEET_NORTH_M = 36_000


def sheet_name(column: int, row: int) -> str:
    """The NZTopo50 sheet at one square of the grid, as LINZ names the file."""

    letters = len(SHEET_LETTERS)
    return f"{SHEET_LETTERS[row // letters]}{SHEET_LETTERS[row % letters]}{column:02d}"


def sheets(bbox) -> list[str]:
    """Every NZTopo50 sheet a WGS84 box touches.

    `projected_box` densifies the box's edges before it transforms them, so a sheet the
    bulge of a curved edge reaches into is named and not missed.
    """

    (lo_x, lo_y, hi_x, hi_y), _ = projected_box(bbox, 2193)
    columns = range(math.floor((lo_x - SHEET_WEST) / SHEET_EAST_M),
                    math.floor((hi_x - SHEET_WEST) / SHEET_EAST_M) + 1)
    rows = range(math.floor((SHEET_NORTH - hi_y) / SHEET_NORTH_M),
                 math.floor((SHEET_NORTH - lo_y) / SHEET_NORTH_M) + 1)
    return [sheet_name(column, row) for row in rows for column in columns
            if 0 <= row < len(SHEET_LETTERS) ** 2 and column >= 0]


class Nztopo50Cogs(CogWindows):
    """Remote COGs named after the map sheet whose square holds them."""

    def urls(self, bbox):
        return [(f"{name}.tiff", f"{self.base}{name}.tiff") for name in sheets(bbox)]


NZ = Nztopo50Cogs(
    "nz", "New Zealand", "LiDAR DEM 1 m (LINZ)", 1.0,
    "CC BY 4.0",
    "Sourced from the LINZ Data Service and licensed by Toitū Te Whenua Land Information "
    "New Zealand, for re-use under CC BY 4.0",
    "NZVD2016 (EPSG:7839), normal-orthometric",
    (166.3, -47.4, 178.9, -34.0),
    base="https://nz-elevation.s3.ap-southeast-2.amazonaws.com"
         "/new-zealand/new-zealand/dem_1m/2193/",
    epsg=2193,
)
