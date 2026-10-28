"""The snow tiles find the longest snow period of each season and write the bytes of the snow tile spec."""

from pathlib import Path
import datetime as dt
import gzip
import tempfile
import unittest
from unittest import mock

from affine import Affine
import numpy as np

from tools import planner_snow as snow


def season(observations, days=365):
    """Planes of pixels fed {day: [state per pixel]}, with None for a day that is not clear."""
    pixels = len(next(iter(observations.values())))
    state = snow.SnowSeason(days, (pixels,))
    for day in sorted(observations):
        values = observations[day]
        state.observe(day, np.array([v is not None for v in values]), np.array([bool(v) for v in values]))
    return state.planes()[0]


def tile_id(z, x, y):
    from pmtiles.tile import zxy_to_tileid

    return zxy_to_tileid(z, x, y)


def read_archive(path):
    """{(z, x, y): body as (seasons, 2, 256, 256)} and the metadata of an archive."""
    from pmtiles.reader import MmapSource, Reader, all_tiles

    with path.open("rb") as file:
        reader = Reader(MmapSource(file))
        seasons = reader.metadata().get("seasons", 1)
        tiles = {zxy: np.frombuffer(gzip.decompress(data), np.uint8).reshape(seasons, 2, 256, 256)
                 for zxy, data in all_tiles(MmapSource(file))}
        return tiles, reader.metadata()


