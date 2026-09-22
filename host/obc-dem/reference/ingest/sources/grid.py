"""A product published as tiles on a national kilometre grid, at a fixed URL.

Several German states publish their DGM this way: no service, no index, just one file per
square kilometre of the UTM grid, named after the square's south-west corner in kilometres.
The tile a box needs is therefore arithmetic, and the adapter is a row.

A square outside the state is not published and answers 404. That is a coverage edge, not
a fault, so it is counted and skipped; a box that reaches no published square at all is
empty, which a country-scale run meets at every corner of the state.
"""

import math

from .bulk import BulkSource
from .protocols import projected_box


def grid_squares(bbox, epsg: int, step_m: int, origin=(0, 0)):
    """The south-west corner, in metres, of every grid square a WGS84 box touches.

    `projected_box` densifies the box's edges before it transforms them, so a square the
    bulge of a curved edge reaches into is named and not missed. `origin` is where the
    grid's squares start when that is not the zone's own origin: Baden-Württemberg's 2 km
    squares begin on an odd kilometre of easting.
    """

    (lo_x, lo_y, hi_x, hi_y), _ = projected_box(bbox, epsg)
    ox, oy = origin
    for x in range(math.floor((lo_x - ox) / step_m) * step_m + ox,
                   math.ceil((hi_x - ox) / step_m) * step_m + ox, step_m):
        for y in range(math.floor((lo_y - oy) / step_m) * step_m + oy,
                       math.ceil((hi_y - oy) / step_m) * step_m + oy, step_m):
            yield x, y


class GridTiles(BulkSource):
    """Tiles named after the kilometre grid of the state's own UTM zone."""

    skip_missing = True

    def __init__(self, *args, base, name, epsg, tile_km, origin_km=(0, 0), **kw):
        super().__init__(*args, **kw)
        self.base, self.name, self.epsg, self.tile_km = base, name, epsg, tile_km
        self.origin = (origin_km[0] * 1000, origin_km[1] * 1000)

    def files(self, bbox):
        squares = grid_squares(bbox, self.epsg, self.tile_km * 1000, self.origin)
        return [(name, self.base + name)
                for name in (self.name.format(east=x // 1000, north=y // 1000) for x, y in squares)]
