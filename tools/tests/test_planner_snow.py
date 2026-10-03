"""The snow bake finds the longest snow period of each season and writes the bytes of the snow tile spec."""

from pathlib import Path
import tempfile
import unittest

from affine import Affine
import numpy as np

from tools import planner_snow as snow

FIRST = 2001  # 2001/02 and 2002/03 have 365 days.


def seasons(observations, count=1, end=None):
    """Planes of pixels fed {day: [state per pixel]}, with None for a day that is not clear."""
    pixels = len(next(iter(observations.values())))
    state = snow.SnowSeasons(FIRST, count, (pixels,))
    for day in sorted(observations):
        values = observations[day]
        state.observe(day, np.array([v is not None for v in values]), np.array([bool(v) for v in values]))
    return state.planes(end if end is not None else state.bounds[-1])


class SeasonTest(unittest.TestCase):
    def test_gap_days_take_the_nearest_clear_day_and_the_earlier_one_on_a_tie(self):
        # Clear days 0, 9, 20 and 40: snow from day 5 (nearer to day 9) to day 30 (the tie goes to day 20).
        planes = seasons({0: [False], 9: [True], 20: [True], 40: [False]})
        self.assertEqual(planes[0, :, 0].tolist(), [5 // 2, 30 // 2])

    def test_the_longest_period_counts_and_the_earliest_one_on_equal_length(self):
        planes = seasons({d: [10 <= d < 20 or 100 <= d < 150, 10 <= d < 20 or 100 <= d < 110] for d in range(365)})
        self.assertEqual(planes[0, :, 0].tolist(), [50, 74])
        self.assertEqual(planes[0, :, 1].tolist(), [5, 9])

    def test_sentinels(self):
        # Never snow, always snow, never clear, and snow up to day 364 of a 365-day season.
        days = {d: [False, True, None, d < 364] for d in range(365)}
        planes = seasons(days)
        self.assertEqual(planes[0, 0].tolist(), [snow.NO_SNOW, snow.FULL, snow.NO_DATA, 0])
        self.assertEqual(planes[0, 1].tolist(), [snow.NO_SNOW, snow.FULL, snow.NO_DATA, 363 // 2])
        self.assertEqual(snow.encode(np.array([0]), np.array([366]), 366, np.array([True])).ravel().tolist(), [snow.FULL] * 2)
        self.assertEqual(snow.encode(np.array([360]), np.array([6]), 366, np.array([True])).ravel().tolist(), [180, 182])

    def test_a_period_across_a_season_boundary_counts_in_both_seasons(self):
        planes = seasons({300: [False], 350: [True], 400: [True], 420: [False]}, count=2)
        self.assertEqual(planes[:, :, 0].tolist(), [[326 // 2, 364 // 2], [0, (410 - 365) // 2]])

    def test_a_period_still_open_where_the_source_ends_is_no_data_if_it_can_still_be_the_longest(self):
        # The source ends on day 300. Pixel 0 melted out on day 200; pixel 1 still has snow.
        days = {d: [50 <= d < 200, 50 <= d] for d in range(300)}
        planes = seasons(days, end=300)
        self.assertEqual(planes[0, :, 0].tolist(), [25, 99])
        self.assertEqual(planes[0, :, 1].tolist(), [snow.NO_DATA] * 2)


class BlendTest(unittest.TestCase):
    def blend(self, values, weights):
        """Blend single-season pixels given as (onset, melt-out) pairs."""
        array = np.array(values, np.uint8).reshape(len(values), 1, 2, 1)
        return snow.blend(array, np.array(weights, np.float32).reshape(-1, 1))[0, :, 0].tolist()

    def test_dates_come_only_from_dated_inputs(self):
        self.assertEqual(self.blend([(10, 20), (12, 30), (254, 254)], [0.25, 0.25, 0.5]), [11, 25])
        self.assertEqual(self.blend([(10, 20), (253, 253), (255, 255)], [0.3, 0.4, 0.3]), [253, 253])

    def test_no_data_needs_more_than_half_the_weight(self):
        self.assertEqual(self.blend([(10, 20), (255, 255)], [0.5, 0.5]), [10, 20])
        self.assertEqual(self.blend([(10, 20), (255, 255)], [0.4, 0.6]), [255, 255])

    def test_ties_prefer_dated_then_full_season(self):
        self.assertEqual(self.blend([(10, 20), (253, 253)], [0.5, 0.5]), [10, 20])
        self.assertEqual(self.blend([(254, 254), (253, 253)], [0.5, 0.5]), [254, 254])

    def test_parent_pixels_blend_their_two_by_two_children(self):
        child = np.full((1, 2, 256, 256), 40, np.uint8)
        child[:, :, 0, 1] = snow.NO_DATA
        child[:, :, 1, 0] = 41
        tile = snow.parent({(1, 0): child, (0, 0): None, (0, 1): None, (1, 1): None}, 1)
        self.assertEqual(tile[0, :, 0, 128].tolist(), [40, 40])  # (40 + 40 + 41) / 3 = 40.33
        self.assertEqual(tile[0, 0, 0, 0], snow.NO_DATA)

    def test_smoothing_takes_the_majority_class_and_the_median_day(self):
        tile = np.full((1, 2, 256, 256), 40, np.uint8)
        tile[:, :, 10, 10] = snow.NO_SNOW
        tile[:, :, 100, 100] = 120
        tile[:, :, 101, 100] = 41
        smoothed = snow.smooth(tile)
        self.assertEqual(smoothed[0, :, 10, 10].tolist(), [40, 40])
        self.assertEqual(smoothed[0, :, 100, 100].tolist(), [40, 40])
        self.assertEqual(smoothed[0, :, 0, 0].tolist(), [40, 40])

    def test_the_max_zoom_has_the_pixel_size_nearest_to_the_source_resolution(self):
        alps, equator = [9.5, 46.3, 10.3, 46.7], [0, -0.1, 1, 0.1]
        self.assertEqual([snow.max_zoom(20, alps), snow.max_zoom(500, alps)], [12, 8])
        # 19.1 m and 38.2 m pixels: 20 m is nearer to zoom 13 at the equator.
        self.assertEqual(snow.max_zoom(20, equator), 13)

    def test_tile_rows_run_from_north_to_south(self):
        west, south, east, north = snow.tile_bounds(9, 268, 179)
        rows = np.full((1, 2, 40, 40), snow.NO_SNOW, np.uint8)
        rows[:, :, :20] = 50
        transform = Affine.translation(west, north) @ Affine.scale((east - west) / 40, -(north - south) / 40)
        tile = snow.sample(rows, snow.Grid("EPSG:4326", transform, (40, 40)), 9, 268, 179)
        self.assertEqual(tile[0, 0, 0, 128], 50)
        self.assertEqual(tile[0, 0, 255, 128], snow.NO_SNOW)


class BakeTest(unittest.TestCase):
    def test_the_bake_reads_one_zoom_9_chunk_and_stores_to_the_max_zoom(self):
        from pmtiles.reader import MmapSource, all_tiles

        # One zoom-11 tile, so no tile has pixels outside the bounds.
        bounds = list(snow.tile_bounds(11, 1079, 724))
        west, north = 9.6, 46.6
        grid = snow.Grid("EPSG:4326", Affine.translation(west, north) @ Affine.scale(0.001, -0.001), (300, 300))
        chunks = []

        def source(chunk):
            chunks.append(chunk)
            return np.full((2, 2, 300, 300), 60, np.uint8), grid

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "snow.pmtiles"
            snow.bake(source, 2016, 2, "copernicus-hr-wsi", bounds, output)
            with output.open("rb") as file:
                stored = {z for (z, _, _), _ in all_tiles(MmapSource(file))}
        self.assertEqual(len(chunks), 1)
        self.assertEqual(max(stored), 12)


class CopernicusTest(unittest.TestCase):
    def test_yearly_rasters_become_consecutive_season_planes(self):
        import rasterio
        from rasterio.warp import transform

        # Four 20 m pixels: dated, no snow, glacier and inland water (420).
        layers = {"SCO": [30, 0, 0, 420], "SCM": [250, 0, 364, 420], "SCD": [200, 0, 365, 420]}
        (x,), (y,) = transform("EPSG:4326", "EPSG:3035", [9.9], [46.5])
        x, y = round(x / 20) * 20, round(y / 20) * 20
        with tempfile.TemporaryDirectory() as directory:
            files = {}
            for name, values in layers.items():
                path = Path(directory) / f"{name}.tif"
                files[name] = [path]
                with rasterio.open(path, "w", driver="GTiff", width=4, height=1, count=1, dtype="uint16", crs="EPSG:3035",
                                   transform=Affine(20, 0, x, 0, -20, y), nodata=65535) as dst:
                    dst.write(np.array([values], np.uint16), 1)
            lon, lat = transform("EPSG:3035", "EPSG:4326", [x + 40], [y - 10])
            bounds = [lon[0] - 0.01, lat[0] - 0.01, lon[0] + 0.01, lat[0] + 0.01]
            planes, grid = snow.copernicus_planes({2021: files, 2023: files}, bounds, range(2021, 2024))
        self.assertEqual(planes.shape[0], 3)
        col, row = ~grid.transform @ (x + 10, y - 10)
        values = planes[0, :, int(row), int(col):int(col) + 4]
        self.assertEqual(values.T.tolist(), [[15, 125], [snow.NO_SNOW] * 2, [snow.FULL] * 2, [snow.NO_DATA] * 2])
        self.assertTrue((planes[1] == snow.NO_DATA).all())


if __name__ == "__main__":
    unittest.main()
