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
        per_cell = forest.cell_stats(cells, heights, tops, 4)[2]
        expected = [sum(1 for n, e in stems if (n >= 50) * 2 + (e >= 50) == cell) / 0.25 for cell in range(4)]
        self.assertEqual(per_cell.tolist(), expected)


if __name__ == "__main__":
    unittest.main()
