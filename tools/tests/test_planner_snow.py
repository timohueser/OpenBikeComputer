"""The snow tiles find the longest snow period of each season and write the bytes of the snow tile spec."""

from pathlib import Path
import datetime as dt
import gzip
import io
import tempfile
import unittest
from unittest import mock

from affine import Affine
import numpy as np
from PIL import Image

from tools import planner_snow as snow, planner_verify


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


def read_archive(path, size=256):
    """{(z, x, y): body as (seasons, 2, size, size)} and the metadata of an archive of gzip planes; the bodies as they
    are for an archive of WebP images."""
    from pmtiles.reader import MmapSource, Reader, all_tiles
    from pmtiles.tile import TileType

    with path.open("rb") as file:
        reader = Reader(MmapSource(file))
        seasons = reader.metadata().get("seasons", 1)
        planes = lambda data: np.frombuffer(gzip.decompress(data), np.uint8).reshape(seasons, 2, size, size)
        webp = reader.header()["tile_type"] == TileType.WEBP
        tiles = {zxy: data if webp else planes(data) for zxy, data in all_tiles(MmapSource(file))}
        return tiles, reader.metadata()


def pixels(image):
    """The red and the green (rows, cols, 2) of a WebP image, and that blue is 0."""
    rgb = np.asarray(Image.open(io.BytesIO(image)).convert("RGB"))
    assert (rgb[..., 2] == 0).all()
    return rgb[..., :2]


def full(value, size=256):
    return np.full((1, 2, size, size), value, np.uint8)


def season_files(directory, seasons):
    """The files of block 33, 22 of `seasons` {season: {(z, x, y): planes (1, 2, 256, 256)}}. A file holds each
    ancestor of its tiles down to zoom 6, as a prepared file does: 100 where the fixture names none. A season of no
    tiles has an empty file."""
    files = {}
    for season, held in seasons.items():
        path = Path(directory, f"{season}/33/22.pmtiles")
        tiles = dict(held)
        for z, x, y in held:
            for k in range(1, z - snow.BLOCK_ZOOM + 1):
                tiles.setdefault((z - k, x >> k, y >> k), full(100))
        if tiles:
            snow.write_archive(path, {tile_id(*zxy): snow.gzipped(body) for zxy, body in tiles.items()},
                               snow.geo.tile_bounds(6, 33, 22), {"season": season})
        else:
            path.parent.mkdir(parents=True)
            path.write_bytes(b"")
        files[f"{season}/33/22.pmtiles"] = path
    return files


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


