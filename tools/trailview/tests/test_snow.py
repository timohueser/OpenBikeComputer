"""The snow days hold, per pixel and band, the seasons with data and the seasons with snow on the band's
day step, as specs/trailview-tiles.md defines them."""

import gzip
import tempfile
import unittest
from pathlib import Path

import numpy as np

from tools.trailview import snow


def tile(*seasons):
    """A snow tile whose every pixel has the (onset, melt-out) of each season: per season an onset plane,
    then a melt-out plane."""
    return np.repeat(np.array(seasons, np.uint8).reshape(-1), snow.AREA).tobytes()


class SnowDaysTest(unittest.TestCase):
    def test_a_band_counts_the_seasons_with_data_and_with_snow_on_its_day_step(self):
        # Snow from step 10 to 50; no data; snow all season.
        days = snow.days(tile((10, 50), (snow.NO_DATA, snow.NO_DATA), (snow.WHOLE_SEASON, snow.WHOLE_SEASON)), 3)
        self.assertEqual(days.shape, (snow.BANDS, snow.AREA))
        self.assertEqual([snow.day_step(band) for band in (2, 12, 45)], [10, 50, 182])
        self.assertEqual(days[2, 0], 2 * 16 + 2, "step 10 is the first day of snow")
        self.assertEqual(days[12, 0], 2 * 16 + 2, "step 50 is the last")
        self.assertEqual(days[13, 0], 2 * 16 + 1)
        self.assertEqual(days[0, 0], 2 * 16 + 1)

    def test_a_tile_joins_its_four_snow_tiles_and_a_tile_without_data_has_no_bands(self):
        empty = gzip.compress(tile((snow.NO_DATA, snow.NO_DATA)))
        self.assertEqual(snow.encode((7, [empty, None, None, None], 1)), (7, None))
        tile_id, bodies = snow.encode((7, [None, gzip.compress(tile((snow.NO_SNOW, snow.NO_SNOW))), None, None], 1))
        self.assertEqual(len(bodies), snow.BANDS)
        plane = np.frombuffer(gzip.decompress(bodies[0]), np.uint8).reshape(snow.SIZE, snow.SIZE)
        self.assertTrue((plane[:256, 256:] == 16).all(), "the north-east tile has one season with data")
        self.assertEqual(int(plane[:256, :256].max()) + int(plane[256:].max()), 0)

    def test_the_snow_tiles_in_tile_order_join_under_their_parents_in_tile_order(self):
        from pmtiles.tile import zxy_to_tileid

        found = sorted([(0, 0, 0), (1, 0, 0), (1, 1, 1), (2, 3, 1), (2, 2, 0), (2, 1, 1)], key=lambda zxy: zxy_to_tileid(*zxy))
        joined = [(tile_id, bodies) for tile_id, bodies, _ in snow.parents(((zxy, zxy) for zxy in found), 1)]
        self.assertEqual(joined, [
            (zxy_to_tileid(0, 0, 0), [(1, 0, 0), None, None, (1, 1, 1)]),
            (zxy_to_tileid(1, 0, 0), [None, None, None, (2, 1, 1)]),
            (zxy_to_tileid(1, 1, 0), [(2, 2, 0), None, None, (2, 3, 1)]),
        ])

    def test_a_snow_archive_without_data_fails_the_bake(self):
        from pmtiles.tile import Compression, TileType
        from pmtiles.writer import Writer

        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "snow.pmtiles"
            with path.open("wb") as stream:
                writer = Writer(stream)
                writer.write_tile(0, gzip.compress(tile((snow.NO_DATA, snow.NO_DATA))))
                writer.finalize({"tile_type": TileType.UNKNOWN, "tile_compression": Compression.GZIP}, {"step_days": 2, "seasons": 1})
            with self.assertRaisesRegex(ValueError, "no tile with data"):
                snow.bake(path, Path(folder) / "out", workers=1)


if __name__ == "__main__":
    unittest.main()
