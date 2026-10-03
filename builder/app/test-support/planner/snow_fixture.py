"""Write a synthetic snow archive in the tile contract of specs/planner-snow-tiles.md.

Snow lasts longer with height, read from a terrain archive; smooth blobs stand in for forest
without data. The values are fake: the archive is for front-end checks until the bake lands.

    uv run --with pmtiles --with pillow --with numpy builder/app/test-support/planner/snow_fixture.py \\
        ~/.cache/obc/planner/baden-wuerttemberg/maps/terrain.pmtiles snow.pmtiles --bbox 7.85,47.78,8.15,47.95
"""

import argparse
import gzip
import io
import json
import math

import numpy as np
from PIL import Image
from pmtiles.reader import MmapSource, Reader
from pmtiles.tile import Compression, TileType, zxy_to_tileid
from pmtiles.writer import Writer

FIRST_SEASON, SEASONS, MIN_ZOOM, MAX_ZOOM = 2016, 9, 8, 13


def tile_range(bbox, z):
    west, south, east, north = bbox
    n = 2**z
    x = lambda lon: int((lon + 180) / 360 * n)
    y = lambda lat: int((1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * n)
    return range(x(west), x(east) + 1), range(y(north), y(south) + 1)


def heights(terrain, z, x, y):
    """Terrain tiles hold 512 px, so zoom z - 1 has one terrain pixel per snow pixel."""
    data = terrain.get(z - 1, x // 2, y // 2)
    if data is None:
        return None
    rgb = np.asarray(Image.open(io.BytesIO(data)).convert("RGB"), dtype=np.float64)
    quarter = rgb[(y % 2) * 256:(y % 2) * 256 + 256, (x % 2) * 256:(x % 2) * 256 + 256]
    return quarter[..., 0] * 256 + quarter[..., 1] + quarter[..., 2] / 256 - 32768


def lonlat(z, x, y):
    n = 256 * 2**z
    px = (x * 256 + np.arange(256) + 0.5) / n
    py = (y * 256 + np.arange(256) + 0.5) / n
    lon = px * 360 - 180
    lat = np.degrees(np.arctan(np.sinh(math.pi * (1 - 2 * py))))
    return np.meshgrid(lon, lat)


def tile(terrain, z, x, y):
    h = heights(terrain, z, x, y)
    if h is None:
        return None
    lon, lat = lonlat(z, x, y)
    forest = np.sin(lon * 140) * np.sin(lat * 190) + 0.6 * np.sin(lon * 61 + lat * 83) > 1.05
    rng = np.random.default_rng(1000 * z + x + y)
    planes = []
    for season in range(SEASONS):
        shift, line = rng.normal(0, 10), rng.normal(750, 120)
        melt = (170 + (h - 750) * 0.12 + shift) // 2
        onset = (85 - (h - 750) * 0.05 + shift / 2) // 2
        none = (h < line) | (melt <= onset)
        onset = np.where(none, 253, np.clip(onset, 0, 182))
        melt = np.where(none, 253, np.clip(melt, 0, 182))
        planes += [np.where(forest, 255, onset), np.where(forest, 255, melt)]
    return gzip.compress(np.stack(planes).astype(np.uint8).tobytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("terrain")
    parser.add_argument("output")
    parser.add_argument("--bbox", required=True, type=lambda text: [float(v) for v in text.split(",")])
    args = parser.parse_args()
    with open(args.terrain, "rb") as source, open(args.output, "wb") as out:
        terrain = Reader(MmapSource(source))
        tiles = {}
        for z in range(MIN_ZOOM, MAX_ZOOM + 1):
            xs, ys = tile_range(args.bbox, z)
            for x in xs:
                for y in ys:
                    data = tile(terrain, z, x, y)
                    if data:
                        tiles[zxy_to_tileid(z, x, y)] = data
        writer = Writer(out)
        for tile_id in sorted(tiles):
            writer.write_tile(tile_id, tiles[tile_id])
        west, south, east, north = args.bbox
        e7 = lambda value: int(value * 1e7)
        writer.finalize({
            "tile_type": TileType.UNKNOWN, "tile_compression": Compression.GZIP, "min_zoom": MIN_ZOOM, "max_zoom": MAX_ZOOM,
            "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
            "center_zoom": MAX_ZOOM - 2, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2),
        }, {"first_season": FIRST_SEASON, "seasons": SEASONS, "step_days": 2, "source": "synthetic",
            "resolution_m": 20, "attribution": "Synthetic snow fixture"})
    print(f"Wrote {len(tiles)} tiles to {args.output}")


if __name__ == "__main__":
    main()
