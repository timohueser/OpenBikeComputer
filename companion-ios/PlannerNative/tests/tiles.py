"""Create independent PMTiles fixtures with the pinned upstream writer."""
import gzip
import json
from pathlib import Path
import sys
from pmtiles.writer import Writer
from pmtiles.tile import Compression, TileType, zxy_to_tileid

root = Path(sys.argv[1])
expected = []
with (root / "tiles.pmtiles").open("wb") as stream:
    writer = Writer(stream)
    try:
        for z, x, y in [(0, 0, 0), (9, 267, 178), *[(14, 12000+x, 5000+y) for x in range(20) for y in range(1000)]]:
            text = f"tile {z}/{x}/{y}"
            writer.write_tile(zxy_to_tileid(z, x, y), gzip.compress(text.encode(), mtime=0))
            if z < 14 or y % 19 == 0:
                expected.append({"z": z, "x": x, "y": y, "text": text})
        writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.GZIP,
                         "min_lon_e7": -1800000000, "min_lat_e7": -850000000,
                         "max_lon_e7": 1800000000, "max_lat_e7": 850000000,
                         "center_zoom": 0, "center_lon_e7": 0, "center_lat_e7": 0}, {})
    finally:
        writer.tile_f.close()
(root / "expected.json").write_text(json.dumps(expected))
