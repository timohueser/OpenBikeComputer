"""The sun index bounds every bilinear patch and retains unknown terrain."""
import io
import unittest

import numpy as np
from PIL import Image
from tools import planner_sun as sun
from tools import planner_sun_horizons as horizons


class SunIndexTest(unittest.TestCase):
    def test_bounds_include_shared_far_edges_and_unknown_vertices(self):
        vertices = np.zeros((9, 9), np.int16)
        vertices[4, 4] = 1400
        self.assertEqual(sun.maxima(vertices, 4).tolist(), [[1400, 1400], [1400, 1400]])
        vertices[8, 8] = sun.UNKNOWN
        self.assertEqual(sun.maxima(vertices, 4).tolist(), [[1400, 1400], [1400, sun.UNKNOWN]])

    def test_bound_quantization_is_conservative_at_signed_extrema(self):
        values = np.array([-32768, -301, -1, 0, 1400, 32766, sun.UNKNOWN], np.int16)
        rounded = sun.quantize(values)
        self.assertTrue((rounded >= values).all())
        self.assertEqual(rounded[-1], sun.UNKNOWN)
        self.assertTrue((rounded[:-2].astype(np.int32) - values[:-2] < sun.BOUND_STEP).all())

    def test_signed_heights_and_the_unknown_sentinel_survive_webp(self):
        values = np.array([[-300, 0, 1], [1400, -32768, sun.UNKNOWN]], np.int16)
        rgba = np.asarray(Image.open(io.BytesIO(sun.encode(values))).convert('RGBA'))
        decoded = rgba[:, :, 0].astype(np.int32) * 256 + rgba[:, :, 1] - 32768
        np.testing.assert_array_equal(decoded, values)
        self.assertTrue((rgba[:, :, 2] == 0).all())
        self.assertTrue((rgba[:, :, 3] == 255).all())

    def test_horizon_delta_planes_round_trip_including_unknown_directions(self):
        values = np.random.default_rng(7).integers(0, 256, (8, 8, 72), dtype=np.uint8)
        np.testing.assert_array_equal(horizons.decode(horizons.encode(values), 8, 72), values)

    def test_overview_shares_a_tile_with_height_bounds_without_changing_either(self):
        values = np.random.default_rng(7).integers(0, 256, (32, 32, 72), dtype=np.uint8)
        heights = np.full((512, 512), -300, np.int16)
        heights[-1, -1] = sun.UNKNOWN
        bounds = np.asarray(Image.open(io.BytesIO(sun.encode(heights))).convert('RGBA'))
        data = horizons.encode(values, bounds)
        np.testing.assert_array_equal(horizons.decode(data, 32, 72), values)
        rgba = np.asarray(Image.open(io.BytesIO(data)).convert('RGBA'))
        np.testing.assert_array_equal(rgba[:512], bounds)
        self.assertEqual(rgba.shape, (560, 512, 4))

    def test_overview_averages_round_up_and_keep_unknown_children(self):
        children = np.array([[[0, 255], [1, 2]], [[1, 2], [3, 2]]], np.uint8)
        np.testing.assert_array_equal(horizons.reduce_grid(children), [[[2, 255]]])

    def test_horizon_includes_an_interior_peak_and_preserves_missing_terrain(self):
        arrays = np.array([0, 10, 10, 0] + [10] * 12, np.int16)
        origins = np.zeros((13, 5), np.int64)
        origins[0, 2:] = [2, 2, 0]
        for level in range(1, 13):
            origins[level, 2:] = [1, 1, level + 3]
        value = horizons.horizon(arrays, origins, 0., 0., 1., 1., 1.)
        expected = np.degrees(np.arctan(20 - 2 * np.sqrt(20)))
        self.assertLessEqual(expected, value * horizons.ANGLE_STEP)
        self.assertLess(value * horizons.ANGLE_STEP - expected, horizons.ANGLE_STEP)
        arrays[3] = horizons.UNKNOWN
        self.assertEqual(horizons.horizon(arrays, origins, 0., 0., 1., 1., 1.), 255)