class TileTest(unittest.TestCase):
    def test_each_level_keeps_its_longest_run_of_bands_and_the_earliest_on_a_tie(self):
        self.assertEqual([snow.day_step(band) for band in (0, 22, 45)], [2, 90, 182])
        none, no_snow, full_season = (255, 255), (253, 253), (254, 254)
        pixels = [
            # Snow from band 2 to 12 in two seasons, from 7 to 22 in one, none in the fourth: k = 4.
            [(10, 50), (10, 50), (30, 90), no_snow],
            [none] * 4,
            # Two runs of three bands, 2 to 4 and 12 to 14, in two seasons.
            [(10, 18), (50, 58), none, none],
            # A run of two bands, then a longer one.
            [(10, 14), (50, 62), none, none],
            [full_season, no_snow, none, none],
        ]
        found = snow.runs(np.array(pixels, np.uint8).transpose(1, 2, 0))
        expected = {
            "any": [(2, 22), none, (2, 4), (12, 15), (0, 45)],
            "quarter": [(2, 22), none, (2, 4), (12, 15), (0, 45)],
            "half": [(2, 12), none, (2, 4), (12, 15), (0, 45)],
            "three quarters": [(7, 12), none, no_snow, no_snow, no_snow],
            "every": [no_snow, none, no_snow, no_snow, no_snow],
        }
        self.assertEqual(list(snow.LEVELS), list(expected))
        for level, runs in zip(expected, found):
            self.assertEqual([tuple(run) for run in runs.T.tolist()], expected[level], level)

    def test_a_snow_levels_tile_holds_four_images_and_a_missing_child_takes_its_part_of_the_tile(self):
        child = np.zeros((5, 2, 256, 256), np.uint8)
        for level in range(5):
            child[level] = np.array([10 + level, 20 + level], np.uint8)[:, None, None]
        own = np.full((5, 2, 256, 256), 40, np.uint8)
        own[:, :, 0, 128] = 41
        below = {(0, 0): child, (1, 0): None, (0, 1): None, (1, 1): None}
        body, levels = snow.snow_tile(below, own)
        images = [pixels(image) for image in planner_verify.images(levels)]
        self.assertEqual([image[0, 0].tolist() for image in images], [[10, 20], [11, 21], [13, 23], [14, 24]])
        half = pixels(body)
        self.assertEqual(half[0, 0].tolist(), [12, 22])
        # The north-east child is the north-east quarter of the tile, each pixel twice in each direction.
        north_east = np.full((256, 256, 2), 40, np.uint8)
        north_east[:2, :2] = 41
        np.testing.assert_array_equal(half[:256, 256:], north_east)
        self.assertTrue((half[256:] == 40).all())
        # Without the tile itself, a missing child has no data, not a run of band 0.
        body, _ = snow.snow_tile(below, None)
        self.assertTrue((pixels(body)[256:] == 255).all() and (pixels(body)[:256, 256:] == 255).all())
        self.assertIsNone(snow.snow_tile({at: None for at in below}, own))
        self.assertIsNone(snow.snow_tile({**below, (0, 0): np.full_like(child, snow.NO_DATA)}, None))

    def test_a_pixel_takes_the_worst_case_of_its_four(self):
        groups = [[(10, 20), (12, 30), (5, 15), (253, 253)],
                  [(10, 20), (254, 254), (255, 255), (253, 253)],
                  [(253, 253), (255, 255), (253, 253), (255, 255)],
                  [(10, 20), (255, 255), (255, 255), (255, 255)],
                  [(255, 255)] * 4]
        child = full(snow.NO_DATA, 64)
        for g, four in enumerate(groups):
            for (dr, dc), pixel in zip([(0, 0), (0, 1), (1, 0), (1, 1)], four):
                child[0, :, dr, 2 * g + dc] = pixel
        tile = snow.worst({(0, 0): None, (1, 0): child, (0, 1): None, (1, 1): None})
        self.assertEqual(tile[0, :, 0, 32:37].T.tolist(), [[5, 30], [254, 254], [253, 253], [10, 20], [255, 255]])
        self.assertTrue((tile[:, :, :, :32] == snow.NO_DATA).all() and (tile[:, :, 32:] == snow.NO_DATA).all())


