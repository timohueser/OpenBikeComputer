"""The canopy tiles hold the bits of specs/trailview-tiles.md, losslessly."""

import io
import unittest

import numpy as np
from PIL import Image

from tools import planner_trailview_canopy as canopy

NODATA = canopy.NODATA


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
        self.assertEqual(code.ravel().tolist(), [spruce, beech, sparse, 0, 0])
        rgb = np.asarray(Image.open(io.BytesIO(canopy.webp(code))).convert("RGB")).astype(np.uint16)
        self.assertEqual((rgb[..., 0] | rgb[..., 1] << 8).tolist(), code.tolist())
        self.assertFalse(rgb[..., 2].any())


if __name__ == "__main__":
    unittest.main()