class SeasonTest(unittest.TestCase):
    def test_gap_days_take_the_nearest_clear_day_and_the_earlier_one_on_a_tie(self):
        # Clear days 0, 9, 20 and 40: snow from day 5 (nearer to day 9) to day 30 (the tie goes to day 20).
        self.assertEqual(season({0: [False], 9: [True], 20: [True], 40: [False]})[:, 0].tolist(), [5 // 2, 30 // 2])

    def test_the_longest_period_counts_and_the_earliest_one_on_equal_length(self):
        planes = season({d: [10 <= d < 20 or 100 <= d < 150, 10 <= d < 20 or 100 <= d < 110] for d in range(365)})
        self.assertEqual(planes[:, 0].tolist(), [50, 74])
        self.assertEqual(planes[:, 1].tolist(), [5, 9])

    def test_sentinels(self):
        # Never snow, always snow, never clear, and snow up to day 364 of a 365-day season.
        planes = season({d: [False, True, None, d < 364] for d in range(365)})
        self.assertEqual(planes[0].tolist(), [snow.NO_SNOW, snow.FULL, snow.NO_DATA, 0])
        self.assertEqual(planes[1].tolist(), [snow.NO_SNOW, snow.FULL, snow.NO_DATA, 363 // 2])
        self.assertEqual(snow.encode(np.array([0]), np.array([366]), 366, np.array([True])).ravel().tolist(), [snow.FULL] * 2)
        self.assertEqual(snow.encode(np.array([360]), np.array([6]), 366, np.array([True])).ravel().tolist(), [180, 182])

    def test_a_season_reads_its_own_days_only(self):
        # Days before the first clear day and after the last one take the state of that day.
        self.assertEqual(season({50: [True], 300: [True]})[:, 0].tolist(), [snow.FULL, snow.FULL])
        self.assertEqual(season({50: [True], 100: [False]})[:, 0].tolist(), [0, 74 // 2])


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

    def test_tile_rows_run_from_north_to_south(self):
        west, south, east, north = snow.geo.tile_bounds(9, 268, 179)
        rows = np.full((1, 2, 40, 40), snow.NO_SNOW, np.uint8)
        rows[:, :, :20] = 50
        transform = Affine.translation(west, north) @ Affine.scale((east - west) / 40, -(north - south) / 40)
        tile = snow.sample(rows, snow.Grid("EPSG:4326", transform, (40, 40)), 9, 268, 179)
        self.assertEqual(tile[0, 0, 0, 128], 50)
        self.assertEqual(tile[0, 0, 255, 128], snow.NO_SNOW)


class BlockTest(unittest.TestCase):
    def test_a_season_has_hr_wsi_tiles_where_it_has_data_and_modis_below_zoom_9(self):
        # Two zoom 12 tiles of block 33, 22; HR-WSI covers the west half of the first only.
        first, second = (2158, 1448), (2159, 1448)
        bounds = [*snow.geo.tile_bounds(12, *first)[:2], *snow.geo.tile_bounds(12, *second)[2:]]
        grid = snow.Grid("EPSG:4326", Affine.translation(9.6, 46.6) @ Affine.scale(0.001, -0.001), (300, 300))
        middle = round(((snow.geo.tile_bounds(12, *first)[0] + snow.geo.tile_bounds(12, *first)[2]) / 2 - 9.6) / 0.001)
        hr = np.full((1, 2, 300, 300), snow.NO_DATA, np.uint8)
        hr[:, :, :, :middle] = 60
        modis = np.full((1, 2, 300, 300), 100, np.uint8), grid
        making = snow.Making({"SCO": ["x"]}, modis, 365, bounds, 2016)
        with tempfile.TemporaryDirectory() as out, \
                mock.patch.object(snow, "copernicus_season", return_value=(hr, grid)):
            sha256 = snow.make_season((33, 22), making, Path(out))
            path = Path(out, "2016/33/22.pmtiles")
            tiles, metadata = read_archive(path)
            self.assertEqual(len(sha256), 64)
        self.assertEqual(metadata, {"season": 2016})
        self.assertNotIn((12, *second), tiles, "a zoom 12 tile without HR-WSI is not in the season")
        top = tiles[(12, *first)][0, 0]
        self.assertTrue((top[:, :100] == 60).all() and (top[:, -100:] == 100).all(), "MODIS fills the tile")
        self.assertEqual({z for z, _, _ in tiles}, set(range(6, 13)))
        self.assertTrue((tiles[(8, 134, 90)] != snow.NO_DATA).any())

    def test_a_block_whose_catalogue_lacks_a_season_fails(self):
        with mock.patch.object(snow, "cdse_credentials"), \
                mock.patch.object(snow, "copernicus_files", return_value={2016: {"SCO": ["x"]}}):
            with self.assertRaisesRegex(SystemExit, r"none of \[2017\]"):
                snow.prepare((33, 22), [2016, 2017], None, None, Path("/nonexistent"))


class JoinTest(unittest.TestCase):
    def test_a_season_without_a_tile_takes_the_part_of_its_ancestor_and_zoom_0_to_5_blend_zoom_6(self):
        # Season 2016 has the zoom 12 tile 2158, 1448; season 2017 has only its zoom 11 parent, whose
        # north-west quarter, the part over that tile, is 30 and the rest 90.
        bounds = snow.geo.tile_bounds(12, 2158, 1448)
        full = lambda value: np.full((1, 2, 256, 256), value, np.uint8)
        quarters = full(90)
        quarters[:, :, :128, :128] = 30
        seasons = {2016: {(12, 2158, 1448): full(60), (11, 1079, 724): full(60), (6, 33, 22): full(60)},
                   2017: {(11, 1079, 724): quarters, (6, 33, 22): full(50)}}
        with tempfile.TemporaryDirectory() as directory:
            files = {}
            for year, held in seasons.items():
                path = Path(directory, f"{year}/33/22.pmtiles")
                snow.write_archive(path, {tile_id(*zxy): snow.gzipped(body) for zxy, body in held.items()},
                                   snow.geo.tile_bounds(6, 33, 22), {"season": year})
                files[f"{year}/33/22.pmtiles"] = path
            # The file of a season without data in the block is empty.
            files["2018/33/22.pmtiles"] = Path(directory, "2018.pmtiles")
            files["2018/33/22.pmtiles"].write_bytes(b"")
            output = Path(directory, "snow.pmtiles")
            count = snow.join(files, bounds, [2016, 2017, 2018], output, "HR-WSI")
            tiles, metadata = read_archive(output)
        self.assertEqual(metadata["source"], "copernicus-hr-wsi")
        self.assertEqual((metadata["first_season"], metadata["seasons"]), (2016, 3))
        self.assertTrue((tiles[(12, 2158, 1448)][0] == 60).all())
        self.assertTrue((tiles[(12, 2158, 1448)][1] == 30).all(), "the quarter of the parent, each pixel doubled")
        self.assertEqual(sorted(z for z, _, _ in tiles), [0, 1, 2, 3, 4, 5, 6, 11, 12])
        self.assertEqual(count, len(tiles))
        self.assertEqual(tiles[(5, 16, 11)][:, 0, 64, 192].tolist(), [60, 50, snow.NO_DATA], "block 33, 22 is the north-east child")


class ModisTest(unittest.TestCase):
    def test_a_day_takes_its_cog_else_its_granule_and_dense_canopy_is_no_data(self):
        import rasterio
        from pyhdf.SD import SD, SDC

        grid = snow.modis_grid([8.30, 46.50, 8.34, 46.53])
        rows, cols = grid.shape
        size = grid.transform.a
        h, v = round((grid.transform.c - snow.MODIS_X0) / size) // 2400, round((snow.MODIS_Y0 - grid.transform.f) / size) // 2400

        def raster(path, values, transform):
            with rasterio.open(path, "w", driver="GTiff", width=values.shape[1], height=values.shape[0], count=1,
                               dtype="uint8", crs=snow.SINUSOIDAL, transform=transform) as dst:
                dst.write(values, 1)
            return str(path)

        def granule(path, value):
            hdf = SD(str(path), SDC.WRITE | SDC.CREATE)
            band = hdf.create("NDSI_Snow_Cover", SDC.UINT8, (2400, 2400))
            band[:] = np.full((2400, 2400), value, np.uint8)
            band.endaccess()
            hdf.end()
            return str(path)

        name = lambda platform, day, kind: f"{platform}10A1.A2016{day:03d}.h{h:02d}v{v:02d}.061.1{kind}"
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            # Day 336 (1 December): a COG with snow, which wins over the bare granule of the day. Day 244
            # (31 August 2017 is day 243): Terra is cloud in its granule, and Aqua sees no snow.
            snowy = raster(directory / name("MOD", 336, "_NDSI_Snow_Cover.tif"), np.full((rows, cols), 50, np.uint8), grid.transform)
            paths = [snowy, granule(directory / name("MOD", 336, ".hdf"), 0)]
            paths += [granule(directory / f"MOD10A1.A2017243.h{h:02d}v{v:02d}.061.1.hdf", 250),
                      granule(directory / f"MYD10A1.A2017243.h{h:02d}v{v:02d}.061.1.hdf", 0)]
            days = snow.modis_files(paths, grid)
            self.assertEqual(days[dt.date(2016, 12, 1)], [("MOD", snowy)])
            dense = np.zeros(grid.shape, bool)
            dense[-1] = True
            with snow.step_pool.pool(2) as pool:
                planes = snow.modis_season(days, grid, 2016, dense, pool)
        # Snow from day 0 to day 227: the last day nearer to the snow day 91 than to the bare day 364.
        self.assertEqual(planes[0, :, 0, 0].tolist(), [0, 227 // 2])
        self.assertTrue((planes[0, :, -1] == snow.NO_DATA).all())


class CopernicusTest(unittest.TestCase):
    def test_yearly_rasters_become_season_planes(self):
        import rasterio
        from rasterio.warp import transform

        # Four 20 m pixels: dated, no snow, glacier and inland water (420).
        layers = {"SCO": [30, 0, 0, 420], "SCM": [250, 0, 364, 420], "SCD": [200, 0, 365, 420]}
        (x,), (y,) = transform("EPSG:4326", "EPSG:3035", [9.9], [46.5])
        x, y = round(x / 20) * 20, round(y / 20) * 20
        with tempfile.TemporaryDirectory() as directory:
            files = {}
            for name, values in layers.items():
                path = str(Path(directory) / f"{name}.tif")
                files[name] = [path]
                with rasterio.open(path, "w", driver="GTiff", width=4, height=1, count=1, dtype="uint16", crs="EPSG:3035",
                                   transform=Affine(20, 0, x, 0, -20, y), nodata=65535) as dst:
                    dst.write(np.array([values], np.uint16), 1)
            lon, lat = transform("EPSG:3035", "EPSG:4326", [x + 40], [y - 10])
            bounds = [lon[0] - 0.01, lat[0] - 0.01, lon[0] + 0.01, lat[0] + 0.01]
            planes, grid = snow.copernicus_season(files, bounds, 365)
            # A catalogue footprint can touch a chunk that the raster itself does not reach.
            outside, _ = snow.copernicus_season(files, [b + 0.1 for b in bounds], 365)
        self.assertTrue((outside == snow.NO_DATA).all())
        col, row = ~grid.transform @ (x + 10, y - 10)
        values = planes[0, :, int(row), int(col):int(col) + 4]
        self.assertEqual(values.T.tolist(), [[15, 125], [snow.NO_SNOW] * 2, [snow.FULL] * 2, [snow.NO_DATA] * 2])


if __name__ == "__main__":
    unittest.main()