class BlockTest(unittest.TestCase):
    """A block on the production path: no bounds, the MODIS grid of the block, and HR-WSI across the seam of two
    zoom 10 chunks, with values that vary from pixel to pixel."""

    def setUp(self):
        self.grid = snow.Grid("EPSG:4326", Affine.translation(9.7, 46.6) @ Affine.scale(0.001, -0.001), (300, 300))
        rows, cols = np.mgrid[0:300, 0:300]
        onset = ((rows + cols) % 150).astype(np.uint8)
        self.hr = np.stack([onset, onset + 20])[None]
        self.hr[:, :, :, :30] = snow.NO_DATA
        self.modis_grid = snow.modis_grid(snow.geo.tile_bounds(6, 33, 22))
        rows, cols = np.mgrid[0:self.modis_grid.shape[0], 0:self.modis_grid.shape[1]]
        onset = ((3 * rows + cols) % 170).astype(np.uint8)
        self.modis = np.stack([onset, onset + 10])[None]

    def make(self, bounds):
        west, north = 9.7, 46.6
        meets = lambda part: part[0] < west + 0.3 and part[2] > west and part[1] < north and part[3] > north - 0.3
        empty = np.full((1, 2, 2, 2), snow.NO_DATA, np.uint8), self.grid
        season = lambda files, part, days: (self.hr, self.grid) if meets(part) else empty
        making = snow.Making({"SCO": ["x"]}, (self.modis, self.modis_grid), 365, bounds, 2016, None)
        with tempfile.TemporaryDirectory() as out, mock.patch.object(snow, "copernicus_season", side_effect=season):
            snow.make_season((33, 22), making, Path(out))
            return read_archive(Path(out, "2016/33/22.pmtiles"))

    def test_each_tile_is_hr_wsi_where_it_has_data_and_modis_elsewhere_at_its_own_zoom(self):
        tiles, metadata = self.make(None)
        self.assertEqual(metadata, {"season": 2016})
        hr = lambda z, x, y: snow.sample(self.hr, self.grid, z, x, y)
        modis = lambda z, x, y: snow.sample(self.modis, self.modis_grid, z, x, y)
        merged = lambda body, zxy: np.where(body == snow.NO_DATA, modis(*zxy), body)
        twelve = {zxy for zxy in tiles if zxy[0] == 12}
        # The HR-WSI grid spans the zoom 10 chunks 539 and 540.
        self.assertEqual({x >> 2 for _, x, _ in twelve}, {539, 540})
        for zxy in twelve:
            np.testing.assert_array_equal(tiles[zxy], merged(hr(*zxy), zxy), err_msg=str(zxy))
        for zxy in [zxy for zxy in tiles if zxy[0] == 11]:
            below = {at: (hr(*child) if child in twelve else None) for at, child in snow.children(*zxy)}
            np.testing.assert_array_equal(tiles[zxy], merged(snow.parent(below, 1, smoothed=True), zxy), err_msg=str(zxy))
        # A zoom 8 tile without HR-WSI below it is the bilinear MODIS of its own zoom.
        quiet = [zxy for zxy in tiles if zxy[0] == 8 and not any(child in tiles for _, child in snow.children(*zxy))]
        self.assertTrue(quiet)
        for zxy in quiet:
            np.testing.assert_array_equal(tiles[zxy], modis(*zxy), err_msg=str(zxy))

    def test_a_tile_has_the_same_bytes_under_other_bounds(self):
        everything, _ = self.make(None)
        some = next(zxy for zxy in everything if zxy[0] == 11)
        boxed, _ = self.make(snow.geo.tile_bounds(*some))
        for zxy in [some, *(child for _, child in snow.children(*some))]:
            self.assertEqual(zxy in boxed, zxy in everything)
            if zxy in boxed:
                np.testing.assert_array_equal(boxed[zxy], everything[zxy], err_msg=str(zxy))


class CatalogueTest(unittest.TestCase):
    def listing(self, products, *answers, seasons=(2016, 2017)):
        files = lambda names: {season: {"SCO": [f"/vsis3/x/CLMS_WSI_SP_020m_{tile}_{season}0901P1Y_S2_V100_SCO.tif"
                                                for tile in tiles]} for season, tiles in names.items()}
        with mock.patch.object(snow, "copernicus_files", side_effect=[files(products), *map(files, answers)]) as listed, \
                mock.patch.object(snow.time, "sleep"):
            return snow.hr_wsi_files((33, 22), list(seasons)), listed.call_count

    def test_a_season_without_products_or_with_other_tiles_fails(self):
        with self.assertRaisesRegex(SystemExit, r"but none of \[2017\]; try again later"):
            self.listing({2016: ["T32UNA"]})
        with self.assertRaisesRegex(SystemExit, r"tile set differs between seasons 2016 and 2017"):
            self.listing({2016: ["T32UNA", "T32TMT"], 2017: ["T32UNA"]})
        self.assertEqual(len(self.listing({2016: ["T32UNA"], 2017: ["T32UNA"]})[0]), 2)
        # A version that adds one season compares no other.
        self.assertEqual(len(self.listing({2016: ["T32UNA", "T32TMT"], 2017: ["T32UNA"]}, seasons=[2017])[0]), 2)

    def test_an_empty_listing_is_asked_once_more(self):
        files, asked = self.listing({}, {2016: ["T32UNA"], 2017: ["T32UNA"]})
        self.assertEqual((sorted(files), asked), ([2016, 2017], 2))
        self.assertEqual(self.listing({}, {}), ({}, 2), "a block outside HR-WSI has no product")

    def test_the_programs_share_four_connections(self):
        import threading
        from contextlib import ExitStack

        with tempfile.TemporaryDirectory() as directory, ExitStack() as held:
            for _ in range(snow.CDSE_CONNECTIONS):
                held.enter_context(snow.cdse_slot(Path(directory)))
            entered = threading.Event()

            def fifth():
                with snow.cdse_slot(Path(directory)):
                    entered.set()

            thread = threading.Thread(target=fifth)
            thread.start()
            self.assertFalse(entered.wait(0.5), "a fifth reader waits")
            held.close()
            self.assertTrue(entered.wait(5))
            thread.join()


