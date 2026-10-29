"""The climate prepare writes the daily values of local solar days, and the bake aggregates them into weeks and writes
the bytes of the climate tile spec."""

import datetime as dt
import gzip
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import urllib.error

import numpy as np

from tools import planner_climate as climate

FIRST = 2016  # a leap year; 2017 is not


def hourly(cells, t2m=10.0, u10=0.0, v10=-1.0):
    """Source values {variable: (hours, cells)} for the ten years from FIRST: no rain, a 1 m/s north wind."""
    days = (dt.date(FIRST + climate.YEARS, 1, 1) - dt.date(FIRST, 1, 1)).days
    hours = 24 * days + 2 * climate.MARGIN_HOURS
    return {"t2m": np.full((hours, cells), t2m + 273.15), "u10": np.full((hours, cells), u10),
            "v10": np.full((hours, cells), v10), "tp": np.zeros((hours, cells))}


def weeks(source, lon):
    """The weekly fields, monthly means and wind rose of the ten years of hourly `source`."""
    days = (dt.date(FIRST + climate.YEARS, 1, 1) - dt.date(FIRST, 1, 1)).days
    return climate.aggregate(*climate.daily(source, lon, days), FIRST)


def utc(day, local_hour, offset):
    """Index of the source hour at local solar `local_hour` of day `day` (stamps past 23 reach into the next day)."""
    return climate.MARGIN_HOURS + 24 * day + local_hour - offset


class AggregateTest(unittest.TestCase):
    def test_weeks_have_seven_days_and_the_last_week_takes_the_rest_of_the_year(self):
        month, week = climate.calendar(FIRST)
        days = np.bincount(week).reshape(climate.YEARS, climate.WEEKS)
        self.assertTrue((days[:, :51] == 7).all())
        self.assertEqual(days[:2, 51].tolist(), [9, 8])
        self.assertEqual((month[59], week[59]), (2, 8))  # 29 February 2016

    def test_the_solar_offset_comes_from_the_longitude_in_whole_hours(self):
        self.assertEqual(climate.solar_offset([0, 7.49, 7.5, 30, -120, 179.9]).tolist(), [0, 0, 1, 2, -8, 12])

    def test_local_solar_days_give_rain_temperature_and_daytime_wind(self):
        source = hourly(2)
        for cell, offset in enumerate(climate.solar_offset([0, 30])):
            def set_(variable, day, hour, value):
                source[variable][utc(day, hour, offset), cell] = value
            wet = climate.WET_MM / 1000
            set_("tp", 0, 24, wet)  # the hour that ends at local midnight belongs to day 0: wet
            set_("tp", 1, 1, wet / 2)
            set_("tp", 1, 24, wet / 2 + 0.0001)  # wet on day 1
            set_("tp", 2, 12, wet - 0.00001)  # just under the threshold: dry
            set_("t2m", 3, 23, 30 + 273.15)
            set_("t2m", 3, 24, 40 + 273.15)  # local 00:00 of day 4
            for hour in climate.DAYTIME:
                set_("u10", 5, hour, 4.0)
                set_("v10", 5, hour, 0.0)
            set_("u10", 5, 8, 100.0)
            set_("u10", 5, 19, 100.0)
        weekly, monthly, rose = weeks(source, np.array([0.0, 30.0]))
        for cell in range(2):
            self.assertEqual(weekly["wet_days"][0, 0, cell], 2)
            self.assertAlmostEqual(weekly["rain"][0, 0, cell], (3 * climate.WET_MM + 0.09) * climate.RAIN_FACTORS[0])
            self.assertAlmostEqual(weekly["tmax"][0, 0, cell], (5 * 10 + 30 + 40) / 7)
            self.assertAlmostEqual(weekly["tmin"][0, 0, cell], 10)
            self.assertAlmostEqual(weekly["wind"][0, 0, cell], (6 * 1 + 4) / 7 * climate.WIND_FACTOR)
            self.assertAlmostEqual(weekly["wind"][0, 1, cell], climate.WIND_FACTOR)
            # January has 3,100 daytime samples in ten years: ten from the west on day 5, the rest from the north.
            self.assertAlmostEqual(rose[0, 12, cell], 100 * 10 / 3100)
            self.assertAlmostEqual(rose[0, 0, cell], 100 * 3090 / 3100)
            self.assertAlmostEqual(monthly["tmax"][6, cell], 10)
            self.assertAlmostEqual(monthly["tmin"][0, cell], 10)

    def test_weekly_rain_takes_the_factor_of_the_month_of_its_fourth_day(self):
        source = hourly(1)
        source["tp"][:] = 0.0001  # 2.4 mm a day: every day is wet
        weekly, _, _ = weeks(source, np.array([0.0]))
        factor = lambda month: climate.RAIN_FACTORS[month - 1]
        rain = weekly["rain"][:, :, 0] / 2.4
        # Week 4 holds 29 January to 4 February: February. Week 8 is March also in the leap year 2016, as in the client.
        for week, month in [(0, 1), (4, 2), (8, 3), (29, 7)]:
            self.assertTrue(np.allclose(rain[:, week], 7 * factor(month)), week)
        self.assertTrue(np.allclose(rain[:2, 51], [9 * factor(12), 8 * factor(12)]))
        self.assertTrue((weekly["wet_days"][:2, 51, 0] == [9, 8]).all())

    def test_weekly_daytime_wind_takes_one_factor_in_every_month(self):
        weekly, _, _ = weeks(hourly(1, u10=3.0, v10=-4.0), np.array([0.0]))
        self.assertTrue(np.allclose(weekly["wind"], 5 * climate.WIND_FACTOR))

    def test_a_missing_hour_makes_its_week_missing(self):
        source = hourly(1)
        source["t2m"][utc(9, 5, 0), 0] = np.nan
        weekly, _, _ = weeks(source, np.array([0.0]))
        self.assertTrue(np.isnan(weekly["tmax"][0, 1, 0]))
        self.assertFalse(np.isnan(weekly["tmax"][0, [0, 2], 0]).any())
        self.assertFalse(np.isnan(weekly["rain"][0, 1, 0]))


