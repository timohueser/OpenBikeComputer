"""The zoom-9 grid of the offline areas."""
from . import planner_geo

ZOOM = 9


def tile(lon, lat, zoom=ZOOM):
    n = 1 << zoom
    return tuple(min(n - 1, max(0, int(value))) for value in planner_geo.mercator(lon, lat, zoom))


def cells(bounds):
    left, top = tile(bounds[0], bounds[3])
    right, bottom = tile(bounds[2], bounds[1])
    for x in range(left, right + 1):
        for y in range(top, bottom + 1):
            b = planner_geo.tile_bounds(ZOOM, x, y)
            clipped = [max(b[0], bounds[0]), max(b[1], bounds[1]), min(b[2], bounds[2]), min(b[3], bounds[3])]
            if clipped[0] < clipped[2] and clipped[1] < clipped[3]:
                yield f"{ZOOM}-{x}-{y}", clipped