class StepTest(unittest.TestCase):
    def test_the_step_joins_the_files_of_its_snapshot_with_the_credit_of_its_year(self):
        request = {"options": {"bounds": [8, 46, 9, 47], "seasons": [2016, 2017], "year": 2026, "attribution": "© EU {year}"},
                   "snapshots": {"snow": {"2016/33/22.pmtiles": "/store/a", "2017/33/22.pmtiles": "/store/b"}},
                   "output": "/out", "metrics": "/out/metrics.json"}
        made = {"snow": (7, 700), "snow-levels": (7, 2800), "snow-seasons": (90, 9000)}
        with mock.patch.object(snow.step_request, "read", return_value=request), \
                mock.patch.object(snow.step_request, "metrics") as metrics, \
                mock.patch.object(snow, "join", return_value=made) as join:
            snow.step()
        files, bounds, seasons, output, credit = join.call_args.args
        self.assertEqual(files, {"2016/33/22.pmtiles": Path("/store/a"), "2017/33/22.pmtiles": Path("/store/b")})
        self.assertEqual((bounds, seasons, output, credit), ([8, 46, 9, 47], [2016, 2017], Path("/out"), "© EU 2026"))
        self.assertEqual(metrics.call_args.args[1], {"snow": {"tiles": 7, "bytes": 700}, "snow-levels": {"tiles": 7, "bytes": 2800},
                                                     "snow-seasons": {"tiles": 90, "bytes": 9000}})