class CodeTest(unittest.TestCase):
    def round_trip(self, name, values):
        return climate.decode(name, climate.encode(name, values)).round(6).tolist()

    def test_values_take_the_nearest_code_and_clamp(self):
        self.assertEqual(self.round_trip("tmax", [-0.25, 0.25, 0.74, -100, 100]), [0, 0.5, 0.5, -63.5, 63.5])
        self.assertEqual(self.round_trip("rain", [0.4, 99.6, 102.4, 103, 2000]), [0, 100, 100, 105, 870])
        self.assertEqual(self.round_trip("wind", [0.24, 0.25, 200]), [0, 0.5, 127])
        self.assertEqual(self.round_trip("lapse_tmin", [-6.54, 20]), [-6.5, 12.7])

    def test_missing_is_its_own_code(self):
        self.assertEqual(climate.encode("tmin", [np.nan, 0]).tolist(), [-128, 0])
        self.assertEqual(climate.encode("wet_days", [np.nan, 0]).tolist(), [255, 0])
        self.assertTrue(np.isnan(climate.decode("orography", [-32768])[0]))


class LapseTest(unittest.TestCase):
    def test_the_height_coefficient_is_separated_from_horizontal_gradients(self):
        rows, cols = 7, 9
        lat = 48 - 0.1 * np.arange(rows)
        lon = 8 + 0.1 * np.arange(cols)
        height = 200 + (np.arange(rows * cols) ** 2 * 37 % 1000).reshape(rows, cols).astype(float)
        mean = np.stack([20 - m - 6.0 * height / 1000 + 0.5 * lon - 0.3 * lat[:, None] for m in range(12)])
        mean[0, 3, 3] = np.nan
        rates = climate.lapse_rates(mean, height, lat, lon)
        inner = rates[:, 2:-2, 2:-2]
        self.assertTrue(np.isnan(rates[:, :2]).all() and np.isnan(rates[:, :, -2:]).all())
        self.assertTrue(np.isnan(inner[:, 1, 1]).all())  # the cell with a missing month
        self.assertTrue(np.allclose(inner[:, 0, 0], -6.0))
        flat = climate.lapse_rates(np.zeros((12, rows, cols)), np.full((rows, cols), 300.0), lat, lon)
        self.assertTrue((flat[:, 2:-2, 2:-2] == climate.STANDARD_LAPSE).all())


