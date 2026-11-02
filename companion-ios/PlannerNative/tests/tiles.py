"""Create independent PMTiles fixtures with the pinned upstream writer."""
import gzip
import json
from pathlib import Path
import sys
from pmtiles.writer import Writer
from pmtiles.tile import Compression, TileType, zxy_to_tileid


def write(path, tiles):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("wb") as stream:
        writer = Writer(stream)
        try:
            for z, x, y in tiles:
                writer.write_tile(zxy_to_tileid(z, x, y), gzip.compress(f"tile {z}/{x}/{y}".encode(), mtime=0))
            writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.GZIP,
                             "min_lon_e7": -1800000000, "min_lat_e7": -850000000,
                             "max_lon_e7": 1800000000, "max_lat_e7": 850000000,
                             "center_zoom": 0, "center_lon_e7": 0, "center_lat_e7": 0}, {})
        finally:
            writer.tile_f.close()


root = Path(sys.argv[1])
tiles = [(0, 0, 0), (9, 267, 178), *[(14, 12000+x, 5000+y) for x in range(20) for y in range(1000)]]
write(root / "tiles.pmtiles", tiles)
(root / "expected.json").write_text(json.dumps([{"z": z, "x": x, "y": y, "text": f"tile {z}/{x}/{y}"}
                                                for z, x, y in tiles if z < 14 or y % 19 == 0]))

# Offline block format 2: one pack per tile at min(z, 11).
packs = root / "format2/maps/tiles/basemap"
write(packs / "9-267-178.pmtiles", [(9, 267, 178)])
write(packs / "11-1068-712.pmtiles", [(11, 1068, 712), (13, 4273, 2849)])
# Offline block format 3: base levels 0 and 7, and the z9 square 267-178 split into its parent pack and one child.
packs = root / "format3/maps/tiles/basemap"
write(packs / "0-0-0.pmtiles", [(0, 0, 0), (6, 33, 22)])
# The z9 tile 264-176 lies in the square 7-66-44, but base level 9 never falls back to that pack.
write(packs / "7-66-44.pmtiles", [(7, 66, 44), (8, 133, 89), (9, 264, 176)])
write(packs / "9-267-178.pmtiles", [(9, 267, 178)])
write(packs / "10-534-356.pmtiles", [(10, 534, 356), (11, 1069, 713)])
