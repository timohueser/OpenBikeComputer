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


if __name__ == "__main__":
    unittest.main()