class GridTest(unittest.TestCase):
    def test_cells_touch_the_bounds_and_map_to_the_zarr_index(self):
        region = climate.Region.around([7.45, 47.5, 10.5, 49.85], pad=0)
        self.assertEqual((region.lat[[0, -1]].tolist(), region.lon[[0, -1]].round(6).tolist()), ([49.8, 47.5], [7.5, 10.5]))
        corner = climate.Region.around([-180, 89.96, -179.96, 90], pad=0)
        self.assertEqual((corner.rows.tolist(), corner.cols.tolist()), ([0], [0]))
        self.assertEqual([index.tolist() for index in corner.zarr_index()], [[1800], [3599]])

    def test_a_year_reads_the_time_chunks_of_its_hours_and_a_block_reads_its_own_chunks(self):
        self.assertEqual(climate.hour(dt.date(1950, 1, 2)), 0)
        region = climate.Region.around([7.45, 47.5, 10.5, 49.85])
        start, end, times, spatial = climate.chunk_plan(region, FIRST)
        self.assertEqual((end - start, list(times), len(spatial)), (24 * 366 + 96, [17], 40))
        # Time chunk 18 starts on 24 May 2019.
        self.assertEqual(list(climate.chunk_plan(region, 2019)[2]), [17, 18])
        block = climate.Region.block(19, 22)
        rows, cols = block.zarr_index()
        self.assertEqual((rows[[0, -1]].tolist(), cols[[0, -1]].tolist()), ([1471, 1408], [1824, 1919]))
        spatial = climate.chunk_plan(block, FIRST)[3]
        self.assertEqual((len(spatial), spatial[0], spatial[-1]), (16 * 12, (352, 228), (367, 239)))
        self.assertEqual(climate.Region.block(37, 28).zarr_index()[1][-1], 3599)

    def test_a_tile_holds_its_cells_in_row_order_at_the_spec_offsets(self):
        region = climate.Region.around([8.0, 48.0, 8.3, 48.1], pad=0)  # rows 419–420, columns 1880–1883
        shape = (len(region.rows), len(region.cols))
        values = {zoom: {name: np.full((count, *shape), 1.0) for name, count in layout} for zoom, (_, _, layout) in climate.LEVELS.items()}
        values[climate.DETAIL]["tmax"][3] = np.arange(np.prod(shape)).reshape(shape)
        tiles = climate.tiles(region, values)
        from pmtiles.tile import zxy_to_tileid
        self.assertEqual(set(tiles), {zxy_to_tileid(9, 156, 52), zxy_to_tileid(8, 78, 26)})
        body = gzip.decompress(tiles[zxy_to_tileid(9, 156, 52)])
        self.assertEqual(len(body), 96 * (2 + 2 * 12 + 5 * 520))
        planes = dict(climate.LEVELS[climate.DETAIL][2])
        offset = 96 * (2 + 2 * 12 + 2 * 520 + 3)  # tmax, index 3 (year 0, week 3)
        tmax = np.frombuffer(body, np.int8, 96, offset).reshape(8, 12)
        # Row 419 is row 3 of the tile (8 × 52 = 416); column 1880 is column 8 (12 × 156 = 1872).
        self.assertEqual(climate.decode("tmax", tmax[3:5, 8:12]).tolist(), [[0, 1, 2, 3], [4, 5, 6, 7]])
        self.assertEqual(tmax[0, 0], -128)
        self.assertEqual(planes["tmax"], 520)


