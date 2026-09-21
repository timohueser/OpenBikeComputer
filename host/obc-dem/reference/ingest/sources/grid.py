"""A product published as tiles on a national kilometre grid, at a fixed URL.

Several German states publish their DGM this way: no service, no index, just one file per
square kilometre of the UTM grid, named after the square's south-west corner in kilometres.
The tile a box needs is therefore arithmetic, and the adapter is a row.

A square outside the state is not published and answers 404. That is a coverage edge, not
a fault, so it is counted and skipped — the run refuses only when the box reaches no
published square at all.
"""

import math

from pyproj import Transformer

from .bulk import BulkSource


def grid_squares(bbox, epsg: int, step_m: int):
    """The south-west corner, in metres, of every grid square a WGS84 box touches."""

    west, south, east, north = bbox
    to_grid = Transformer.from_crs("EPSG:4326", f"EPSG:{epsg}", always_xy=True)
    # All four corners, because a projected grid's north edge is not a line of latitude.
    xs, ys = to_grid.transform([west, east, west, east], [south, south, north, north])
    for x in range(math.floor(min(xs) / step_m) * step_m,
                   math.ceil(max(xs) / step_m) * step_m, step_m):
        for y in range(math.floor(min(ys) / step_m) * step_m,
                       math.ceil(max(ys) / step_m) * step_m, step_m):
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
