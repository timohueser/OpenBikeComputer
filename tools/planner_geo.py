"""Web Mercator tile math and the bounds argument of the planner tools. It reads no file, so a
step that imports it reads only its request."""

import argparse
import math


def bounds(value):
    try:
        west, south, east, north = map(float, value.split(","))
        if -180 <= west < east <= 180 and -85 <= south < north <= 85:
            return [west, south, east, north]
    except ValueError:
        pass
    raise argparse.ArgumentTypeError("Use west,south,east,north in degrees.")


def mercator(lon, lat, zoom):
    """Fractional Web Mercator XYZ tile coordinates of a point."""
    count = 1 << zoom
    return (lon + 180) / 360 * count, (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * count


def tile_bounds(z, x, y):
    n = 1 << z
    latitude = lambda row: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * row / n))))
    return [x / n * 360 - 180, latitude(y + 1), (x + 1) / n * 360 - 180, latitude(y)]