class SourceTest(unittest.TestCase):
    def chunk(self, cache, time_chunk, hours, data=b""):
        path = Path(cache) / "t2m" / f"{time_chunk}.1.2.{hours}.{hashlib.sha256(data).hexdigest()}"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def test_a_cached_chunk_serves_every_bake_whose_hours_it_holds_as_final_data(self):
        final = climate.hour(dt.date(FIRST + climate.YEARS, 1, 1)) + climate.FINAL_AFTER_DAYS * 24
        next_year = final + 365 * 24
        margin = climate.FINAL_AFTER_DAYS * 24
        with tempfile.TemporaryDirectory() as cache:
            sources = [climate.Source(hour, cache=Path(cache)) for hour in (final, next_year)]
            # Chunk 19 holds the end of the years: it is final for this bake only from `final` on.
            self.chunk(cache, 19, final - 1)
            with self.assertRaisesRegex(ValueError, "not in the cache"):
                sources[0].path("t2m", "19.1.2")
            newest = self.chunk(cache, 19, final)
            self.assertEqual(sources[0].path("t2m", "19.1.2"), newest)
            self.assertTrue(np.isnan(sources[0].read(newest)).all())  # a chunk that the store omits
            with self.assertRaisesRegex(ValueError, "not in the cache"):
                sources[1].path("t2m", "19.1.2")
            # Chunk 17 ends years earlier: once final, it serves this bake and every yearly update.
            self.chunk(cache, 17, 18 * climate.TIME_CHUNK + margin - 1)
            with self.assertRaisesRegex(ValueError, "not in the cache"):
                sources[0].path("t2m", "17.1.2")
            complete = self.chunk(cache, 17, 18 * climate.TIME_CHUNK + margin)
            self.assertEqual([source.path("t2m", "17.1.2") for source in sources], [complete, complete])
            newest.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "damaged"):
                sources[0].read(newest)


# Blocks of one Zarr chunk, so a test reads and writes small files.
SMALL = mock.patch.multiple(climate, BLOCK_ROWS=climate.CHUNK_ROWS, BLOCK_COLS=climate.CHUNK_COLS)


class PrepareTest(unittest.TestCase):
    @SMALL
    def test_a_block_writes_the_planes_of_each_year_from_the_final_chunks_and_keeps_no_chunk(self):
        values = {"t2m": 283.15, "u10": 0.0, "v10": -1.0, "tp": 0.001}  # 1 mm an hour, a 1 m/s north wind
        shape = (climate.TIME_CHUNK, climate.CHUNK_ROWS, climate.CHUNK_COLS)
        # The chunks are raw float32 here; `blosc` passes them on.
        chunk = lambda variable: np.full(shape, values[variable], np.float32).tobytes()
        hours = climate.final_hour(FIRST + 1)
        layout = {variable: (f"https://arco.test/{variable}", hours) for variable in climate.SOURCE}
        asked = []
        answer = lambda url, key=None: asked.append((url, key)) or chunk(url.rsplit("/", 2)[1])
        z = np.full((climate.CHUNK_ROWS, climate.CHUNK_COLS), 9806.65, np.float64)
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(climate, "token", return_value="KEY"), \
                mock.patch.object(climate, "stores", return_value=layout), mock.patch.object(climate, "fetch", answer), \
                mock.patch.object(climate, "geopotential", return_value=z), \
                mock.patch.object(climate, "blosc", lambda data, dtype: np.frombuffer(data, dtype)):
            out, work = Path(directory, "out"), Path(directory, "work")
            climate.prepare((235, 344), [FIRST + 1, FIRST], work, out, orography=Path(directory, "orography"))
            self.assertFalse(work.exists())
            files = sorted(str(path.relative_to(out)) for path in out.rglob("*") if path.is_file())
            self.assertEqual(files, ["2016/235/344.gz", "2017/235/344.gz", "235-344.2016.sha256", "235-344.2017.sha256",
                                     "235-344.orography.sha256", "orography/235/344.gz"])
            for name, path in [("2016", "2016/235/344.gz"), ("orography", "orography/235/344.gz")]:
                mark = (out / f"235-344.{name}.sha256").read_text().strip()
                self.assertEqual(mark, hashlib.sha256((out / path).read_bytes()).hexdigest())
            body = gzip.decompress((out / "2016/235/344.gz").read_bytes())
            orography = np.frombuffer(gzip.decompress((out / "orography/235/344.gz").read_bytes()), "<f8")
        # Each variable has one chunk of the two years: time chunk 17, downloaded once.
        self.assertEqual(sorted(asked), [(f"https://arco.test/{v}/{v}/17.344.235", "KEY") for v in sorted(climate.SOURCE)])
        planes, sector = climate.read_block(body, 366, 32)
        self.assertTrue((planes["tmax"] == np.float32(283.15)).all() and (planes["tmin"] == np.float32(283.15)).all())
        self.assertTrue(np.allclose(planes["rain"], 24) and np.allclose(planes["wind"], 1))
        self.assertTrue((sector == 0).all())
        self.assertTrue((orography == 9806.65).all())


