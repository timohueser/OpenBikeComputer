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

    def test_a_tile_without_data_has_no_bands(self):
        empty = gzip.compress(tile((snow.NO_DATA, snow.NO_DATA)))
        self.assertEqual(snow.encode((7, empty, 1)), (7, None))
        tile_id, bodies = snow.encode((7, gzip.compress(tile((snow.NO_SNOW, snow.NO_SNOW))), 1))
        self.assertEqual(len(bodies), snow.BANDS)
        self.assertEqual(set(gzip.decompress(bodies[0])), {16})

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
