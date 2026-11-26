"""The ways archive draws the stretches of `ways.sqlite` with their steepness and trail grades."""

import gzip
from pathlib import Path
import sqlite3
import tempfile
import unittest

from tools import planner_mvt as mvt
from tools import planner_ways as ways
from tools.tests.test_planner_overlays import blob


def features(tile):
    """The `ways` layer of a tile as [(feature ID, properties)]."""
    (number, layer), = mvt.fields(gzip.decompress(tile))
    parts = list(mvt.fields(layer))
    keys = [v.decode() for f, v in parts if f == 3]
    values = [next(v.decode() if f == 1 else bool(v) if f == 7 else v for f, v in mvt.fields(value)) for f, value in parts if f == 4]
    assert number == 3 and next(v for f, v in parts if f == 1) == b"ways"
    result = []
    for fields in (dict(mvt.fields(feature)) for f, feature in parts if f == 2):
        tags = mvt.packed(fields[2])
        result.append((fields[1], {keys[k]: values[v] for k, v in zip(tags[::2], tags[1::2])}))
    return sorted(result, key=lambda feature: feature[0])


class WaysArchive(unittest.TestCase):
    def test_tiles_draw_the_stretches_of_their_block_with_their_grades(self):
        from pmtiles.reader import MmapSource, Reader
        # A block of zoom 9 ends at 7.734375° E, where the next one starts. At 48.2° N the east block has the
        # lower tile ID, so the archive is clustered only when the blocks go in tile-ID order.
        edge, lat = 7.734375, 48.2
        x, y = ways.coordinates(blob([(edge, lat)]))
        west, east = (round(x * 512) - 1) * 512 + int(y * 512), round(x * 512) * 512 + int(y * 512)
        rows = [
            # Two stretches of equal properties meet at one node, and a third of another class goes on.
            (west, 1, 12, 2, None, None, 0, blob([(7.70, lat), (7.701, lat)])),
            (west, 2, 12, 2, None, None, 0, blob([(7.701, lat), (7.702, lat)])),
            (west, 3, 12, 4, None, None, 0, blob([(7.702, lat), (7.703, lat)])),
            # A path with both grades, and a service way that starts at zoom 13.
            (west, 4, 12, None, 3, 5, 1, blob([(7.71, lat), (7.711, lat)])),
            (west, 5, 13, 0, None, None, 0, blob([(7.72, lat), (7.721, lat)])),
            # One stretch across the edge has a row in each block.
            (west, 6, 12, 1, None, None, 1, blob([(edge - 0.001, lat), (edge + 0.001, lat)])),
            (east, 6, 12, 1, None, None, 1, blob([(edge - 0.001, lat), (edge + 0.001, lat)])),
        ]
        with tempfile.TemporaryDirectory() as directory:
            database, target = Path(directory) / "ways.sqlite", Path(directory) / "ways.pmtiles"
            with sqlite3.connect(database) as db:
                db.executescript("""PRAGMA user_version=1; CREATE TABLE stretches(block INTEGER, way INTEGER, minzoom INTEGER,
                    slope INTEGER, mtb INTEGER, sac INTEGER, trail INTEGER, coordinates BLOB);""")
                db.executemany("INSERT INTO stretches VALUES (?, ?, ?, ?, ?, ?, ?, ?)", rows)
            db.close()
            metrics = ways.derive(database, target, [7.6, 48.1, 7.8, 48.3], "© OpenStreetMap contributors")
            with target.open("rb") as stream:
                reader = Reader(MmapSource(stream))
                header, metadata = reader.header(), reader.metadata()
                tile = lambda z, lon: features(reader.get(z, *(int(v * (1 << z)) for v in ways.coordinates(blob([(lon, lat)])))))
                west12, west13, east12 = tile(12, 7.70), tile(13, 7.72), tile(12, edge + 0.0005)
        self.assertEqual((metadata["ways_format"], metadata["vector_layers"]), (1, [{"id": "ways", "minzoom": 12, "maxzoom": 14}]))
        self.assertEqual((header["min_zoom"], header["max_zoom"], header["clustered"]), (12, 14, True))
        # Ways 1 and 2 are one line, and the service way starts at zoom 13.
        self.assertEqual(west12, [(1, {"slope": 2}), (3, {"slope": 4}), (4, {"mtb": 3, "sac": 5, "trail": True}),
                                  (6, {"slope": 1, "trail": True})])
        self.assertIn((5, {"slope": 0}), west13)
        # The tile of the east block draws the stretch from its own row, once.
        self.assertEqual(east12, [(6, {"slope": 1, "trail": True})])
        self.assertEqual(sorted(metrics["largest_bytes"]), [12, 13, 14])


if __name__ == "__main__":
    unittest.main()
