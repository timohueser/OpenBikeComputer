"""A product published as tiles on a national kilometre grid, at a fixed URL.

Several German states publish their DGM this way: no service, no index, just one file per
square kilometre of the UTM grid, named after the square's south-west corner in kilometres.
The tile a box needs is therefore arithmetic, and the adapter is a row.

A square outside the state is not published and answers 404. That is a coverage edge, not
a fault, so it is counted and skipped — the run refuses only when the box reaches no
published square at all.
"""

import math

from .bulk import BulkSource
from .protocols import projected_box


def grid_squares(bbox, epsg: int, step_m: int):
    """The south-west corner, in metres, of every grid square a WGS84 box touches.

    `projected_box` densifies the box's edges before it transforms them, so a square the
    bulge of a curved edge reaches into is named and not missed.
    """

    (lo_x, lo_y, hi_x, hi_y), _ = projected_box(bbox, epsg)
    for x in range(math.floor(lo_x / step_m) * step_m,
                   math.ceil(hi_x / step_m) * step_m, step_m):
        for y in range(math.floor(lo_y / step_m) * step_m,
                       math.ceil(hi_y / step_m) * step_m, step_m):
            yield x, y


class GridTiles(BulkSource):
    """Tiles named after the kilometre grid of the state's own UTM zone."""

    skip_missing = True

    def __init__(self, *args, base, name, epsg, tile_km, **kw):
        super().__init__(*args, **kw)
        self.base, self.name, self.epsg, self.tile_km = base, name, epsg, tile_km

    def files(self, bbox):
        return [(name, self.base + name)
                for name in (self.name.format(east=x // 1000, north=y // 1000)
                             for x, y in grid_squares(bbox, self.epsg, self.tile_km * 1000))]
