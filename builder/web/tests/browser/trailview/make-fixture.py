"""Writes the TrailView fixture from a `planner-dem` MBTiles of BOX: the tiles with known heights, unknown outside BOX;
and the tiles of a feature archive of BOX, as it is.

uv run --locked --group planner-maps python builder/web/tests/browser/trailview/make-fixture.py TERRAIN.mbtiles FEATURES.pmtiles
"""

import io
import json
import math
import sqlite3
import sys
from pathlib import Path

from PIL import Image

BOX = (8.37, 46.765, 8.46, 46.83)
OUT = Path(__file__).parent / "fixture"
# The planner hosts that the journey answers in the page. Only the terrain and feature tiles have content.
HOST = "https://tiles.test"


def world(lon: float, lat: float, n: int) -> tuple[float, float]:
    """The Web Mercator position in a world `n` units wide."""
    y = (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2
    return (lon + 180) / 360 * n, y * n


def main(mbtiles: str, features: str) -> None:
    db = sqlite3.connect(mbtiles)
    meta = dict(db.execute("SELECT name, value FROM metadata"))
    for z, x, row, data in db.execute("SELECT zoom_level, tile_column, tile_row, tile_data FROM tiles"):
        y = 2**z - 1 - row
        image = Image.open(io.BytesIO(data)).convert("RGBA")
        size = image.width
        west, north = world(BOX[0], BOX[3], 2**z * size)
        east, south = world(BOX[2], BOX[1], 2**z * size)
        pixels = image.load()
        known = 0
        for j in range(size):
            for i in range(size):
                px, py = x * size + i + 0.5, y * size + j + 0.5
                if west <= px <= east and north <= py <= south:
                    known += pixels[i, j][3] == 255
                else:
                    pixels[i, j] = (0, 0, 0, 0)
        if known:
            path = OUT / f"terrain/{z}/{x}/{y}.webp"
            path.parent.mkdir(parents=True, exist_ok=True)
            image.save(path, lossless=True, quality=100, method=6)
    urls = {key: f"{HOST}/{key}.json" for key in ("basemap", "places", "overlays")}
    config = {
        "name": "TrailView fixture", "bounds": list(BOX), **urls,
        "terrain": f"{HOST}/terrain/{{z}}/{{x}}/{{y}}.webp",
        "attribution": "No map data", "terrain_attribution": meta["attribution"],
        "glyphs": f"{HOST}/fonts/{{fontstack}}/{{range}}.pbf", "sprites": f"{HOST}/sprites",
        "routing": "https://api.test/routing", "search": "https://api.test/search",
        "layers": {"trailview-features": f"{HOST}/trailview-features.json"},
    }
    (OUT / "config.json").write_text(json.dumps(config, indent=2, ensure_ascii=False) + "\n")
    from pmtiles.reader import MmapSource, all_tiles

    with open(features, "rb") as stream:
        for (z, x, y), data in all_tiles(MmapSource(stream)):
            path = OUT / f"features/{z}/{x}/{y}.bin"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
