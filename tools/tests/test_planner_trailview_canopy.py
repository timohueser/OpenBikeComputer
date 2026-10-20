"""The canopy tiles hold the bits of specs/trailview-tiles.md, losslessly."""

import io
import tempfile
import unittest
from pathlib import Path

import numpy as np
from PIL import Image

from tools import forest_reference, planner_trailview_canopy as canopy

NODATA = canopy.NODATA


def decoded(webp):
    rgb = np.asarray(Image.open(io.BytesIO(webp)).convert("RGB")).astype(np.uint16)
    return rgb[..., 0] | rgb[..., 1] << 8, rgb[..., 2]


class CanopyCodeTest(unittest.TestCase):
    def test_a_cell_packs_its_height_cover_stems_class_and_leaf_type_and_the_webp_keeps_the_code(self):
        # height dm, cover %, stems per ha, conifer %: a spruce stand, a beech stand, a mixed stand
        # without a leaf type sample, a meadow and a cell that no reference knows.
        cells = np.array([[253, 64, 220, 80], [312, 95, 40, 10], [41, 5, 600, NODATA], [0, 0, 0, NODATA],
                          [NODATA] * 4], np.uint16).T.reshape(4, 1, 5)
        code = canopy.encode(cells)
        spruce = 51 | 6 << 7 | 4 << 11 | 2 << 14
        beech = 62 | 10 << 7 | 0 << 11 | 1 << 14
        sparse = 8 | 1 << 7 | 7 << 11 | 0 << 14
        self.assertEqual(code.ravel().tolist(), [spruce, beech, sparse, 0, canopy.UNKNOWN])
        values, blue = decoded(canopy.webp(code))
        self.assertEqual(values.tolist(), code.tolist())
        self.assertFalse(blue.any())

    def test_a_tile_of_open_land_is_in_the_archive_and_a_tile_without_a_reference_is_not(self):
        from pmtiles.reader import MmapSource, Reader

        x, y = 2143, 1443
        bands = np.zeros((4, 256, 256), np.uint16)
        bands[:, :128] = NODATA
        with tempfile.TemporaryDirectory() as directory:
            reference = Path(directory) / "12/2143/1443.tif"
            forest_reference.write_tile(reference, x, y, bands)
            west, south, east, north = 8.33, 46.80, 8.45, 46.86
            canopy.bake([{(x, y): reference}], [west, south, east, north], "Test", Path(directory))
            with open(Path(directory) / "trailview-canopy.pmtiles", "rb") as stream:
                reader = Reader(MmapSource(stream))
                code, _ = decoded(reader.get(12, x, y))
                self.assertIsNone(reader.get(12, x - 1, y), "no reference has the tile west of it")
        self.assertEqual(np.unique(code[:128]).tolist(), [canopy.UNKNOWN])
        self.assertEqual(np.unique(code[128:]).tolist(), [0])


if __name__ == "__main__":
    unittest.main()
