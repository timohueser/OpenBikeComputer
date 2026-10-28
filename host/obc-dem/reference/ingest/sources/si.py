"""Slovenia: DMR1, the 1 m terrain model of the national laser scan of 2011–2015 (ARSO, GURS).

ARSO publishes one file per square kilometre of D96/TM, a semicolon XYZ text named after the
square's south-west corner in kilometres, under the folder of the flight block it came in:
`b_37/D96TM/TM1_410_138.txt`. The block is not arithmetic, so `si_blocks.txt` is the index. It is
ARSO's fishnet `lidar_fishnet_D96TM.zip` (field `BLOK`) as runs: one line per northing in
kilometres, then `first-last:block` per run of eastings. A square the index does not name has no
file, so it is not asked for.

GDAL reads an XYZ grid but no CRS beside it, so `grid_epsg` places it, and the download keeps the
`.xyz` suffix that the tail reads. The heights are orthometric on SVS2000: the contractor's
ellipsoidal heights were converted with the national geoid.
"""

from functools import cache
from pathlib import Path

from .bulk import BulkSource
from .grid import grid_squares

INDEX = Path(__file__).with_name("si_blocks.txt")


@cache
def blocks() -> dict[tuple[int, int], int]:
    """The flight block of every published square, by its corner in kilometres."""

    index = {}
    for line in INDEX.read_text(encoding="utf-8").splitlines():
        north, *runs = line.split()
        for run in runs:
            span, block = run.split(":")
            first, _, last = span.partition("-")
            for east in range(int(first), int(last or first) + 1):
                index[east, int(north)] = int(block)
    return index


class ArsoTiles(BulkSource):
    """Square-kilometre XYZ files in the folder of their flight block."""

    def __init__(self, *args, base, **kw):
        super().__init__(*args, **kw)
        self.base = base

    def files(self, bbox):
        wanted = []
        for east, north in grid_squares(bbox, self.grid_epsg, 1000):
            square = (east // 1000, north // 1000)
            if square in blocks():
                name = f"TM1_{square[0]}_{square[1]}"
                wanted.append((f"{name}.xyz", f"{self.base}b_{blocks()[square]}/D96TM/{name}.txt"))
        return wanted


SI = ArsoTiles(
    "si", "Slovenia", "DMR1 1 m, laser scan of Slovenia 2011–2015 (ARSO, GURS)", 1.0,
    "CC BY 4.0",
    "SVS2000, orthometric (D96/TM + SVS2000)",
    grid_epsg=3794,
    base="https://gis.arso.gov.si/lidar/dmr1/",
)