class JoinTest(unittest.TestCase):
    """Block 33, 22 holds the per-year tiles A = 12/2158/1448, its parent B = 11/1079/724 and B's parent
    C = 10/539/362."""

    def join(self, seasons, bounds):
        """The tiles and the metadata of each archive that `join` writes from season files of block 33, 22, each
        archive checked as the release check reads it."""
        with tempfile.TemporaryDirectory() as directory:
            files = season_files(directory, seasons)
            output = Path(directory, "out")
            made = snow.join(files, bounds, list(seasons), output, "HR-WSI")
            self.assertEqual(sorted(path.name for path in output.iterdir()),
                             ["snow-levels.pmtiles", "snow-seasons.pmtiles", "snow.pmtiles"])
            archives = {}
            for archive in snow.ARCHIVES:
                planner_verify.archive(output / f"{archive}.pmtiles", archive)
                archives[archive] = read_archive(output / f"{archive}.pmtiles", size=snow.SEASON_TILE)
                self.assertEqual(made[archive][0], len(archives[archive][0]))
        return archives

    def test_a_zoom_14_tile_holds_its_part_of_the_per_year_tile_and_a_season_takes_the_part_of_its_ancestor(self):
        # 2016 has A; 2017 only B, whose north-west quarter, the part over A, is 30 and the rest 90; 2018 only C,
        # whose third column and first row of 4 × 4 is over A; 2019 has no data in the block.
        rows, cols = np.mgrid[0:256, 0:256]
        a = np.stack([(rows + cols) % 150, (rows + cols) % 150 + 20])[None].astype(np.uint8)
        b = full(90)
        b[:, :, :128, :128] = 30
        c = np.stack([cols % 180, cols % 180])[None].astype(np.uint8)
        seasons = {2016: {(12, 2158, 1448): a}, 2017: {(11, 1079, 724): b}, 2018: {(10, 539, 362): c}, 2019: {}}
        archives = self.join(seasons, snow.geo.tile_bounds(12, 2158, 1448))
        per_year = np.concatenate([a, full(30), c[:, :, :64, 128:192].repeat(4, 2).repeat(4, 3), full(snow.NO_DATA)])
        tiles, metadata = archives["snow-seasons"]
        for j in range(4):
            for i in range(4):
                np.testing.assert_array_equal(tiles[(14, 4 * 2158 + i, 4 * 1448 + j)],
                                              per_year[:, :, 64 * j:64 * (j + 1), 64 * i:64 * (i + 1)], err_msg=f"{i}, {j}")
        self.assertEqual(metadata, {"snow_format": 1, "first_season": 2016, "seasons": 4, "source": "copernicus-hr-wsi",
                                    "resolution_m": 20, "attribution": "HR-WSI", "step_days": 2})
        self.assertEqual(sorted({z for z, _, _ in tiles}), list(range(2, 15)))
        runs, metadata = archives["snow"]
        self.assertEqual(sorted({z for z, _, _ in runs}), list(range(12)))
        self.assertNotIn("step_days", metadata)
        # A missing child of zoom 9 takes its part of the tile; a missing child of zoom 8 has no data.
        self.assertTrue((pixels(runs[(8, 134, 90)])[..., 0] != snow.NO_DATA).all())
        seven = pixels(runs[(7, 67, 45)])[..., 0]
        self.assertTrue((seven[:256, :256] != snow.NO_DATA).all())
        self.assertEqual((seven == snow.NO_DATA).sum(), 3 * 256 * 256)

    def test_a_quarter_without_a_tile_one_zoom_deeper_takes_its_part_of_the_per_year_tile(self):
        # A is dated, with no data in its parts (1, 0) and (0, 1); the join lacks 11/1078/724, child (0, 0) of C.
        rows, cols = np.mgrid[0:256, 0:256]
        onset = ((3 * rows + cols) % 120).astype(np.uint8)
        a = np.stack([onset, onset + 40])[None]
        a[:, :, :64, 64:128] = a[:, :, 64:128, :64] = snow.NO_DATA
        b = np.stack([onset // 2, onset // 2 + 10])[None]
        c = np.stack([onset // 3, onset // 3 + 5])[None]
        seasons = {2016: {(12, 2158, 1448): a, (11, 1079, 724): b, (10, 539, 362): c}}
        tiles, _ = self.join(seasons, snow.geo.tile_bounds(10, 539, 362))["snow-seasons"]
        self.assertNotIn((14, 8633, 5792), tiles)
        self.assertNotIn((14, 8632, 5793), tiles)

        def worst(part):
            four = part.reshape(1, 2, 32, 2, 32, 2)
            return np.stack([four[:, 0].min(axis=(2, 4)), four[:, 1].max(axis=(2, 4))], 1)

        # Tile (0, 0) of B: two quarters from the worst case of A, two from B.
        thirteen = tiles[(13, 4316, 2896)]
        np.testing.assert_array_equal(thirteen[:, :, :32, :32], worst(a[:, :, :64, :64]))
        np.testing.assert_array_equal(thirteen[:, :, 32:, 32:], worst(a[:, :, 64:128, 64:128]))
        np.testing.assert_array_equal(thirteen[:, :, :32, 32:], b[:, :, :32, 32:64])
        np.testing.assert_array_equal(thirteen[:, :, 32:, :32], b[:, :, 32:64, :32])
        # Tile (0, 0) of C lies over the missing 11/1078/724: all four quarters from C.
        np.testing.assert_array_equal(tiles[(12, 2156, 1448)], c[:, :, :64, :64])


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
