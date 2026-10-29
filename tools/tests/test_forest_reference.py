"""The forest reference counts one stem per visible tree crown of a canopy height model."""

import unittest

import numpy as np

from tools import forest_reference as forest


class TreeTopTest(unittest.TestCase):
    def test_each_crown_of_a_synthetic_canopy_is_one_stem_and_a_bush_is_none(self):
        pixel, side = 0.5, 200
        rng = np.random.default_rng(7)
        rows, cols = np.mgrid[0:side, 0:side] * pixel + pixel / 2
        heights = np.zeros((side, side), np.float32)
        # Crowns 9 m apart do not touch: a spruce cone, a flat broadleaved crown and a bush of 3 m,
        # each on a pixel centre.
        trees = []
        for i, north in enumerate(np.arange(4.75, 100, 9)):
            for j, east in enumerate(np.arange(4.75, 100, 9)):
                height = 3.0 if (i + j) % 7 == 0 else rng.uniform(8, 38)
                distance = np.hypot(rows - north, cols - east)
                crown = height * (1 - distance / min(0.8 + 0.1 * height, 4.4))
                if (i + j) % 3 == 0:
                    crown = np.minimum(crown, height - 1.0)
                heights = np.maximum(heights, crown + rng.normal(0, 0.05, crown.shape) * (crown > 0))
                trees.append((north, east, height))
        tops = forest.tree_tops(heights, pixel)
        stems = [(n, e) for n, e, h in trees if h >= forest.TOP_M]
        self.assertEqual(int(tops.sum()), len(stems))
        # Four cells of 50 m: the stems per hectare of each are its trees over a quarter hectare.
        cells = (np.arange(side)[:, None] // 100) * 2 + np.arange(side)[None, :] // 100
        sums = forest.totals(4)
        forest.accumulate(cells, heights, tops, *sums)
        per_cell = forest.cell_stats(sums, pixel)[2]
        expected = [sum(1 for n, e in stems if (n >= 50) * 2 + (e >= 50) == cell) / 0.25 for cell in range(4)]
        self.assertEqual(per_cell.tolist(), expected)

    def test_a_top_within_the_crown_radius_of_a_higher_one_and_a_flat_crown_are_no_second_stem(self):
        pixel, side = 0.5, 60
        rows, cols = np.mgrid[0:side, 0:side] * pixel + pixel / 2

        def stems(*crowns):
            """Tree tops of round crowns (north, east, height, radius, flat): a paraboloid, or a
            flat top that falls 1 m in height per 1 m outside its radius."""
            heights = np.zeros((side, side), np.float32)
            for north, east, height, radius, flat in crowns:
                distance = np.hypot(rows - north, cols - east)
                crown = np.where(distance <= radius, height, height * (radius + 1 - distance)) if flat else \
                    height * (1 - (distance / radius) ** 2)
                heights = np.maximum(heights, crown)
            return int(forest.tree_tops(heights, pixel).sum())

        # The smoothed apexes are about 16 m high, with a crown radius of about 1.8 m: a lower apex
        # 1.5 m away is no stem, one 3 m away is. Each apex is a maximum of its 3 × 3 neighbours.
        self.assertEqual(stems((15.25, 13.25, 20, 1.2, False), (15.25, 14.75, 19, 1.2, False)), 1)
        self.assertEqual(stems((15.25, 13.25, 20, 1.2, False), (15.25, 16.25, 19, 1.2, False)), 2)
        # A flat crown wider than the crown radius is one tree.
        self.assertEqual(stems((15.25, 15.25, 25, 5.0, True)), 1)


class BlockTest(unittest.TestCase):
    def test_a_block_holds_each_named_tile_at_its_place_and_a_cell_takes_the_first_model_with_a_pixel(self):
        import argparse
        import hashlib
        import tempfile
        import unittest.mock
        from pathlib import Path

        import rasterio

        side, nodata = forest.SIZE, forest.NODATA
        national = np.full((3, side, side), 7, np.uint16)
        national[:, :, :128] = nodata

        def tile_ch(x, y):
            return national if (x, y) == (2143, 1443) else None

        def tile_global(x, y):
            return np.full((3, side, side), x - 2140 + 4 * (y - 1440), np.uint16)

        with tempfile.TemporaryDirectory() as directory, \
                unittest.mock.patch.dict(forest.TILES, {"ch": tile_ch, "global": tile_global}), \
                unittest.mock.patch.object(forest, "leaf_type", lambda x, y: 60):
            args = argparse.Namespace(block="10-535-360", out=directory,
                                      tiles=["12-2143-1443=ch,global", "12-2140-1440=global"])
            self.assertEqual(forest.command_fetch(args), 0)
            path = Path(directory) / "10/535/360.tif"
            with rasterio.open(path) as block:
                bands = block.read()
                self.assertEqual(block.shape, (4 * side, 4 * side))
            digest = hashlib.sha256(bands.astype("<u2").tobytes()).hexdigest()
            self.assertEqual((Path(directory) / "10-535-360.sha256").read_text().strip(), digest)
            self.assertRaises(SystemExit, forest.command_fetch,
                              argparse.Namespace(block="10-535-360", out=directory, tiles=["12-2144-1443=global"]))
        # The south-east tile takes `ch` where it has a pixel and `global` elsewhere; the north-west
        # tile is `global`; a tile that no argument names is unknown.
        # Tile x, y of block 535/360 is the internal tile (x - 4 * 535, y - 4 * 360).
        cells = lambda x, y: bands[:, (y - 1440) * side:(y - 1439) * side, (x - 2140) * side:(x - 2139) * side]
        south_east = cells(2143, 1443)
        self.assertEqual((south_east[0, 0, 0], south_east[0, 0, 200], south_east[3, 0, 0]), (15, 7, 60))
        self.assertEqual(int(cells(2140, 1440)[0, 5, 5]), 0)
        self.assertTrue((cells(2141, 1440) == nodata).all())

    def test_a_dlt_tile_that_clms_does_not_publish_is_no_leaf_type_when_gdal_reports_a_plain_404(self):
        import argparse
        import tempfile
        import unittest.mock
        from pathlib import Path

        import rasterio
        from rasterio.errors import RasterioIOError

        from tools import planner_snow

        real_open, opened = rasterio.open, []

        def open_dlt(path, *args, **kwargs):
            """GDAL answers the first open of a missing key NoSuchKey and a later open a plain 404."""
            if not str(path).startswith("/vsis3/"):
                return real_open(path, *args, **kwargs)
            opened.append(path)
            raise RasterioIOError("HTTP response code: 404" if len(opened) > 1 else "ObjectNotFound: NoSuchKey")

        canopy = np.full((3, forest.SIZE, forest.SIZE), 50, np.uint16)
        with tempfile.TemporaryDirectory() as directory, \
                unittest.mock.patch.dict(forest.TILES, {"global": lambda x, y: canopy}), \
                unittest.mock.patch.object(forest, "ABSENT", set()), \
                unittest.mock.patch.object(planner_snow, "cdse_credentials"), \
                unittest.mock.patch.object(forest.time, "sleep", side_effect=AssertionError("retried")), \
                unittest.mock.patch("rasterio.open", open_dlt):
            # Two tiles of Flores in the Azores, both in the DLT tile of easting 9.
            args = argparse.Namespace(block="10-423-389", out=directory,
                                      tiles=["12-1692-1556=global", "12-1693-1556=global"])
            self.assertEqual(forest.command_fetch(args), 0)
            with real_open(Path(directory) / "10/423/389.tif") as block:
                leaf = block.read(4)
        self.assertEqual(len(opened), 1)
        self.assertIn("_E09N27_", opened[0])
        self.assertTrue((leaf == forest.NODATA).all())


if __name__ == "__main__":
    unittest.main()