class BakeTest(unittest.TestCase):
    @SMALL
    def test_the_same_files_give_the_same_archive(self):
        from pmtiles.reader import MmapSource, Reader

        bounds = [8.0, 48.0, 8.2, 48.1]
        rng = np.random.default_rng(1)
        with tempfile.TemporaryDirectory() as directory:
            files = {}
            region = climate.Region.around(bounds)
            rows, cols = region.zarr_index()
            for y in np.unique(rows // climate.BLOCK_ROWS):
                for x in np.unique(cols // climate.BLOCK_COLS):
                    def put(path, planes):
                        files[path] = Path(directory, path)
                        climate.write(files[path], planes)
                    put(f"orography/{x}/{y}.gz", [(300 + 700 * rng.random(32) * climate.GRAVITY)])
                    for year in range(FIRST, FIRST + climate.YEARS):
                        days = (dt.date(year + 1, 1, 1) - dt.date(year, 1, 1)).days
                        put(f"{year}/{x}/{y}.gz", [
                            (280 + 10 * rng.random((days, 32))).astype("<f4"), (270 + 10 * rng.random((days, 32))).astype("<f4"),
                            12 * rng.random((days, 32)) ** 4, 8 * rng.random((days, 32)),
                            rng.integers(0, climate.SECTORS, (days, 10, 32)).astype(np.uint8)])
            paths = [Path(directory) / f"{k}.pmtiles" for k in range(2)]
            for path in paths:
                counts = climate.bake(bounds, FIRST, files, path, "ERA5-Land {year}", "2026-10-01")
            self.assertEqual(paths[0].read_bytes(), paths[1].read_bytes())
            self.assertEqual(counts, {8: 1, 9: 1})
            with paths[0].open("rb") as stream:
                reader = Reader(MmapSource(stream))
                header, metadata = reader.header(), reader.metadata()
        self.assertEqual((header["min_zoom"], header["max_zoom"], metadata["first_year"], metadata["years"]), (8, 9, FIRST, 10))
        self.assertEqual((sorted(metadata["inputs"]), metadata["wet_day_mm"]), (["climate", "doi", "orography_sha256"], climate.WET_MM))
        self.assertEqual((metadata["rain_factors"], metadata["wind_factor"]), (list(climate.RAIN_FACTORS), climate.WIND_FACTOR))


class FetchTest(unittest.TestCase):
    def test_a_cut_connection_is_tried_again_and_a_missing_chunk_is_not(self):
        import http.client
        import urllib.error

        response = mock.MagicMock()
        response.__enter__.return_value.read.return_value = b"chunk"
        cut = [http.client.RemoteDisconnected("closed"), http.client.IncompleteRead(b""), ConnectionResetError(), response]
        with mock.patch("urllib.request.urlopen", side_effect=cut) as urlopen, mock.patch("time.sleep") as sleep:
            self.assertEqual(climate.fetch("https://arco.test/a"), b"chunk")
        self.assertEqual((urlopen.call_count, [call.args[0] for call in sleep.call_args_list]), (4, list(climate.RETRY_SECONDS[:3])))
        missing = urllib.error.HTTPError("https://arco.test/b", 404, "Not Found", {}, None)
        with mock.patch("urllib.request.urlopen", side_effect=missing) as urlopen, mock.patch("time.sleep"):
            with self.assertRaises(urllib.error.HTTPError):
                climate.fetch("https://arco.test/b")
        self.assertEqual(urlopen.call_count, 1)


class StoresTest(unittest.TestCase):
    def test_a_newest_time_chunk_that_the_daily_update_has_not_written_yet_is_read_again_after_a_wait(self):
        hours = climate.TIME_CHUNK + 24  # the store's daily update starts time chunk 1
        array = {"dtype": "<f4", "shape": [hours, climate.GRID_ROWS, climate.GRID_COLS], "order": "C", "filters": None,
                 "chunks": [climate.TIME_CHUNK, climate.CHUNK_ROWS, climate.CHUNK_COLS], "compressor": {"id": "blosc"}}
        meta = {"time/.zarray": {"chunks": [climate.TIME_CHUNK]}, "time/.zattrs": {"units": "hours since 1970-01-01"},
                **{f"{variable}/.zarray": array for variable in climate.SOURCE}}
        first = (climate.EPOCH - dt.datetime(1970, 1, 1)) // dt.timedelta(hours=1)

        def missing(url, values):
            raise urllib.error.HTTPError(url, 404, "Not Found", {}, None)

        def stale(url, values):
            values[23:] = 0  # the fill value in the slots that the update has not written yet
            return values.tobytes()

        for old_chunk in (missing, stale):
            asked = []

            def answer(url, key=None):
                name = url.split("geoChunked.zarr/")[1]
                asked.append(name)
                if name.startswith("time/"):
                    values = (first + int(name[5:]) * climate.TIME_CHUNK + np.arange(climate.TIME_CHUNK)).astype("<i8")
                    return old_chunk(url, values) if name == "time/1" and asked.count(name) == 1 else values.tobytes()
                return {".zmetadata": json.dumps({"metadata": meta}).encode(),
                        "latitude/0": (np.arange(climate.GRID_ROWS) / 10 - 90).tobytes(),
                        "longitude/0": (np.arange(1, climate.GRID_COLS + 1) / 10 - 180).tobytes()}[name]

            with self.subTest(old_chunk.__name__), mock.patch.object(climate, "fetch", answer), \
                    mock.patch("time.sleep") as sleep, \
                    mock.patch.object(climate, "blosc", lambda data, dtype: np.frombuffer(data, dtype)):
                layout = climate.stores("KEY")
                self.assertEqual({variable: count for variable, (_, count) in layout.items()},
                                 dict.fromkeys(climate.SOURCE, hours))
                self.assertEqual([call.args[0] for call in sleep.call_args_list], [climate.RETRY_SECONDS[0]])
                self.assertEqual(asked[:3], [".zmetadata", "time/1", ".zmetadata"])


class OracleSource(climate.Source):
    """Chunks in which each cell tells where it is: a 2 m temperature of 273.15 K + (its Zarr row − 1790) + (its Zarr
    column − 3550) / 100, no rain and a 1 m/s north wind."""

    def __init__(self, *args, **kwargs):
        super().__init__(0)

    def path(self, variable, name):
        return Path(variable, name)

    def read(self, path):
        _, y, x = map(int, path.name.split("."))
        rows, cols = climate.CHUNK_ROWS * y + np.arange(climate.CHUNK_ROWS), climate.CHUNK_COLS * x + np.arange(climate.CHUNK_COLS)
        value = {"t2m": 273.15 + (rows[:, None] - 1790) + (cols[None, :] - 3550) / 100, "u10": 0.0, "v10": -1.0, "tp": 0.0}
        shape = (climate.TIME_CHUNK, climate.CHUNK_ROWS, climate.CHUNK_COLS)
        return np.broadcast_to(np.asarray(value[path.parent.name], np.float32), shape)


class OrientationTest(unittest.TestCase):
    def test_each_cell_of_two_edge_blocks_keeps_its_place_from_the_chunks_to_the_bake(self):
        # Block 37, 28 holds the 9 northernmost rows and the 48 easternmost columns; block 36, 28 is west of it.
        block = climate.Region.block(37, 28)
        self.assertEqual((len(block.rows), len(block.cols)), (9, 48))
        geopotential = lambda region, key, cache: np.add.outer(1000.0 * region.zarr_index()[0], region.zarr_index()[1])
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(climate, "token", return_value="KEY"), \
                mock.patch.object(climate, "Source", OracleSource), mock.patch.object(climate, "geopotential", geopotential):
            out = Path(directory, "out")
            for x in (36, 37):
                climate.prepare((x, 28), range(FIRST, FIRST + climate.YEARS), Path(directory, "work"), out, Path(directory))
            files = {str(path.relative_to(out)): path for path in out.rglob("*.gz")}
            region = climate.Region.around([175.1, 89.6, 175.3, 89.8])
            weekly, _, _, z = climate.climate(region, FIRST, files)
        rows, cols = region.zarr_index()
        # Columns 3548 to 3551 are in block 36, the others in block 37.
        self.assertEqual((rows[[0, -1]].tolist(), cols[[0, -1]].tolist()), ([1800, 1794], [3548, 3554]))
        self.assertTrue(np.allclose(weekly["tmax"][0, 0], (rows[:, None] - 1790) + (cols[None, :] - 3550) / 100, atol=1e-4))
        self.assertTrue((z == 1000 * rows[:, None] + cols[None, :]).all())


if __name__ == "__main__":
    unittest.main()
